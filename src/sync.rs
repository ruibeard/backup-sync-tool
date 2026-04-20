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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

const REMOTE_SYNC_INTERVAL: Duration = Duration::from_secs(60);
const MANIFEST_NAME: &str = ".backupsynctool-manifest.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FileState {
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SyncManifest {
    #[serde(default)]
    files: HashMap<String, FileState>,
}

pub struct SyncEngine {
    _watcher: RecommendedWatcher,
}

pub type LogFn = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum ActivityState {
    Checking,
    Syncing,
    Idle,
}

#[derive(Clone, Copy)]
pub struct ActivityInfo {
    pub state: ActivityState,
    pub completed: usize,
    pub total: usize,
}

pub type ActivityFn = Arc<dyn Fn(ActivityInfo) + Send + Sync>;

impl SyncEngine {
    pub fn start(
        cfg: Config,
        password: String,
        log: LogFn,
        activity: ActivityFn,
    ) -> Result<Self, String> {
        let pending: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let suppressed: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_clone = pending.clone();
        let suppressed_clone = suppressed.clone();
        let cfg_arc = Arc::new(cfg);
        let pass_arc = Arc::new(password);
        let log_clone = log.clone();
        let activity_clone = activity.clone();
        let cfg_watcher = cfg_arc.clone();
        let pass_watcher = pass_arc.clone();

        std::thread::spawn(move || {
            let had_local_manifest = has_local_manifest(&cfg_watcher);
            let manifest = Arc::new(Mutex::new(load_local_manifest(&cfg_watcher)));

            activity_clone(ActivityInfo {
                state: ActivityState::Checking,
                completed: 0,
                total: 0,
            });

            let remote_manifest = fetch_remote_manifest(&cfg_watcher, &pass_watcher, &log_clone);
            sync_startup(
                &cfg_watcher,
                &pass_watcher,
                &manifest,
                had_local_manifest,
                remote_manifest.as_ref(),
                &suppressed_clone,
                &log_clone,
                &activity_clone,
            );

            activity_clone(ActivityInfo {
                state: ActivityState::Idle,
                completed: 0,
                total: 0,
            });

            let mut last_remote_sync = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(500));
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
                    });
                    let completed = upload_paths_parallel(
                        &cfg_watcher,
                        &pass_watcher,
                        &due,
                        &manifest,
                        &log_clone,
                        cfg_watcher.parallel_uploads,
                        Some(&activity_clone),
                    );
                    activity_clone(ActivityInfo {
                        state: ActivityState::Idle,
                        completed,
                        total: due.len(),
                    });
                }

                if cfg_watcher.sync_remote_changes
                    && last_remote_sync.elapsed() >= REMOTE_SYNC_INTERVAL
                {
                    if let Some(remote_manifest) =
                        fetch_remote_manifest(&cfg_watcher, &pass_watcher, &log_clone)
                    {
                        apply_remote_manifest(
                            &cfg_watcher,
                            &pass_watcher,
                            &manifest,
                            &remote_manifest,
                            &suppressed_clone,
                            &log_clone,
                        );
                    }
                    last_remote_sync = Instant::now();
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

        Ok(SyncEngine { _watcher: watcher })
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
) {
    let local_state = scan_local_state(cfg);

    if !had_local_manifest {
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
            });
            upload_paths_parallel(
                cfg,
                password,
                &uploads,
                manifest,
                log,
                cfg.parallel_uploads,
                Some(activity),
            );
            return;
        }

        {
            let mut guard = manifest.lock().unwrap();
            *guard = local_state.clone();
            save_local_manifest(cfg, &guard);
        }
        save_remote_manifest(cfg, password, &local_state, log);
        return;
    }

    let local_manifest = manifest.lock().unwrap().clone();
    let remote_manifest = remote_manifest.cloned().unwrap_or_default();

    let mut uploads = Vec::new();
    let mut downloads = Vec::new();

    for (relative, current_local) in &local_state.files {
        let local_baseline = local_manifest.files.get(relative);
        let remote_baseline = remote_manifest.files.get(relative);

        let local_changed = local_baseline != Some(current_local);
        let remote_changed = remote_baseline != local_baseline;

        if local_changed || remote_baseline.is_none() {
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
        );
    }

    if !uploads.is_empty() {
        log(format!("{} file(s) to upload", uploads.len()));
        activity(ActivityInfo {
            state: ActivityState::Syncing,
            completed: 0,
            total: uploads.len(),
        });
        upload_paths_parallel(
            cfg,
            password,
            &uploads,
            manifest,
            log,
            cfg.parallel_uploads,
            Some(activity),
        );
    }

    let refreshed = scan_local_state(cfg);
    {
        let mut guard = manifest.lock().unwrap();
        *guard = refreshed.clone();
        save_local_manifest(cfg, &guard);
    }

    save_remote_manifest(cfg, password, &refreshed, log);
}

fn apply_remote_manifest(
    cfg: &Config,
    password: &str,
    manifest: &Arc<Mutex<SyncManifest>>,
    remote_manifest: &SyncManifest,
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
) {
    let local_manifest = manifest.lock().unwrap().clone();
    let mut downloads = Vec::new();

    for (relative, remote_state) in &remote_manifest.files {
        let local_baseline = local_manifest.files.get(relative);
        if local_baseline != Some(remote_state) {
            downloads.push(relative.clone());
        }
    }

    if downloads.is_empty() {
        return;
    }

    download_remote_paths(
        cfg,
        password,
        manifest,
        remote_manifest,
        &downloads,
        suppressed,
        log,
    );

    let refreshed = scan_local_state(cfg);
    {
        let mut guard = manifest.lock().unwrap();
        *guard = refreshed;
        save_local_manifest(cfg, &guard);
    }
}

fn download_remote_paths(
    cfg: &Config,
    password: &str,
    manifest: &Arc<Mutex<SyncManifest>>,
    remote_manifest: &SyncManifest,
    paths: &[String],
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
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
        let remote_data = match webdav::get_file(cfg, password, &remote_url) {
            Ok(data) => data,
            Err(err) => {
                log(format!("Remote download failed {}: {}", relative, err));
                continue;
            }
        };

        if let Some(parent) = local_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
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
) {
    if !path.is_file() || should_ignore_path(&cfg.watch_folder, path) {
        return;
    }

    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            log(format!("Read error {}: {}", path.display(), err));
            return;
        }
    };
    let size = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(err) => {
            log(format!("Read error {}: {}", path.display(), err));
            return;
        }
    };

    let Some(relative) = relative_path_for_watch(&cfg.watch_folder, path) else {
        return;
    };
    let remote_url = remote_file_url(cfg, &relative);

    if let Some(parent) = parent_folder_url(&remote_url) {
        if let Err(err) =
            ensure_remote_dirs(cfg, password, cfg.webdav_url.trim_end_matches('/'), &parent)
        {
            log(format!("Create folder failed {}: {}", relative, err));
            return;
        }
    }

    match webdav::put_file(cfg, password, &remote_url, file, size) {
        Ok(_) => {
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
        }
        Err(err) => log(format!("Upload failed {}: {}", relative, err)),
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
) -> usize {
    let width = max_parallel.max(1).min(paths.len().max(1));
    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let total = paths.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(paths.to_vec())));
    std::thread::scope(|scope| {
        for _ in 0..width {
            let queue = queue.clone();
            let completed = completed.clone();
            scope.spawn(move || loop {
                let Some(path) = queue.lock().unwrap().pop_front() else {
                    break;
                };
                upload_path(cfg, password, &path, manifest, log);
                let done = completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if let Some(activity) = activity {
                    activity(ActivityInfo {
                        state: ActivityState::Syncing,
                        completed: done,
                        total,
                    });
                }
            });
        }
    });
    if total > 0 {
        let snapshot = manifest.lock().unwrap().clone();
        save_remote_manifest(cfg, password, &snapshot, log);
    }
    completed.load(std::sync::atomic::Ordering::SeqCst)
}

fn fetch_remote_manifest(cfg: &Config, password: &str, log: &LogFn) -> Option<SyncManifest> {
    let remote_url = remote_manifest_url(cfg);
    match webdav::get_file(cfg, password, &remote_url) {
        Ok(data) => Some(serde_json::from_slice(&data).unwrap_or_default()),
        Err(err) => {
            log(format!("Remote manifest unavailable: {}", err));
            None
        }
    }
}

fn save_remote_manifest(cfg: &Config, password: &str, manifest: &SyncManifest, log: &LogFn) {
    let data = match serde_json::to_vec_pretty(manifest) {
        Ok(data) => data,
        Err(err) => {
            log(format!("Manifest serialise failed: {}", err));
            return;
        }
    };

    let remote_url = remote_manifest_url(cfg);
    if let Some(parent) = parent_folder_url(&remote_url) {
        if let Err(err) =
            ensure_remote_dirs(cfg, password, cfg.webdav_url.trim_end_matches('/'), &parent)
        {
            log(format!("Manifest folder create failed: {}", err));
            return;
        }
    }

    let reader = Cursor::new(data.clone());
    if let Err(err) = webdav::put_file(cfg, password, &remote_url, reader, data.len() as u64) {
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
    let (base, file_name) = remote_url.rsplit_once('/')?;
    if file_name.is_empty() || !file_name.contains('.') {
        return Some(remote_url.trim_end_matches('/').to_string());
    }
    Some(base.to_string())
}

fn ensure_remote_dirs(
    cfg: &Config,
    password: &str,
    remote_base: &str,
    folder_url: &str,
) -> Result<(), String> {
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
    is_manifest_path(path)
        || relative_path_for_watch(watch_folder, path)
            .map(|relative| relative == MANIFEST_NAME)
            .unwrap_or(false)
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

fn mark_suppressed(suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>, path: &PathBuf) {
    if let Ok(mut guard) = suppressed.lock() {
        guard.push((path.clone(), Instant::now() + Duration::from_secs(3)));
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
