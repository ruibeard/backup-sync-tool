//! Pair UI matching Windows QR window: modeless panel, big QR, code, status.

use dispatch::Queue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType,
    NSBezelStyle, NSButton, NSFont, NSImage, NSImageView, NSImageScaling, NSPanel, NSTextAlignment,
    NSTextField, NSView, NSWindow, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSData, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use qrcodegen::{QrCode, QrCodeEcc};
use std::cell::RefCell;
use std::io::Write;
use std::process::{Command, Stdio};
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
        .write_to(
            &mut std::io::Cursor::new(&mut buf),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("QR PNG encode: {e}"))?;
    Ok(buf)
}

fn label(mtm: MainThreadMarker, text: &str, frame: NSRect, size: f64, bold: bool) -> Retained<NSTextField> {
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
    let height = 520.0;
    let rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize { width, height },
    };
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
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
            origin: NSPoint { x: 18.0, y: 150.0 },
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
            origin: NSPoint { x: 18.0, y: 128.0 },
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
            origin: NSPoint { x: 18.0, y: 106.0 },
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
        "This code expires in 5 minutes · copied to clipboard",
        NSRect {
            origin: NSPoint { x: 18.0, y: 84.0 },
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
        origin: NSPoint { x: 18.0, y: 32.0 },
        size: NSSize {
            width: width - 36.0,
            height: 40.0,
        },
    });
    let font = NSFont::systemFontOfSize(10.0);
    link.setFont(Some(&font));
    link.setContentTintColor(Some(&crate::macos::brand::green()));
    unsafe {
        link.setTarget(Some(&*( &*link_target as *const PairLinkTarget as *const AnyObject)));
        link.setAction(Some(sel!(actOpenLink:)));
    }
    content.addSubview(&link);

    panel.center();
    panel.makeKeyAndOrderFront(None);
    panel.orderFrontRegardless();
    crate::logs::append("pair panel showing (Windows-style)");

    if let Ok(mut guard) = PAIR_PANEL.lock() {
        *guard = Some(PairPanel {
            panel,
            link_target,
        });
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
    let run = || close_pair_panel_inner();
    if MainThreadMarker::new().is_some() {
        run();
    } else {
        Queue::main().exec_async(run);
    }
}

pub fn pair_panel_is_open() -> bool {
    PAIR_PANEL
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
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

fn restore_accessory_policy() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
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

    let keep = PAIR_PANEL
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    if !keep {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }

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
    let keep = PAIR_PANEL
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    if !keep {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
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

