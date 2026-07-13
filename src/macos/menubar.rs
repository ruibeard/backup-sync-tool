//! macOS menu-bar status item with tray icons (idle / syncing / complete).

use crate::config;
use crate::host::SyncHost;
use crate::logs;
use crate::macos::launchd;
use crate::macos::notify;
use crate::updater::{self, CheckResult};
use dispatch::Queue;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_IDLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/menubar-icon.png"
));
const ICON_SYNCING: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/menubar-syncing.png"
));
const ICON_COMPLETE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/menubar-complete.png"
));

/// TrayIcon is !Send on macOS; we only touch it on the main queue.
struct MainTray(TrayIcon);
unsafe impl Send for MainTray {}
unsafe impl Sync for MainTray {}

struct Ids {
    open_window: String,
    open_logs: String,
    quit: String,
}

#[derive(Clone, Copy)]
enum IconKind {
    Idle,
    Syncing,
    Complete,
}

#[derive(Clone)]
struct Shared {
    host: Arc<Mutex<SyncHost>>,
    tray: Arc<Mutex<MainTray>>,
    icons: Arc<TrayIcons>,
    busy: Arc<AtomicBool>,
}

/// Run accessory menu-bar app (icon in the macOS status bar). Blocks until Quit.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("menubar must run on main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let host = Arc::new(Mutex::new(SyncHost::load()));
    let _ = logs::ensure_logs_dir();

    {
        let mut h = host.lock().expect("host lock");
        if h.is_configured() {
            if let Err(err) = h.restart_sync() {
                logs::append(&format!("menubar auto-start: {err}"));
            }
        }
    }

    if host.lock().expect("host").config.auto_update {
        thread::spawn(|| {
            let line = updater::check_status_line(env!("CARGO_PKG_VERSION"));
            logs::append(&line);
        });
    }

    let icon_idle = png_to_icon(ICON_IDLE).expect("menubar idle icon");
    let icon_syncing = png_to_icon(ICON_SYNCING).expect("menubar syncing icon");
    let icon_complete = png_to_icon(ICON_COMPLETE).expect("menubar complete icon");

    let open_window = MenuItem::new("Open Backup Sync Tool…", true, None);
    let open_logs = MenuItem::new("Open Logs", true, None);
    let quit = MenuItem::new("Quit Backup Sync", true, None);

    let ids = Ids {
        open_window: open_window.id().as_ref().to_string(),
        open_logs: open_logs.id().as_ref().to_string(),
        quit: quit.id().as_ref().to_string(),
    };

    let menu = Menu::new();
    let _ = menu.append(&open_window);
    let _ = menu.append(&open_logs);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    let tray = TrayIconBuilder::new()
        .with_tooltip("Backup Sync Tool")
        .with_icon(icon_idle.clone())
        // Template → system paints white/black for menu bar (usual macOS look).
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .build()
        .expect("create menubar status item");

    let tray = Arc::new(Mutex::new(MainTray(tray)));
    let icons = Arc::new(TrayIcons {
        idle: icon_idle,
        syncing: icon_syncing,
        complete: icon_complete,
    });
    let busy = Arc::new(AtomicBool::new(false));
    let shared = Shared {
        host: host.clone(),
        tray: tray.clone(),
        icons: icons.clone(),
        busy: busy.clone(),
    };
    apply_status(&shared);

    let running = Arc::new(AtomicBool::new(true));
    let shared_ev = shared.clone();
    let running_ev = running.clone();

    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let id = event.id().as_ref().to_string();
        if id == ids.quit {
            running_ev.store(false, Ordering::SeqCst);
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
            return;
        }
        if id == ids.open_window {
            open_main_window(&shared_ev);
            return;
        }
        if id == ids.open_logs {
            do_open_logs();
        }
    }));

    let shared_tick = shared.clone();
    let running_tick = running.clone();
    thread::spawn(move || {
        while running_tick.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(2));
            if shared_tick.busy.load(Ordering::SeqCst) {
                continue;
            }
            apply_status(&shared_tick);
        }
    });

    logs::append("macOS menubar started (status icon)");
    eprintln!("Menu bar icon is live — look at the top-right of the screen.");
    app.run();
}

fn status_line(host: &SyncHost) -> String {
    if host.auth_failed() {
        "Status: re-pair required.".into()
    } else if host.is_configured() {
        format!("Status: paired · watching {}", host.config.watch_folder)
    } else if host.is_paired() {
        "Status: paired · set a watch folder.".into()
    } else if !host.config.watch_folder.trim().is_empty() {
        format!("Status: not paired · folder {}", host.config.watch_folder)
    } else {
        "Status: not set up yet.".into()
    }
}

fn open_main_window(shared: &Shared) {
    let line = shared
        .host
        .lock()
        .map(|h| status_line(&h))
        .unwrap_or_else(|_| "Status: unknown".into());
    match notify::prompt_home(&line) {
        notify::HomeChoice::SetWatch => start_set_watch(shared),
        notify::HomeChoice::Pair => start_pair(shared),
        notify::HomeChoice::Restore => start_restore(shared),
        notify::HomeChoice::ToggleLogin => do_toggle_login(shared),
        notify::HomeChoice::Update => start_update(shared),
        notify::HomeChoice::Close => tip(&shared.tray, "Running in menu bar"),
    }
}

fn do_open_logs() {
    let dir = logs::ensure_logs_dir();
    let _ = std::process::Command::new("open").arg(&dir).status();
}

fn do_toggle_login(shared: &Shared) {
    if let Ok(mut h) = shared.host.lock() {
        h.config.start_with_windows = !h.config.start_with_windows;
        if let Err(err) = config::save(&h.config) {
            tip(&shared.tray, &format!("Save failed: {err}"));
            return;
        }
        match launchd::sync_from_config(&h.config) {
            Ok(()) => {
                let msg = if h.config.start_with_windows {
                    "Start at login ON"
                } else {
                    "Start at login OFF"
                };
                tip(&shared.tray, msg);
                logs::append(msg);
            }
            Err(err) => tip(&shared.tray, &format!("Login item error: {err}")),
        }
    }
}

fn start_set_watch(shared: &Shared) {
    let shared = shared.clone();
    thread::spawn(move || {
        if let Some(path) = pick_folder("Choose watch folder") {
            match shared.host.lock().expect("host").set_watch_folder(path.clone()) {
                Ok(()) => {
                    tip(&shared.tray, &format!("Watching {}", path.display()));
                    apply_status(&shared);
                }
                Err(err) => tip(&shared.tray, &format!("Watch folder: {err}")),
            }
        }
    });
}

fn start_pair(shared: &Shared) {
    if shared.busy.swap(true, Ordering::SeqCst) {
        tip(&shared.tray, "Already busy…");
        return;
    }
    let shared = shared.clone();
    set_icon(&shared.tray, &shared.icons, IconKind::Syncing);
    tip(&shared.tray, "Pairing…");
    logs::append("menubar: Pair Device clicked");
    thread::spawn(move || {
        let finish_busy = |shared: &Shared| {
            shared.busy.store(false, Ordering::SeqCst);
        };

        let (start, api_base) = {
            let h = match shared.host.lock() {
                Ok(h) => h,
                Err(_) => {
                    tip(&shared.tray, "Pair failed: host lock");
                    set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                    finish_busy(&shared);
                    return;
                }
            };
            let api_base = h.config.pair_api_base.clone();
            (h.pair_start_request(), api_base)
        }; // host lock dropped before network/UI

        let start = match start {
            Ok(s) => s,
            Err(err) => {
                if err.contains("watch folder") {
                    notify::pair_watch_folder_required();
                } else {
                    notify::pair_failed(&err);
                }
                tip(&shared.tray, &format!("Pair failed: {err}"));
                set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                finish_busy(&shared);
                return;
            }
        };

        eprintln!("Pairing code: {}", start.code);
        eprintln!("Approve URL:  {}", start.approve_url);
        let waiting = format!("Pairing… code {} — waiting for approval", start.code);
        tip(&shared.tray, &waiting);
        notify::pair_started(&start.code, &start.approve_url);

        let sleep_ms = start.poll_interval_ms.clamp(1000, 10_000);
        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        let result = loop {
            if std::time::Instant::now() > deadline {
                let msg = format!(
                    "Pairing timed out.\nCode: {}\nApprove URL: {}",
                    start.code, start.approve_url
                );
                break Err(msg);
            }
            thread::sleep(Duration::from_millis(sleep_ms));
            let Some(status) = crate::pairing::poll_pairing(&api_base, &start.poll_token) else {
                tip(&shared.tray, &waiting);
                continue;
            };
            match status.status.as_str() {
                "approved" => {
                    let apply = shared
                        .host
                        .lock()
                        .map_err(|_| "host lock poisoned".to_string())
                        .and_then(|mut h| h.pair_apply_and_sync(status));
                    break apply;
                }
                "rejected" => break Err("Pairing rejected by server.".into()),
                "expired" => break Err("Pairing code expired. Try again.".into()),
                "pending" | "waiting" => {
                    tip(&shared.tray, &waiting);
                    eprint!(".");
                }
                other => {
                    logs::append(&format!("Pair poll status: {other}"));
                    tip(&shared.tray, &waiting);
                }
            }
        };

        match result {
            Ok(()) => {
                notify::pair_finished();
                tip(&shared.tray, "Paired — sync started");
                set_icon(&shared.tray, &shared.icons, IconKind::Complete);
                thread::sleep(Duration::from_secs(2));
                finish_busy(&shared);
                apply_status(&shared);
            }
            Err(err) => {
                notify::pair_failed(&err);
                tip(&shared.tray, &format!("Pair failed: {err}"));
                set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                finish_busy(&shared);
            }
        }
    });
}

fn start_restore(shared: &Shared) {
    let shared = shared.clone();
    thread::spawn(move || {
        let Some(parent) = pick_folder("Choose parent folder for restore") else {
            return;
        };
        if shared.busy.swap(true, Ordering::SeqCst) {
            tip(&shared.tray, "Already busy…");
            return;
        }
        set_icon(&shared.tray, &shared.icons, IconKind::Syncing);
        tip(&shared.tray, "Restoring…");
        // Restore holds host lock across I/O; apply_status uses try_lock so menubar stays live.
        match shared.host.lock().expect("host").restore_blocking(&parent) {
            Ok(path) => {
                tip(&shared.tray, &format!("Restored to {}", path.display()));
                set_icon(&shared.tray, &shared.icons, IconKind::Complete);
                let _ = std::process::Command::new("open").arg(&path).status();
                shared.busy.store(false, Ordering::SeqCst);
            }
            Err(err) => {
                tip(&shared.tray, &format!("Restore failed: {err}"));
                set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                shared.busy.store(false, Ordering::SeqCst);
            }
        }
    });
}

fn start_update(shared: &Shared) {
    let tray = shared.tray.clone();
    thread::spawn(move || {
        tip(&tray, "Checking for update…");
        match updater::check(env!("CARGO_PKG_VERSION")) {
            CheckResult::UpdateAvailable(info) => {
                tip(&tray, &format!("Downloading v{}…", info.version));
                if let Err(err) = updater::download_and_replace(&info.url, |_| {}) {
                    tip(&tray, &format!("Update failed: {err}"));
                }
            }
            CheckResult::UpToDate => tip(&tray, "Up to date"),
            CheckResult::Error(e) => tip(&tray, &format!("Update: {e}")),
        }
    });
}

struct TrayIcons {
    idle: Icon,
    syncing: Icon,
    complete: Icon,
}

fn png_to_icon(png: &[u8]) -> Result<Icon, String> {
    let img = image::load_from_memory(png)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
}

fn tip(tray: &Arc<Mutex<MainTray>>, text: &str) {
    let text = text.to_string();
    let tray = tray.clone();
    Queue::main().exec_async(move || {
        if let Ok(t) = tray.lock() {
            let _ = t.0.set_tooltip(Some(text.as_str()));
        }
    });
}

fn set_icon(tray: &Arc<Mutex<MainTray>>, icons: &Arc<TrayIcons>, kind: IconKind) {
    let tray = tray.clone();
    let icons = icons.clone();
    Queue::main().exec_async(move || {
        let icon = match kind {
            IconKind::Idle => icons.idle.clone(),
            IconKind::Syncing => icons.syncing.clone(),
            IconKind::Complete => icons.complete.clone(),
        };
        if let Ok(t) = tray.lock() {
            let _ = t.0.set_icon_with_as_template(Some(icon), true);
        }
    });
}

fn apply_status(shared: &Shared) {
    let (kind, tip_text) = {
        // try_lock: never freeze menubar if pair/restore briefly holds host
        let Ok(h) = shared.host.try_lock() else {
            return;
        };
        if h.auth_failed() {
            (IconKind::Idle, "Backup Sync — re-pair required".into())
        } else if h.engine_running() {
            (
                IconKind::Complete,
                format!(
                    "Backup Sync — watching {}",
                    if h.config.watch_folder.is_empty() {
                        "(no folder)".into()
                    } else {
                        h.config.watch_folder.clone()
                    }
                ),
            )
        } else if h.is_configured() {
            (IconKind::Complete, "Backup Sync — idle / configured".into())
        } else if h.is_paired() {
            (IconKind::Idle, "Backup Sync — set watch folder".into())
        } else {
            (IconKind::Idle, "Backup Sync — not paired".into())
        }
    };
    set_icon(&shared.tray, &shared.icons, kind);
    tip(&shared.tray, &tip_text);
}

fn pick_folder(prompt: &str) -> Option<PathBuf> {
    let script = format!(
        "POSIX path of (choose folder with prompt \"{}\")",
        prompt.replace('"', "\\\"")
    );
    let out = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}
