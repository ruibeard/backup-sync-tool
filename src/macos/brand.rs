//! Brand — shield green `#016630`.

use objc2::rc::Retained;
use objc2_app_kit::NSColor;

/// Brand green from shield SVG stroke `#016630`.
pub fn green() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        0x01 as f64 / 255.0,
        0x66 as f64 / 255.0,
        0x30 as f64 / 255.0,
        1.0,
    )
}
