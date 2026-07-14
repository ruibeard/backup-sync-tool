//! macOS menu-bar status item with tray icons (idle / syncing / complete).

use crate::config;
use crate::host::SyncHost;
use crate::logs;
use crate::macos::launchd;
use crate::macos::notify;
use crate::macos::status_window::{self, StatusAction, StatusSnapshot};
use crate::updater::{self, CheckResult};
use dispatch::Queue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem as TrayMenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Default)]
struct MenuTargetIvars;

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "BstMenuTarget"]
    #[ivars = MenuTargetIvars]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        /// ⌘W — close the pair panel or hide the status window.
        #[unsafe(method(bstCloseFront:))]
        fn bst_close_front(&self, _: Option<&AnyObject>) {
            close_frontmost_window();
        }
    }
);

fn close_frontmost_window() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    if let Some(key) = app.keyWindow() {
        if notify::is_pair_panel_window(&key) {
            notify::close_pair_panel();
        } else {
            // Status window: windowShouldClose → orderOut (hide to menubar).
            key.performClose(None);
        }
        return;
    }
    if status_window::is_open() {
        status_window::close();
    } else if notify::pair_panel_is_open() {
        notify::close_pair_panel();
    }
}

/// Minimal main menu so ⌘Q / ⌘W work (LSUIElement has no default menu).
fn install_main_menu(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let target: Retained<MenuTarget> = unsafe {
        msg_send![
            super(MenuTarget::alloc(mtm).set_ivars(MenuTargetIvars)),
            init
        ]
    };

    let main_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("MainMenu"));

    // App menu (title becomes process name in the menu bar).
    let app_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("App"));
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit Backup Sync Tool"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        )
    };
    unsafe {
        quit.setTarget(Some(&*app));
    }
    app_menu.addItem(&quit);
    let app_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("App"),
            None,
            &NSString::from_str(""),
        )
    };
    app_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_item);

    // File → Close (⌘W)
    let file_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("File"));
    let close = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Close"),
            Some(sel!(bstCloseFront:)),
            &NSString::from_str("w"),
        )
    };
    unsafe {
        close.setTarget(Some(&*target));
    }
    file_menu.addItem(&close);
    let file_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("File"),
            None,
            &NSString::from_str(""),
        )
    };
    file_item.setSubmenu(Some(&file_menu));
    main_menu.addItem(&file_item);

    app.setMainMenu(Some(&main_menu));
    // Keep menu target alive for the process lifetime.
    std::mem::forget(target);
}

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
    set_control_plane: String,
    repair_installation: String,
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
    update_available: Arc<AtomicBool>,
}

/// Run accessory menu-bar app (icon in the macOS status bar). Blocks until Quit.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("menubar must run on main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    install_main_menu(mtm);

    let host = Arc::new(Mutex::new(SyncHost::load()));
    let _ = logs::ensure_logs_dir();
    let repair_required = crate::paths::validate_bundled_engine_installation().is_err();

    if !repair_required {
        let mut h = host.lock().expect("host lock");
        if h.is_configured() {
            if let Err(err) = h.restart_sync() {
                logs::append(&format!("menubar auto-start: {err}"));
            }
        }
    }

    let icon_idle = png_to_icon(ICON_IDLE).expect("menubar idle icon");
    let icon_syncing = png_to_icon(ICON_SYNCING).expect("menubar syncing icon");
    let icon_complete = png_to_icon(ICON_COMPLETE).expect("menubar complete icon");

    let open_window = TrayMenuItem::new("Open Backup Sync Tool…", true, None);
    let open_logs = TrayMenuItem::new("Open Logs", true, None);
    let set_control_plane = TrayMenuItem::new("Control plane URL…", true, None);
    let repair_installation = TrayMenuItem::new("Repair Installation…", repair_required, None);
    let quit = TrayMenuItem::new("Quit Backup Sync", true, None);

    let ids = Ids {
        open_window: open_window.id().as_ref().to_string(),
        open_logs: open_logs.id().as_ref().to_string(),
        set_control_plane: set_control_plane.id().as_ref().to_string(),
        repair_installation: repair_installation.id().as_ref().to_string(),
        quit: quit.id().as_ref().to_string(),
    };

    let menu = Menu::new();
    let _ = menu.append(&open_window);
    let _ = menu.append(&open_logs);
    let _ = menu.append(&set_control_plane);
    let _ = menu.append(&repair_installation);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    let tray = TrayIconBuilder::new()
        .with_tooltip("Backup Sync Tool")
        .with_icon(icon_idle.clone())
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .build()
        .expect("create menubar status item");
    // Primary click opens the full status window; menu stays for secondary click.
    tray.set_show_menu_on_left_click(false);

    let tray = Arc::new(Mutex::new(MainTray(tray)));
    let icons = Arc::new(TrayIcons {
        idle: icon_idle,
        syncing: icon_syncing,
        complete: icon_complete,
    });
    let busy = Arc::new(AtomicBool::new(false));
    let update_available = Arc::new(AtomicBool::new(false));
    let shared = Shared {
        host: host.clone(),
        tray: tray.clone(),
        icons: icons.clone(),
        busy: busy.clone(),
        update_available: update_available.clone(),
    };
    apply_status(&shared);

    // Repair the old-updater rollout even when the installed version equals
    // the latest release. Pairing is intentionally not required for repair.
    if repair_required {
        let shared_repair = shared.clone();
        thread::spawn(move || run_bundle_repair(&shared_repair));
    } else {
        // Always check once so Update link can appear when needed (Windows parity).
        let shared_up = shared.clone();
        let auto = host.lock().expect("host").config.auto_update;
        thread::spawn(move || match updater::check(env!("CARGO_PKG_VERSION")) {
            CheckResult::UpdateAvailable(info) => {
                logs::append(&format!("Update available: v{}", info.version));
                shared_up.update_available.store(true, Ordering::SeqCst);
                refresh_status_window(&shared_up);
                if auto {
                    tip(&shared_up.tray, &format!("Downloading v{}…", info.version));
                    if let Err(err) = updater::download_and_replace(&info.url, |_| {}) {
                        tip(&shared_up.tray, &format!("Update failed: {err}"));
                    }
                }
            }
            CheckResult::UpToDate => {
                logs::append("updater: up to date");
                shared_up.update_available.store(false, Ordering::SeqCst);
                refresh_status_window(&shared_up);
            }
            CheckResult::Error(e) => {
                logs::append(&format!("Update check error: {e}"));
            }
        });
    }

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
            return;
        }
        if id == ids.set_control_plane {
            start_set_control_plane(&shared_ev);
            return;
        }
        if id == ids.repair_installation {
            let shared = shared_ev.clone();
            thread::spawn(move || run_bundle_repair(&shared));
        }
    }));

    let shared_tray = shared.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let shared = shared_tray.clone();
            Queue::main().exec_async(move || {
                logs::append("menubar: primary click → status window");
                open_main_window(&shared);
            });
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
    open_main_window(&shared);
    app.run();
}

fn run_bundle_repair(shared: &Shared) {
    if shared.busy.swap(true, Ordering::SeqCst) {
        tip(&shared.tray, "Repair already running…");
        return;
    }
    tip(&shared.tray, "Repairing installation…");
    if let Err(error) = updater::repair_current_bundle(|_| {}) {
        logs::append(&format!("Installation repair failed: {error}"));
        tip(
            &shared.tray,
            "Repair failed — choose Repair Installation to retry",
        );
        notify::alert(
            "Backup Sync — Repair Failed",
            &format!("{error}\n\nChoose Repair Installation from the menu bar to try again."),
        );
        shared.busy.store(false, Ordering::SeqCst);
    }
}

fn snapshot_from(host: &SyncHost, update_available: bool) -> StatusSnapshot {
    let app = host.app_snapshot();
    let paired = host.is_paired();
    let connected = matches!(app.connection, crate::app::ConnectionState::Connected);
    let server_status = match &app.connection {
        crate::app::ConnectionState::ReconnectRequired { .. } => "Reconnect required".into(),
        crate::app::ConnectionState::Connecting => "Connecting…".into(),
        crate::app::ConnectionState::Connected => "Connected".into(),
        crate::app::ConnectionState::Disconnected if paired => "Hub offline".into(),
        crate::app::ConnectionState::Disconnected => "Not connected".into(),
    };
    let syncing = matches!(
        app.work,
        crate::app::WorkState::Scanning | crate::app::WorkState::Syncing
    );
    let sync_status = if matches!(app.work, crate::app::WorkState::Scanning) {
        "Checking files…".into()
    } else if matches!(app.work, crate::app::WorkState::Syncing) {
        if app.need_files > 0 {
            format!("Syncing… {} file(s) remaining", app.need_files)
        } else {
            "Syncing…".into()
        }
    } else if connected {
        "Idle".into()
    } else {
        String::new()
    };
    StatusSnapshot {
        watch_folder: host.config.watch_folder.clone(),
        folder_label: host.config.syncthing_folder_label.clone(),
        connected,
        server_status,
        start_at_login: host.config.start_with_windows,
        auto_update: host.config.auto_update,
        activity_lines: app.activity.iter().rev().cloned().collect(),
        syncing,
        sync_status,
        local_files: app.local_files,
        global_files: app.global_files,
        need_files: app.need_files,
        need_bytes: app.need_bytes,
        version: env!("CARGO_PKG_VERSION").into(),
        update_available,
    }
}

fn window_snapshot(shared: &Shared, host: &SyncHost) -> StatusSnapshot {
    snapshot_from(host, shared.update_available.load(Ordering::SeqCst))
}

fn open_main_window(shared: &Shared) {
    let snap = shared
        .host
        .lock()
        .map(|h| window_snapshot(shared, &h))
        .unwrap_or_default();
    let shared_cb = shared.clone();
    let on_action = Arc::new(move |action| match action {
        StatusAction::OpenWatch => {
            do_open_finder(&shared_cb);
        }
        StatusAction::ChooseWatch => start_set_watch(&shared_cb),
        StatusAction::Pair => start_pair(&shared_cb),
        StatusAction::ToggleLogin => {
            do_toggle_login(&shared_cb);
            refresh_status_window(&shared_cb);
        }
        StatusAction::ToggleAutoUpdate => {
            if let Ok(mut h) = shared_cb.host.lock() {
                h.config.auto_update = !h.config.auto_update;
                let on = h.config.auto_update;
                if let Err(err) = config::save(&h.config) {
                    tip(&shared_cb.tray, &format!("Save failed: {err}"));
                } else {
                    tip(
                        &shared_cb.tray,
                        if on {
                            "Auto-update ON"
                        } else {
                            "Auto-update OFF"
                        },
                    );
                }
            }
            refresh_status_window(&shared_cb);
        }
        StatusAction::Update => start_update(&shared_cb),
        StatusAction::OpenGithub => {
            status_window::open_url("https://github.com/ruibeard/backup-sync-tool")
        }
        StatusAction::OpenAuthor => status_window::open_url("https://rui.cam"),
    });
    status_window::show(snap, on_action);
}

fn refresh_status_window(shared: &Shared) {
    if status_window::is_open() {
        if let Ok(h) = shared.host.lock() {
            status_window::refresh(window_snapshot(shared, &h));
        }
    }
}

fn do_open_logs() {
    let dir = logs::ensure_logs_dir();
    let _ = std::process::Command::new("open").arg(&dir).status();
}

fn do_open_finder(shared: &Shared) {
    if let Ok(h) = shared.host.lock() {
        let path = h.config.watch_folder.clone();
        if !path.trim().is_empty() {
            let _ = std::process::Command::new("open").arg(&path).status();
        } else {
            tip(&shared.tray, "No watch folder set");
        }
    }
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
    logs::append("menubar: Change watch folder…");
    let shared = shared.clone();
    thread::spawn(move || {
        let path = notify::pick_folder("Choose watch folder");
        let Some(path) = path else {
            logs::append("menubar: watch folder pick cancelled");
            return;
        };
        let path_disp = path.display().to_string();
        logs::append(&format!("menubar: watch folder picked: {path_disp}"));
        let result = shared
            .host
            .lock()
            .map_err(|_| "host lock poisoned".to_string())
            .and_then(|mut host| host.set_watch_folder(path));
        match result {
            Ok(()) => {
                tip(&shared.tray, &format!("Watching {path_disp}"));
                apply_status(&shared);
            }
            Err(err) => {
                logs::append(&format!("menubar: set watch folder failed: {err}"));
                tip(&shared.tray, &format!("Watch folder: {err}"));
            }
        }
    });
}

fn start_set_control_plane(shared: &Shared) {
    let shared = shared.clone();
    thread::spawn(move || {
        let current = shared
            .host
            .lock()
            .map(|h| {
                let base = h.config.pair_api_base.trim();
                if base.is_empty() {
                    "https://backup.rui.cam".into()
                } else {
                    h.config.pair_api_base.clone()
                }
            })
            .unwrap_or_else(|_| "https://backup.rui.cam".into());
        let Some(raw) = notify::prompt_url(
            "Control plane URL",
            "Laravel site root used for pairing (not /api).",
            &current,
        ) else {
            return;
        };
        match shared.host.lock().expect("host").set_pair_api_base(&raw) {
            Ok(()) => {
                let url = shared
                    .host
                    .lock()
                    .map(|h| h.config.pair_api_base.clone())
                    .unwrap_or(raw);
                tip(&shared.tray, &format!("Control plane: {url}"));
                logs::append(&format!("menubar: control plane URL → {url}"));
            }
            Err(err) => tip(&shared.tray, &format!("Control plane: {err}")),
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
    thread::spawn(move || loop {
        match run_pair_attempt(&shared) {
            PairAttempt::Complete => {
                notify::pair_finished();
                tip(&shared.tray, "Paired — sync started");
                set_icon(&shared.tray, &shared.icons, IconKind::Complete);
                thread::sleep(Duration::from_secs(2));
                shared.busy.store(false, Ordering::SeqCst);
                apply_status(&shared);
                break;
            }
            PairAttempt::ChangeServer => {
                let current = shared
                    .host
                    .lock()
                    .map(|h| h.config.pair_api_base.clone())
                    .unwrap_or_else(|_| "https://backup.rui.cam".into());
                let Some(raw) = notify::prompt_url(
                    "Change Server",
                    "Control plane URL used for pairing.",
                    &current,
                ) else {
                    shared.busy.store(false, Ordering::SeqCst);
                    set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                    break;
                };
                let changed = shared
                    .host
                    .lock()
                    .map_err(|_| "host lock poisoned".to_string())
                    .and_then(|mut host| host.set_pair_api_base(&raw));
                if let Err(err) = changed {
                    notify::pair_failed(&format!("Control plane URL: {err}"));
                    shared.busy.store(false, Ordering::SeqCst);
                    set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                    break;
                }
                logs::append("Pairing request cancelled; restarting with changed server");
            }
            PairAttempt::Cancelled => {
                if let Ok(host) = shared.host.lock() {
                    host.cancel_pairing();
                }
                logs::append("Pairing cancelled by user; existing connection retained");
                tip(&shared.tray, "Pairing cancelled");
                shared.busy.store(false, Ordering::SeqCst);
                apply_status(&shared);
                break;
            }
            PairAttempt::Failed(err) => {
                if let Ok(host) = shared.host.lock() {
                    host.pairing_failed(err.clone(), true);
                }
                if err.contains("watch folder") {
                    notify::pair_watch_folder_required();
                } else {
                    notify::pair_failed(&err);
                }
                tip(&shared.tray, &format!("Pair failed: {err}"));
                set_icon(&shared.tray, &shared.icons, IconKind::Idle);
                shared.busy.store(false, Ordering::SeqCst);
                break;
            }
        }
    });
}

enum PairAttempt {
    Complete,
    Cancelled,
    ChangeServer,
    Failed(String),
}

fn run_pair_attempt(shared: &Shared) -> PairAttempt {
    let (start, api_base) = {
        let h = match shared.host.lock() {
            Ok(host) => host,
            Err(_) => return PairAttempt::Failed("host lock poisoned".into()),
        };
        let api_base = if h.config.pair_api_base.trim().is_empty() {
            "https://backup.rui.cam".into()
        } else {
            h.config.pair_api_base.clone()
        };
        (h.pair_start_request(), api_base)
    };
    let start = match start {
        Ok(start) => start,
        Err(err) => return PairAttempt::Failed(err),
    };

    let waiting = format!("Pairing… code {} — {api_base}", start.code);
    tip(&shared.tray, &waiting);
    notify::pair_started(&start.code, &start.approve_url, &api_base);
    let sleep_ms = start.poll_interval_ms.clamp(1000, 10_000);
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        if std::time::Instant::now() > deadline {
            return PairAttempt::Failed(format!(
                "Pairing timed out.\nCode: {}\nApprove URL: {}",
                start.code, start.approve_url
            ));
        }
        thread::sleep(Duration::from_millis(sleep_ms));
        match notify::take_pair_panel_action() {
            Some(notify::PairPanelAction::Cancel) => return PairAttempt::Cancelled,
            Some(notify::PairPanelAction::ChangeServer) => return PairAttempt::ChangeServer,
            None => {}
        }
        let status = match crate::pairing::poll_pairing_result(&api_base, &start.poll_token) {
            Ok(status) => status,
            Err(err) if err.is_transient() => {
                logs::append(&format!("Pair poll retry: {err}"));
                continue;
            }
            Err(err) => return PairAttempt::Failed(err.to_string()),
        };
        match status.status.as_str() {
            "approved" => {
                let result = shared
                    .host
                    .lock()
                    .map_err(|_| "host lock poisoned".to_string())
                    .and_then(|mut host| host.pair_apply_and_sync(status));
                return result
                    .map(|_| PairAttempt::Complete)
                    .unwrap_or_else(PairAttempt::Failed);
            }
            "rejected" => return PairAttempt::Failed("Pairing rejected by server.".into()),
            "expired" => return PairAttempt::Failed("Pairing code expired. Try again.".into()),
            "failed" => {
                return PairAttempt::Failed("Pairing failed on the server. Try again.".into())
            }
            "consumed" => {
                return PairAttempt::Failed(
                    "Pairing credentials were already collected. Start a new pairing.".into(),
                )
            }
            "pending" | "waiting" | "provisioning" => tip(&shared.tray, &waiting),
            other => logs::append(&format!("Pair poll status: {other}")),
        }
    }
}

fn start_update(shared: &Shared) {
    let shared = shared.clone();
    thread::spawn(move || {
        tip(&shared.tray, "Checking for update…");
        match updater::check(env!("CARGO_PKG_VERSION")) {
            CheckResult::UpdateAvailable(info) => {
                shared.update_available.store(true, Ordering::SeqCst);
                refresh_status_window(&shared);
                tip(&shared.tray, &format!("Downloading v{}…", info.version));
                if let Err(err) = updater::download_and_replace(&info.url, |_| {}) {
                    tip(&shared.tray, &format!("Update failed: {err}"));
                }
            }
            CheckResult::UpToDate => {
                shared.update_available.store(false, Ordering::SeqCst);
                refresh_status_window(&shared);
                tip(&shared.tray, "Up to date");
            }
            CheckResult::Error(e) => tip(&shared.tray, &format!("Update: {e}")),
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
        // try_lock: never freeze the menubar while pairing holds the host.
        let Ok(h) = shared.host.try_lock() else {
            return;
        };
        let app = h.app_snapshot();
        if h.auth_failed() {
            (IconKind::Idle, "Backup Sync — re-pair required".into())
        } else if matches!(
            app.work,
            crate::app::WorkState::Scanning | crate::app::WorkState::Syncing
        ) {
            (
                IconKind::Syncing,
                if app.need_files > 0 {
                    format!("Backup Sync — {} file(s) remaining", app.need_files)
                } else {
                    "Backup Sync — syncing".into()
                },
            )
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
            (IconKind::Complete, "Backup Sync — configured".into())
        } else if h.is_paired() {
            (IconKind::Idle, "Backup Sync — set watch folder".into())
        } else {
            (IconKind::Idle, "Backup Sync — not paired".into())
        }
    };
    set_icon(&shared.tray, &shared.icons, kind);
    tip(&shared.tray, &tip_text);
    refresh_status_window(shared);
}
