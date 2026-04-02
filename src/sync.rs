// sync.rs — watch local folder and upload changed files to WebDAV
// Uses notify v6 for file-system events with a 500ms debounce.

use crate::config::Config;
use crate::webdav;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct SyncEngine {
    _watcher: RecommendedWatcher,
}

/// Callback type for status/log messages sent back to the UI
pub type LogFn = Arc<dyn Fn(String) + Send + Sync>;

impl SyncEngine {
    pub fn start(cfg: Config, password: String, log: LogFn) -> Result<Self, String> {
        // Pending paths and the time they were last touched (for debounce)
        let pending: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let pending_clone = pending.clone();
        let cfg_arc = Arc::new(cfg);
        let pass_arc = Arc::new(password);
        let log_clone = log.clone();
        let cfg_watcher = cfg_arc.clone();
        let pass_watcher = pass_arc.clone();

        // Spawn upload worker thread
        std::thread::spawn(move || {
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
                    if !path.is_file() {
                        continue;
                    }
                    let data = match std::fs::read(&path) {
                        Ok(d) => d,
                        Err(e) => {
                            log_clone(format!("Read error {}: {}", path.display(), e));
                            continue;
                        }
                    };

                    // Build remote URL
                    let watch = &cfg_watcher.watch_folder;
                    let relative = match path.strip_prefix(watch) {
                        Ok(r) => r.to_string_lossy().replace('\\', "/"),
                        Err(_) => continue,
                    };
                    let remote_base = cfg_watcher.webdav_url.trim_end_matches('/').to_string()
                        + "/"
                        + cfg_watcher.remote_folder.trim_matches('/');
                    let remote_url = format!("{}/{}", remote_base, relative);

                    match webdav::put_file(&cfg_watcher, &pass_watcher, &remote_url, &data) {
                        Ok(_) => log_clone(format!("Uploaded: {}", relative)),
                        Err(e) => log_clone(format!("Upload failed {}: {}", relative, e)),
                    }
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
