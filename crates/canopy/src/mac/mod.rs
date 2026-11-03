//! Everything that talks to macOS directly.

pub mod app_look;
pub mod ax;
pub mod ax_service;
pub mod capture;
pub mod conflicts;
pub mod displays;
pub mod event_tap;
pub mod features;
pub mod hotkeys;
pub mod menu;
pub mod panel;
pub mod run_loop;
pub mod spaces;
pub mod tracker;
pub mod window_server;
pub mod workspace;

/// A menu-bar app often has nothing on screen, so macOS would put it in App Nap:
/// late timers and low priority, felt as a stutter on the next drag. This lasts
/// as long as Canopy runs. The Mac can still go to sleep.
pub fn stay_out_of_app_nap() {
    let reason = objc2_foundation::ns_string!("Arranging windows");
    let options = objc2_foundation::NSActivityOptions::UserInitiatedAllowingIdleSystemSleep;
    std::mem::forget(objc2_foundation::NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, reason));
}

use objc2::encode::{Encode, Encoding};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

impl CGPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl CGSize {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

impl CGRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { origin: CGPoint::new(x, y), size: CGSize::new(width, height) }
    }
}

// SAFETY: these match the layout of CoreGraphics' structs.
unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[Encoding::Double, Encoding::Double]);
}
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
}
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}
