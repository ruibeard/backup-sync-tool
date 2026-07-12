//! Pair UI matching Windows QR window: modeless panel, big QR, code, status.

use dispatch::Queue;
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont, NSImage,
    NSImageView, NSImageScaling, NSPanel, NSTextAlignment, NSTextField, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString, NSURL};
use qrcodegen::{QrCode, QrCodeEcc};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// NSPanel is !Send; only touch on main queue.
struct PairPanel(Retained<NSPanel>);
unsafe impl Send for PairPanel {}
unsafe impl Sync for PairPanel {}

static PAIR_PANEL: Mutex<Option<PairPanel>> = Mutex::new(None);

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

pub fn pair_started(code: &str, approve_url: &str) {
    crate::logs::append(&format!("pair_started code={code}"));
    pbcopy(code);

    let code = code.to_string();
    let url = approve_url.to_string();
    let png = write_qr_png(approve_url).ok();

    Queue::main().exec_async(move || {
        show_pair_panel(&code, &url, png.as_deref());
    });
}

fn show_pair_panel(code: &str, approve_url: &str, png: Option<&[u8]>) {
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
            origin: NSPoint { x: 18.0, y: 118.0 },
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
        "This code expires in 5 minutes · copied to clipboard",
        NSRect {
            origin: NSPoint { x: 18.0, y: 92.0 },
            size: NSSize {
                width: width - 36.0,
                height: 20.0,
            },
        },
        12.0,
        false,
    ));

    let link = label(
        mtm,
        approve_url,
        NSRect {
            origin: NSPoint { x: 18.0, y: 48.0 },
            size: NSSize {
                width: width - 36.0,
                height: 36.0,
            },
        },
        10.0,
        false,
    );
    link.setTextColor(Some(&NSColor::linkColor()));
    content.addSubview(&link);

    open_url(approve_url);

    panel.center();
    panel.makeKeyAndOrderFront(None);
    panel.orderFrontRegardless();
    crate::logs::append("pair panel showing (Windows-style)");

    if let Ok(mut guard) = PAIR_PANEL.lock() {
        *guard = Some(PairPanel(panel));
    }
}

pub fn pair_finished() {
    Queue::main().exec_async(|| {
        close_pair_panel_inner();
        let mtm = match MainThreadMarker::new() {
            Some(m) => m,
            None => return,
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    });
}

fn close_pair_panel_inner() {
    if let Ok(mut guard) = PAIR_PANEL.lock() {
        if let Some(PairPanel(panel)) = guard.take() {
            panel.orderOut(None);
            panel.close();
        }
    }
}

fn open_url(url: &str) {
    let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    let _ = NSWorkspace::sharedWorkspace().openURL(&nsurl);
}

pub fn alert(title: &str, message: &str) {
    let title = title.to_string();
    let message = message.to_string();
    Queue::main().exec_sync(move || {
        let mtm = MainThreadMarker::new().expect("alert on main");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&title));
        alert.setInformativeText(&NSString::from_str(&message));
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
    });
}

pub fn pair_watch_folder_required() {
    alert(
        "Backup Sync — Pair Device",
        "Set a Watch Folder first (menu: Set Watch Folder…), then try Pair Device again.",
    );
}

pub fn pair_failed(message: &str) {
    pair_finished();
    alert("Backup Sync — Pair Failed", message);
}

/// Launch / home chooser — what user can do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeChoice {
    SetWatch,
    Pair,
    Restore,
    Continue,
}

/// Modal popup: pick an action. Safe from any thread; if already on main, runs inline.
pub fn prompt_home(status_line: &str) -> HomeChoice {
    if MainThreadMarker::new().is_some() {
        prompt_home_inner(status_line)
    } else {
        let status_line = status_line.to_string();
        Queue::main().exec_sync(move || prompt_home_inner(&status_line))
    }
}

fn prompt_home_inner(status_line: &str) -> HomeChoice {
    use objc2_app_kit::{
        NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertThirdButtonReturn,
    };

    let mtm = MainThreadMarker::new().expect("home chooser on main");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Backup Sync Tool"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "{status_line}\n\nWhat do you want to do?"
    )));
    alert.addButtonWithTitle(&NSString::from_str("Set Watch Folder…"));
    alert.addButtonWithTitle(&NSString::from_str("Pair Device…"));
    alert.addButtonWithTitle(&NSString::from_str("Restore Backup…"));
    alert.addButtonWithTitle(&NSString::from_str("Continue in Menu Bar"));

    let window = alert.window();
    window.center();
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();

    let response = alert.runModal();
    let choice = if response == NSAlertFirstButtonReturn {
        HomeChoice::SetWatch
    } else if response == NSAlertSecondButtonReturn {
        HomeChoice::Pair
    } else if response == NSAlertThirdButtonReturn {
        HomeChoice::Restore
    } else {
        HomeChoice::Continue
    };

    if choice == HomeChoice::Continue {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
    crate::logs::append(&format!("home choice: {choice:?}"));
    choice
}
