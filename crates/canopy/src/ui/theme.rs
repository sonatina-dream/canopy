//! Colours for what Canopy draws, following the system's appearance.

use gpui::{hsla, rgb, Hsla, Rgba, Window, WindowAppearance};
use objc2_app_kit::{NSColor, NSColorSpace};

#[derive(Clone, Copy)]
pub struct Theme {
    pub window: Hsla,
    pub group: Hsla,
    pub separator: Hsla,
    pub text: Hsla,
    pub secondary: Hsla,
    pub control: Hsla,
    pub control_border: Hsla,
    pub track: Hsla,
    pub accent: Hsla,
    pub green: Hsla,
    pub red: Hsla,
}

pub fn is_dark(window: &Window) -> bool {
    matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

pub fn theme(window: &Window) -> Theme {
    let accent = accent();
    if is_dark(window) {
        Theme {
            window: Hsla::from(rgb(0x1e1e1e)),
            group: Hsla::from(rgb(0x2a2a2a)),
            separator: hsla(0.0, 0.0, 1.0, 0.09),
            text: hsla(0.0, 0.0, 1.0, 0.9),
            secondary: hsla(0.0, 0.0, 1.0, 0.55),
            control: hsla(0.0, 0.0, 1.0, 0.11),
            control_border: hsla(0.0, 0.0, 1.0, 0.08),
            track: hsla(0.0, 0.0, 1.0, 0.16),
            accent,
            green: Hsla::from(rgb(0x32d74b)),
            red: Hsla::from(rgb(0xff453a)),
        }
    } else {
        Theme {
            window: Hsla::from(rgb(0xf0f0f0)),
            group: Hsla::from(rgb(0xffffff)),
            separator: hsla(0.0, 0.0, 0.0, 0.08),
            text: hsla(0.0, 0.0, 0.0, 0.85),
            secondary: hsla(0.0, 0.0, 0.0, 0.5),
            control: hsla(0.0, 0.0, 1.0, 1.0),
            control_border: hsla(0.0, 0.0, 0.0, 0.14),
            track: hsla(0.0, 0.0, 0.0, 0.12),
            accent,
            green: Hsla::from(rgb(0x28a745)),
            red: Hsla::from(rgb(0xd70015)),
        }
    }
}

pub fn window_background(window: &Window) -> Hsla {
    theme(window).window
}

/// The accent colour the user picked in System Settings.
pub fn accent() -> Hsla {
    let colour = NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace());
    colour.map_or(Hsla::from(rgb(0x007aff)), |c| {
        Hsla::from(Rgba {
            r: c.redComponent() as f32,
            g: c.greenComponent() as f32,
            b: c.blueComponent() as f32,
            a: 1.0,
        })
    })
}
