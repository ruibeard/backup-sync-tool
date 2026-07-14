//! Menu-bar glance popover — status + delayed recent-upload list.
//! Full controls live in the real `status_window` NSWindow.

use crate::macos::status_window::{StatusSnapshot, TrayAnchor};
use dispatch::{Queue, QueuePriority};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSEvent, NSFont, NSImageView,
    NSImageScaling, NSLineBreakMode, NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSTextAlignment,
    NSTextField, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const W: f64 = 300.0;
const H: f64 = 236.0;
const PAD: f64 = 14.0;
const RADIUS: f64 = 12.0;
const PREVIEW_N: usize = 5;
const UPLOAD_FILL_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverAction {
    OpenWindow,
}

type ActionFn = Arc<dyn Fn(PopoverAction) + Send + Sync>;

struct Widgets {
    panel: Retained<NSPanel>,
    #[allow(dead_code)]
    target: Retained<PopoverTarget>,
    status_title: Retained<NSTextField>,
    status_detail: Retained<NSTextField>,
    activity: Retained<NSTextField>,
}

struct Live {
    widgets: Widgets,
    on_action: ActionFn,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
}
static OPEN: AtomicBool = AtomicBool::new(false);
/// Bumped on open (new fill) and close (cancel). Timer must match captured gen.
static FILL_GEN: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct PopoverTargetIvars;

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "BstPopoverTarget"]
    #[ivars = PopoverTargetIvars]
    struct PopoverTarget;

    unsafe impl NSObjectProtocol for PopoverTarget {}

    impl PopoverTarget {
        #[unsafe(method(actOpenWindow:))]
        fn act_open_window(&self, _: Option<&AnyObject>) {
            fire(PopoverAction::OpenWindow);
        }
    }

    unsafe impl NSWindowDelegate for PopoverTarget {
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _: &NSNotification) {
            close_inner();
        }
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            cancel_pending_fill();
            // May run re-entrantly from close_inner's panel.close(); never panic.
            LIVE.with(|c| {
                if let Ok(mut slot) = c.try_borrow_mut() {
                    *slot = None;
                }
            });
            OPEN.store(false, Ordering::SeqCst);
        }
    }
);

fn fire(action: PopoverAction) {
    let cb = LIVE.with(|c| c.borrow().as_ref().map(|l| l.on_action.clone()));
    if let Some(cb) = cb {
        cb(action);
    }
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

pub fn close() {
    let run = || close_inner();
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

fn cancel_pending_fill() {
    FILL_GEN.fetch_add(1, Ordering::SeqCst);
}

fn close_inner() {
    cancel_pending_fill();
    // Take LIVE *before* panel.close() — close posts windowWillClose synchronously
    // and must not re-enter RefCell::borrow_mut (panic → SIGABRT).
    let taken = LIVE.with(|c| c.borrow_mut().take());
    OPEN.store(false, Ordering::SeqCst);
    if let Some(live) = taken {
        live.widgets.panel.orderOut(None);
        live.widgets.panel.close();
        // Drop Retained panel/target after close returns.
    }
}

pub fn toggle(snapshot: StatusSnapshot, on_action: ActionFn, anchor: Option<TrayAnchor>) {
    let run = move || {
        if OPEN.load(Ordering::SeqCst) {
            close_inner();
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

pub fn refresh(snapshot: StatusSnapshot) {
    let run = move || {
        LIVE.with(|c| {
            if let Some(live) = c.borrow().as_ref() {
                // Status only — do not clobber delayed upload list / re-read logs.
                paint_status(&live.widgets, &snapshot);
            }
        });
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

fn show_main(snapshot: StatusSnapshot, on_action: ActionFn, anchor: Option<TrayAnchor>) {
    let mtm = MainThreadMarker::new().expect("popover on main");

    let focused = LIVE.with(|c| {
        let mut slot = c.borrow_mut();
        if let Some(live) = slot.as_mut() {
            live.on_action = on_action.clone();
            paint_status(&live.widgets, &snapshot);
            paint_uploads_placeholder(&live.widgets);
            position_panel(&live.widgets.panel, anchor);
            live.widgets.panel.makeKeyAndOrderFront(None);
            live.widgets.panel.orderFrontRegardless();
            true
        } else {
            false
        }
    });
    if focused {
        OPEN.store(true, Ordering::SeqCst);
        schedule_upload_fill();
        return;
    }

    let widgets = build(mtm);
    paint_status(&widgets, &snapshot);
    paint_uploads_placeholder(&widgets);
    position_panel(&widgets.panel, anchor);
    widgets.panel.makeKeyAndOrderFront(None);
    widgets.panel.orderFrontRegardless();
    OPEN.store(true, Ordering::SeqCst);
    LIVE.with(|c| *c.borrow_mut() = Some(Live { widgets, on_action }));
    schedule_upload_fill();
}

/// ~1s after open: read upload names off main, then paint on main. Close bumps
/// `FILL_GEN` so a pending timer does no I/O / UI work.
fn schedule_upload_fill() {
    let gen = FILL_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    Queue::global(QueuePriority::Background).exec_after(UPLOAD_FILL_DELAY, move || {
        if FILL_GEN.load(Ordering::SeqCst) != gen || !OPEN.load(Ordering::SeqCst) {
            return;
        }
        let names = crate::logs::recent_sync_lines(PREVIEW_N);
        crate::logs::append(&format!(
            "popover: filled {} recent upload(s)",
            names.len()
        ));
        if FILL_GEN.load(Ordering::SeqCst) != gen || !OPEN.load(Ordering::SeqCst) {
            return;
        }
        Queue::main().exec_async(move || {
            if FILL_GEN.load(Ordering::SeqCst) != gen || !OPEN.load(Ordering::SeqCst) {
                return;
            }
            LIVE.with(|c| {
                if let Some(live) = c.borrow().as_ref() {
                    paint_uploads(&live.widgets, &names);
                }
            });
        });
    });
}

fn position_panel(panel: &NSPanel, anchor: Option<TrayAnchor>) {
    let mtm = MainThreadMarker::new().expect("main");
    // TrayAnchor = status-item button in Cocoa screen coords (origin bottom-left).
    // MenuMeters-style: flush under icon (oy = y - H - gap), left-aligned to icon.
    // Do NOT use tray-icon physical/flipped event.rect (that centered mid-screen).
    const GAP: f64 = 2.0;
    let (ox, oy) = if let Some(a) = anchor.filter(|a| a.w > 0.0 && a.h > 0.0) {
        (a.x, a.y - H - GAP)
    } else {
        // Fallback: mouse in Cocoa coords (still under menubar when click is fresh).
        let mouse = NSEvent::mouseLocation();
        (mouse.x - W / 2.0, mouse.y - H - GAP)
    };

    let vis = NSScreen::mainScreen(mtm)
        .map(|s| s.visibleFrame())
        .unwrap_or(NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: 1280.0,
                height: 800.0,
            },
        });
    let mut ox = ox;
    let mut oy = oy;
    // Horizontal clamp only — never push vertically to screen middle.
    if ox < vis.origin.x + 4.0 {
        ox = vis.origin.x + 4.0;
    }
    if ox + W > vis.origin.x + vis.size.width - 4.0 {
        ox = vis.origin.x + vis.size.width - W - 4.0;
    }
    if oy < vis.origin.y + 4.0 {
        oy = vis.origin.y + 4.0;
    }
    panel.setFrameOrigin(NSPoint { x: ox, y: oy });
    panel.setContentSize(NSSize {
        width: W,
        height: H,
    });
}

fn paint_status(ui: &Widgets, s: &StatusSnapshot) {
    let (title, detail) = if s.connected {
        (
            "Connected",
            if s.syncing {
                if s.sync_status.is_empty() {
                    "Uploading…"
                } else {
                    s.sync_status.as_str()
                }
            } else {
                "Backups running"
            },
        )
    } else if s.server_status.contains("pair") || s.server_status.contains("Re-pair") {
        ("Not Connected", s.server_status.as_str())
    } else {
        ("Not Connected", "Open window to connect")
    };
    ui.status_title.setStringValue(&NSString::from_str(title));
    ui.status_detail.setStringValue(&NSString::from_str(detail));
}

fn paint_uploads_placeholder(ui: &Widgets) {
    ui.activity.setStringValue(&NSString::from_str("…"));
    ui.activity
        .setTextColor(Some(&crate::macos::brand::caption()));
}

fn paint_uploads(ui: &Widgets, names: &[String]) {
    let (text, empty) = if names.is_empty() {
        ("No recent uploads".to_string(), true)
    } else {
        (
            names
                .iter()
                .take(PREVIEW_N)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            false,
        )
    };
    ui.activity.setStringValue(&NSString::from_str(&text));
    if empty {
        ui.activity
            .setTextColor(Some(&crate::macos::brand::caption()));
    } else {
        ui.activity
            .setTextColor(Some(&crate::macos::brand::ink()));
    }
}

fn build(mtm: MainThreadMarker) -> Widgets {
    let target: Retained<PopoverTarget> =
        unsafe { msg_send![super(PopoverTarget::alloc(mtm).set_ivars(PopoverTargetIvars)), init] };

    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        rect(0.0, 0.0, W, H),
        style,
        NSBackingStoreType::Buffered,
        false,
    );
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setFloatingPanel(true);
    panel.setLevel(NSPopUpMenuWindowLevel);
    panel.setHasShadow(true);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setMovableByWindowBackground(false);
    panel.setDelegate(Some(ProtocolObject::from_ref(&*target)));

    let fx = NSVisualEffectView::new(mtm);
    fx.setFrame(rect(0.0, 0.0, W, H));
    fx.setMaterial(NSVisualEffectMaterial::Popover);
    fx.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    fx.setState(NSVisualEffectState::Active);
    fx.setWantsLayer(true);
    if let Some(layer) = fx.layer() {
        unsafe {
            let _: () = msg_send![&*layer, setCornerRadius: RADIUS];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    panel.setContentView(Some(&fx));

    let inner = W - PAD * 2.0;
    let mut y = H - PAD;

    y -= 22.0;
    let brand_icon = NSImageView::new(mtm);
    brand_icon.setFrame(rect(PAD, y, 18.0, 18.0));
    brand_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    let mark = crate::macos::brand::mark();
    brand_icon.setImage(Some(&mark));
    if mark.isTemplate() {
        brand_icon.setContentTintColor(Some(&crate::macos::brand::green()));
    }
    fx.addSubview(&brand_icon);
    let brand_name = label(
        mtm,
        "Backup Sync Tool",
        rect(PAD + 24.0, y, inner - 24.0, 18.0),
        12.0,
        true,
    );
    brand_name.setTextColor(Some(&crate::macos::brand::green()));
    fx.addSubview(&brand_name);

    y -= 22.0;
    let status_title = label(mtm, "Not Connected", rect(PAD, y, inner, 18.0), 14.0, true);
    status_title.setTextColor(Some(&crate::macos::brand::ink()));
    fx.addSubview(&status_title);

    y -= 18.0;
    let status_detail = label(mtm, "", rect(PAD, y, inner, 15.0), 12.0, false);
    status_detail.setTextColor(Some(&crate::macos::brand::caption()));
    fx.addSubview(&status_detail);

    y -= 16.0;
    let cap = label(mtm, "Recent uploads", rect(PAD, y, inner, 14.0), 11.0, true);
    cap.setTextColor(Some(&crate::macos::brand::ink()));
    fx.addSubview(&cap);

    // ~5 lines of filenames; multiline must be off single-line mode or \n is invisible.
    y -= 92.0;
    let activity = label(mtm, "…", rect(PAD, y, inner, 88.0), 11.0, false);
    activity.setUsesSingleLineMode(false);
    activity.setMaximumNumberOfLines(PREVIEW_N as isize);
    activity.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    activity.setTextColor(Some(&crate::macos::brand::caption()));
    fx.addSubview(&activity);

    y -= 36.0;
    let open = NSButton::new(mtm);
    open.setTitle(&NSString::from_str("Open Window…"));
    open.setBezelStyle(NSBezelStyle::Push);
    open.setControlSize(objc2_app_kit::NSControlSize::Small);
    open.setFrame(rect(PAD, y, inner, 28.0));
    open.setBezelColor(Some(&crate::macos::brand::green()));
    unsafe {
        open.setTarget(Some(&*( &*target as *const PopoverTarget as *const AnyObject)));
        open.setAction(Some(sel!(actOpenWindow:)));
    }
    fx.addSubview(&open);

    Widgets {
        panel,
        target,
        status_title,
        status_detail,
        activity,
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
