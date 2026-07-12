//! Single-instance guard via pid file (macOS).
//!
//! Interactive and `--daemon` share one lock so two sync engines never run.
//! Interactive can take over a daemon (kill + replace lock) so pair/restore stay usable.

use crate::logs;
use crate::paths;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

pub struct InstanceGuard {
    path: std::path::PathBuf,
}

#[derive(Clone, Copy)]
pub enum AcquireMode {
    /// Fail if another instance holds the lock.
    Strict,
    /// If a daemon holds the lock, stop it and take over (for interactive UI).
    Takeover,
}

impl InstanceGuard {
    pub fn acquire(mode: AcquireMode) -> Result<Self, String> {
        let dir = paths::app_support_dir();
        paths::ensure_dir(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("backupsynctool.pid");

        if path.is_file() {
            if let Ok(raw) = fs::read_to_string(&path) {
                let pid = raw.trim().to_string();
                if !pid.is_empty() && pid_alive(&pid) {
                    match mode {
                        AcquireMode::Strict => {
                            return Err(format!(
                                "already running (pid {pid}). Quit the other instance first."
                            ));
                        }
                        AcquireMode::Takeover => {
                            logs::append(&format!("takeover: stopping pid {pid}"));
                            eprintln!("Stopping background instance (pid {pid})...");
                            let _ = Command::new("kill").arg(&pid).status();
                            for _ in 0..20 {
                                if !pid_alive(&pid) {
                                    break;
                                }
                                thread::sleep(Duration::from_millis(100));
                            }
                            if pid_alive(&pid) {
                                let _ = Command::new("kill").args(["-9", &pid]).status();
                                thread::sleep(Duration::from_millis(200));
                            }
                            if pid_alive(&pid) {
                                return Err(format!(
                                    "could not stop background pid {pid}; unload LaunchAgent or kill it"
                                ));
                            }
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }

        fs::write(&path, std::process::id().to_string()).map_err(|e| e.to_string())?;
        logs::append(&format!("instance lock pid={}", std::process::id()));
        Ok(Self { path })
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn pid_alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
