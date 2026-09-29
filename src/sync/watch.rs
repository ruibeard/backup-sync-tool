//! OS filesystem watcher for the watch folder.
//!
//! Uses `notify` (FSEvents on macOS, ReadDirectoryChangesW on Windows). Falls
//! back to poll-only when the watcher cannot start.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct FolderWatcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Kept alive so the watcher continues delivering events.
    _watcher: Option<RecommendedWatcher>,
}

impl FolderWatcher {
    /// Start watching `root` recursively. Sets `dirty` on any create/modify/remove.
    pub fn start(root: &Path, dirty: Arc<AtomicBool>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if !root.is_dir() {
            return Self {
                stop,
                handle: None,
                _watcher: None,
            };
        }

        let (tx, rx) = mpsc::channel();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(err) => {
                crate::logs::append(&format!(
                    "sync: FS watcher unavailable ({err}); using poll only"
                ));
                return Self {
                    stop,
                    handle: None,
                    _watcher: None,
                };
            }
        };
        if let Err(err) = watcher.watch(root, RecursiveMode::Recursive) {
            crate::logs::append(&format!(
                "sync: FS watcher failed to watch {}: {err}; using poll only",
                root.display()
            ));
            return Self {
                stop,
                handle: None,
                _watcher: None,
            };
        }

        let stop_thread = Arc::clone(&stop);
        let dirty_thread = Arc::clone(&dirty);
        let handle = thread::Builder::new()
            .name("fs-watch".into())
            .spawn(move || {
                while !stop_thread.load(Ordering::Acquire) {
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(Ok(event)) => {
                            if event_is_interesting(&event.kind) && !only_temp_files(&event.paths) {
                                dirty_thread.store(true, Ordering::Release);
                            }
                        }
                        Ok(Err(_)) => {
                            dirty_thread.store(true, Ordering::Release);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .ok();

        crate::logs::append(&format!("sync: FS watcher active on {}", root.display()));
        Self {
            stop,
            handle,
            _watcher: Some(watcher),
        }
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Downloads in progress write `*.bst-tmp` files; they are not user changes.
fn only_temp_files(paths: &[std::path::PathBuf]) -> bool {
    !paths.is_empty()
        && paths.iter().all(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(super::TEMP_SUFFIX))
        })
}

fn event_is_interesting(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn temp_only_events_are_ignored() {
        let tmp = PathBuf::from("/w/docs/.7.bst-tmp");
        let real = PathBuf::from("/w/docs/a.txt");
        assert!(only_temp_files(std::slice::from_ref(&tmp)));
        // A rename from temp to target must still wake the engine.
        assert!(!only_temp_files(&[tmp, real.clone()]));
        assert!(!only_temp_files(&[real]));
        assert!(!only_temp_files(&[]));
    }
}
