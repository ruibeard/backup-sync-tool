//! macOS status window — same features as Windows, Form IA (not bridge).
//! Frame layout only — NSStackView/Auto Layout was broken in this embedding.

use dispatch::Queue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezelStyle, NSBorderType,
    NSBox, NSBoxType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSImage, NSImageScaling, NSImageView, NSLineBreakMode,
    NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTextAlignment, NSTextField,
    NSTitlePosition, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const W: f64 = 440.0;
const H: f64 = 660.0;
const PAD: f64 = 20.0;
const INNER: f64 = 12.0;
const BTN_H: f64 = 24.0;
const ACT_H: f64 = 130.0;
const FOOTER_RESERVE: f64 = 52.0;

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
}

type ActionFn = Arc<dyn Fn(StatusAction) + Send + Sync>;

struct Widgets {
    window: Retained<NSWindow>,
    #[allow(dead_code)]
    target: Retained<StatusTarget>,
    status_icon: Retained<NSImageView>,
    status_title: Retained<NSTextField>,
    status_detail: Retained<NSTextField>,
    watch: Retained<NSTextField>,
    server_status: Retained<NSTextField>,
    pair: Retained<NSButton>,
    restore: Retained<NSButton>,
    sync_spinner: Retained<NSProgressIndicator>,
    sync_label: Retained<NSTextField>,
    act_empty: Retained<NSTextField>,
    act_list: Retained<NSTextField>,
    login: Retained<NSButton>,
    auto_update: Retained<NSButton>,
    version: Retained<NSTextField>,
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
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            LIVE.with(|c| *c.borrow_mut() = None);
            OPEN.store(false, Ordering::SeqCst);
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm)
                    .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }
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
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let focused = LIVE.with(|c| {
        let mut slot = c.borrow_mut();
        if let Some(live) = slot.as_mut() {
            live.on_action = on_action.clone();
            paint(&live.widgets, &snapshot);
            live.widgets.window.makeKeyAndOrderFront(None);
            live.widgets.window.orderFrontRegardless();
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
    widgets.window.orderFrontRegardless();
    OPEN.store(true, Ordering::SeqCst);
    crate::logs::append("status window showing");
    LIVE.with(|c| *c.borrow_mut() = Some(Live { widgets, on_action }));
}

fn paint(ui: &Widgets, s: &StatusSnapshot) {
    let watch = if s.watch_folder.trim().is_empty() {
        "No folder selected"
    } else {
        s.watch_folder.as_str()
    };
    ui.watch.setStringValue(&NSString::from_str(watch));
    ui.server_status
        .setStringValue(&NSString::from_str(&s.server_status));

    let (sym, tint, title) = if s.connected {
        (
            "checkmark.shield.fill",
            NSColor::systemGreenColor(),
            "Connected",
        )
    } else {
        (
            "exclamationmark.shield.fill",
            NSColor::systemRedColor(),
            "Not Connected",
        )
    };
    if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(sym),
        Some(&NSString::from_str(title)),
    ) {
        ui.status_icon.setImage(Some(&img));
    }
    ui.status_icon.setContentTintColor(Some(&tint));
    ui.status_title.setStringValue(&NSString::from_str(title));
    ui.server_status.setTextColor(Some(&tint));

    if s.connected {
        ui.pair.setTitle(&NSString::from_str("Reconnect Server"));
        ui.pair.setKeyEquivalent(&NSString::from_str(""));
        ui.restore.setHidden(false);
    } else {
        ui.pair.setTitle(&NSString::from_str("Connect Server"));
        ui.pair.setKeyEquivalent(&NSString::from_str("\r"));
        ui.restore.setHidden(true);
    }

    let syncing = s.syncing;
    ui.sync_spinner.setHidden(!syncing);
    ui.sync_label.setHidden(!syncing);
    if syncing {
        ui.sync_label.setStringValue(&NSString::from_str(if s.sync_status.is_empty() {
            "Uploading…"
        } else {
            s.sync_status.as_str()
        }));
        unsafe {
            ui.sync_spinner.startAnimation(None);
        }
    } else {
        unsafe {
            ui.sync_spinner.stopAnimation(None);
        }
    }

    let empty = s.activity_lines.is_empty();
    ui.act_empty.setHidden(!empty);
    ui.act_list.setHidden(empty);
    if !empty {
        ui.act_list
            .setStringValue(&NSString::from_str(&s.activity_lines.join("\n")));
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
    ui.version
        .setStringValue(&NSString::from_str(&format!("v{}", s.version)));

    let detail = if syncing && !s.sync_status.is_empty() {
        s.sync_status.as_str()
    } else if s.connected {
        "Ready to back up"
    } else {
        "Pair this Mac with the backup server"
    };
    ui.status_detail
        .setStringValue(&NSString::from_str(detail));
}

fn build(mtm: MainThreadMarker) -> Widgets {
    let target: Retained<StatusTarget> =
        unsafe { msg_send![super(StatusTarget::alloc(mtm).set_ivars(StatusTargetIvars)), init] };

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, W, H),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Backup Sync Tool"));
    window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
    window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    let content = window.contentView().expect("content");

    // Layout from top (AppKit y grows up — we place from top using H - y).
    let mut y = H - PAD;

    // —— Status headline ——
    y -= 28.0;
    let status_icon = NSImageView::new(mtm);
    status_icon.setFrame(rect(PAD, y, 28.0, 28.0));
    status_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    content.addSubview(&status_icon);

    let status_title = label(mtm, "Not Connected", rect(PAD + 36.0, y + 10.0, 280.0, 18.0), 15.0, true);
    content.addSubview(&status_title);
    let status_detail = label(
        mtm,
        "Pair this Mac with the backup server",
        rect(PAD + 36.0, y - 6.0, 340.0, 16.0),
        12.0,
        false,
    );
    status_detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
    content.addSubview(&status_detail);

    y -= 28.0; // below detail

    // —— Watch Folder ——
    y -= 18.0;
    content.addSubview(&section_hdr(mtm, "Watch Folder", rect(PAD, y, 200.0, 16.0)));
    y -= 8.0;
    let watch_card_h = INNER * 2.0 + 18.0 + 8.0 + BTN_H;
    y -= watch_card_h;
    card_bg(&content, mtm, rect(PAD, y, W - PAD * 2.0, watch_card_h));

    let watch = label(
        mtm,
        "No folder selected",
        rect(PAD + INNER, y + INNER + BTN_H + 8.0, W - PAD * 2.0 - INNER * 2.0, 18.0),
        13.0,
        false,
    );
    watch.setTextColor(Some(&NSColor::secondaryLabelColor()));
    watch.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    content.addSubview(&watch);

    content.addSubview(&push(
        mtm,
        &target,
        "Open",
        sel!(actOpen:),
        rect(PAD + INNER, y + INNER, 64.0, BTN_H),
    ));
    content.addSubview(&push(
        mtm,
        &target,
        "Choose…",
        sel!(actChoose:),
        rect(PAD + INNER + 72.0, y + INNER, 80.0, BTN_H),
    ));

    y -= 14.0;

    // —— Server ——
    y -= 18.0;
    content.addSubview(&section_hdr(mtm, "Server", rect(PAD, y, 200.0, 16.0)));
    y -= 8.0;
    let server_card_h = INNER * 2.0 + 18.0 + 8.0 + BTN_H;
    y -= server_card_h;
    card_bg(&content, mtm, rect(PAD, y, W - PAD * 2.0, server_card_h));

    let server_status = label(
        mtm,
        "Not connected",
        rect(PAD + INNER, y + INNER + BTN_H + 8.0, W - PAD * 2.0 - INNER * 2.0, 18.0),
        13.0,
        false,
    );
    content.addSubview(&server_status);

    let restore = push(
        mtm,
        &target,
        "Restore…",
        sel!(actRestore:),
        rect(W - PAD - INNER - 220.0, y + INNER, 88.0, BTN_H),
    );
    restore.setHidden(true);
    content.addSubview(&restore);
    let pair = push(
        mtm,
        &target,
        "Connect Server",
        sel!(actPair:),
        rect(W - PAD - INNER - 124.0, y + INNER, 124.0, BTN_H),
    );
    pair.setKeyEquivalent(&NSString::from_str("\r"));
    content.addSubview(&pair);

    y -= 12.0;

    // —— Sync row (hidden when idle) ——
    y -= 20.0;
    let sync_spinner = NSProgressIndicator::new(mtm);
    sync_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    sync_spinner.setIndeterminate(true);
    sync_spinner.setDisplayedWhenStopped(false);
    sync_spinner.setFrame(rect(PAD, y + 2.0, 16.0, 16.0));
    sync_spinner.setHidden(true);
    content.addSubview(&sync_spinner);
    let sync_label = label(
        mtm,
        "Uploading…",
        rect(PAD + 24.0, y, W - PAD * 2.0 - 24.0, 18.0),
        12.0,
        false,
    );
    sync_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    sync_label.setHidden(true);
    content.addSubview(&sync_label);

    y -= 14.0;

    // —— Recent Activity ——
    y -= 18.0;
    content.addSubview(&section_hdr(mtm, "Recent Activity", rect(PAD, y, 200.0, 16.0)));
    let act_cap = label(mtm, "Last 200", rect(W - PAD - 80.0, y, 80.0, 16.0), 11.0, false);
    act_cap.setAlignment(NSTextAlignment::Right);
    act_cap.setTextColor(Some(&NSColor::tertiaryLabelColor()));
    content.addSubview(&act_cap);
    y -= 8.0;
    let act_card_h = ACT_H + 8.0;
    y -= act_card_h;
    card_bg(&content, mtm, rect(PAD, y, W - PAD * 2.0, act_card_h));

    let act_scroll = NSScrollView::new(mtm);
    act_scroll.setFrame(rect(PAD + 4.0, y + 4.0, W - PAD * 2.0 - 8.0, ACT_H));
    act_scroll.setHasVerticalScroller(true);
    act_scroll.setHasHorizontalScroller(false);
    act_scroll.setAutohidesScrollers(true);
    act_scroll.setBorderType(NSBorderType::NoBorder);
    act_scroll.setDrawsBackground(false);

    let act_list = label(mtm, "", rect(0.0, 0.0, W - PAD * 2.0 - 24.0, ACT_H), 12.0, false);
    act_list.setAlignment(NSTextAlignment::Left);
    act_list.setSelectable(true);
    act_list.setHidden(true);
    act_list.setMaximumNumberOfLines(0);
    act_scroll.setDocumentView(Some(&act_list));
    content.addSubview(&act_scroll);

    let act_empty = label(
        mtm,
        "No recent activity",
        rect(PAD + 4.0, y + 4.0, W - PAD * 2.0 - 8.0, ACT_H),
        13.0,
        false,
    );
    act_empty.setAlignment(NSTextAlignment::Center);
    act_empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
    content.addSubview(&act_empty);

    y -= 14.0;

    // —— Options ——
    y -= 18.0;
    content.addSubview(&section_hdr(mtm, "Options", rect(PAD, y, 200.0, 16.0)));
    y -= 8.0;
    let opts_h = INNER * 2.0 + 22.0 * 2.0 + 4.0;
    y -= opts_h;
    if y < FOOTER_RESERVE {
        // Keep options above footer; shrink was already applied via ACT_H/H.
        y = FOOTER_RESERVE;
    }
    card_bg(&content, mtm, rect(PAD, y, W - PAD * 2.0, opts_h));
    let login = checkbox(
        mtm,
        &target,
        "Start at Login",
        sel!(actLogin:),
        rect(PAD + INNER, y + INNER + 26.0, 200.0, 22.0),
    );
    let auto_update = checkbox(
        mtm,
        &target,
        "Auto-update",
        sel!(actAuto:),
        rect(PAD + INNER, y + INNER, 200.0, 22.0),
    );
    content.addSubview(&login);
    content.addSubview(&auto_update);

    // —— Footer ——
    let foot_y = 16.0;
    let sep = NSBox::new(mtm);
    sep.setBoxType(NSBoxType::Separator);
    sep.setTitlePosition(NSTitlePosition::NoTitle);
    sep.setFrame(rect(PAD, foot_y + 28.0, W - PAD * 2.0, 1.0));
    content.addSubview(&sep);

    let version = label(
        mtm,
        &format!("v{}", env!("CARGO_PKG_VERSION")),
        rect(PAD, foot_y, 70.0, 18.0),
        12.0,
        false,
    );
    version.setTextColor(Some(&NSColor::secondaryLabelColor()));
    content.addSubview(&version);
    content.addSubview(&text_btn(
        mtm,
        &target,
        "GitHub",
        sel!(actGithub:),
        rect(PAD + 70.0, foot_y - 2.0, 56.0, 22.0),
    ));
    content.addSubview(&text_btn(
        mtm,
        &target,
        "Update",
        sel!(actUpdate:),
        rect(PAD + 130.0, foot_y - 2.0, 60.0, 22.0),
    ));
    content.addSubview(&text_btn(
        mtm,
        &target,
        "Rui Almeida",
        sel!(actAuthor:),
        rect(W - PAD - 100.0, foot_y - 2.0, 100.0, 22.0),
    ));

    Widgets {
        window,
        target,
        status_icon,
        status_title,
        status_detail,
        watch,
        server_status,
        pair,
        restore,
        sync_spinner,
        sync_label,
        act_empty,
        act_list,
        login,
        auto_update,
        version,
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

fn card_bg(parent: &NSView, mtm: MainThreadMarker, frame: NSRect) {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setCornerRadius(10.0);
    b.setBorderWidth(0.5);
    b.setBorderColor(&NSColor::separatorColor());
    b.setFillColor(&NSColor::controlBackgroundColor());
    b.setFrame(frame);
    parent.addSubview(&b);
}

fn section_hdr(mtm: MainThreadMarker, title: &str, frame: NSRect) -> Retained<NSTextField> {
    let t = label(mtm, title, frame, 12.0, true);
    t.setTextColor(Some(&NSColor::secondaryLabelColor()));
    t
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
    f
}

fn push(
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

fn text_btn(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    action: Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::new(mtm);
    b.setTitle(&NSString::from_str(title));
    b.setBezelStyle(NSBezelStyle::AccessoryBar);
    b.setBordered(false);
    b.setFrame(frame);
    unsafe {
        b.setTarget(Some(&*(target as *const StatusTarget as *const AnyObject)));
        b.setAction(Some(action));
    }
    b
}

fn checkbox(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    action: Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::new(mtm);
    b.setTitle(&NSString::from_str(title));
    b.setButtonType(NSButtonType::Switch);
    b.setFrame(frame);
    unsafe {
        b.setTarget(Some(&*(target as *const StatusTarget as *const AnyObject)));
        b.setAction(Some(action));
    }
    b
}
