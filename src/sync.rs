// sync.rs — watch local folder and sync against a remote manifest file.
// Startup scans the local folder and fetches one remote manifest file.

use crate::config::Config;
use crate::webdav;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, UNIX_EPOCH};

const REMOTE_FULL_SYNC_INTERVAL: Duration = Duration::from_secs(60);
const REMOTE_MARKER_FAST_INTERVAL: Duration = Duration::from_secs(10);
const REMOTE_MARKER_IDLE_INTERVAL: Duration = Duration::from_secs(30);
const REMOTE_MARKER_FAST_WINDOW: Duration = Duration::from_secs(5 * 60);
const REMOTE_HEAL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MANIFEST_NAME: &str = ".backupsynctool-manifest.json";
/// Google Drive staging dirs — never scan or upload.
const IGNORED_DIR_NAME: &str = ".tmp.driveupload";
/// Files at or above this size upload one-at-a-time; smaller files keep `parallel_uploads`.
const LARGE_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FileState {
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SyncManifest {
    #[serde(default)]
    files: HashMap<String, FileState>,
}

pub struct SyncEngine {
    _watcher: RecommendedWatcher,
    stop: Arc<AtomicBool>,
}

pub type LogFn = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum ActivityState {
    Checking,
    Syncing,
    Idle,
}

#[derive(Clone)]
pub struct ActivityInfo {
    pub state: ActivityState,
    pub completed: usize,
    pub total: usize,
    pub failed: usize,
    pub failed_paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UploadBatchResult {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub failed_paths: Vec<String>,
}

impl UploadBatchResult {
    fn absorb(&mut self, other: &UploadBatchResult) {
        self.attempted += other.attempted;
        self.succeeded += other.succeeded;
        self.failed += other.failed;
        self.failed_paths.extend(other.failed_paths.iter().cloned());
    }
}

enum UploadOutcome {
    Success,
    Failed(String),
    Skipped,
}

pub type ActivityFn = Arc<dyn Fn(ActivityInfo) + Send + Sync>;
pub type AuthFailedFn = Arc<dyn Fn() + Send + Sync>;

impl SyncEngine {
    pub fn start(
        cfg: Config,
        password: String,
        log: LogFn,
        activity: ActivityFn,
        auth_failed: AuthFailedFn,
    ) -> Result<Self, String> {
        let pending: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let suppressed: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let pending_clone = pending.clone();
        let suppressed_clone = suppressed.clone();
        let stop_clone = stop.clone();
        let cfg_arc = Arc::new(cfg);
        let pass_arc = Arc::new(password);
        let log_clone = log.clone();
        let activity_clone = activity.clone();
        let auth_failed_clone = auth_failed.clone();
        let cfg_watcher = cfg_arc.clone();
        let pass_watcher = pass_arc.clone();

        std::thread::spawn(move || {
            let had_local_manifest = has_local_manifest(&cfg_watcher);
            let manifest = Arc::new(Mutex::new(load_local_manifest(&cfg_watcher)));

            activity_clone(ActivityInfo {
                state: ActivityState::Checking,
                completed: 0,
                total: 0,
                failed: 0,
                failed_paths: Vec::new(),
            });

            let remote_manifest =
                fetch_remote_state(&cfg_watcher, &pass_watcher, &log_clone, &auth_failed_clone);
            let startup_batch = sync_startup(
                &cfg_watcher,
                &pass_watcher,
                &manifest,
                had_local_manifest,
                remote_manifest.as_ref(),
                &suppressed_clone,
                &log_clone,
                &activity_clone,
                &auth_failed_clone,
            );

            activity_clone(ActivityInfo {
                state: ActivityState::Idle,
                completed: startup_batch.succeeded,
                total: startup_batch.attempted,
                failed: startup_batch.failed,
                failed_paths: startup_batch.failed_paths,
            });

            let mut last_remote_full_sync = Instant::now();
            let mut last_remote_marker_check = Instant::now();
            let mut last_remote_marker = if cfg_watcher.sync_remote_changes {
                fetch_remote_manifest_marker(&cfg_watcher, &pass_watcher, &auth_failed_clone)
            } else {
                None
            };
            let mut fast_remote_marker_until = Instant::now() + REMOTE_MARKER_FAST_WINDOW;
            let mut last_remote_heal = Instant::now();
            while !stop_clone.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                let now = Instant::now();
                let due: Vec<PathBuf> = {
                    let mut guard = pending_clone.lock().unwrap();
                    let (ready, keep): (Vec<_>, Vec<_>) = guard
                        .drain(..)
                        .partition(|(_, t)| now.duration_since(*t) >= Duration::from_millis(500));
                    *guard = keep;
                    let mut seen = HashSet::new();
                    ready
                        .into_iter()
                        .map(|(p, _)| p)
                        .filter(|p| seen.insert(p.clone()))
                        .collect()
                };

                if !due.is_empty() {
                    activity_clone(ActivityInfo {
                        state: ActivityState::Syncing,
                        completed: 0,
                        total: due.len(),
                        failed: 0,
                        failed_paths: Vec::new(),
                    });
                    let batch = upload_paths_parallel(
                        &cfg_watcher,
                        &pass_watcher,
                        &due,
                        &manifest,
                        &log_clone,
                        cfg_watcher.parallel_uploads,
                        Some(&activity_clone),
                        &auth_failed_clone,
                    );
                    if cfg_watcher.sync_remote_changes && batch.succeeded > 0 {
                        fast_remote_marker_until = Instant::now() + REMOTE_MARKER_FAST_WINDOW;
                        last_remote_marker = fetch_remote_manifest_marker(
                            &cfg_watcher,
                            &pass_watcher,
                            &auth_failed_clone,
                        );
                    }
                    activity_clone(ActivityInfo {
                        state: ActivityState::Idle,
                        completed: batch.succeeded,
                        total: batch.attempted,
                        failed: batch.failed,
                        failed_paths: batch.failed_paths,
                    });
                }

                if last_remote_heal.elapsed() >= REMOTE_HEAL_INTERVAL {
                    let batch = heal_missing_uploads(
                        &cfg_watcher,
                        &pass_watcher,
                        &manifest,
                        &log_clone,
                        &activity_clone,
                        &auth_failed_clone,
                    );
                    if batch.attempted > 0 {
                        activity_clone(ActivityInfo {
                            state: ActivityState::Idle,
                            completed: batch.succeeded,
                            total: batch.attempted,
                            failed: batch.failed,
                            failed_paths: batch.failed_paths,
                        });
                    }
                    last_remote_heal = Instant::now();
                }

                if cfg_watcher.sync_remote_changes {
                    let now = Instant::now();
                    let marker_interval = if now <= fast_remote_marker_until {
                        REMOTE_MARKER_FAST_INTERVAL
                    } else {
                        REMOTE_MARKER_IDLE_INTERVAL
                    };

                    if last_remote_marker_check.elapsed() >= marker_interval {
                        if let Some(remote_manifest) = fetch_remote_manifest_marker(
                            &cfg_watcher,
                            &pass_watcher,
                            &auth_failed_clone,
                        ) {
                            if last_remote_marker.as_ref() != Some(&remote_manifest) {
                                log_clone(
                                    "Remote manifest changed, downloading server updates"
                                        .to_string(),
                                );
                                apply_remote_manifest(
                                    &cfg_watcher,
                                    &pass_watcher,
                                    &manifest,
                                    &remote_manifest,
                                    &suppressed_clone,
                                    &log_clone,
                                    &auth_failed_clone,
                                );
                                last_remote_marker = Some(remote_manifest);
                                fast_remote_marker_until =
                                    Instant::now() + REMOTE_MARKER_FAST_WINDOW;
                            }
                        }
                        last_remote_marker_check = Instant::now();
                    }

                    if last_remote_full_sync.elapsed() >= REMOTE_FULL_SYNC_INTERVAL {
                        if let Some(remote_manifest) = fetch_remote_state(
                            &cfg_watcher,
                            &pass_watcher,
                            &log_clone,
                            &auth_failed_clone,
                        ) {
                            apply_remote_manifest(
                                &cfg_watcher,
                                &pass_watcher,
                                &manifest,
                                &remote_manifest,
                                &suppressed_clone,
                                &log_clone,
                                &auth_failed_clone,
                            );
                            last_remote_marker = Some(remote_manifest);
                        }
                        last_remote_full_sync = Instant::now();
                        last_remote_marker_check = Instant::now();
                    }
                }
            }
        });

        let watch_path = cfg_arc.watch_folder.clone();
        let watch_path_for_events = watch_path.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        let mut guard = pending.lock().unwrap();
                        for path in event.paths {
                            if should_ignore_path(&watch_path_for_events, &path)
                                || should_suppress(&suppressed, &path)
                            {
                                continue;
                            }
                            if let Some(entry) = guard.iter_mut().find(|(p, _)| p == &path) {
                                entry.1 = Instant::now();
                            } else {
                                guard.push((path, Instant::now()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| e.to_string())?;

        watcher
            .watch(Path::new(&watch_path), RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;

        Ok(SyncEngine {
            _watcher: watcher,
            stop,
        })
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn sync_startup(
    cfg: &Config,
    password: &str,
    manifest: &Arc<Mutex<SyncManifest>>,
    had_local_manifest: bool,
    remote_manifest: Option<&SyncManifest>,
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
) -> UploadBatchResult {
    let mut total = UploadBatchResult::default();
    let local_state = scan_local_state(cfg);
    log(format!(
        "Startup scan of {}: {} file(s)",
        cfg.watch_folder,
        local_state.files.len()
    ));
    let remote_manifest = remote_manifest.cloned().unwrap_or_default();

    if !had_local_manifest {
        if cfg.sync_remote_changes && !remote_manifest.files.is_empty() {
            log("No local manifest, downloading server files as baseline".to_string());
            let mut downloads: Vec<String> = remote_manifest
                .files
                .keys()
                .filter(|relative| *relative != MANIFEST_NAME)
                .cloned()
                .collect();
            downloads.sort();

            if !downloads.is_empty() {
                log(format!("{} file(s) to download", downloads.len()));
                activity(ActivityInfo {
                    state: ActivityState::Syncing,
                    completed: 0,
                    total: downloads.len(),
                    failed: 0,
                    failed_paths: Vec::new(),
                });
                download_remote_paths(
                    cfg,
                    password,
                    manifest,
                    &remote_manifest,
                    &downloads,
                    suppressed,
                    log,
                    auth_failed,
                );
            }

            save_remote_manifest_from_server(cfg, password, log, auth_failed);
            return total;
        }

        log("No local manifest, using local files as baseline".to_string());

        let mut uploads: Vec<PathBuf> = local_state
            .files
            .keys()
            .map(|relative| local_path_for_relative(cfg, relative))
            .collect();
        uploads.sort();

        if !uploads.is_empty() {
            log(format!("{} file(s) to upload", uploads.len()));
            activity(ActivityInfo {
                state: ActivityState::Syncing,
                completed: 0,
                total: uploads.len(),
                failed: 0,
                failed_paths: Vec::new(),
            });
            let batch = upload_paths_parallel(
                cfg,
                password,
                &uploads,
                manifest,
                log,
                cfg.parallel_uploads,
                Some(activity),
                auth_failed,
            );
            total.absorb(&batch);
            return total;
        }

        save_remote_manifest_from_server(cfg, password, log, auth_failed);
        return total;
    }

    let local_manifest = manifest.lock().unwrap().clone();
    let remote_on_server = remote_file_states(cfg, password, log, auth_failed);

    let mut uploads = Vec::new();
    let mut downloads = Vec::new();

    for (relative, current_local) in &local_state.files {
        let local_baseline = local_manifest.files.get(relative);
        let remote_baseline = remote_manifest.files.get(relative);

        let local_changed = local_baseline != Some(current_local);
        let remote_changed = remote_baseline != local_baseline;
        let missing_on_server = remote_on_server
            .as_ref()
            .is_some_and(|present| !present.contains_key(relative));
        let size_mismatch = remote_on_server.as_ref().and_then(|present| {
            present
                .get(relative)
                .map(|remote| remote.size != current_local.size)
        });
        let listing_unavailable = remote_on_server.is_none();

        if local_changed
            || (listing_unavailable && remote_baseline.is_none())
            || missing_on_server
            || size_mismatch == Some(true)
        {
            uploads.push(local_path_for_relative(cfg, relative));
            continue;
        }

        if cfg.sync_remote_changes && remote_changed {
            downloads.push(relative.clone());
        }
    }

    if cfg.sync_remote_changes {
        for relative in remote_manifest.files.keys() {
            if !local_state.files.contains_key(relative) {
                downloads.push(relative.clone());
            }
        }
    }

    if !downloads.is_empty() {
        log(format!("{} file(s) to download", downloads.len()));
        download_remote_paths(
            cfg,
            password,
            manifest,
            &remote_manifest,
            &downloads,
            suppressed,
            log,
            auth_failed,
        );
    }

    if !uploads.is_empty() {
        log(format!("{} file(s) to upload", uploads.len()));
        activity(ActivityInfo {
            state: ActivityState::Syncing,
            completed: 0,
            total: uploads.len(),
            failed: 0,
            failed_paths: Vec::new(),
        });
        let batch = upload_paths_parallel(
            cfg,
            password,
            &uploads,
            manifest,
            log,
            cfg.parallel_uploads,
            Some(activity),
            auth_failed,
        );
        total.absorb(&batch);
    }

    save_remote_manifest_from_server(cfg, password, log, auth_failed);
    total
}

fn apply_remote_manifest(
    cfg: &Config,
    password: &str,
    manifest: &Arc<Mutex<SyncManifest>>,
    remote_manifest: &SyncManifest,
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> usize {
    let local_manifest = manifest.lock().unwrap().clone();
    let mut downloads = Vec::new();

    for (relative, remote_state) in &remote_manifest.files {
        let local_baseline = local_manifest.files.get(relative);
        if local_baseline != Some(remote_state) {
            downloads.push(relative.clone());
        }
    }

    if downloads.is_empty() {
        return 0;
    }
    let download_count = downloads.len();

    download_remote_paths(
        cfg,
        password,
        manifest,
        remote_manifest,
        &downloads,
        suppressed,
        log,
        auth_failed,
    );

    save_remote_manifest_from_server(cfg, password, log, auth_failed);
    download_count
}

fn download_remote_paths(
    cfg: &Config,
    password: &str,
    manifest: &Arc<Mutex<SyncManifest>>,
    remote_manifest: &SyncManifest,
    paths: &[String],
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) {
    for relative in paths {
        if relative == MANIFEST_NAME {
            continue;
        }
        if !remote_manifest.files.contains_key(relative) {
            continue;
        }

        let local_path = local_path_for_relative(cfg, relative);
        let remote_url = remote_file_url(cfg, relative);
        log(format!("Downloading: {}", relative));
        let remote_data = match webdav::get_file(cfg, password, &remote_url) {
            Ok(data) => data,
            Err(err) => {
                if err.is_auth_failed() {
                    auth_failed();
                }
                log(format!("Remote download failed {}: {}", relative, err));
                continue;
            }
        };

        if let Some(parent) = local_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        mark_suppressed(suppressed, &local_path);
        if let Err(err) = fs::write(&local_path, &remote_data) {
            log(format!("Local write failed {}: {}", relative, err));
            continue;
        }

        mark_suppressed(suppressed, &local_path);
        let mut guard = manifest.lock().unwrap();
        guard.files.insert(
            relative.clone(),
            FileState {
                size: file_size(&local_path),
                mtime: file_mtime_epoch(&local_path),
            },
        );
        save_local_manifest(cfg, &guard);
        log(format!("Downloaded: {}", relative));
    }
}

fn upload_path(
    cfg: &Config,
    password: &str,
    path: &PathBuf,
    manifest: &Arc<Mutex<SyncManifest>>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> UploadOutcome {
    if !path.is_file() || should_ignore_path(&cfg.watch_folder, path) {
        return UploadOutcome::Skipped;
    }

    let Some(relative) = relative_path_for_watch(&cfg.watch_folder, path) else {
        return UploadOutcome::Skipped;
    };

    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            let msg = format!("Read error: {err}");
            log(format!("Upload failed {}: {}", relative, msg));
            return UploadOutcome::Failed(relative);
        }
    };
    let size = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(err) => {
            let msg = format!("Read error: {err}");
            log(format!("Upload failed {}: {}", relative, msg));
            return UploadOutcome::Failed(relative);
        }
    };

    let remote_url = remote_file_url(cfg, &relative);

    if let Some(parent) = parent_folder_url(&remote_url) {
        if let Err(err) = ensure_remote_dirs(
            cfg,
            password,
            remote_base_url(cfg).trim_end_matches('/'),
            &parent,
        ) {
            if err.is_auth_failed() {
                auth_failed();
                let msg = err.to_string();
                log(format!("Upload failed {}: {}", relative, msg));
                return UploadOutcome::Failed(relative);
            }
            log(format!("Create folder failed {}: {}", relative, err));
        }
    }

    log(format!("Uploading: {}", relative));
    log(format!("Upload progress: {}|0", relative));
    match webdav::put_file(cfg, password, &remote_url, file, size) {
        Ok(_) => {
            log(format!("Upload progress: {}|100", relative));
            let mtime = file_mtime_epoch(path);
            if let Err(err) = webdav::set_sar_last_modified(cfg, password, &remote_url, mtime) {
                log(format!("Timestamp preserve failed {}: {}", relative, err));
            }
            let mut guard = manifest.lock().unwrap();
            guard.files.insert(
                relative.clone(),
                FileState {
                    size: file_size(path),
                    mtime,
                },
            );
            save_local_manifest(cfg, &guard);
            log(format!("Uploaded: {}", relative));
            UploadOutcome::Success
        }
        Err(err) => {
            if err.is_auth_failed() {
                auth_failed();
            }
            let msg = err.to_string();
            log(format!("Upload failed {}: {}", relative, msg));
            UploadOutcome::Failed(relative)
        }
    }
}

fn upload_paths_parallel(
    cfg: &Config,
    password: &str,
    paths: &[PathBuf],
    manifest: &Arc<Mutex<SyncManifest>>,
    log: &LogFn,
    max_parallel: usize,
    activity: Option<&ActivityFn>,
    auth_failed: &AuthFailedFn,
) -> UploadBatchResult {
    let total = paths.len();
    if total == 0 {
        return UploadBatchResult::default();
    }

    // Small files keep the configured fan-out; large ones go one-by-one so a
    // single big PUT cannot starve or time out under parallel_uploads=10.
    let mut small = Vec::new();
    let mut large = Vec::new();
    for path in paths {
        if file_size(path) >= LARGE_UPLOAD_BYTES {
            large.push(path.clone());
        } else {
            small.push(path.clone());
        }
    }
    if !large.is_empty() {
        log(format!(
            "Upload schedule: {} small (up to {} parallel), {} large (>=50 MB, serial)",
            small.len(),
            max_parallel.max(1),
            large.len()
        ));
    }

    let processed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let succeeded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_paths = Arc::new(Mutex::new(Vec::<String>::new()));

    run_upload_queue(
        cfg,
        password,
        &small,
        manifest,
        log,
        max_parallel,
        activity,
        auth_failed,
        total,
        &processed,
        &succeeded,
        &failed_paths,
    );
    run_upload_queue(
        cfg,
        password,
        &large,
        manifest,
        log,
        1,
        activity,
        auth_failed,
        total,
        &processed,
        &succeeded,
        &failed_paths,
    );

    save_remote_manifest_from_server(cfg, password, log, auth_failed);
    let ok = succeeded.load(std::sync::atomic::Ordering::SeqCst);
    let paths_failed = failed_paths.lock().unwrap().clone();
    UploadBatchResult {
        attempted: total,
        succeeded: ok,
        failed: paths_failed.len(),
        failed_paths: paths_failed,
    }
}

fn run_upload_queue(
    cfg: &Config,
    password: &str,
    paths: &[PathBuf],
    manifest: &Arc<Mutex<SyncManifest>>,
    log: &LogFn,
    max_parallel: usize,
    activity: Option<&ActivityFn>,
    auth_failed: &AuthFailedFn,
    total: usize,
    processed: &Arc<std::sync::atomic::AtomicUsize>,
    succeeded: &Arc<std::sync::atomic::AtomicUsize>,
    failed_paths: &Arc<Mutex<Vec<String>>>,
) {
    if paths.is_empty() {
        return;
    }
    let width = max_parallel.max(1).min(paths.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(paths.to_vec())));
    std::thread::scope(|scope| {
        for _ in 0..width {
            let queue = queue.clone();
            let processed = processed.clone();
            let succeeded = succeeded.clone();
            let failed_paths = failed_paths.clone();
            scope.spawn(move || loop {
                let Some(path) = queue.lock().unwrap().pop_front() else {
                    break;
                };
                match upload_path(cfg, password, &path, manifest, log, auth_failed) {
                    UploadOutcome::Success => {
                        succeeded.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    UploadOutcome::Failed(relative) => {
                        failed_paths.lock().unwrap().push(relative);
                    }
                    UploadOutcome::Skipped => {}
                }
                let done = processed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if let Some(activity) = activity {
                    activity(ActivityInfo {
                        state: ActivityState::Syncing,
                        completed: done,
                        total,
                        failed: 0,
                        failed_paths: Vec::new(),
                    });
                }
            });
        }
    });
}

/// Re-upload specific paths under `watch_folder` (relative paths as logged by sync).
pub fn retry_uploads(
    cfg: &Config,
    password: &str,
    relative_paths: &[String],
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
) -> UploadBatchResult {
    let watch = Path::new(cfg.watch_folder.trim());
    if watch.as_os_str().is_empty() {
        return UploadBatchResult::default();
    }
    let manifest = Arc::new(Mutex::new(load_local_manifest(cfg)));
    let paths: Vec<PathBuf> = relative_paths
        .iter()
        .map(|rel| watch.join(rel))
        .filter(|p| p.is_file())
        .collect();
    upload_paths_parallel(
        cfg,
        password,
        &paths,
        &manifest,
        log,
        cfg.parallel_uploads.max(1),
        Some(activity),
        auth_failed,
    )
}

pub fn refresh_remote_changes(
    cfg: &Config,
    password: &str,
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
) -> usize {
    let mut pull_cfg = cfg.clone();
    pull_cfg.sync_remote_changes = true;
    let manifest = Arc::new(Mutex::new(load_local_manifest(&pull_cfg)));
    let suppressed: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));

    activity(ActivityInfo {
        state: ActivityState::Checking,
        completed: 0,
        total: 0,
        failed: 0,
        failed_paths: Vec::new(),
    });
    log("Manual server refresh started".to_string());

    let downloaded = fetch_remote_state(&pull_cfg, password, log, auth_failed)
        .map(|remote_manifest| {
            apply_remote_manifest(
                &pull_cfg,
                password,
                &manifest,
                &remote_manifest,
                &suppressed,
                log,
                auth_failed,
            )
        })
        .unwrap_or(0);

    if downloaded == 0 {
        log("Manual server refresh complete: no server changes found".to_string());
    } else {
        log(format!(
            "Manual server refresh complete: {} file(s) pulled from server",
            downloaded
        ));
    }

    activity(ActivityInfo {
        state: ActivityState::Idle,
        completed: downloaded,
        total: downloaded,
        failed: 0,
        failed_paths: Vec::new(),
    });
    downloaded
}

fn fetch_remote_manifest(
    cfg: &Config,
    password: &str,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let remote_url = remote_manifest_url(cfg);
    match webdav::get_file(cfg, password, &remote_url) {
        Ok(data) => Some(serde_json::from_slice(&data).unwrap_or_default()),
        Err(err) => {
            if err.is_auth_failed() {
                auth_failed();
            }
            log(format!("Remote manifest unavailable: {}", err));
            None
        }
    }
}

fn fetch_remote_manifest_marker(
    cfg: &Config,
    password: &str,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let remote_url = remote_manifest_url(cfg);
    match webdav::get_file(cfg, password, &remote_url) {
        Ok(data) => Some(serde_json::from_slice(&data).unwrap_or_default()),
        Err(err) => {
            if err.is_auth_failed() {
                auth_failed();
            }
            None
        }
    }
}

fn remote_file_states(
    cfg: &Config,
    password: &str,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<HashMap<String, FileState>> {
    match webdav::list_files_recursive(cfg, password, &remote_base_url(cfg)) {
        Ok(files) => Some(
            files
                .into_iter()
                .filter(|file| file.relative_path != MANIFEST_NAME)
                .map(|file| {
                    (
                        file.relative_path,
                        FileState {
                            size: file.size,
                            mtime: file.mtime,
                        },
                    )
                })
                .collect(),
        ),
        Err(err) => {
            if err.is_auth_failed() {
                auth_failed();
            }
            log(format!("Remote file listing unavailable: {}", err));
            None
        }
    }
}

fn manifest_from_server_listing(
    cfg: &Config,
    password: &str,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let files = remote_file_states(cfg, password, log, auth_failed)?;
    Some(SyncManifest { files })
}

fn save_remote_manifest_from_server(
    cfg: &Config,
    password: &str,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) {
    if let Some(manifest) = manifest_from_server_listing(cfg, password, log, auth_failed) {
        save_remote_manifest(cfg, password, &manifest, log, auth_failed);
    }
}

fn heal_missing_uploads(
    cfg: &Config,
    password: &str,
    manifest: &Arc<Mutex<SyncManifest>>,
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
) -> UploadBatchResult {
    let Some(remote_on_server) = remote_file_states(cfg, password, log, auth_failed) else {
        return UploadBatchResult::default();
    };

    let local_state = scan_local_state(cfg);
    let mut uploads = Vec::new();
    for (relative, current_local) in &local_state.files {
        let needs_upload = match remote_on_server.get(relative) {
            None => true,
            Some(remote) => remote.size != current_local.size,
        };
        if needs_upload {
            uploads.push(local_path_for_relative(cfg, relative));
        }
    }

    if uploads.is_empty() {
        return UploadBatchResult::default();
    }

    log(format!(
        "{} file(s) missing or mismatched on server, re-uploading",
        uploads.len()
    ));
    activity(ActivityInfo {
        state: ActivityState::Syncing,
        completed: 0,
        total: uploads.len(),
        failed: 0,
        failed_paths: Vec::new(),
    });
    upload_paths_parallel(
        cfg,
        password,
        &uploads,
        manifest,
        log,
        cfg.parallel_uploads,
        Some(activity),
        auth_failed,
    )
}

fn fetch_remote_state(
    cfg: &Config,
    password: &str,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let mut manifest = fetch_remote_manifest(cfg, password, log, auth_failed).unwrap_or_default();

    if cfg.sync_remote_changes {
        match webdav::list_files_recursive(cfg, password, &remote_base_url(cfg)) {
            Ok(files) => {
                let mut discovered = 0usize;
                for file in files {
                    if file.relative_path == MANIFEST_NAME {
                        continue;
                    }
                    if !manifest.files.contains_key(&file.relative_path) {
                        manifest.files.insert(
                            file.relative_path,
                            FileState {
                                size: file.size,
                                mtime: file.mtime,
                            },
                        );
                        discovered += 1;
                    }
                }
                if discovered > 0 {
                    log(format!("Discovered {} server file(s)", discovered));
                }
            }
            Err(err) => {
                if err.is_auth_failed() {
                    auth_failed();
                }
                log(format!("Server folder scan unavailable: {}", err));
            }
        }
    }

    Some(manifest)
}

fn save_remote_manifest(
    cfg: &Config,
    password: &str,
    manifest: &SyncManifest,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) {
    let data = match serde_json::to_vec_pretty(manifest) {
        Ok(data) => data,
        Err(err) => {
            log(format!("Manifest serialise failed: {}", err));
            return;
        }
    };

    let remote_url = remote_manifest_url(cfg);
    if let Some(parent) = parent_folder_url(&remote_url) {
        if let Err(err) = ensure_remote_dirs(
            cfg,
            password,
            remote_base_url(cfg).trim_end_matches('/'),
            &parent,
        ) {
            if err.is_auth_failed() {
                auth_failed();
            }
            log(format!("Manifest folder create failed: {}", err));
            return;
        }
    }

    let reader = Cursor::new(data.clone());
    if let Err(err) = webdav::put_file(cfg, password, &remote_url, reader, data.len() as u64) {
        if err.is_auth_failed() {
            auth_failed();
        }
        log(format!("Manifest upload failed: {}", err));
    }
}

fn scan_local_state(cfg: &Config) -> SyncManifest {
    let mut files = HashMap::new();
    for path in collect_local_files(&cfg.watch_folder) {
        let Some(relative) = relative_path_for_watch(&cfg.watch_folder, &path) else {
            continue;
        };
        files.insert(
            relative,
            FileState {
                size: file_size(&path),
                mtime: file_mtime_epoch(&path),
            },
        );
    }
    SyncManifest { files }
}

fn collect_local_files(root: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_local_files_recursive(PathBuf::from(root), &mut files);
    files
}

fn collect_local_files_recursive(path: PathBuf, files: &mut Vec<PathBuf>) {
    if is_ignored_dir_name(path.file_name()) {
        return;
    }
    let Ok(meta) = fs::metadata(&path) else {
        return;
    };
    if meta.is_file() {
        if !is_manifest_path(&path) && !path_contains_ignored_dir(&path) {
            files.push(path);
        }
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(&path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_local_files_recursive(entry.path(), files);
    }
}

fn relative_path_for_watch(watch_folder: &str, path: &Path) -> Option<String> {
    path.strip_prefix(watch_folder)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty() && p != MANIFEST_NAME)
}

fn local_path_for_relative(cfg: &Config, relative: &str) -> PathBuf {
    PathBuf::from(&cfg.watch_folder).join(relative.replace('/', "\\"))
}

fn load_local_manifest(cfg: &Config) -> SyncManifest {
    match fs::read_to_string(local_manifest_path(cfg)) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => SyncManifest::default(),
    }
}

fn save_local_manifest(cfg: &Config, manifest: &SyncManifest) {
    if let Ok(data) = serde_json::to_string_pretty(manifest) {
        let _ = fs::write(local_manifest_path(cfg), data);
    }
}

fn local_manifest_path(cfg: &Config) -> PathBuf {
    PathBuf::from(&cfg.watch_folder).join(MANIFEST_NAME)
}

fn has_local_manifest(cfg: &Config) -> bool {
    local_manifest_path(cfg).is_file()
}

fn remote_manifest_url(cfg: &Config) -> String {
    format!(
        "{}/{}",
        remote_base_url(cfg).trim_end_matches('/'),
        MANIFEST_NAME
    )
}

fn remote_file_url(cfg: &Config, relative: &str) -> String {
    format!(
        "{}/{}",
        remote_base_url(cfg).trim_end_matches('/'),
        relative
    )
}

fn remote_base_url(cfg: &Config) -> String {
    format!(
        "{}/{}/",
        cfg.webdav_url.trim_end_matches('/'),
        cfg.remote_folder.trim_matches('/')
    )
}

fn parent_folder_url(remote_url: &str) -> Option<String> {
    let trimmed = remote_url.trim_end_matches('/');
    let (base, _) = trimmed.rsplit_once('/')?;
    if base.is_empty() {
        return None;
    }
    Some(base.to_string())
}

fn ensure_remote_dirs(
    cfg: &Config,
    password: &str,
    remote_base: &str,
    folder_url: &str,
) -> Result<(), crate::webdav::WebDavError> {
    let base = remote_base.trim_end_matches('/');
    let folder = folder_url.trim_end_matches('/');
    let relative = folder
        .strip_prefix(base)
        .unwrap_or(folder)
        .trim_matches('/');
    if relative.is_empty() {
        return Ok(());
    }
    let mut current = base.to_string();
    for segment in relative.split('/') {
        if segment.is_empty() {
            continue;
        }
        current.push('/');
        current.push_str(segment);
        current.push('/');
        webdav::mkcol(cfg, password, &current)?;
    }
    Ok(())
}

fn should_ignore_path(watch_folder: &str, path: &Path) -> bool {
    if is_manifest_path(path) || path_contains_ignored_dir(path) {
        return true;
    }
    if watch_folder.is_empty() {
        return false;
    }
    relative_path_for_watch(watch_folder, path)
        .map(|relative| relative == MANIFEST_NAME || relative_contains_ignored_dir(&relative))
        .unwrap_or(false)
}

fn path_contains_ignored_dir(path: &Path) -> bool {
    path.components().any(|component| match component {
        std::path::Component::Normal(name) => is_ignored_dir_name(Some(name)),
        _ => false,
    })
}

fn relative_contains_ignored_dir(relative: &str) -> bool {
    relative
        .split(['/', '\\'])
        .any(|segment| segment.eq_ignore_ascii_case(IGNORED_DIR_NAME))
}

fn is_ignored_dir_name(name: Option<&std::ffi::OsStr>) -> bool {
    name.and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(IGNORED_DIR_NAME))
}

fn is_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == MANIFEST_NAME)
        .unwrap_or(false)
}

fn file_mtime_epoch(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn mark_suppressed(suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>, path: &Path) {
    if let Ok(mut guard) = suppressed.lock() {
        guard.push((path.to_path_buf(), Instant::now() + Duration::from_secs(3)));
        guard.retain(|(_, until)| *until > Instant::now());
    }
}

fn should_suppress(suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>, path: &PathBuf) -> bool {
    let Ok(mut guard) = suppressed.lock() else {
        return false;
    };
    let now = Instant::now();
    guard.retain(|(_, until)| *until > now);
    guard.iter().any(|(pending_path, _)| pending_path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_tmp_driveupload_paths() {
        assert!(relative_contains_ignored_dir("foo/.tmp.driveupload/bar.bin"));
        assert!(relative_contains_ignored_dir(".tmp.driveupload/x"));
        assert!(path_contains_ignored_dir(Path::new(
            r"C:\XDSoftware\backups\.tmp.driveupload\file.bin"
        )));
        assert!(!relative_contains_ignored_dir("foo/bar.bin"));
        assert!(!should_ignore_path(
            r"C:\XDSoftware\backups",
            Path::new(r"C:\XDSoftware\backups\invoice.pdf")
        ));
        assert!(should_ignore_path(
            r"C:\XDSoftware\backups",
            Path::new(r"C:\XDSoftware\backups\.tmp.driveupload\tmp.bin")
        ));
    }
}
