// sync.rs — watch local folder and sync via BackupTransport.
// Startup scans the local folder and fetches one remote manifest file.

use crate::config::{self, Config};
use crate::transport::{BackupTransport, FileMetadata, TransferControl, TransportError};
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
const _: () = assert!(
    MANIFEST_VERSION == 2,
    "paths::manifest_state_dir assumes state-v2"
);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferKind {
    Upload,
    Download,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Started,
    Progress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferEvent {
    pub kind: TransferKind,
    pub state: TransferState,
    pub relative_path: String,
    pub transferred: u64,
    pub total: u64,
    pub error: Option<String>,
    pub auth_failed: bool,
}

pub type TransferEventFn = Arc<dyn Fn(TransferEvent) + Send + Sync>;

fn transfer_control(
    events: &TransferEventFn,
    kind: TransferKind,
    relative_path: &str,
    total: u64,
    cancel: &Arc<AtomicBool>,
) -> TransferControl {
    events(TransferEvent {
        kind,
        state: TransferState::Started,
        relative_path: relative_path.to_string(),
        transferred: 0,
        total,
        error: None,
        auth_failed: false,
    });
    let events = events.clone();
    let relative_path = relative_path.to_string();
    TransferControl::new(
        cancel.clone(),
        Arc::new(move |progress| {
            events(TransferEvent {
                kind,
                state: TransferState::Progress,
                relative_path: relative_path.clone(),
                transferred: progress.transferred,
                total: progress.total,
                error: None,
                auth_failed: false,
            });
        }),
    )
}

fn emit_transfer_terminal(
    events: &TransferEventFn,
    kind: TransferKind,
    relative_path: &str,
    transferred: u64,
    total: u64,
    error: Option<&TransportError>,
) {
    events(TransferEvent {
        kind,
        state: match error {
            None => TransferState::Completed,
            Some(err) if err.is_cancelled() => TransferState::Cancelled,
            Some(_) => TransferState::Failed,
        },
        relative_path: relative_path.to_string(),
        transferred,
        total,
        error: error.map(ToString::to_string),
        auth_failed: error.is_some_and(TransportError::is_auth_failed),
    });
}

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
        Self::start_with_events(cfg, transport, log, activity, auth_failed, Arc::new(|_| {}))
    }

    pub fn start_with_events(
        cfg: Config,
        transport: Arc<dyn BackupTransport>,
        log: LogFn,
        activity: ActivityFn,
        auth_failed: AuthFailedFn,
        events: TransferEventFn,
    ) -> Result<Self, String> {
        let pending: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let auth_failed = pause_engine_on_auth(stop.clone(), auth_failed);
        let pending_clone = pending.clone();
        let stop_clone = stop.clone();
        let cfg_arc = Arc::new(cfg);
        let transport_clone = transport.clone();
        let log_clone = log.clone();
        let activity_clone = activity.clone();
        let auth_failed_clone = auth_failed.clone();
        let events_clone = events.clone();
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
                &stop_clone,
                &events_clone,
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
                        &stop_clone,
                        &events_clone,
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
                        &stop_clone,
                        &events_clone,
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

fn pause_engine_on_auth(stop: Arc<AtomicBool>, callback: AuthFailedFn) -> AuthFailedFn {
    Arc::new(move || {
        stop.store(true, Ordering::Relaxed);
        callback();
    })
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
    cancel: &Arc<AtomicBool>,
    events: &TransferEventFn,
) -> UploadBatchResult {
    let mut total = UploadBatchResult::default();
    let local_state = scan_local_state(cfg);
    log(format!(
        "Startup scan of {}: {} file(s)",
        cfg.watch_folder,
        local_state.files.len()
    ));
    if !had_local_manifest {
        log("No local manifest; uploading every local file".to_string());

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
                cancel,
                events,
            );
            total.absorb(&batch);
            return total;
        }

        return total;
    }

    let local_manifest = manifest.lock().unwrap().clone();
    let remote_on_server = remote_file_states(cfg, transport, log, auth_failed);

    let mut uploads = Vec::new();
    for (relative, current_local) in &local_state.files {
        let local_baseline = local_manifest.files.get(relative);

        let local_changed = local_baseline != Some(current_local);
        let missing_on_server = remote_on_server
            .as_ref()
            .is_some_and(|present| !present.contains_key(relative));
        let size_mismatch = remote_on_server.as_ref().and_then(|present| {
            present
                .get(relative)
                .map(|remote| remote.size != current_local.size)
        });
        let listing_unavailable = remote_on_server.is_none();

        if local_changed || listing_unavailable || missing_on_server || size_mismatch == Some(true) {
            uploads.push(local_path_for_relative(cfg, relative));
        }
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
            cancel,
            events,
        );
        total.absorb(&batch);
    }

    total
}

fn upload_path(
    cfg: &Config,
    transport: &Arc<dyn BackupTransport>,
    path: &PathBuf,
    manifest: &Arc<Mutex<SyncManifest>>,
    log: &LogFn,
    auth_failed: &AuthFailedFn,
    cancel: &Arc<AtomicBool>,
    events: &TransferEventFn,
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
            let error = TransportError::Other(msg.clone());
            let _control = transfer_control(events, TransferKind::Upload, &relative, 0, cancel);
            emit_transfer_terminal(events, TransferKind::Upload, &relative, 0, 0, Some(&error));
            log(format!("Upload failed {}: {}", relative, msg));
            return UploadOutcome::Failed(relative);
        }
    };
    let mtime = file_mtime_epoch(path);
    let source_mtime_ns = file_mtime_ns(path);
    let metadata = FileMetadata { size, mtime };
    let control = transfer_control(events, TransferKind::Upload, &relative, size, cancel);

    log(format!("Uploading: {}", relative));
    log(format!("Upload progress: {}|0", relative));
    match transport.upload_file_with(&relative, path, &metadata, &control) {
        Ok(_) => {
            let current_size = file_size(path);
            let current_mtime = file_mtime_epoch(path);
            if current_size != size
                || current_mtime != mtime
                || file_mtime_ns(path) != source_mtime_ns
            {
                let err = TransportError::SourceChanged;
                emit_transfer_terminal(
                    events,
                    TransferKind::Upload,
                    &relative,
                    size,
                    size,
                    Some(&err),
                );
                log(format!("Upload failed {}: {}", relative, err));
                return UploadOutcome::Failed(relative);
            }
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
            emit_transfer_terminal(events, TransferKind::Upload, &relative, size, size, None);
            log(format!("Uploaded: {}", relative));
            UploadOutcome::Success
        }
        Err(err) => {
            emit_transfer_terminal(
                events,
                TransferKind::Upload,
                &relative,
                control.transferred(),
                size,
                Some(&err),
            );
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
    cancel: &Arc<AtomicBool>,
    events: &TransferEventFn,
) -> UploadBatchResult {
    let total = paths.len();
    if total == 0 {
        return UploadBatchResult::default();
    }
    let width = upload_worker_width(max_parallel, total);
    let processed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let succeeded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed_paths = Arc::new(Mutex::new(Vec::<String>::new()));
    let queue = Arc::new(Mutex::new(VecDeque::from(paths.to_vec())));
    let auth_failed_seen = Arc::new(AtomicBool::new(false));
    let auth_failed_callback = auth_failed.clone();
    let batch_auth_failed: AuthFailedFn = {
        let auth_failed_seen = auth_failed_seen.clone();
        Arc::new(move || {
            auth_failed_seen.store(true, Ordering::Relaxed);
            auth_failed_callback();
        })
    };
    std::thread::scope(|scope| {
        for _ in 0..width {
            let queue = queue.clone();
            let processed = processed.clone();
            let succeeded = succeeded.clone();
            let failed_paths = failed_paths.clone();
            let auth_failed_seen = auth_failed_seen.clone();
            let batch_auth_failed = batch_auth_failed.clone();
            let cancel = cancel.clone();
            let events = events.clone();
            scope.spawn(move || loop {
                if auth_failed_seen.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(path) = queue.lock().unwrap().pop_front() else {
                    break;
                };
                match upload_path(
                    cfg,
                    transport,
                    &path,
                    manifest,
                    log,
                    &batch_auth_failed,
                    &cancel,
                    &events,
                ) {
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
    let attempted = processed.load(std::sync::atomic::Ordering::SeqCst);
    let ok = succeeded.load(std::sync::atomic::Ordering::SeqCst);
    let paths_failed = failed_paths.lock().unwrap().clone();
    UploadBatchResult {
        attempted,
        succeeded: ok,
        failed: paths_failed.len(),
        failed_paths: paths_failed,
    }
}

fn upload_worker_width(configured: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        configured.clamp(1, 2).min(total)
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
    retry_uploads_with_events(
        cfg,
        transport,
        relative_paths,
        log,
        activity,
        auth_failed,
        Arc::new(|_| {}),
    )
}

pub fn retry_uploads_with_events(
    cfg: &Config,
    transport: Arc<dyn BackupTransport>,
    relative_paths: &[String],
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
    events: TransferEventFn,
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
    let cancel = Arc::new(AtomicBool::new(false));
    upload_paths_parallel(
        cfg,
        &transport,
        &paths,
        &manifest,
        log,
        config::effective_parallel_uploads(cfg),
        Some(activity),
        auth_failed,
        &cancel,
        &events,
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
    restore_customer_backup_with_events(
        cfg,
        transport,
        destination_parent,
        cancel,
        log,
        activity,
        auth_failed,
        Arc::new(|_| {}),
    )
}

pub fn restore_customer_backup_with_events(
    cfg: &Config,
    transport: Arc<dyn BackupTransport>,
    destination_parent: &Path,
    cancel: &Arc<AtomicBool>,
    log: &LogFn,
    activity: &ActivityFn,
    auth_failed: &AuthFailedFn,
    events: TransferEventFn,
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
        let control = transfer_control(
            &events,
            TransferKind::Restore,
            &remote.relative_path,
            remote.size,
            cancel,
        );
        match transport.download_file_with(&remote.relative_path, &destination, &control) {
            Ok(metadata) => {
                emit_transfer_terminal(
                    &events,
                    TransferKind::Restore,
                    &remote.relative_path,
                    metadata.size,
                    metadata.size.max(remote.size),
                    None,
                );
                completed += 1;
            }
            Err(err) => {
                emit_transfer_terminal(
                    &events,
                    TransferKind::Restore,
                    &remote.relative_path,
                    control.transferred(),
                    remote.size,
                    Some(&err),
                );
                if err.is_auth_failed() {
                    auth_failed();
                }
                if err.is_cancelled() {
                    return Err(format!(
                        "Restore cancelled. Completed files remain in {}",
                        restore_root.display()
                    ));
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
    cancel: &Arc<AtomicBool>,
    events: &TransferEventFn,
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
        cancel,
        events,
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
    relative.split('/').filter(|part| !part.is_empty()).fold(
        PathBuf::from(&cfg.watch_folder),
        |mut path, part| {
            path.push(part);
            path
        },
    )
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
            let _ = crate::paths::ensure_dir(parent);
        }
        let temporary = path.with_extension("tmp");
        if fs::write(&temporary, data).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

fn local_manifest_path(cfg: &Config) -> PathBuf {
    use sha2::{Digest, Sha256};

    let root = crate::paths::manifest_state_dir();
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

fn file_mtime_ns(path: &Path) -> u128 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
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
    use super::{
        emit_transfer_terminal, is_safe_remote_relative, pause_engine_on_auth,
        safe_restore_relative_path, upload_worker_width, TransferKind, TransferState,
    };
    use crate::transport::TransportError;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn auth_failure_pauses_engine_and_notifies_host() {
        let stopped = Arc::new(AtomicBool::new(false));
        let notified = Arc::new(AtomicBool::new(false));
        let notified_callback = notified.clone();
        let callback = pause_engine_on_auth(
            stopped.clone(),
            Arc::new(move || notified_callback.store(true, Ordering::Relaxed)),
        );

        callback();

        assert!(stopped.load(Ordering::Relaxed));
        assert!(notified.load(Ordering::Relaxed));
    }

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

    #[test]
    fn upload_worker_count_never_exceeds_two() {
        assert_eq!(upload_worker_width(0, 10), 1);
        assert_eq!(upload_worker_width(1, 10), 1);
        assert_eq!(upload_worker_width(20, 10), 2);
        assert_eq!(upload_worker_width(20, 1), 1);
        assert_eq!(upload_worker_width(20, 0), 0);
    }

    #[test]
    fn terminal_events_distinguish_cancel_auth_and_success() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_callback = seen.clone();
        let callback: super::TransferEventFn =
            Arc::new(move |event| seen_callback.lock().unwrap().push(event));

        emit_transfer_terminal(&callback, TransferKind::Upload, "a", 1, 2, None);
        emit_transfer_terminal(
            &callback,
            TransferKind::Upload,
            "b",
            1,
            2,
            Some(&TransportError::Cancelled),
        );
        emit_transfer_terminal(
            &callback,
            TransferKind::Upload,
            "c",
            0,
            2,
            Some(&TransportError::AuthFailed("denied".into())),
        );

        let events = seen.lock().unwrap();
        assert_eq!(events[0].state, TransferState::Completed);
        assert_eq!(events[1].state, TransferState::Cancelled);
        assert_eq!(events[2].state, TransferState::Failed);
        assert!(events[2].auth_failed);
    }
}
