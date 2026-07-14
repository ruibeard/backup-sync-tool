//! Pair UI matching Windows QR window: modeless panel, big QR, code, status.

use dispatch::Queue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType,
    NSBezelStyle, NSButton, NSFont, NSImage, NSImageScaling, NSImageView, NSPanel, NSTextAlignment,
    NSTextField, NSView, NSWindow, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSData, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use qrcodegen::{QrCode, QrCodeEcc};
use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;

/// NSPanel is !Send; only touch on main queue.
struct PairPanel {
    panel: Retained<NSPanel>,
    #[allow(dead_code)]
    link_target: Retained<PairLinkTarget>,
}
unsafe impl Send for PairPanel {}
unsafe impl Sync for PairPanel {}

static PAIR_PANEL: Mutex<Option<PairPanel>> = Mutex::new(None);
static EXTERNAL_PICKER_ACTIVE: AtomicBool = AtomicBool::new(false);
static PAIR_ACTION: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairPanelAction {
    Cancel,
    ChangeServer,
}

struct PairLinkIvars {
    url: RefCell<String>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "BstPairLinkTarget"]
    #[ivars = PairLinkIvars]
    struct PairLinkTarget;

    unsafe impl NSObjectProtocol for PairLinkTarget {}

    impl PairLinkTarget {
        #[unsafe(method(actOpenLink:))]
        fn act_open_link(&self, _: Option<&AnyObject>) {
            let url = self.ivars().url.borrow().clone();
            if let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(&url)) {
                let _ = NSWorkspace::sharedWorkspace().openURL(&nsurl);
            }
        }

        #[unsafe(method(actCancelPair:))]
        fn act_cancel_pair(&self, _: Option<&AnyObject>) {
            PAIR_ACTION.store(1, Ordering::SeqCst);
            close_pair_panel_inner();
        }

        #[unsafe(method(actChangeServer:))]
        fn act_change_server(&self, _: Option<&AnyObject>) {
            PAIR_ACTION.store(2, Ordering::SeqCst);
            close_pair_panel_inner();
        }
    }
);

fn pbcopy(text: &str) {
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn write_qr_png(approve_url: &str) -> Result<Vec<u8>, String> {
    let qr = QrCode::encode_text(approve_url, QrCodeEcc::Medium)
        .map_err(|e| format!("QR encode failed: {e}"))?;
    let modules = qr.size();
    let scale = 8i32;
    let quiet = 4i32;
    let px = ((modules + quiet * 2) * scale) as u32;
    let mut img = image::RgbImage::from_pixel(px, px, image::Rgb([255, 255, 255]));
    for y in 0..modules {
        for x in 0..modules {
            if !qr.get_module(x, y) {
                continue;
            }
            let x0 = ((x + quiet) * scale) as u32;
            let y0 = ((y + quiet) * scale) as u32;
            for dy in 0..scale as u32 {
                for dx in 0..scale as u32 {
                    img.put_pixel(x0 + dx, y0 + dy, image::Rgb([0, 0, 0]));
                }
            }
        }
    }
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| format!("QR PNG encode: {e}"))?;
    Ok(buf)
}

fn label(
    mtm: MainThreadMarker,
    text: &str,
    frame: NSRect,
    size: f64,
    bold: bool,
) -> Retained<NSTextField> {
    let field = NSTextField::new(mtm);
    field.setStringValue(&NSString::from_str(text));
    field.setEditable(false);
    field.setBezeled(false);
    field.setDrawsBackground(false);
    field.setSelectable(true);
    field.setFrame(frame);
    field.setAlignment(NSTextAlignment::Center);
    let font = if bold {
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    field.setFont(Some(&font));
    field
}

pub fn pair_started(code: &str, approve_url: &str, api_base: &str) {
    crate::logs::append(&format!("pair_started code={code} api_base={api_base}"));
    pbcopy(code);

    let code = code.to_string();
    let url = approve_url.to_string();
    let api_base = api_base.to_string();
    let png = write_qr_png(approve_url).ok();
    PAIR_ACTION.store(0, Ordering::SeqCst);

    Queue::main().exec_async(move || {
        show_pair_panel(&code, &url, &api_base, png.as_deref());
    });
}

fn show_pair_panel(code: &str, approve_url: &str, api_base: &str, png: Option<&[u8]>) {
    let mtm = MainThreadMarker::new().expect("pair panel on main thread");
    let app = NSApplication::sharedApplication(mtm);

    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    close_pair_panel_inner();

    // Match Windows PAIR_QR_CLIENT ~380×500.
    let width = 380.0;
    let height = 560.0;
    let rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize { width, height },
    };
    let style = NSWindowStyleMask::Titled;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        rect,
        style,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setTitle(&NSString::from_str("Pair Backup Sync Tool"));
    panel.setFloatingPanel(true);

    let content = panel.contentView().expect("content view");

    content.addSubview(&label(
        mtm,
        "Scan to pair with the server",
        NSRect {
            origin: NSPoint {
                x: 18.0,
                y: height - 42.0,
            },
            size: NSSize {
                width: width - 36.0,
                height: 24.0,
            },
        },
        14.0,
        true,
    ));

    let qr_size = 240.0;
    let qr_x = (width - qr_size) / 2.0;
    let qr_y = height - 52.0 - qr_size;
    if let Some(bytes) = png {
        let data = NSData::with_bytes(bytes);
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            let view = NSImageView::new(mtm);
            view.setImage(Some(&image));
            view.setFrame(NSRect {
                origin: NSPoint { x: qr_x, y: qr_y },
                size: NSSize {
                    width: qr_size,
                    height: qr_size,
                },
            });
            view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
            content.addSubview(&view);
        }
    }

    content.addSubview(&label(
        mtm,
        "Waiting for admin approval...",
        NSRect {
            origin: NSPoint { x: 18.0, y: 188.0 },
            size: NSSize {
                width: width - 36.0,
                height: 22.0,
            },
        },
        13.0,
        true,
    ));

    content.addSubview(&label(
        mtm,
        &format!("Code: {code}"),
        NSRect {
            origin: NSPoint { x: 18.0, y: 160.0 },
            size: NSSize {
                width: width - 36.0,
                height: 28.0,
            },
        },
        20.0,
        true,
    ));

    content.addSubview(&label(
        mtm,
        &format!("Server: {api_base}"),
        NSRect {
            origin: NSPoint { x: 18.0, y: 138.0 },
            size: NSSize {
                width: width - 36.0,
                height: 18.0,
            },
        },
        11.0,
        false,
    ));

    content.addSubview(&label(
        mtm,
        "This code expires in 10 minutes · copied to clipboard",
        NSRect {
            origin: NSPoint { x: 18.0, y: 116.0 },
            size: NSSize {
                width: width - 36.0,
                height: 20.0,
            },
        },
        12.0,
        false,
    ));

    let link_target: Retained<PairLinkTarget> = unsafe {
        msg_send![
            super(PairLinkTarget::alloc(mtm).set_ivars(PairLinkIvars {
                url: RefCell::new(approve_url.to_string()),
            })),
            init
        ]
    };
    let link = NSButton::new(mtm);
    link.setTitle(&NSString::from_str(approve_url));
    link.setBezelStyle(NSBezelStyle::AccessoryBar);
    link.setBordered(false);
    link.setFrame(NSRect {
        origin: NSPoint { x: 18.0, y: 72.0 },
        size: NSSize {
            width: width - 36.0,
            height: 40.0,
        },
    });
    let font = NSFont::systemFontOfSize(10.0);
    link.setFont(Some(&font));
    link.setContentTintColor(Some(&crate::macos::brand::green()));
    unsafe {
        link.setTarget(Some(
            &*(&*link_target as *const PairLinkTarget as *const AnyObject),
        ));
        link.setAction(Some(sel!(actOpenLink:)));
    }
    content.addSubview(&link);

    let change = NSButton::new(mtm);
    change.setTitle(&NSString::from_str("Change Server…"));
    change.setBezelStyle(NSBezelStyle::Push);
    change.setFrame(NSRect {
        origin: NSPoint { x: 54.0, y: 28.0 },
        size: NSSize {
            width: 130.0,
            height: 28.0,
        },
    });
    unsafe {
        change.setTarget(Some(
            &*(&*link_target as *const PairLinkTarget as *const AnyObject),
        ));
        change.setAction(Some(sel!(actChangeServer:)));
    }
    content.addSubview(&change);

    let cancel = NSButton::new(mtm);
    cancel.setTitle(&NSString::from_str("Cancel"));
    cancel.setBezelStyle(NSBezelStyle::Push);
    cancel.setFrame(NSRect {
        origin: NSPoint { x: 196.0, y: 28.0 },
        size: NSSize {
            width: 130.0,
            height: 28.0,
        },
    });
    unsafe {
        cancel.setTarget(Some(
            &*(&*link_target as *const PairLinkTarget as *const AnyObject),
        ));
        cancel.setAction(Some(sel!(actCancelPair:)));
    }
    content.addSubview(&cancel);

    panel.center();
    panel.makeKeyAndOrderFront(None);
    panel.orderFrontRegardless();
    crate::logs::append("pair panel showing (Windows-style)");

    if let Ok(mut guard) = PAIR_PANEL.lock() {
        *guard = Some(PairPanel { panel, link_target });
    }
}

pub fn pair_finished() {
    Queue::main().exec_async(|| {
        close_pair_panel_inner();
        restore_accessory_policy();
    });
}

/// Close pair QR panel if showing (Cmd+W / finish).
pub fn close_pair_panel() {
    let run = || {
        PAIR_ACTION.store(1, Ordering::SeqCst);
        close_pair_panel_inner();
    };
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn pair_panel_is_open() -> bool {
    PAIR_PANEL.lock().map(|g| g.is_some()).unwrap_or(false)
}

pub fn take_pair_panel_action() -> Option<PairPanelAction> {
    match PAIR_ACTION.swap(0, Ordering::SeqCst) {
        1 => Some(PairPanelAction::Cancel),
        2 => Some(PairPanelAction::ChangeServer),
        _ => None,
    }
}

pub fn is_pair_panel_window(window: &NSWindow) -> bool {
    let Ok(guard) = PAIR_PANEL.lock() else {
        return false;
    };
    match guard.as_ref() {
        Some(PairPanel { panel, .. }) => {
            // NSPanel is NSWindow; compare object identity.
            std::ptr::eq(
                Retained::as_ptr(panel) as *const NSWindow,
                window as *const NSWindow,
            )
        }
        None => false,
    }
}

fn close_pair_panel_inner() {
    let taken = PAIR_PANEL.lock().ok().and_then(|mut guard| guard.take());
    if let Some(PairPanel { panel, .. }) = taken {
        panel.orderOut(None);
        panel.close();
    }
}

pub(crate) fn restore_accessory_policy() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // Status window needs Regular while visible — flipping to Accessory here
    // can force-close it and SIGABRT in window teardown.
    if super::status_window::is_visible() {
        return;
    }
    let keep_pair = PAIR_PANEL.lock().ok().map(|g| g.is_some()).unwrap_or(false);
    if keep_pair {
        return;
    }
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

pub fn alert(title: &str, message: &str) {
    let title = title.to_string();
    let message = message.to_string();
    // Never exec_sync from a background thread — deadlocks if main is blocked.
    if MainThreadMarker::new().is_some() {
        alert_inner(&title, &message);
    } else {
        Queue::main().exec_async(move || alert_inner(&title, &message));
    }
}

/// Folder picker via AppleScript — never touches our AppKit windows.
///
/// AppKit window teardown used to abort while the picker was open. Keeping the
/// dialog in the `osascript` process isolates it from our retained windows.
pub fn pick_folder(prompt: &str) -> Option<PathBuf> {
    if EXTERNAL_PICKER_ACTIVE.swap(true, Ordering::SeqCst) {
        crate::logs::append("pick_folder: ignored duplicate request");
        return None;
    }
    struct ResetPicker;
    impl Drop for ResetPicker {
        fn drop(&mut self) {
            EXTERNAL_PICKER_ACTIVE.store(false, Ordering::SeqCst);
        }
    }
    let _reset = ResetPicker;

    match pick_folder_via_osascript(prompt) {
        PickerResult::Picked(path) => Some(path),
        PickerResult::Cancelled | PickerResult::Failed => None,
    }
}

enum PickerResult {
    Picked(PathBuf),
    Cancelled,
    Failed,
}

fn pick_folder_via_osascript(prompt: &str) -> PickerResult {
    let escaped = escape_as_string(prompt);
    let script = format!(
        r#"try
  set theFolder to choose folder with prompt "{escaped}"
  return "BST_PICKED:" & POSIX path of theFolder
on error errorMessage number errorNumber
  if errorNumber is -128 then return "BST_CANCELLED"
  return "BST_FAILED:" & errorNumber & ":" & errorMessage
end try"#
    );
    run_osascript_path(&script)
}

fn escape_as_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn run_osascript_path(script: &str) -> PickerResult {
    let out = match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(o) => o,
        Err(err) => {
            crate::logs::append(&format!("pick_folder: spawn failed: {err}"));
            return PickerResult::Failed;
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        crate::logs::append(&format!(
            "pick_folder: failed status={}: {}",
            out.status,
            err.trim()
        ));
        return PickerResult::Failed;
    }
    let result = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if result == "BST_CANCELLED" {
        return PickerResult::Cancelled;
    }
    if let Some(path) = result.strip_prefix("BST_PICKED:") {
        if !path.is_empty() {
            return PickerResult::Picked(PathBuf::from(path));
        }
    }
    crate::logs::append(&format!("pick_folder: {result}"));
    PickerResult::Failed
}

/// Modal text prompt (NSAlert + NSTextField). Safe from a background thread via channel.
pub fn prompt_url(title: &str, message: &str, default: &str) -> Option<String> {
    let title = title.to_string();
    let message = message.to_string();
    let default = default.to_string();
    if MainThreadMarker::new().is_some() {
        return prompt_url_inner(&title, &message, &default);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    Queue::main().exec_async(move || {
        let _ = tx.send(prompt_url_inner(&title, &message, &default));
    });
    rx.recv().ok().flatten()
}

fn prompt_url_inner(title: &str, message: &str, default: &str) -> Option<String> {
    let mtm = MainThreadMarker::new().expect("prompt on main");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));

    let field = NSTextField::new(mtm);
    field.setFrame(NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 320.0,
            height: 24.0,
        },
    });
    field.setEditable(true);
    field.setBezeled(true);
    field.setDrawsBackground(true);
    field.setStringValue(&NSString::from_str(default));
    unsafe {
        field.selectText(None);
    }
    alert.setAccessoryView(Some(&*field as &NSView));

    let window = alert.window();
    window.center();
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();
    let response = alert.runModal();
    restore_accessory_policy();

    if response != NSAlertFirstButtonReturn {
        return None;
    }
    let value = field.stringValue().to_string();
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn alert_inner(title: &str, message: &str) {
    let mtm = MainThreadMarker::new().expect("alert on main");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    let window = alert.window();
    window.center();
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();
    let _ = alert.runModal();
    restore_accessory_policy();
}

pub fn pair_watch_folder_required() {
    alert(
        "Backup Sync — Pair Device",
        "Set a Watch Folder first (Open → Choose), then pair again.",
    );
}

pub fn pair_failed(message: &str) {
    let message = message.to_string();
    // Close panel + alert on one main turn (avoids finish/alert race).
    if MainThreadMarker::new().is_some() {
        close_pair_panel_inner();
        alert_inner("Backup Sync — Pair Failed", &message);
    } else {
        Queue::main().exec_async(move || {
            close_pair_panel_inner();
            alert_inner("Backup Sync — Pair Failed", &message);
        });
    }
}
