//! macOS entry — menubar (default) or `--daemon` for LaunchAgent.

mod brand;
mod instance;
mod launchd;
mod menubar;
pub(crate) mod notify;
mod status_window;

use crate::host::SyncHost;
use crate::logs;
use crate::updater;
use instance::AcquireMode;
use std::thread;
use std::time::Duration;

pub fn run() {
    let version = env!("CARGO_PKG_VERSION");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let daemon = args.iter().any(|a| a == "--daemon" || a == "-d");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Backup Sync Tool v{version} (macOS)");
        println!("  backupsynctool           Menu bar app (default)");
        println!("  backupsynctool --daemon  Background sync (LaunchAgent)");
        return;
    }

    let _ = logs::ensure_logs_dir();
    let mode = if daemon {
        AcquireMode::Strict
    } else {
        AcquireMode::Takeover
    };
    let guard = match instance::InstanceGuard::acquire(mode) {
        Ok(g) => g,
        Err(err) => {
            eprintln!("{err}");
            logs::append(&err);
            std::process::exit(1);
        }
    };

    if daemon {
        logs::append(&format!("macOS daemon v{version}"));
        if let Err(error) = crate::paths::validate_bundled_engine_installation() {
            logs::append(&format!("daemon: installation repair required: {error}"));
            if let Err(error) = updater::repair_current_bundle(|_| {}) {
                logs::append(&format!("daemon: installation repair failed: {error}"));
                eprintln!("daemon: installation repair failed: {error}");
            }
            drop(guard);
            return;
        }
        run_daemon();
        drop(guard);
        return;
    }

    println!("Backup Sync Tool v{version} — menu bar icon (top right)");
    logs::append("macOS menubar started");
    // Never `open` this binary — macOS opens Terminal.app for raw executables.
    // NSApp::run never returns on Quit.
    std::mem::forget(guard);
    menubar::run();
}

fn run_daemon() {
    let mut host = SyncHost::load();
    if !host.is_configured() {
        let msg = "daemon: not configured (pair + watch folder required). Exiting.";
        logs::append(msg);
        eprintln!("{msg}");
        return;
    }
    if host.config.auto_update {
        thread::spawn(|| {
            logs::append(&updater::check_status_line(env!("CARGO_PKG_VERSION")));
        });
    }
    match host.restart_sync() {
        Ok(()) => logs::append("daemon: sync engine started"),
        Err(err) => {
            logs::append(&format!("daemon: sync failed to start: {err}"));
            eprintln!("daemon: {err}");
            return;
        }
    }
    loop {
        thread::sleep(Duration::from_secs(30));
        if host.auth_failed() {
            host.stop_sync();
            logs::append(
                "daemon: Syncthing assignment rejected — stopped. Re-pair, then relaunch.",
            );
            break;
        }
    }
}
