//! Everything Canopy draws, all in GPUI.

pub mod desk;
pub mod overlays;
pub mod settings;
pub mod stage;
pub mod theme;
pub mod welcome;
pub mod widgets;

use canopy_core::Rect;
use gpui::{point, px, size, Bounds, Pixels};

pub const CORNER_RADIUS: f32 = 10.0;

/// A rect in global coordinates as bounds in a window whose top-left corner is
/// at `origin`.
pub fn local(r: Rect, origin: (f64, f64)) -> Bounds<Pixels> {
    Bounds::new(
        point(px((r.x - origin.0) as f32), px((r.y - origin.1) as f32)),
        size(px(r.width as f32), px(r.height as f32)),
    )
}
