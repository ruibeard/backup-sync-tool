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
    NSImageScaling, NSImageView, NSLineBreakMode, NSProgressIndicator, NSProgressIndicatorStyle,
    NSScrollView, NSTextAlignment, NSTextField, NSTitlePosition, NSUnderlineStyle,
    NSUnderlineStyleAttributeName, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSNotification, NSNumber, NSObject, NSObjectProtocol,
    NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const W: f64 = 560.0;
const H: f64 = 560.0;
const PAD: f64 = 20.0;
const COL_GAP: f64 = 16.0;
const ACT_H: f64 = 200.0;
/// Shared short Push height (Open / Change / Connect / Restore).
const BTN_H: f64 = 22.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusAction {
    OpenWatch,
    ChooseWatch,
    Pair,
    Restore,
    ToggleLogin,
    ToggleAutoUpdate,
    Update,
    OpenGithub,
    OpenAuthor,
}

#[derive(Clone, Default)]
pub struct StatusSnapshot {
    pub watch_folder: String,
    pub connected: bool,
    pub server_status: String,
    pub start_at_login: bool,
    pub auto_update: bool,
    pub activity_lines: Vec<String>,
    pub syncing: bool,
    pub sync_status: String,
    pub version: String,
    pub update_available: bool,
}

/// Physical tray icon rect is NOT used anymore — `TrayAnchor` holds the
/// status-item button window frame in Cocoa screen coords (bottom-left origin).
#[derive(Clone, Copy, Default)]
pub struct TrayAnchor {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

type ActionFn = Arc<dyn Fn(StatusAction) + Send + Sync>;

struct Widgets {
    window: Retained<NSWindow>,
    #[allow(dead_code)]
    target: Retained<StatusTarget>,
    watch: Retained<NSTextField>,
    server_icon: Retained<NSImageView>,
    server_title: Retained<NSTextField>,
    server_detail: Retained<NSTextField>,
    pair: Retained<NSButton>,
    restore: Retained<NSButton>,
    sync_spinner: Retained<NSProgressIndicator>,
    act_body: Retained<NSTextField>,
    act_sub: Retained<NSTextField>,
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
        #[unsafe(method(actRestore:))]
        fn act_restore(&self, _: Option<&AnyObject>) {
            fire(StatusAction::Restore);
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
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm)
                    .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }
            false
        }
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            LIVE.with(|c| {
                if let Ok(mut slot) = c.try_borrow_mut() {
                    *slot = None;
                }
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

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

pub fn close() {
    let run = || {
        LIVE.with(|c| {
            if let Some(live) = c.borrow().as_ref() {
                live.widgets.window.orderOut(None);
            }
        });
        OPEN.store(false, Ordering::SeqCst);
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm)
                .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn toggle(snapshot: StatusSnapshot, on_action: ActionFn, anchor: Option<TrayAnchor>) {
    let run = move || {
        if OPEN.load(Ordering::SeqCst) {
            close();
        } else {
            show_main(snapshot, on_action, anchor);
        }
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn show(snapshot: StatusSnapshot, on_action: ActionFn) {
    show_anchored(snapshot, on_action, None);
}

pub fn show_anchored(snapshot: StatusSnapshot, on_action: ActionFn, anchor: Option<TrayAnchor>) {
    let run = move || show_main(snapshot, on_action, anchor);
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

fn show_main(snapshot: StatusSnapshot, on_action: ActionFn, _anchor: Option<TrayAnchor>) {
    let mtm = MainThreadMarker::new().expect("status window on main");
    let app = NSApplication::sharedApplication(mtm);
    // Regular while window visible so it can take focus like a real Mac app.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.activate();

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
    widgets.window.center();
    widgets.window.makeKeyAndOrderFront(None);
    OPEN.store(true, Ordering::SeqCst);
    crate::logs::append("status window showing");
    LIVE.with(|c| *c.borrow_mut() = Some(Live { widgets, on_action }));
}

fn paint(ui: &Widgets, s: &StatusSnapshot) {
    let watch = if s.watch_folder.trim().is_empty() {
        "Choose a folder to back up…"
    } else {
        s.watch_folder.as_str()
    };
    ui.watch.setStringValue(&NSString::from_str(watch));

    let (sym, tint, title, detail) = if s.connected {
        (
            "checkmark.shield.fill",
            crate::macos::brand::green(),
            "Server",
            if s.syncing {
                if s.sync_status.is_empty() {
                    "Uploading…"
                } else {
                    s.sync_status.as_str()
                }
            } else {
                "Connected"
            },
        )
    } else {
        (
            "exclamationmark.shield.fill",
            NSColor::systemRedColor(),
            "Server",
            if s.server_status.contains("pair") || s.server_status.contains("Re-pair") {
                s.server_status.as_str()
            } else {
                "Not connected"
            },
        )
    };
    if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(sym),
        Some(&NSString::from_str(title)),
    ) {
        ui.server_icon.setImage(Some(&img));
    }
    ui.server_icon.setContentTintColor(Some(&tint));
    ui.server_title.setStringValue(&NSString::from_str(title));
    ui.server_detail.setStringValue(&NSString::from_str(detail));
    ui.server_detail.setTextColor(Some(&tint));

    if s.connected {
        ui.pair.setTitle(&NSString::from_str("Reconnect Server"));
        ui.pair.setKeyEquivalent(&NSString::from_str(""));
        ui.pair.setBezelColor(Some(&crate::macos::brand::green()));
        ui.restore.setHidden(false);
    } else {
        ui.pair.setTitle(&NSString::from_str("Connect Server"));
        ui.pair.setKeyEquivalent(&NSString::from_str("\r"));
        ui.pair.setBezelColor(Some(&crate::macos::brand::green()));
        ui.restore.setHidden(true);
    }

    let syncing = s.syncing;
    ui.sync_spinner.setHidden(!syncing);
    if syncing {
        unsafe {
            ui.sync_spinner.startAnimation(None);
        }
    } else {
        unsafe {
            ui.sync_spinner.stopAnimation(None);
        }
    }

    let n = s.activity_lines.len();
    ui.act_sub
        .setStringValue(&NSString::from_str(&format!("Showing last {n} events")));
    let preview = if s.activity_lines.is_empty() {
        "No recent activity".to_string()
    } else {
        s.activity_lines.join("\n")
    };
    ui.act_body.setStringValue(&NSString::from_str(&preview));
    if s.activity_lines.is_empty() {
        ui.act_body
            .setTextColor(Some(&crate::macos::brand::caption()));
    } else {
        ui.act_body.setTextColor(Some(&crate::macos::brand::ink()));
    }
    let line_h = 15.0;
    let lines = n.max(1) as f64;
    let doc_h = (lines * line_h + 12.0).max(ACT_H);
    let aw = W - PAD * 2.0 - 16.0;
    ui.act_body.setFrame(rect(0.0, 0.0, aw, doc_h));

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
    ui.version
        .setStringValue(&NSString::from_str(&format!("v{}", s.version)));
    ui.update_btn.setHidden(!s.update_available);
}

fn build(mtm: MainThreadMarker) -> Widgets {
    let target: Retained<StatusTarget> =
        unsafe { msg_send![super(StatusTarget::alloc(mtm).set_ivars(StatusTargetIvars)), init] };

    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable;
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
    let mark = crate::macos::brand::mark();
    window.setBackgroundColor(Some(&crate::macos::brand::surface()));
    window.setDelegate(Some(ProtocolObject::from_ref(&*target)));

    let root = NSView::new(mtm);
    root.setFrame(rect(0.0, 0.0, W, H));
    window.setContentView(Some(&root));

    let inner = W - PAD * 2.0;
    let col_w = (inner - COL_GAP) / 2.0;
    let mut y = H - PAD;

    // —— Brand header ——
    y -= 32.0;
    let brand_icon = NSImageView::new(mtm);
    brand_icon.setFrame(rect(PAD, y, 28.0, 28.0));
    brand_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    brand_icon.setImage(Some(&mark));
    if mark.isTemplate() {
        brand_icon.setContentTintColor(Some(&crate::macos::brand::green()));
    }
    root.addSubview(&brand_icon);

    let brand_name = label(
        mtm,
        "Backup Sync Tool",
        rect(PAD + 36.0, y + 3.0, inner - 36.0, 22.0),
        15.0,
        true,
    );
    brand_name.setTextColor(Some(&crate::macos::brand::green()));
    root.addSubview(&brand_name);

    y -= 14.0;
    add_sep(&root, mtm, rect(PAD, y, inner, 1.0));

    // —— Top bridge: This Mac | Server ——
    y -= 22.0;
    fx_caption(&root, mtm, "This Mac", rect(PAD, y, col_w, 16.0));
    fx_caption(
        &root,
        mtm,
        "Server",
        rect(PAD + col_w + COL_GAP, y, col_w, 16.0),
    );

    y -= 44.0;
    let icon_y = y;
    let folder_icon = NSImageView::new(mtm);
    folder_icon.setFrame(rect(PAD, icon_y, 36.0, 36.0));
    folder_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("folder.fill"),
        Some(&NSString::from_str("This Mac")),
    ) {
        folder_icon.setImage(Some(&img));
    }
    folder_icon.setContentTintColor(Some(&crate::macos::brand::green()));
    root.addSubview(&folder_icon);

    let server_icon = NSImageView::new(mtm);
    server_icon.setFrame(rect(PAD + col_w + COL_GAP, icon_y, 36.0, 36.0));
    server_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    root.addSubview(&server_icon);

    // Path centered vertically with folder icon (single truncated line).
    let watch = label(
        mtm,
        "Choose a folder…",
        rect(PAD + 44.0, icon_y + 9.0, col_w - 44.0, 18.0),
        12.0,
        false,
    );
    watch.setMaximumNumberOfLines(1);
    watch.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    watch.setTextColor(Some(&crate::macos::brand::ink()));
    root.addSubview(&watch);

    let server_title = label(
        mtm,
        "Server",
        rect(PAD + col_w + COL_GAP + 44.0, icon_y + 18.0, col_w - 44.0, 16.0),
        12.0,
        true,
    );
    server_title.setTextColor(Some(&crate::macos::brand::ink()));
    root.addSubview(&server_title);

    let server_detail = label(
        mtm,
        "Not connected",
        rect(PAD + col_w + COL_GAP + 44.0, icon_y + 1.0, col_w - 64.0, 16.0),
        12.0,
        false,
    );
    root.addSubview(&server_detail);

    let sync_spinner = NSProgressIndicator::new(mtm);
    sync_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    sync_spinner.setIndeterminate(true);
    sync_spinner.setDisplayedWhenStopped(false);
    sync_spinner.setFrame(rect(
        PAD + col_w + COL_GAP + col_w - 18.0,
        icon_y + 11.0,
        14.0,
        14.0,
    ));
    sync_spinner.setHidden(true);
    root.addSubview(&sync_spinner);

    y = icon_y - 10.0;
    y -= 24.0;
    let half = (col_w - 8.0) / 2.0;
    root.addSubview(&button(
        mtm,
        &target,
        "Open",
        sel!(actOpen:),
        rect(PAD, y, half, BTN_H),
    ));
    root.addSubview(&button(
        mtm,
        &target,
        "Change…",
        sel!(actChoose:),
        rect(PAD + half + 8.0, y, half, BTN_H),
    ));

    // Primary Connect: same short Push as Open/Change (not a tall green slab).
    let pair = button(
        mtm,
        &target,
        "Connect Server",
        sel!(actPair:),
        rect(PAD + col_w + COL_GAP, y, col_w, BTN_H),
    );
    pair.setKeyEquivalent(&NSString::from_str("\r"));
    pair.setBezelColor(Some(&crate::macos::brand::green()));
    root.addSubview(&pair);

    y -= 28.0;
    let restore = button(
        mtm,
        &target,
        "Restore Backup…",
        sel!(actRestore:),
        rect(PAD + col_w + COL_GAP, y, col_w, BTN_H),
    );
    restore.setHidden(true);
    root.addSubview(&restore);

    // —— Activity ——
    y -= 20.0;
    add_sep(&root, mtm, rect(PAD, y, inner, 1.0));

    y -= 24.0;
    fx_caption(&root, mtm, "Recent Activity", rect(PAD, y, 160.0, 16.0));
    let act_sub = label(
        mtm,
        "Showing last 0 events",
        rect(W - PAD - 160.0, y, 160.0, 16.0),
        11.0,
        false,
    );
    act_sub.setAlignment(NSTextAlignment::Right);
    act_sub.setTextColor(Some(&crate::macos::brand::caption()));
    root.addSubview(&act_sub);

    y -= ACT_H + 8.0;
    let scroll = NSScrollView::new(mtm);
    scroll.setFrame(rect(PAD, y, inner, ACT_H));
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(false);
    scroll.setAutohidesScrollers(true);
    scroll.setBorderType(NSBorderType::BezelBorder);
    scroll.setDrawsBackground(true);
    scroll.setBackgroundColor(&crate::macos::brand::surface());

    let act_body = label(
        mtm,
        "No recent activity",
        rect(0.0, 0.0, inner - 16.0, ACT_H),
        11.0,
        false,
    );
    act_body.setSelectable(true);
    // Without this, `\n` collapses — Recent Activity looks like one raw blob.
    act_body.setUsesSingleLineMode(false);
    act_body.setMaximumNumberOfLines(0);
    act_body.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    act_body.setTextColor(Some(&crate::macos::brand::caption()));
    let mono = NSFont::systemFontOfSize(11.0);
    act_body.setFont(Some(&mono));
    scroll.setDocumentView(Some(&act_body as &NSView));
    root.addSubview(&scroll);

    // —— Prefs + footer ——
    y -= 18.0;
    add_sep(&root, mtm, rect(PAD, y, inner, 1.0));

    y -= 30.0;
    let login = switch(
        mtm,
        &target,
        "Start at Login",
        sel!(actLogin:),
        rect(PAD, y, 160.0, 22.0),
    );
    root.addSubview(&login);
    let auto_update = switch(
        mtm,
        &target,
        "Auto-update",
        sel!(actAuto:),
        rect(PAD + 170.0, y, 140.0, 22.0),
    );
    root.addSubview(&auto_update);

    let foot_y = 12.0;
    let version = label(
        mtm,
        &format!("v{}", env!("CARGO_PKG_VERSION")),
        rect(PAD, foot_y + 4.0, 56.0, 16.0),
        11.0,
        false,
    );
    version.setTextColor(Some(&crate::macos::brand::caption()));
    root.addSubview(&version);
    root.addSubview(&link_btn(
        mtm,
        &target,
        "GitHub",
        sel!(actGithub:),
        rect(PAD + 60.0, foot_y + 2.0, 52.0, 20.0),
    ));
    let update_btn = button(
        mtm,
        &target,
        "Update",
        sel!(actUpdate:),
        rect(PAD + 118.0, foot_y, 64.0, 24.0),
    );
    update_btn.setHidden(true);
    update_btn.setBezelColor(Some(&crate::macos::brand::green()));
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
        server_icon,
        server_title,
        server_detail,
        pair,
        restore,
        sync_spinner,
        act_body,
        act_sub,
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

fn fx_caption(parent: &NSView, mtm: MainThreadMarker, s: &str, frame: NSRect) {
    let t = label(mtm, s, frame, 11.0, true);
    t.setTextColor(Some(&crate::macos::brand::caption()));
    parent.addSubview(&t);
}

fn label(mtm: MainThreadMarker, s: &str, frame: NSRect, size: f64, bold: bool) -> Retained<NSTextField> {
    let f = NSTextField::new(mtm);
    f.setStringValue(&NSString::from_str(s));
    f.setEditable(false);
    f.setBezeled(false);
    f.setDrawsBackground(false);
    f.setSelectable(false);
    f.setFrame(frame);
    f.setAlignment(NSTextAlignment::Left);
    let font = if bold {
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    f.setFont(Some(&font));
    f.setTextColor(Some(&crate::macos::brand::ink()));
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
    b.setControlSize(NSControlSize::Small);
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
    let green = crate::macos::brand::green();
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
    // Best-effort brand tint; AppKit may ignore for the track (title still OK).
    b.setContentTintColor(Some(&crate::macos::brand::green()));
    unsafe {
        b.setTarget(Some(&*(target as *const StatusTarget as *const AnyObject)));
        b.setAction(Some(action));
    }
    b
}
