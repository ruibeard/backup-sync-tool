//! macOS main status window — same jobs as Windows UI, native AppKit chrome.
//! Titled NSWindow (not menu-bar popover). Close hides to menubar.

use dispatch::Queue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezelStyle, NSBorderType,
    NSBox, NSBoxType, NSButton, NSButtonType, NSColor, NSControlSize, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage,
    NSAutoresizingMaskOptions, NSImageScaling, NSImageView, NSLineBreakMode, NSMenu, NSMenuItem, NSProgressIndicator, NSProgressIndicatorStyle,
    NSControlTextEditingDelegate, NSScrollView, NSTableColumn, NSTableColumnResizingOptions,
    NSTableView, NSTableViewColumnAutoresizingStyle, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewStyle, NSTextAlignment, NSTextField, NSTitlePosition, NSUnderlineStyle,
    NSUnderlineStyleAttributeName, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSNotification, NSNumber, NSObject, NSObjectProtocol,
    NSInteger, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const W: f64 = 540.0;
const H: f64 = 588.0;
const PAD: f64 = 20.0;
const COL_GAP: f64 = 16.0;
const ACT_H: f64 = 200.0;
const BTN_H: f64 = 28.0;
const ROW_H: f64 = 24.0;
/// Room for "Yesterday 23:10".
const TIME_W: f64 = 96.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusAction {
    OpenWatch,
    ChooseWatch,
    Pair,
    ToggleLogin,
    ToggleAutoUpdate,
    Update,
    OpenGithub,
    OpenAuthor,
}

#[derive(Clone, Default)]
pub struct StatusSnapshot {
    pub watch_folder: String,
    pub folder_label: String,
    pub connected: bool,
    pub server_status: String,
    pub start_at_login: bool,
    pub auto_update: bool,
    /// Newest first, each with the Unix time it arrived.
    pub activity_lines: Vec<(u64, String)>,
    pub syncing: bool,
    pub need_files: u64,
    /// Percent done in this round.
    pub pct: u64,
    /// "12 min left", or empty.
    pub eta: String,
    pub version: String,
    pub update_available: bool,
    /// The engine is comparing files (no transfer yet).
    pub checking: bool,
    pub paused: bool,
    /// "Upload 2.0 GB · 5 MB/s   Download 10 GB · 20 MB/s", or empty.
    pub transfer_detail: String,
    /// "Comparing 120 of 3400 files · …" while hashing, or empty.
    pub check_detail: String,
    pub check_pct: u64,
    /// Engine started, server not answered yet: not an error.
    pub connecting: bool,
    /// Unix time the folder was last fully in sync; 0 = not yet.
    pub synced_at: u64,
    /// "1,234 files · 2.3 GB", or empty before the first count.
    pub folder_summary: String,
}

type ActionFn = Arc<dyn Fn(StatusAction) + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// Upload, download or delete; `done` is false while it moves.
    File(crate::app::Direction, bool),
    Error,
    Info,
}

/// One activity row, as on Windows.
#[derive(Clone, PartialEq, Eq)]
struct Row {
    kind: RowKind,
    label: String,
    /// Path under the watch folder; double-click opens it.
    path: Option<String>,
    /// When it finished. Rows still running show no time.
    at: Option<u64>,
}

struct Widgets {
    window: Retained<NSWindow>,
    #[allow(dead_code)]
    target: Retained<StatusTarget>,
    watch: Retained<NSTextField>,
    size: Retained<NSTextField>,
    server_icon: Retained<NSImageView>,
    server_title: Retained<NSTextField>,
    server_detail: Retained<NSTextField>,
    pair: Retained<NSButton>,
    head: Retained<NSTextField>,
    pct: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    detail: Retained<NSTextField>,
    eta: Retained<NSTextField>,
    table: Retained<NSTableView>,
    login: Retained<NSButton>,
    auto_update: Retained<NSButton>,
    version: Retained<NSTextField>,
    update_btn: Retained<NSButton>,
}

struct Live {
    widgets: Widgets,
    on_action: ActionFn,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    /// Rows the table shows. Separate from LIVE: the table reads it during reloadData.
    static ROWS: RefCell<Vec<Row>> = const { RefCell::new(Vec::new()) };
    static ROOT: RefCell<String> = const { RefCell::new(String::new()) };
}
static OPEN: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct StatusTargetIvars;

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "BstStatusTarget"]
    #[ivars = StatusTargetIvars]
    struct StatusTarget;

    unsafe impl NSObjectProtocol for StatusTarget {}

    impl StatusTarget {
        #[unsafe(method(actOpen:))]
        fn act_open(&self, _: Option<&AnyObject>) {
            fire(StatusAction::OpenWatch);
        }
        #[unsafe(method(actChoose:))]
        fn act_choose(&self, _: Option<&AnyObject>) {
            fire(StatusAction::ChooseWatch);
        }
        #[unsafe(method(actPair:))]
        fn act_pair(&self, _: Option<&AnyObject>) {
            fire(StatusAction::Pair);
        }
        #[unsafe(method(actLogin:))]
        fn act_login(&self, _: Option<&AnyObject>) {
            fire(StatusAction::ToggleLogin);
        }
        #[unsafe(method(actAuto:))]
        fn act_auto(&self, _: Option<&AnyObject>) {
            fire(StatusAction::ToggleAutoUpdate);
        }
        #[unsafe(method(actUpdate:))]
        fn act_update(&self, _: Option<&AnyObject>) {
            fire(StatusAction::Update);
        }
        #[unsafe(method(actGithub:))]
        fn act_github(&self, _: Option<&AnyObject>) {
            fire(StatusAction::OpenGithub);
        }
        #[unsafe(method(actAuthor:))]
        fn act_author(&self, _: Option<&AnyObject>) {
            fire(StatusAction::OpenAuthor);
        }
        #[unsafe(method(actReveal:))]
        fn act_reveal(&self, _: Option<&AnyObject>) {
            let row = LIVE.with(|c| c.borrow().as_ref().map(|l| l.widgets.table.clickedRow()));
            if let Some(row) = row.filter(|r| *r >= 0) {
                reveal_row(row as usize);
            }
        }
        #[unsafe(method(actRow:))]
        fn act_row(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let row: NSInteger = unsafe { msg_send![sender, clickedRow] };
            if row >= 0 {
                open_row(row as usize);
            }
        }
    }

    unsafe impl NSControlTextEditingDelegate for StatusTarget {}

    unsafe impl NSTableViewDataSource for StatusTarget {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            ROWS.with(|r| r.borrow().len().max(1) as NSInteger)
        }
    }

    unsafe impl NSTableViewDelegate for StatusTarget {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(
            &self,
            _table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            let mtm = self.mtm();
            let col = column.map(|c| c.identifier().to_string()).unwrap_or_default();
            Some(cell(mtm, &col, row))
        }
    }

    unsafe impl NSWindowDelegate for StatusTarget {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _: Option<&AnyObject>) -> bool {
            // Hide to menubar (Windows parity) — keep widgets alive.
            LIVE.with(|c| {
                if let Some(live) = c.borrow().as_ref() {
                    live.widgets.window.orderOut(None);
                }
            });
            OPEN.store(false, Ordering::SeqCst);
            // Changing activation policy can synchronously close windows. Wait
            // until AppKit has returned from this delegate callback.
            Queue::main().exec_async(crate::macos::notify::restore_accessory_policy);
            false
        }
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            // Releasing the retained NSWindow inside this callback aborts in
            // _finishClosingWindow. Clear it on the next main-queue turn.
            Queue::main().exec_async(|| {
                LIVE.with(|cell| {
                    if let Ok(mut live) = cell.try_borrow_mut() {
                        *live = None;
                    }
                });
            });
            OPEN.store(false, Ordering::SeqCst);
        }
    }
);

fn fire(action: StatusAction) {
    let cb = LIVE.with(|c| c.borrow().as_ref().map(|l| l.on_action.clone()));
    if let Some(cb) = cb {
        cb(action);
    }
}

/// One table cell, centered in the row height.
fn cell(mtm: MainThreadMarker, col: &str, row: NSInteger) -> Retained<NSView> {
    let f = cell_text(mtm, col, row);
    let h = f.fittingSize().height.ceil();
    let wrap = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 100.0, ROW_H));
    wrap.setAutoresizesSubviews(true);
    f.setFrame(rect(0.0, ((ROW_H - h) / 2.0).floor(), 100.0, h));
    f.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    wrap.addSubview(&f);
    wrap
}

/// The text of one cell: icon, label or time column of an activity row.
fn cell_text(mtm: MainThreadMarker, col: &str, row: NSInteger) -> Retained<NSTextField> {
    let r = ROWS.with(|rows| rows.borrow().get(row.max(0) as usize).cloned());
    let f = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    f.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    f.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    let Some(r) = r else {
        if col == "label" {
            f.setStringValue(&NSString::from_str("No recent activity"));
            f.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        return f;
    };
    let (icon, tint) = match r.kind {
        // Green when done, grey while it moves.
        RowKind::File(dir, true) => (dir.icon(), crate::macos::brand::green()),
        RowKind::File(dir, false) => (dir.icon(), NSColor::secondaryLabelColor()),
        RowKind::Error => ("!", NSColor::systemRedColor()),
        RowKind::Info => ("i", NSColor::secondaryLabelColor()),
    };
    if col == "icon" {
        f.setStringValue(&NSString::from_str(icon));
        f.setAlignment(NSTextAlignment::Center);
        f.setTextColor(Some(&tint));
    } else if col == "time" {
        if let Some(at) = r.at {
            f.setStringValue(&NSString::from_str(&crate::app::time_label(at)));
        }
        f.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        f.setAlignment(NSTextAlignment::Right);
        f.setTextColor(Some(&NSColor::secondaryLabelColor()));
    } else {
        f.setStringValue(&NSString::from_str(&r.label));
        let color = match r.kind {
            RowKind::Info => NSColor::secondaryLabelColor(),
            RowKind::Error => NSColor::systemRedColor(),
            _ => NSColor::labelColor(),
        };
        f.setTextColor(Some(&color));
        if let Some(path) = &r.path {
            f.setToolTip(Some(&NSString::from_str(path)));
        }
    }
    f
}

/// Activity lines (newest first) to rows, the same rules as Windows:
/// one row per file with its latest state.
fn rows_from(lines: &[(u64, String)]) -> Vec<Row> {
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for (at, line) in lines {
        use crate::app::Activity;
        let (kind, label, path) = match crate::app::parse_activity(line) {
            Some(Activity::File { dir, done, path }) => {
                (RowKind::File(dir, done), dir.label(done, file_name(path)), Some(path))
            }
            Some(Activity::Waiting { dir, path }) => (
                RowKind::File(dir, false),
                format!("{} changed, will retry", file_name(path)),
                Some(path),
            ),
            Some(Activity::Failed { path, reason }) => (
                RowKind::Error,
                failed_label(file_name(path), reason),
                Some(path),
            ),
            Some(Activity::Info(m)) => (RowKind::Info, m.to_string(), None),
            None => continue,
        };
        if let Some(p) = path {
            if !seen.insert(p.to_string()) {
                continue;
            }
        }
        let running = matches!(kind, RowKind::File(_, false));
        rows.push(Row {
            kind,
            label,
            path: path.map(str::to_string),
            at: (!running).then_some(*at),
        });
    }
    rows
}

fn failed_label(name: &str, reason: &str) -> String {
    if reason.is_empty() {
        format!("Could not sync {name}")
    } else {
        format!("Could not sync {name}: {reason}")
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Open the file of a row. If it is gone, open the nearest folder that exists.
fn open_row(row: usize) {
    if let Some(url) = row_url(row) {
        let _ = objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Show the file of a row selected in Finder.
fn reveal_row(row: usize) {
    if let Some(url) = row_url(row) {
        objc2_app_kit::NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }
}

/// The file of a row. If it is gone, the nearest folder that exists.
fn row_url(row: usize) -> Option<Retained<NSURL>> {
    let rel = ROWS.with(|r| r.borrow().get(row).and_then(|r| r.path.clone()))?;
    let root = ROOT.with(|r| r.borrow().clone());
    if root.trim().is_empty() {
        return None;
    }
    let root = std::path::PathBuf::from(root.trim());
    let mut target = root.join(&rel);
    while !target.exists() {
        match target.parent() {
            Some(p) if p.starts_with(&root) => target = p.to_path_buf(),
            _ => return None,
        }
    }
    Some(NSURL::fileURLWithPath(&NSString::from_str(&target.to_string_lossy())))
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

/// Actual AppKit visibility, used before changing the app activation policy.
/// If LIVE is currently borrowed, conservatively report visible.
pub fn is_visible() -> bool {
    if MainThreadMarker::new().is_none() {
        return OPEN.load(Ordering::SeqCst);
    }
    LIVE.with(|c| {
        c.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(|live| live.widgets.window.isVisible()))
            .unwrap_or(true)
    })
}

pub fn close() {
    let run = || {
        LIVE.with(|c| {
            if let Some(live) = c.borrow().as_ref() {
                live.widgets.window.orderOut(None);
            }
        });
        OPEN.store(false, Ordering::SeqCst);
        Queue::main().exec_async(crate::macos::notify::restore_accessory_policy);
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn show(snapshot: StatusSnapshot, on_action: ActionFn) {
    let run = move || show_main(snapshot, on_action);
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn refresh(snapshot: StatusSnapshot) {
    let run = move || {
        LIVE.with(|c| {
            if let Some(live) = c.borrow().as_ref() {
                paint(&live.widgets, &snapshot);
            }
        });
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn open_url(url: &str) {
    let url = url.to_string();
    Queue::main().exec_async(move || {
        if let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(&url)) {
            let _ = objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&nsurl);
        }
    });
}

fn show_main(snapshot: StatusSnapshot, on_action: ActionFn) {
    let mtm = MainThreadMarker::new().expect("status window on main");
    let app = NSApplication::sharedApplication(mtm);
    // Regular while window visible so it can take focus like a real Mac app.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // `activate` is macOS 14+ but this app supports macOS 12.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let focused = LIVE.with(|c| {
        let mut slot = c.borrow_mut();
        if let Some(live) = slot.as_mut() {
            live.on_action = on_action.clone();
            paint(&live.widgets, &snapshot);
            live.widgets.window.makeKeyAndOrderFront(None);
            true
        } else {
            false
        }
    });
    if focused {
        OPEN.store(true, Ordering::SeqCst);
        return;
    }

    let widgets = build(mtm);
    paint(&widgets, &snapshot);
    widgets.table.scrollRowToVisible(0);
    widgets.window.center();
    widgets.window.makeKeyAndOrderFront(None);
    OPEN.store(true, Ordering::SeqCst);
    crate::logs::append("status window showing");
    LIVE.with(|c| *c.borrow_mut() = Some(Live { widgets, on_action }));
}

fn set_text(f: &NSTextField, s: &str) {
    if f.stringValue().to_string() != s {
        f.setStringValue(&NSString::from_str(s));
    }
}

fn paint(ui: &Widgets, s: &StatusSnapshot) {
    ROOT.with(|r| *r.borrow_mut() = s.watch_folder.clone());
    let watch = if s.watch_folder.trim().is_empty() {
        "Choose a folder to back up…"
    } else {
        s.watch_folder.as_str()
    };
    set_text(&ui.watch, watch);
    ui.watch.setToolTip(Some(&NSString::from_str(watch)));
    set_text(&ui.size, &s.folder_summary);

    let lower = s.server_status.to_ascii_lowercase();
    let (sym, tint, title, detail) = if s.connecting {
        let title = if s.folder_label.trim().is_empty() {
            "Server"
        } else {
            s.folder_label.as_str()
        };
        ("icloud", NSColor::secondaryLabelColor(), title, "Connecting…")
    } else if s.connected {
        let title = if s.folder_label.trim().is_empty() {
            "Backup destination"
        } else {
            s.folder_label.as_str()
        };
        ("checkmark.icloud.fill", crate::macos::brand::green(), title, "Connected")
    } else {
        let detail = if lower.contains("pair") || lower.contains("reconnect") || lower.contains("connecting") || lower.contains("offline") {
            s.server_status.as_str()
        } else {
            "Not connected"
        };
        ("exclamationmark.icloud.fill", NSColor::systemRedColor(), "Server", detail)
    };
    if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(sym),
        Some(&NSString::from_str(title)),
    ) {
        ui.server_icon.setImage(Some(&img));
    }
    ui.server_icon.setContentTintColor(Some(&tint));
    set_text(&ui.server_title, title);
    set_text(&ui.server_detail, detail);

    let linked = s.connected || s.connecting;
    let pair_title = if linked { "Reconnect Server" } else { "Connect Server" };
    if ui.pair.title().to_string() != pair_title {
        ui.pair.setTitle(&NSString::from_str(pair_title));
        // Connect is the main action while not connected: default (accent) button.
        ui.pair
            .setKeyEquivalent(&NSString::from_str(if linked { "" } else { "\r" }));
    }

    // Sync band: same states and words as Windows.
    let moving = s.syncing && !s.checking;
    let head = if s.paused {
        "Paused".to_string()
    } else if !s.connected && !s.connecting {
        detail.to_string()
    } else if s.checking || s.connecting {
        "Checking…".to_string()
    } else if moving {
        format!("Syncing {} file(s)", s.need_files)
    } else {
        "All synced".to_string()
    };
    set_text(&ui.head, &head);
    let all_synced = head == "All synced";
    let checking_known = s.checking && !s.check_detail.is_empty();
    // Done: no bar and no "100%" (that reads as still finishing). Say when.
    let (font, color) = if all_synced {
        (NSFont::systemFontOfSize(12.0), NSColor::secondaryLabelColor())
    } else {
        (NSFont::boldSystemFontOfSize(13.0), NSColor::labelColor())
    };
    ui.pct.setFont(Some(&font));
    ui.pct.setTextColor(Some(&color));
    ui.bar.setHidden(all_synced);
    set_text(
        &ui.pct,
        &if all_synced && s.synced_at > 0 {
            format!(
                "Last sync {}",
                crate::app::time_label(s.synced_at)
            )
        } else if all_synced {
            String::new()
        } else if moving {
            format!("{}%", s.pct)
        } else if checking_known {
            format!("{}%", s.check_pct)
        } else {
            String::new()
        },
    );
    // Moving bar only while nothing can be counted yet (listing, folder scan).
    let indeterminate = s.checking && !s.paused && !checking_known;
    if ui.bar.isIndeterminate() != indeterminate {
        ui.bar.setIndeterminate(indeterminate);
    }
    if indeterminate {
        unsafe { ui.bar.startAnimation(None) };
    } else {
        unsafe { ui.bar.stopAnimation(None) };
        ui.bar.setDoubleValue(if checking_known {
            s.check_pct as f64
        } else if moving {
            s.pct as f64
        } else {
            0.0
        });
    }
    let detail_text = if s.checking && !s.check_detail.is_empty() {
        s.check_detail.clone()
    } else if moving {
        if !s.transfer_detail.is_empty() {
            s.transfer_detail.clone()
        } else if s.need_files > 0 {
            format!("{} file(s) left", s.need_files)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    set_text(&ui.detail, &detail_text);
    set_text(&ui.eta, if moving { &s.eta } else { "" });

    let rows = rows_from(&s.activity_lines);
    let changed = ROWS.with(|r| {
        let mut r = r.borrow_mut();
        if *r == rows {
            false
        } else {
            *r = rows;
            true
        }
    });
    if changed {
        ui.table.reloadData();
        // Rows move on every update: a kept selection would point at another file.
        unsafe { ui.table.deselectAll(None) };
    }

    ui.login.setState(if s.start_at_login {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    ui.auto_update.setState(if s.auto_update {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    set_text(&ui.version, &format!("v{}", s.version));
    ui.update_btn.setHidden(!s.update_available);
}

fn build(mtm: MainThreadMarker) -> Widgets {
    let target: Retained<StatusTarget> = unsafe {
        msg_send![
            super(StatusTarget::alloc(mtm).set_ivars(StatusTargetIvars)),
            init
        ]
    };

    let style =
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, W, H),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Backup Sync Tool"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*target)));

    let root = NSView::new(mtm);
    root.setFrame(rect(0.0, 0.0, W, H));
    window.setContentView(Some(&root));

    let inner = W - PAD * 2.0;
    let col_w = (inner - COL_GAP) / 2.0;
    let left_x = PAD;
    let right_x = PAD + col_w + COL_GAP;
    // Layout is written top-down: `top(y, h)` turns it into AppKit's bottom-up y.
    let top = |y: f64, h: f64| H - y - h;

    // —— Two devices, side by side (as on Windows) ——
    let icon = 56.0;
    let folder_icon = NSImageView::new(mtm);
    folder_icon.setFrame(rect(left_x + (col_w - icon) / 2.0, top(18.0, icon), icon, icon));
    folder_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("laptopcomputer"),
        Some(&NSString::from_str("This Mac")),
    ) {
        folder_icon.setImage(Some(&img));
    }
    folder_icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
    root.addSubview(&folder_icon);

    let server_icon = NSImageView::new(mtm);
    server_icon.setFrame(rect(right_x + (col_w - icon) / 2.0, top(18.0, icon), icon, icon));
    server_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    root.addSubview(&server_icon);

    // Same two lines as the server side: the folder, then its size.
    let watch = label(mtm, "", rect(left_x, top(80.0, 18.0), col_w, 18.0), 12.0, false);
    watch.setAlignment(NSTextAlignment::Center);
    watch.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    root.addSubview(&watch);
    let size = label(mtm, "", rect(left_x, top(98.0, 16.0), col_w, 16.0), 12.0, false);
    size.setAlignment(NSTextAlignment::Center);
    size.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&size);

    let server_title = label(mtm, "Server", rect(right_x, top(80.0, 18.0), col_w, 18.0), 13.0, true);
    server_title.setAlignment(NSTextAlignment::Center);
    root.addSubview(&server_title);
    let server_detail = label(mtm, "", rect(right_x, top(98.0, 16.0), col_w, 16.0), 12.0, false);
    server_detail.setAlignment(NSTextAlignment::Center);
    server_detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&server_detail);

    let btn_y = top(124.0, BTN_H);
    let (open_w, change_w, gap) = (80.0, 100.0, 8.0);
    let bx = left_x + (col_w - open_w - gap - change_w) / 2.0;
    root.addSubview(&button(mtm, &target, "Open", sel!(actOpen:), rect(bx, btn_y, open_w, BTN_H)));
    root.addSubview(&button(
        mtm,
        &target,
        "Change…",
        sel!(actChoose:),
        rect(bx + open_w + gap, btn_y, change_w, BTN_H),
    ));
    let pair_w = 160.0;
    let pair = button(
        mtm,
        &target,
        "Connect Server",
        sel!(actPair:),
        rect(right_x + (col_w - pair_w) / 2.0, btn_y, pair_w, BTN_H),
    );
    root.addSubview(&pair);

    add_sep(&root, mtm, rect(PAD, top(166.0, 1.0), inner, 1.0));

    // —— Sync band: headline + %, bar, upload/download + time left ——
    let head = label(mtm, "", rect(PAD, top(180.0, 20.0), inner - 170.0, 20.0), 14.0, true);
    root.addSubview(&head);
    // "42%" while moving; "Last sync Yesterday 23:10" when done.
    let pct = label(mtm, "", rect(W - PAD - 170.0, top(180.0, 20.0), 170.0, 20.0), 13.0, true);
    pct.setAlignment(NSTextAlignment::Right);
    root.addSubview(&pct);
    let bar = NSProgressIndicator::new(mtm);
    bar.setStyle(NSProgressIndicatorStyle::Bar);
    bar.setControlSize(NSControlSize::Small);
    bar.setIndeterminate(false);
    bar.setMinValue(0.0);
    bar.setMaxValue(100.0);
    bar.setFrame(rect(PAD, top(204.0, 12.0), inner, 12.0));
    root.addSubview(&bar);
    let detail = label(mtm, "", rect(PAD, top(220.0, 16.0), inner - 120.0, 16.0), 11.0, false);
    detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&detail);
    let eta = label(mtm, "", rect(W - PAD - 120.0, top(220.0, 16.0), 120.0, 16.0), 11.0, false);
    eta.setAlignment(NSTextAlignment::Right);
    eta.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&eta);

    // —— Activity ——
    let caption = label(mtm, "CURRENT ACTIVITY", rect(PAD, top(250.0, 16.0), 200.0, 16.0), 11.0, true);
    caption.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&caption);

    let scroll = NSScrollView::new(mtm);
    scroll.setFrame(rect(PAD, top(272.0, ACT_H), inner, ACT_H));
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(false);
    scroll.setAutohidesScrollers(true);
    scroll.setBorderType(NSBorderType::BezelBorder);

    let table = NSTableView::new(mtm);
    table.setFrame(rect(0.0, 0.0, inner, ACT_H));
    // Room for the scroller and the cell spacing of the plain style.
    for (id, w) in [
        ("icon", 22.0),
        ("label", inner - 22.0 - TIME_W - 48.0),
        ("time", TIME_W),
    ] {
        let col = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &NSString::from_str(id));
        col.setWidth(w);
        col.setResizingMask(NSTableColumnResizingOptions::NoResizing);
        table.addTableColumn(&col);
    }
    table.setHeaderView(None);
    table.setStyle(NSTableViewStyle::Plain);
    table.setRowHeight(ROW_H);
    table.setUsesAlternatingRowBackgroundColors(true);
    table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::NoColumnAutoresizing);
    table.setAllowsEmptySelection(true);
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(&*target)));
        table.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        table.setTarget(Some(&*(&*target as *const StatusTarget as *const AnyObject)));
        table.setDoubleAction(Some(sel!(actRow:)));
    }
    // Right-click a row: show the file in Finder.
    let menu = NSMenu::new(mtm);
    let reveal = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Show in Finder"),
            Some(sel!(actReveal:)),
            &NSString::from_str(""),
        )
    };
    unsafe { reveal.setTarget(Some(&*(&*target as *const StatusTarget as *const AnyObject))) };
    menu.addItem(&reveal);
    unsafe { table.setMenu(Some(&menu)) };
    scroll.setDocumentView(Some(&table as &NSView));
    root.addSubview(&scroll);

    // —— Settings + footer ——
    let set_y = 272.0 + ACT_H + 16.0;
    add_sep(&root, mtm, rect(PAD, top(set_y, 1.0), inner, 1.0));
    let login = switch(mtm, &target, "Start at Login", sel!(actLogin:), rect(PAD, top(set_y + 12.0, 22.0), 130.0, 22.0));
    root.addSubview(&login);
    let auto_update = switch(
        mtm,
        &target,
        "Auto-update",
        sel!(actAuto:),
        rect(PAD + 136.0, top(set_y + 12.0, 22.0), 120.0, 22.0),
    );
    root.addSubview(&auto_update);
    let hint = label(
        mtm,
        "Changes and deletions sync across devices",
        rect(W - PAD - 250.0, top(set_y + 15.0, 16.0), 250.0, 16.0),
        11.0,
        false,
    );
    hint.setAlignment(NSTextAlignment::Right);
    hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&hint);

    let foot_y = 12.0;
    let version = label(
        mtm,
        &format!("v{}", env!("CARGO_PKG_VERSION")),
        rect(PAD, foot_y + 4.0, 70.0, 16.0),
        11.0,
        false,
    );
    version.setTextColor(Some(&NSColor::secondaryLabelColor()));
    root.addSubview(&version);
    root.addSubview(&link_btn(
        mtm,
        &target,
        "GitHub",
        sel!(actGithub:),
        rect(PAD + 70.0, foot_y + 2.0, 52.0, 20.0),
    ));
    let update_btn = button(
        mtm,
        &target,
        "Update",
        sel!(actUpdate:),
        rect(PAD + 128.0, foot_y, 70.0, 24.0),
    );
    update_btn.setHidden(true);
    root.addSubview(&update_btn);
    root.addSubview(&link_btn(
        mtm,
        &target,
        "Rui Almeida",
        sel!(actAuthor:),
        rect(W - PAD - 88.0, foot_y + 2.0, 88.0, 20.0),
    ));

    Widgets {
        window,
        target,
        watch,
        size,
        server_icon,
        server_title,
        server_detail,
        pair,
        head,
        pct,
        bar,
        detail,
        eta,
        table,
        login,
        auto_update,
        version,
        update_btn,
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect {
        origin: NSPoint { x, y },
        size: NSSize {
            width: w,
            height: h,
        },
    }
}

fn add_sep(parent: &NSView, mtm: MainThreadMarker, frame: NSRect) {
    let sep = NSBox::new(mtm);
    sep.setBoxType(NSBoxType::Separator);
    sep.setTitlePosition(NSTitlePosition::NoTitle);
    sep.setFrame(frame);
    parent.addSubview(&sep);
}

fn label(
    mtm: MainThreadMarker,
    s: &str,
    frame: NSRect,
    size: f64,
    bold: bool,
) -> Retained<NSTextField> {
    let f = NSTextField::new(mtm);
    f.setStringValue(&NSString::from_str(s));
    f.setEditable(false);
    f.setBezeled(false);
    f.setDrawsBackground(false);
    f.setSelectable(false);
    f.setFrame(frame);
    f.setAlignment(NSTextAlignment::Left);
    let font = if bold {
        NSFont::systemFontOfSize_weight(size, unsafe { objc2_app_kit::NSFontWeightSemibold })
    } else {
        NSFont::systemFontOfSize(size)
    };
    f.setFont(Some(&font));
    f.setTextColor(Some(&NSColor::labelColor()));
    f
}

fn button(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    action: Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::new(mtm);
    b.setTitle(&NSString::from_str(title));
    b.setBezelStyle(NSBezelStyle::Push);
    b.setFrame(frame);
    unsafe {
        b.setTarget(Some(&*(target as *const StatusTarget as *const AnyObject)));
        b.setAction(Some(action));
    }
    b
}

fn link_btn(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    action: Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::new(mtm);
    b.setBezelStyle(NSBezelStyle::AccessoryBar);
    b.setBordered(false);
    b.setControlSize(NSControlSize::Small);
    b.setFrame(frame);
    // Underlined brand-green attributed title — reads as a real text link.
    let font = NSFont::systemFontOfSize(12.0);
    let green = NSColor::linkColor();
    let underline = NSNumber::numberWithInteger(NSUnderlineStyle::Single.0);
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
        NSDictionary::from_slices(
            &[
                NSForegroundColorAttributeName,
                NSUnderlineStyleAttributeName,
                NSFontAttributeName,
            ],
            &[
                &*green as &AnyObject,
                &*underline as &AnyObject,
                &*font as &AnyObject,
            ],
        )
    };
    let attributed = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(title),
            Some(&attrs),
        )
    };
    b.setAttributedTitle(&attributed);
    unsafe {
        b.setTarget(Some(&*(target as *const StatusTarget as *const AnyObject)));
        b.setAction(Some(action));
    }
    b
}

fn switch(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    action: Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::new(mtm);
    b.setTitle(&NSString::from_str(title));
    b.setButtonType(NSButtonType::Switch);
    b.setControlSize(NSControlSize::Small);
    b.setFrame(frame);
    unsafe {
        b.setTarget(Some(&*(target as *const StatusTarget as *const AnyObject)));
        b.setAction(Some(action));
    }
    b
}
