//! Connected displays, with frames in top-left-origin global coordinates (the
//! same space as Accessibility and CGWindowList).

use canopy_core::{Point, Rect};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation::uuid::CFUUID;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen};
use objc2_foundation::{ns_string, NSNumber, NSRect};

use super::CGRect;

#[link(name = "ColorSync", kind = "framework")]
extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> core_foundation_sys::uuid::CFUUIDRef;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub id: u32,
    pub uuid: String,
    pub frame: Rect,
    pub visible: Rect,
    pub name: String,
    pub scale: f64,
}

fn display_id(screen: &NSScreen) -> Option<u32> {
    let description = screen.deviceDescription();
    let value = description.objectForKey(ns_string!("NSScreenNumber"))?;
    value.downcast_ref::<NSNumber>().map(|n| n.as_u32())
}

fn screens() -> Vec<Retained<NSScreen>> {
    let mtm = MainThreadMarker::new().expect("screens are read on the main thread");
    NSScreen::screens(mtm).iter().collect()
}

impl Display {
    pub fn current() -> Vec<Display> {
        let primary = primary_height();
        screens()
            .iter()
            .filter_map(|s| {
                let id = display_id(s)?;
                // SAFETY: returns a UUID under the create rule, or null.
                let raw = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
                let uuid = if raw.is_null() {
                    format!("display-{id}")
                } else {
                    // SAFETY: `raw` is non-null and ours to release.
                    let u = unsafe { CFUUID::wrap_under_create_rule(raw) };
                    // SAFETY: returns a string under the create rule.
                    unsafe {
                        CFString::wrap_under_create_rule(core_foundation_sys::uuid::CFUUIDCreateString(
                            std::ptr::null(),
                            u.as_concrete_TypeRef(),
                        ))
                    }
                    .to_string()
                };
                Some(Display {
                    id,
                    uuid,
                    frame: ax_from_ns(s.frame(), primary),
                    visible: ax_from_ns(s.visibleFrame(), primary),
                    name: s.localizedName().to_string(),
                    scale: s.backingScaleFactor(),
                })
            })
            .collect()
    }

    pub fn screen(&self) -> Option<Retained<NSScreen>> {
        screens().into_iter().find(|s| display_id(s) == Some(self.id))
    }
}

/// Conversions between AppKit's bottom-left-origin screen space and the
/// top-left-origin space of Accessibility. Height of the primary display (the
/// one at the origin).
pub fn primary_height() -> f64 {
    screens().first().map_or(0.0, |s| s.frame().size.height)
}

fn ax_from_ns(r: NSRect, primary: f64) -> Rect {
    Rect::new(r.origin.x, primary - (r.origin.y + r.size.height), r.size.width, r.size.height)
}

pub fn ns_from_ax(r: Rect, primary: f64) -> CGRect {
    CGRect::new(r.x, primary - r.max_y(), r.width, r.height)
}

/// Whether the left mouse button is down right now, whatever events said.
pub fn left_button_held() -> bool {
    NSEvent::pressedMouseButtons() & 1 != 0
}

/// The pointer, on the main thread.
pub fn mouse() -> Point {
    let p = NSEvent::mouseLocation();
    Point::new(p.x, primary_height() - p.y)
}
