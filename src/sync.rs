// sync.rs — watch local folder and upload changed files to WebDAV
// Uses notify v6 for file-system events with a 500ms debounce.

use crate::config::Config;
use crate::webdav;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REMOTE_SYNC_INTERVAL: Duration = Duration::from_secs(60);

pub struct SyncEngine {
    _watcher: RecommendedWatcher,
}

/// Callback type for status/log messages sent back to the UI
pub type LogFn = Arc<dyn Fn(String) + Send + Sync>;

impl SyncEngine {
    pub fn start(cfg: Config, password: String, log: LogFn) -> Result<Self, String> {
        // Pending paths and the time they were last touched (for debounce)
        let pending: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let suppressed: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_clone = pending.clone();
        let suppressed_clone = suppressed.clone();
        let cfg_arc = Arc::new(cfg);
        let pass_arc = Arc::new(password);
        let log_clone = log.clone();
        let cfg_watcher = cfg_arc.clone();
        let pass_watcher = pass_arc.clone();

        let remote_existing = fetch_remote_existing(&cfg_arc, &pass_arc, &log_clone);

        sync_initial_local_to_remote(&cfg_arc, &pass_arc, &remote_existing, &log_clone);

        if cfg_arc.sync_remote_changes {
            sync_remote_to_local(&cfg_arc, &pass_arc, &suppressed, &log_clone);
        }

        // Spawn upload worker thread
        std::thread::spawn(move || {
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
                    // Deduplicate
                    let mut seen = HashSet::new();
                    ready
                        .into_iter()
                        .map(|(p, _)| p)
                        .filter(|p| seen.insert(p.clone()))
                        .collect()
                };

                for path in due {
                    upload_path(&cfg_watcher, &pass_watcher, &path, &log_clone);
                }

                if cfg_watcher.sync_remote_changes
                    && last_remote_sync.elapsed() >= REMOTE_SYNC_INTERVAL
                {
                    sync_remote_to_local(
                        &cfg_watcher,
                        &pass_watcher,
                        &suppressed_clone,
                        &log_clone,
                    );
                    last_remote_sync = Instant::now();
                }
            }
        });

        // Set up the file watcher
        let watch_path = cfg_arc.watch_folder.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        let mut guard = pending.lock().unwrap();
                        for path in event.paths {
                            if should_suppress(&suppressed, &path) {
                                continue;
                            }
                            // Replace existing entry for this path
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
            .watch(std::path::Path::new(&watch_path), RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;

        Ok(SyncEngine { _watcher: watcher })
    }
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

fn upload_path(cfg: &Config, password: &str, path: &PathBuf, log: &LogFn) {
    if !path.is_file() {
        return;
    }

    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            log(format!("Read error {}: {}", path.display(), e));
            return;
        }
    };

    let watch = &cfg.watch_folder;
    let relative = match path.strip_prefix(watch) {
        Ok(r) => r.to_string_lossy().replace('\\', "/"),
        Err(_) => return,
    };

    let remote_base = remote_base_url(cfg).trim_end_matches('/').to_string();
    let remote_url = format!("{}/{}", remote_base, relative);

    if let Some(parent) = parent_folder_url(&remote_url) {
        if let Err(e) =
            ensure_remote_dirs(cfg, password, cfg.webdav_url.trim_end_matches('/'), &parent)
        {
            log(format!("Create folder failed {}: {}", relative, e));
            return;
        }
    }

    match webdav::put_file(cfg, password, &remote_url, &data) {
        Ok(_) => log(format!("Uploaded: {}", relative)),
        Err(e) => log(format!("Upload failed {}: {}", relative, e)),
    }
}

fn fetch_remote_existing(cfg: &Config, password: &str, log: &LogFn) -> HashSet<String> {
    let remote_base = remote_base_url(cfg);
    match webdav::list_entries_recursive(cfg, password, &remote_base) {
        Ok(entries) => entries
            .into_iter()
            .filter(|entry| !entry.is_collection)
            .filter_map(|entry| remote_relative_path(&remote_base, &entry.href))
            .collect(),
        Err(err) => {
            log(format!("Remote scan failed: {err}"));
            HashSet::new()
        }
    }
}

fn sync_initial_local_to_remote(
    cfg: &Config,
    password: &str,
    remote_existing: &HashSet<String>,
    log: &LogFn,
) {
    let local_files = collect_local_files(&cfg.watch_folder);
    for path in local_files {
        let relative = match path.strip_prefix(&cfg.watch_folder) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        if remote_existing.contains(&relative) {
            continue;
        }

        upload_path(cfg, password, &path, log);
    }
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
        files.push(path);
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

fn remote_base_url(cfg: &Config) -> String {
    format!(
        "{}/{}/",
        cfg.webdav_url.trim_end_matches('/'),
        cfg.remote_folder.trim_matches('/')
    )
}

fn remote_relative_path(remote_base: &str, href: &str) -> Option<String> {
    href.strip_prefix(remote_base)
        .or_else(|| href.strip_prefix(remote_base.trim_end_matches('/')))
        .map(|value| value.trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn sync_remote_to_local(
    cfg: &Config,
    password: &str,
    suppressed: &Arc<Mutex<Vec<(PathBuf, Instant)>>>,
    log: &LogFn,
) {
    let remote_base = remote_base_url(cfg);
    let entries = match webdav::list_entries_recursive(cfg, password, &remote_base) {
        Ok(entries) => entries,
        Err(err) => {
            log(format!("Remote sync list failed: {err}"));
            return;
        }
    };

    for entry in entries {
        if entry.is_collection {
            continue;
        }

        let Some(relative_owned) = remote_relative_path(&remote_base, &entry.href) else {
            continue;
        };
        let relative = relative_owned.as_str();
        if relative.is_empty() {
            continue;
        }

        let local_path = PathBuf::from(&cfg.watch_folder).join(relative.replace('/', "\\"));
        let remote_data = match webdav::get_file(cfg, password, &entry.href) {
            Ok(data) => data,
            Err(err) => {
                log(format!("Remote download failed {}: {}", relative, err));
                continue;
            }
        };

        let local_data = fs::read(&local_path).ok();
        if local_data.as_deref() == Some(remote_data.as_slice()) {
            continue;
        }

        if let Some(parent) = local_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(err) = fs::write(&local_path, &remote_data) {
            log(format!("Local write failed {}: {}", relative, err));
            continue;
        }

        mark_suppressed(suppressed, &local_path);
        log(format!("Downloaded: {}", relative));
    }
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
    guard.iter().any(|(p, _)| p == path)
}
