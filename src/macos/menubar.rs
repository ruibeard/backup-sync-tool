//! macOS menu-bar status item with tray icons (idle / syncing / complete).

use super::host::SyncHost;
use crate::config;
use crate::logs;
use crate::macos::launchd;
use crate::macos::notify;
use crate::macos::status_window::{self, StatusAction, StatusSnapshot};
use crate::updater::{self, CheckResult};
use dispatch::Queue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSButton, NSColor, NSFont, NSImage, NSImageView, NSMenu,
    NSMenuItem, NSTextField, NSView,
};
use objc2_foundation::{NSAttributedString, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
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
        #[unsafe(method(bstOpenWindow:))]
        fn bst_open_window(&self, _: Option<&AnyObject>) {
            close_tray_menu();
            with_shared(open_main_window);
        }
        #[unsafe(method(bstTogglePause:))]
        fn bst_toggle_pause(&self, _: Option<&AnyObject>) {
            close_tray_menu();
            with_shared(toggle_pause);
        }
        #[unsafe(method(bstOpenFolder:))]
        fn bst_open_folder(&self, _: Option<&AnyObject>) {
            with_shared(do_open_finder);
        }
        #[unsafe(method(bstReveal:))]
        fn bst_reveal(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let tag: isize = unsafe { msg_send![sender, tag] };
            let path = RECENT.with(|r| r.borrow().get(tag as usize).cloned());
            if let Some(path) = path {
                let _ = std::process::Command::new("open").arg("-R").arg(&path).status();
            }
        }
        #[unsafe(method(bstLogin:))]
        fn bst_login(&self, _: Option<&AnyObject>) {
            with_shared(|s| {
                do_toggle_login(s);
                refresh_status_window(s);
            });
        }
        #[unsafe(method(bstAuto:))]
        fn bst_auto(&self, _: Option<&AnyObject>) {
            with_shared(do_toggle_auto_update);
        }
        #[unsafe(method(bstLogs:))]
        fn bst_logs(&self, _: Option<&AnyObject>) {
            do_open_logs();
        }
        #[unsafe(method(bstControlPlane:))]
        fn bst_control_plane(&self, _: Option<&AnyObject>) {
            with_shared(start_set_control_plane);
        }
        #[unsafe(method(bstQuit:))]
        fn bst_quit(&self, _: Option<&AnyObject>) {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
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
    {
        let mut h = host.lock().expect("host lock");
        if h.is_configured() {
            if let Err(err) = h.restart_sync() {
                logs::append(&format!("menubar auto-start: {err}"));
            }
        } else if config::needs_repair(&h.config) {
            logs::append(config::REPAIR_MESSAGE);
            let _ = h.restart_sync();
        } else {
            logs::append("Sync not started: not paired or no watch folder.");
        }
    }

    let icon_idle = png_to_icon(ICON_IDLE).expect("menubar idle icon");
    let icon_syncing = png_to_icon(ICON_SYNCING).expect("menubar syncing icon");
    let icon_complete = png_to_icon(ICON_COMPLETE).expect("menubar complete icon");

    let tray = TrayIconBuilder::new()
        .with_tooltip("Backup Sync Tool")
        .with_icon(icon_idle.clone())
        .with_icon_as_template(true)
        .build()
        .expect("create menubar status item");
    // Primary click opens the status window; secondary click shows our own menu.

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

    let running = Arc::new(AtomicBool::new(true));
    SHARED.with(|c| *c.borrow_mut() = Some(shared.clone()));

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
        } else if let TrayIconEvent::Click {
            button: MouseButton::Right,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            Queue::main().exec_async(show_tray_menu);
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

thread_local! {
    /// Main thread only: the tray menu target and the state it acts on.
    static SHARED: std::cell::RefCell<Option<Shared>> = const { std::cell::RefCell::new(None) };
    static MENU_TARGET: std::cell::RefCell<Option<Retained<MenuTarget>>> = const { std::cell::RefCell::new(None) };
    static OPEN_MENU: std::cell::RefCell<Option<Retained<NSMenu>>> = const { std::cell::RefCell::new(None) };
    /// Full paths of the "Recent" rows, by menu item tag.
    static RECENT: std::cell::RefCell<Vec<std::path::PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Pause or resume. Stopping can wait for a file part, so it runs off the main thread.
fn toggle_pause(shared: &Shared) {
    let paused = match shared.host.try_lock() {
        Ok(h) => !h.is_paused(),
        Err(_) => return,
    };
    let shared = shared.clone();
    thread::spawn(move || {
        if let Ok(mut h) = shared.host.lock() {
            if let Err(err) = h.set_paused(paused) {
                logs::append(&format!("pause: {err}"));
            }
        }
        apply_status(&shared);
        refresh_status_window(&shared);
    });
}

fn snapshot_from(host: &SyncHost, update_available: bool) -> StatusSnapshot {
    let app = host.app_snapshot();
    let paired = host.is_paired();
    let connected = matches!(app.connection, crate::app::ConnectionState::Connected);
    let server_status = match &app.connection {
        crate::app::ConnectionState::ReconnectRequired { .. } => "Reconnect required".into(),
        crate::app::ConnectionState::Connecting => "Connecting…".into(),
        crate::app::ConnectionState::Connected => "Connected".into(),
        crate::app::ConnectionState::Disconnected if paired => "Offline, retrying…".into(),
        crate::app::ConnectionState::Disconnected => "Not connected".into(),
    };
    let syncing = matches!(
        app.work,
        crate::app::WorkState::Scanning | crate::app::WorkState::Syncing
    );
    StatusSnapshot {
        watch_folder: host.config.watch_folder.clone(),
        folder_label: host.config.customer.clone(),
        connected,
        server_status,
        activity_lines: app.activity.iter().rev().cloned().collect(),
        syncing,
        need_files: app.need_files,
        pct: app.progress_pct(),
        eta: app
            .eta_secs()
            .map(crate::app::format_eta)
            .unwrap_or_default(),
        version: env!("CARGO_PKG_VERSION").into(),
        update_available,
        checking: matches!(app.work, crate::app::WorkState::Scanning),
        paused: host.is_paused(),
        transfer_detail: app.direction_detail(),
        check_detail: app.checking_detail(),
        check_pct: app.checking_pct(),
        connecting: matches!(app.connection, crate::app::ConnectionState::Connecting),
        synced_at: app.synced_at,
        folder_summary: if app.folder_bytes > 0 || app.local_files > 0 {
            crate::app::folder_summary(app.local_files, app.folder_bytes)
        } else {
            String::new()
        },
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

fn do_toggle_auto_update(shared: &Shared) {
    if let Ok(mut h) = shared.host.lock() {
        h.config.auto_update = !h.config.auto_update;
        let on = h.config.auto_update;
        if let Err(err) = config::save(&h.config) {
            tip(&shared.tray, &format!("Save failed: {err}"));
        } else {
            tip(&shared.tray, if on { "Auto-update ON" } else { "Auto-update OFF" });
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
        } else if h.is_paused() {
            (IconKind::Idle, "Backup Sync — paused".into())
        } else if matches!(
            app.work,
            crate::app::WorkState::Scanning | crate::app::WorkState::Syncing
        ) {
            (
                IconKind::Syncing,
                if app.need_files > 0 {
                    format!(
                        "Backup Sync — {}% · {} file(s) remaining",
                        app.progress_pct(),
                        app.need_files
                    )
                } else {
                    "Backup Sync — syncing".into()
                },
            )
        } else if h.engine_running()
            && matches!(app.connection, crate::app::ConnectionState::Disconnected)
        {
            (IconKind::Idle, "Backup Sync — offline, retrying".into())
        } else if h.engine_running() && app.synced_at > 0 {
            (
                IconKind::Complete,
                format!(
                    "Backup Sync — all synced · last sync {}",
                    crate::app::time_label(app.synced_at)
                ),
            )
        } else if h.engine_running() {
            (IconKind::Complete, "Backup Sync — checking".into())
        } else if h.is_configured() {
            (IconKind::Complete, "Backup Sync — configured".into())
        } else if h.is_paired() {
            (IconKind::Idle, "Backup Sync — set watch folder".into())
        } else {
            let text = if config::needs_repair(&h.config) {
                "Backup Sync — pair again"
            } else {
                "Backup Sync — not paired"
            };
            (IconKind::Idle, text.into())
        }
    };
    set_icon(&shared.tray, &shared.icons, kind);
    tip(&shared.tray, &tip_text);
    refresh_status_window(shared);
}

// —— Tray menu (secondary click) ——————————————————————————————————————————
// Native NSMenu, built fresh on each click so it shows the current state:
// a header row (app, state, pause/play), the folder, the last 10 files,
// Advanced, Quit.

const MENU_W: f64 = 300.0;
const RECENT_ROWS: usize = 10;

fn with_shared(f: impl FnOnce(&Shared)) {
    let shared = SHARED.with(|c| c.borrow().clone());
    if let Some(shared) = shared {
        f(&shared);
    }
}

fn close_tray_menu() {
    OPEN_MENU.with(|m| {
        if let Some(menu) = m.borrow().as_ref() {
            menu.cancelTracking();
        }
    });
}

fn menu_target(mtm: MainThreadMarker) -> Retained<MenuTarget> {
    MENU_TARGET.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| unsafe {
                msg_send![super(MenuTarget::alloc(mtm).set_ivars(MenuTargetIvars)), init]
            })
            .clone()
    })
}

fn target_obj(t: &MenuTarget) -> &AnyObject {
    unsafe { &*(t as *const MenuTarget as *const AnyObject) }
}

fn menu_item(mtm: MainThreadMarker, t: &MenuTarget, title: &str, action: Option<objc2::runtime::Sel>, key: &str) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if action.is_some() {
        unsafe { item.setTarget(Some(target_obj(t))) };
    }
    item
}

fn symbol(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
    img.setSize(NSSize::new(size, size));
    Some(img)
}

fn menu_label(mtm: MainThreadMarker, text: &str, frame: NSRect, size: f64, bold: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFrame(frame);
    let font = if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
    l.setFont(Some(&font));
    l.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    l
}

/// "All synced · 21:35", "Syncing 8 file(s) · 3%", "Paused", …
fn short_state(s: &StatusSnapshot) -> String {
    if s.paused {
        "Paused".into()
    } else if !s.connected && !s.connecting {
        s.server_status.clone()
    } else if s.checking || s.connecting {
        "Checking…".into()
    } else if s.syncing {
        format!("Syncing {} file(s) · {}%", s.need_files, s.pct)
    } else if s.synced_at > 0 {
        format!("All synced · {}", crate::app::time_label(s.synced_at))
    } else {
        "All synced".into()
    }
}

/// App icon, name, state; the whole row opens the window, the round button
/// pauses or resumes.
fn header_view(mtm: MainThreadMarker, t: &MenuTarget, s: &StatusSnapshot) -> Retained<NSView> {
    let h = 48.0;
    let view = NSView::new(mtm);
    view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(MENU_W, h)));
    // The menu stretches the row to its own width; the button stays at the edge.
    view.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);

    let open = NSButton::new(mtm);
    open.setBordered(false);
    open.setTitle(&NSString::from_str(""));
    open.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(MENU_W - 44.0, h)));
    open.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
    unsafe {
        open.setTarget(Some(target_obj(t)));
        open.setAction(Some(sel!(bstOpenWindow:)));
    }
    view.addSubview(&open);

    let icon = NSImageView::new(mtm);
    // The shield itself, not the app icon (macOS draws a grey plate around it).
    icon.setFrame(NSRect::new(NSPoint::new(16.0, 10.0), NSSize::new(28.0, 28.0)));
    icon.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
    use objc2::AnyThread;
    let data = objc2_foundation::NSData::with_bytes(ICON_IDLE);
    if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
        img.setTemplate(true);
        icon.setImage(Some(&img));
    }
    icon.setContentTintColor(Some(&crate::macos::brand::green()));
    view.addSubview(&icon);

    view.addSubview(&menu_label(mtm, "Backup Sync Tool", NSRect::new(NSPoint::new(52.0, 24.0), NSSize::new(190.0, 17.0)), 13.0, true));
    let state = menu_label(mtm, &short_state(s), NSRect::new(NSPoint::new(52.0, 8.0), NSSize::new(190.0, 15.0)), 11.0, false);
    state.setTextColor(Some(&NSColor::secondaryLabelColor()));
    view.addSubview(&state);

    let pause = NSButton::new(mtm);
    pause.setBordered(false);
    pause.setFrame(NSRect::new(NSPoint::new(MENU_W - 56.0, 12.0), NSSize::new(24.0, 24.0)));
    pause.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewMinXMargin);
    let (sym, tip) = if s.paused { ("play.circle.fill", "Resume") } else { ("pause.circle.fill", "Pause") };
    if let Some(img) = symbol(sym, 22.0) {
        pause.setImage(Some(&img));
    }
    pause.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
    pause.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    pause.setToolTip(Some(&NSString::from_str(tip)));
    unsafe {
        pause.setTarget(Some(target_obj(t)));
        pause.setAction(Some(sel!(bstTogglePause:)));
    }
    view.addSubview(&pause);
    view
}

/// Menu item text with an icon in front (item images do not show on all
/// macOS versions, text attachments do) and an optional grey right part.
fn icon_title(img: Option<&NSImage>, text: &str, right: Option<&str>, tab_at: f64) -> Retained<NSAttributedString> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSAttributedStringAttachmentConveniences, NSFontAttributeName, NSForegroundColorAttributeName,
        NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextAlignment, NSTextAttachment, NSTextTab,
    };
    use objc2_foundation::{NSDictionary, NSMutableAttributedString, NSRange};
    let text: String = if text.chars().count() > 34 {
        let head: String = text.chars().take(33).collect();
        format!("{head}…")
    } else {
        text.to_string()
    };
    let out = NSMutableAttributedString::new();
    if let Some(img) = img {
        let att = NSTextAttachment::new();
        att.setImage(Some(img));
        let sz = img.size();
        att.setBounds(NSRect::new(NSPoint::new(0.0, -3.0), sz));
        out.appendAttributedString(&NSAttributedString::attributedStringWithAttachment(&att));
        out.appendAttributedString(&NSAttributedString::from_nsstring(&NSString::from_str("  ")));
    }
    let body_start = out.length();
    let body = match right {
        Some(r) => format!("{text}\t{r}"),
        None => text,
    };
    out.appendAttributedString(&NSAttributedString::from_nsstring(&NSString::from_str(&body)));
    let all = NSRange::new(0, out.length());
    let para = NSMutableParagraphStyle::new();
    let tab = unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Right,
            tab_at,
            &NSDictionary::new(),
        )
    };
    para.setTabStops(Some(&objc2_foundation::NSArray::from_retained_slice(&[tab])));
    let font = NSFont::menuFontOfSize(13.0);
    unsafe {
        out.addAttribute_value_range(NSParagraphStyleAttributeName, &para, all);
        out.addAttribute_value_range(NSFontAttributeName, &font, all);
        if let Some(r) = right {
            let n = r.encode_utf16().count();
            let start = out.length() - n;
            out.addAttribute_value_range(NSForegroundColorAttributeName, &NSColor::secondaryLabelColor(), NSRange::new(start, n));
        }
    }
    let _ = body_start;
    Retained::into_super(out)
}

/// SF Symbol tinted for use inside text.
fn tinted_symbol(name: &str, size: f64, color: &NSColor) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
    let conf = objc2_app_kit::NSImageSymbolConfiguration::configurationWithHierarchicalColor(color);
    let img = img.imageWithSymbolConfiguration(&conf)?;
    img.setSize(NSSize::new(size, size));
    Some(img)
}

/// Last finished files, newest first, one row per file.
fn recent_files(s: &StatusSnapshot) -> Vec<(crate::app::Direction, String, u64)> {
    use crate::app::Activity;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (at, line) in &s.activity_lines {
        if let Some(Activity::File { dir, done: true, path }) = crate::app::parse_activity(line) {
            if seen.insert(path.to_string()) {
                out.push((dir, path.to_string(), *at));
                if out.len() == RECENT_ROWS {
                    break;
                }
            }
        }
    }
    out
}

fn show_tray_menu() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(shared) = SHARED.with(|c| c.borrow().clone()) else { return };
    let snap = match shared.host.try_lock() {
        Ok(h) => window_snapshot(&shared, &h),
        Err(_) => return,
    };
    let (login_on, auto_on) = match shared.host.try_lock() {
        Ok(h) => (h.config.start_with_windows, h.config.auto_update),
        Err(_) => (false, false),
    };
    let t = menu_target(mtm);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);

    let header = NSMenuItem::new(mtm);
    header.setView(Some(&header_view(mtm, &t, &snap)));
    menu.addItem(&header);
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // Folder: Finder icon and name; click opens it.
    let folder = snap.watch_folder.trim();
    if !folder.is_empty() {
        let name = std::path::Path::new(folder)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.to_string());
        let item = menu_item(mtm, &t, &name, Some(sel!(bstOpenFolder:)), "");
        let img = objc2_app_kit::NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(folder));
        img.setSize(NSSize::new(18.0, 18.0));
        item.setAttributedTitle(Some(&icon_title(Some(&img), &name, None, 0.0)));
        item.setToolTip(Some(&NSString::from_str(folder)));
        menu.addItem(&item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }

    // Recent files: click shows the file in Finder.
    let recent = recent_files(&snap);
    if !recent.is_empty() {
        let head = menu_item(mtm, &t, "Recent", None, "");
        head.setEnabled(false);
        menu.addItem(&head);
        let mut paths = Vec::new();
        for (i, (dir, path, at)) in recent.iter().enumerate() {
            let name = path.rsplit('/').next().unwrap_or(path);
            let item = menu_item(mtm, &t, name, Some(sel!(bstReveal:)), "");
            item.setTag(i as isize);
            let sym = match dir {
                crate::app::Direction::Up => "arrow.up",
                crate::app::Direction::Down => "arrow.down",
                crate::app::Direction::Moved => "arrow.right",
                _ => "xmark",
            };
            let img = tinted_symbol(sym, 12.0, &NSColor::secondaryLabelColor());
            let time = crate::app::time_label(*at);
            item.setAttributedTitle(Some(&icon_title(img.as_deref(), name, Some(&time), MENU_W)));
            item.setToolTip(Some(&NSString::from_str(path)));
            menu.addItem(&item);
            paths.push(std::path::Path::new(folder).join(path));
        }
        RECENT.with(|r| *r.borrow_mut() = paths);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }

    // Advanced: settings and tools.
    let adv = NSMenu::new(mtm);
    adv.setAutoenablesItems(false);
    let login = menu_item(mtm, &t, "Start at Login", Some(sel!(bstLogin:)), "");
    login.setState(if login_on { 1 } else { 0 });
    adv.addItem(&login);
    let auto = menu_item(mtm, &t, "Auto-update", Some(sel!(bstAuto:)), "");
    auto.setState(if auto_on { 1 } else { 0 });
    adv.addItem(&auto);
    adv.addItem(&NSMenuItem::separatorItem(mtm));
    adv.addItem(&menu_item(mtm, &t, "Control plane URL…", Some(sel!(bstControlPlane:)), ""));
    adv.addItem(&menu_item(mtm, &t, "Open Logs", Some(sel!(bstLogs:)), ""));
    let adv_item = menu_item(mtm, &t, "Advanced", None, "");
    let img = tinted_symbol("slider.horizontal.3", 14.0, &NSColor::labelColor());
    adv_item.setAttributedTitle(Some(&icon_title(img.as_deref(), "Advanced", None, 0.0)));
    adv_item.setSubmenu(Some(&adv));
    menu.addItem(&adv_item);
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let quit = menu_item(mtm, &t, "Quit", Some(sel!(bstQuit:)), "");
    let img = tinted_symbol("power", 14.0, &NSColor::labelColor());
    quit.setAttributedTitle(Some(&icon_title(img.as_deref(), "Quit", None, 0.0)));
    menu.addItem(&quit);

    menu.setMinimumWidth(MENU_W);
    OPEN_MENU.with(|m| *m.borrow_mut() = Some(menu.clone()));
    // Just under the menu bar, at the pointer.
    let mouse = objc2_app_kit::NSEvent::mouseLocation();
    let top = objc2_app_kit::NSScreen::mainScreen(mtm)
        .map(|sc| {
            let f = sc.visibleFrame();
            f.origin.y + f.size.height
        })
        .unwrap_or(mouse.y);
    menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(mouse.x - 12.0, top - 2.0), None);
    OPEN_MENU.with(|m| *m.borrow_mut() = None);
}
