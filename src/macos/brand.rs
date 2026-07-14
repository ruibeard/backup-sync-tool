//! Brand — shield green `#016630` + surfaces matching Windows white UI.

use objc2::rc::Retained;
use objc2_app_kit::NSColor;
use objc2_app_kit::NSImage;
use objc2_foundation::NSString;

/// Brand green from shield SVG stroke `#016630`.
pub fn green() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        0x01 as f64 / 255.0,
        0x66 as f64 / 255.0,
        0x30 as f64 / 255.0,
        1.0,
    )
}

/// Window / panel fill — Windows `C_WIN_BG` white (not system grey).
pub fn surface() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0)
}

/// Primary body text — dark, not secondary grey.
pub fn ink() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        0x33 as f64 / 255.0,
        0x33 as f64 / 255.0,
        0x33 as f64 / 255.0,
        1.0,
    )
}

/// Section captions — readable muted, not tertiary wash.
pub fn caption() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        0x55 as f64 / 255.0,
        0x55 as f64 / 255.0,
        0x55 as f64 / 255.0,
        1.0,
    )
}

/// App mark for window / popover headers (bundle icon, else SF Symbol).
pub fn mark() -> Retained<NSImage> {
    if let Some(img) = NSImage::imageNamed(&NSString::from_str("AppIcon")) {
        return img;
    }
    if let Some(img) = NSImage::imageNamed(&NSString::from_str("NSApplicationIcon")) {
        return img;
    }
    if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("checkmark.shield.fill"),
        Some(&NSString::from_str("Backup Sync Tool")),
    ) {
        img.setTemplate(true);
        return img;
    }
    NSImage::new()
}
