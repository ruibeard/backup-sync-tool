// sync.rs — watch local folder and sync via BackupTransport.
// Startup scans the local folder and fetches one remote manifest file.

use crate::config::{self, Config};
use crate::transport::{BackupTransport, FileMetadata, TransportError};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REMOTE_HEAL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MANIFEST_NAME: &str = ".backupsynctool-manifest.json";
const MANIFEST_VERSION: u32 = 2;
const REMOTE_MANIFEST_NAME_S3: &str = ".backupsynctool-remote-manifest.json";

fn remote_manifest_name(_cfg: &Config) -> &'static str {
    REMOTE_MANIFEST_NAME_S3
}

fn is_remote_manifest_name(cfg: &Config, relative: &str) -> bool {
    relative == remote_manifest_name(cfg)
        || relative == MANIFEST_NAME
        || relative == REMOTE_MANIFEST_NAME_S3
}

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
        transport: Arc<dyn BackupTransport>,
        log: LogFn,
        activity: ActivityFn,
        auth_failed: AuthFailedFn,
    ) -> Result<Self, String> {
        let pending: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let pending_clone = pending.clone();
        let stop_clone = stop.clone();
        let cfg_arc = Arc::new(cfg);
        let transport_clone = transport.clone();
        let log_clone = log.clone();
        let activity_clone = activity.clone();
        let auth_failed_clone = auth_failed.clone();
        let cfg_watcher = cfg_arc.clone();
        let transport_watcher = transport.clone();

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

            let startup_batch = sync_startup(
                &cfg_watcher,
                &transport_watcher,
                &manifest,
                had_local_manifest,
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
                        &transport_clone,
                        &due,
                        &manifest,
                        &log_clone,
                        config::effective_parallel_uploads(&cfg_watcher),
                        Some(&activity_clone),
                        &auth_failed_clone,
                    );
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
                        &transport_clone,
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
                            if should_ignore_path(&watch_path_for_events, &path) {
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
    transport: &Arc<dyn BackupTransport>,
    manifest: &Arc<Mutex<SyncManifest>>,
    had_local_manifest: bool,
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
    let remote_manifest = SyncManifest::default();
    let suppressed: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));

    if !had_local_manifest {
        if cfg.sync_remote_changes && !remote_manifest.files.is_empty() {
            log("No local manifest, downloading server files as baseline".to_string());
            let mut downloads: Vec<String> = remote_manifest
                .files
                .keys()
                .filter(|relative| !is_remote_manifest_name(cfg, relative))
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
                    transport,
                    manifest,
                    &remote_manifest,
                    &downloads,
                    &suppressed,
                    log,
                    auth_failed,
                );
            }

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
                transport,
                &uploads,
                manifest,
                log,
                config::effective_parallel_uploads(cfg),
                Some(activity),
                auth_failed,
            );
            total.absorb(&batch);
            return total;
        }

        return total;
    }

    let local_manifest = manifest.lock().unwrap().clone();
    let remote_on_server = remote_file_states(cfg, transport, log, auth_failed);

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
            transport,
            manifest,
            &remote_manifest,
            &downloads,
            &suppressed,
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
            transport,
            &uploads,
            manifest,
            log,
            config::effective_parallel_uploads(cfg),
            Some(activity),
            auth_failed,
        );
        total.absorb(&batch);
    }

    total
}

fn apply_remote_manifest(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
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
        transport,
        manifest,
        remote_manifest,
        &downloads,
        suppressed,
        log,
        auth_failed,
    );

    download_count
}

fn download_remote_paths(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    manifest: &Arc<Mutex<SyncManifest>>,
    remote_manifest: &SyncManifest,
    paths: &[String],
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) {
    for relative in paths {
        if is_remote_manifest_name(cfg, relative) {
            continue;
        }
        if !is_safe_remote_relative(relative) {
            log(format!("Rejected unsafe server path: {}", relative));
            continue;
        }
        if !remote_manifest.files.contains_key(relative) {
            continue;
        }

        let local_path = local_path_for_relative(cfg, relative);
        log(format!("Downloading: {}", relative));
        mark_suppressed(suppressed, &local_path);
        let meta = match transport.download_file(relative, &local_path) {
            Ok(meta) => meta,
            Err(err) => {
                handle_transport_err(&err, auth_failed);
                log(format!("Remote download failed {}: {}", relative, err));
                continue;
            }
        };

        // Prefer source mtime from remote manifest when transport did not supply one.
        let mtime = if meta.mtime > 0 {
            meta.mtime
        } else {
            remote_manifest
                .files
                .get(relative)
                .map(|s| s.mtime)
                .unwrap_or(0)
        };
        if mtime > 0 {
            let _ = set_local_mtime(&local_path, mtime);
        }

        mark_suppressed(suppressed, &local_path);
        let mut guard = manifest.lock().unwrap();
        guard.files.insert(
            relative.clone(),
            FileState {
                size: if meta.size > 0 {
                    meta.size
                } else {
                    file_size(&local_path)
                },
                mtime: if mtime > 0 {
                    mtime
                } else {
                    file_mtime_epoch(&local_path)
                },
            },
        );
        save_local_manifest(cfg, &guard);
        log(format!("Downloaded: {}", relative));
    }
}

fn upload_path(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
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

    let size = match fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(err) => {
            let msg = format!("Read error: {err}");
            log(format!("Upload failed {}: {}", relative, msg));
            return UploadOutcome::Failed(relative);
        }
    };
    let mtime = file_mtime_epoch(path);
    let metadata = FileMetadata { size, mtime };

    log(format!("Uploading: {}", relative));
    log(format!("Upload progress: {}|0", relative));
    match transport.upload_file(&relative, path, &metadata) {
        Ok(_) => {
            log(format!("Upload progress: {}|100", relative));
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
            handle_transport_err(&err, auth_failed);
            let msg = err.to_string();
            log(format!("Upload failed {}: {}", relative, msg));
            UploadOutcome::Failed(relative)
        }
    }
}

fn upload_paths_parallel(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
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
    let width = max_parallel.max(1).min(total);
    let processed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let succeeded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_paths = Arc::new(Mutex::new(Vec::<String>::new()));
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
                match upload_path(cfg, transport, &path, manifest, log, auth_failed) {
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
    let ok = succeeded.load(std::sync::atomic::Ordering::SeqCst);
    let paths_failed = failed_paths.lock().unwrap().clone();
    UploadBatchResult {
        attempted: total,
        succeeded: ok,
        failed: paths_failed.len(),
        failed_paths: paths_failed,
    }
}

/// Re-upload specific paths under `watch_folder` (relative paths as logged by sync).
pub fn retry_uploads(
    cfg: &Config,
    transport: Arc<dyn BackupTransport>,
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
        &transport,
        &paths,
        &manifest,
        log,
        config::effective_parallel_uploads(cfg),
        Some(activity),
        auth_failed,
    )
}

pub fn restore_customer_backup(
    cfg: &Config,
    transport: Arc<dyn BackupTransport>,
    destination_parent: &Path,
    cancel: &Arc<AtomicBool>,
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
) -> Result<PathBuf, String> {
    if !destination_parent.is_dir() {
        return Err("Restore destination does not exist.".into());
    }

    let customer = safe_restore_directory_name(&cfg.remote_folder);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let restore_root = destination_parent.join(format!("{customer}-restore-{timestamp}"));
    fs::create_dir(&restore_root)
        .map_err(|err| format!("Could not create restore folder: {err}"))?;

    activity(ActivityInfo {
        state: ActivityState::Checking,
        completed: 0,
        total: 0,
        failed: 0,
        failed_paths: Vec::new(),
    });
    log(format!("Restore started: {}", restore_root.display()));

    let files = transport.list_files().map_err(|err| {
        if err.is_auth_failed() {
            auth_failed();
        }
        format!("Could not list customer backup: {err}")
    })?;
    let total = files.len();
    let mut completed = 0usize;
    let mut failed_paths = Vec::new();

    for remote in files {
        if cancel.load(Ordering::Relaxed) {
            return Err(format!(
                "Restore cancelled. Partial files remain in {}",
                restore_root.display()
            ));
        }
        let relative = safe_restore_relative_path(&remote.relative_path)
            .ok_or_else(|| format!("Unsafe server path: {}", remote.relative_path))?;
        let destination = restore_root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        match transport.download_file(&remote.relative_path, &destination) {
            Ok(_) => completed += 1,
            Err(err) => {
                if err.is_auth_failed() {
                    auth_failed();
                }
                failed_paths.push(remote.relative_path);
            }
        }
        activity(ActivityInfo {
            state: ActivityState::Syncing,
            completed,
            total,
            failed: failed_paths.len(),
            failed_paths: failed_paths.clone(),
        });
    }

    activity(ActivityInfo {
        state: ActivityState::Idle,
        completed,
        total,
        failed: failed_paths.len(),
        failed_paths: failed_paths.clone(),
    });

    if failed_paths.is_empty() {
        log(format!("Restore complete: {} file(s)", completed));
        Ok(restore_root)
    } else {
        Err(format!(
            "Restore completed with {} failed file(s) in {}",
            failed_paths.len(),
            restore_root.display()
        ))
    }
}

fn safe_restore_directory_name(value: &str) -> String {
    let name = value
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ if character.is_control() => '-',
            _ => character,
        })
        .collect::<String>();
    let trimmed = name.trim().trim_matches('.');
    if trimmed.is_empty() {
        "customer".to_string()
    } else {
        trimmed.to_string()
    }
}

fn safe_restore_relative_path(value: &str) -> Option<PathBuf> {
    let normalized = value.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains('\0') {
        return None;
    }
    let path = Path::new(&normalized);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let result = path
        .components()
        .fold(PathBuf::new(), |mut result, component| {
            if let Component::Normal(value) = component {
                result.push(value);
            }
            result
        });
    (!result.as_os_str().is_empty()).then_some(result)
}

fn fetch_remote_manifest(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let name = remote_manifest_name(cfg);
    let temp = std::env::temp_dir().join(format!(
        "bst-remote-manifest-{}-{}.json",
        std::process::id(),
        unix_now_nanos()
    ));
    match transport.download_file(name, &temp) {
        Ok(_) => {
            let data = fs::read(&temp).ok();
            let _ = fs::remove_file(&temp);
            data.and_then(|bytes| serde_json::from_slice(&bytes).ok())
        }
        Err(TransportError::NotFound) => {
            let _ = fs::remove_file(&temp);
            None
        }
        Err(err) => {
            let _ = fs::remove_file(&temp);
            handle_transport_err(&err, auth_failed);
            log(format!("Remote manifest unavailable: {}", err));
            None
        }
    }
}

fn fetch_remote_manifest_marker(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let name = remote_manifest_name(cfg);
    let temp = std::env::temp_dir().join(format!(
        "bst-remote-marker-{}-{}.json",
        std::process::id(),
        unix_now_nanos()
    ));
    match transport.download_file(name, &temp) {
        Ok(_) => {
            let data = fs::read(&temp).ok();
            let _ = fs::remove_file(&temp);
            data.and_then(|bytes| serde_json::from_slice(&bytes).ok())
        }
        Err(TransportError::NotFound) => {
            let _ = fs::remove_file(&temp);
            None
        }
        Err(err) => {
            let _ = fs::remove_file(&temp);
            handle_transport_err(&err, auth_failed);
            None
        }
    }
}

fn remote_file_states(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<HashMap<String, FileState>> {
    match transport.list_files() {
        Ok(files) => Some(
            files
                .into_iter()
                .filter(|file| {
                    !is_remote_manifest_name(cfg, &file.relative_path)
                        && is_safe_remote_relative(&file.relative_path)
                })
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
            handle_transport_err(&err, auth_failed);
            log(format!("Remote file listing unavailable: {}", err));
            None
        }
    }
}

fn manifest_from_server_listing(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let mut files = remote_file_states(cfg, transport, log, auth_failed)?;
    let local = load_local_manifest(cfg);
    let previous = fetch_remote_manifest(cfg, transport, log, auth_failed).unwrap_or_default();

    for (relative, state) in &mut files {
        state.mtime = previous
            .files
            .get(relative)
            .filter(|candidate| candidate.size == state.size)
            .or_else(|| {
                local
                    .files
                    .get(relative)
                    .filter(|candidate| candidate.size == state.size)
            })
            .map(|candidate| candidate.mtime)
            .unwrap_or(state.mtime);
    }
    Some(SyncManifest { files })
}

fn save_remote_manifest_from_server(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) {
    if let Some(manifest) = manifest_from_server_listing(cfg, transport, log, auth_failed) {
        save_remote_manifest(cfg, transport, &manifest, log, auth_failed);
    }
}

fn heal_missing_uploads(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    manifest: &Arc<Mutex<SyncManifest>>,
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
) -> UploadBatchResult {
    let Some(remote_on_server) = remote_file_states(cfg, transport, log, auth_failed) else {
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
        transport,
        &uploads,
        manifest,
        log,
        config::effective_parallel_uploads(cfg),
        Some(activity),
        auth_failed,
    )
}

fn fetch_remote_state(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
) -> Option<SyncManifest> {
    let mut manifest = fetch_remote_manifest(cfg, transport, log, auth_failed).unwrap_or_default();

    if cfg.sync_remote_changes {
        match transport.list_files() {
            Ok(files) => {
                let mut discovered = 0usize;
                for file in files {
                    if is_remote_manifest_name(cfg, &file.relative_path) {
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
                handle_transport_err(&err, auth_failed);
                log(format!("Server folder scan unavailable: {}", err));
            }
        }
    }

    Some(manifest)
}

fn save_remote_manifest(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
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

    let temp = std::env::temp_dir().join(format!(
        "bst-upload-manifest-{}-{}.json",
        std::process::id(),
        unix_now_nanos()
    ));
    if let Err(err) = fs::write(&temp, &data) {
        log(format!("Manifest temp write failed: {}", err));
        return;
    }

    let metadata = FileMetadata {
        size: data.len() as u64,
        mtime: unix_now(),
    };
    if let Err(err) = transport.upload_file(remote_manifest_name(cfg), &temp, &metadata) {
        handle_transport_err(&err, auth_failed);
        log(format!("Manifest upload failed: {}", err));
    }
    let _ = fs::remove_file(&temp);
}

fn handle_transport_err(err: &TransportError, auth_failed: &AuthFailedFn) {
    if err.is_auth_failed() {
        auth_failed();
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
    let Ok(meta) = fs::metadata(&path) else {
        return;
    };
    if meta.is_file() {
        if !is_manifest_path(&path) {
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
        .filter(|p| !p.is_empty() && p != MANIFEST_NAME && p != REMOTE_MANIFEST_NAME_S3)
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
        let path = local_manifest_path(cfg);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary = path.with_extension("tmp");
        if fs::write(&temporary, data).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

fn local_manifest_path(cfg: &Config) -> PathBuf {
    use sha2::{Digest, Sha256};

    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("BackupSyncTool")
        .join(format!("state-v{MANIFEST_VERSION}"));
    let identity = if cfg.device_uuid.trim().is_empty() {
        format!("{}|{}", cfg.s3_endpoint, cfg.s3_bucket)
    } else {
        cfg.device_uuid.clone()
    };
    let digest = hex::encode(Sha256::digest(identity.as_bytes()));
    root.join(format!("{}.json", &digest[..32]))
}

fn has_local_manifest(cfg: &Config) -> bool {
    local_manifest_path(cfg).is_file()
}

fn should_ignore_path(watch_folder: &str, path: &Path) -> bool {
    is_manifest_path(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".backupsynctool.part"))
        || relative_path_for_watch(watch_folder, path)
            .map(|relative| relative == MANIFEST_NAME || relative == REMOTE_MANIFEST_NAME_S3)
            .unwrap_or(false)
}

fn is_safe_remote_relative(relative: &str) -> bool {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.starts_with('\\')
        || relative.contains('\\')
        || relative.contains(':')
        || relative.contains('\0')
    {
        return false;
    }

    relative
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn is_manifest_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == MANIFEST_NAME || name == REMOTE_MANIFEST_NAME_S3)
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

fn set_local_mtime(path: &Path, mtime: u64) -> std::io::Result<()> {
    use std::fs::{File, FileTimes};
    let modified = UNIX_EPOCH + Duration::from_secs(mtime);
    let file = File::options().write(true).open(path)?;
    file.set_times(FileTimes::new().set_modified(modified))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn mark_suppressed(suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>, path: &Path) {
    if let Ok(mut guard) = suppressed.lock() {
        guard.push((path.to_path_buf(), Instant::now() + Duration::from_secs(3)));
        guard.retain(|(_, until)| *until > Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::{is_safe_remote_relative, safe_restore_relative_path};

    #[test]
    fn remote_paths_cannot_escape_watch_folder() {
        assert!(is_safe_remote_relative("folder/file.zip"));
        assert!(!is_safe_remote_relative("../outside.zip"));
        assert!(!is_safe_remote_relative("folder/../outside.zip"));
        assert!(!is_safe_remote_relative("/absolute.zip"));
        assert!(!is_safe_remote_relative("C:/outside.zip"));
        assert!(!is_safe_remote_relative("folder\\outside.zip"));
    }

    #[test]
    fn restore_paths_cannot_escape_destination() {
        assert_eq!(
            safe_restore_relative_path("folder/file.zip")
                .unwrap()
                .to_string_lossy(),
            std::path::Path::new("folder/file.zip").to_string_lossy()
        );
        assert!(safe_restore_relative_path("../outside.zip").is_none());
        assert!(safe_restore_relative_path("folder/../../outside.zip").is_none());
        assert!(safe_restore_relative_path("/absolute.zip").is_none());
        assert!(safe_restore_relative_path("").is_none());
    }
}
