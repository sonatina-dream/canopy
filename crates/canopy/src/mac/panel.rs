//! The borderless panels Canopy draws in. GPUI opens them; this gives them the
//! character overlays need, which GPUI has no options for: a window level, no
//! title bar or shadow, ignoring the mouse, belonging to a Space, and never
//! taking the keyboard from the app in use.

use std::ffi::{c_char, c_void};
use std::sync::Once;

use canopy_core::Rect;
use dispatch2::DispatchQueue;
use gpui::{AnyWindowHandle, App, Bounds, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{class, msg_send, sel, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSColor, NSView, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowLevel,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::displays::{ns_from_ax, primary_height};

/// `NSNormalWindowLevel`, `NSFloatingWindowLevel`, and the level of utility
/// windows, which sits above ordinary windows and below the menu bar.
pub const LEVEL_NORMAL: isize = 0;
pub const LEVEL_FLOATING: isize = 3;
pub const LEVEL_UTILITY: isize = 19;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowLevelForKey(key: i32) -> i32;
}

extern "C" {
    fn class_replaceMethod(class: &AnyClass, name: Sel, imp: extern "C" fn(&AnyObject, Sel) -> Bool, types: *const c_char) -> *const c_void;
}

/// Just above the desktop icons, below every window.
pub fn level_above_desktop_icons() -> isize {
    const DESKTOP_ICON_LEVEL_KEY: i32 = 18;
    // SAFETY: plain constant lookup.
    unsafe { CGWindowLevelForKey(DESKTOP_ICON_LEVEL_KEY) as isize + 1 }
}

#[derive(Clone, Copy)]
pub struct PanelStyle {
    pub level: isize,
    /// Whether the panel takes the mouse.
    pub mouse: bool,
    /// Transient panels (motion, previews) appear on every Space. Others
    /// belong to the Space they were shown on, like the windows they decorate.
    pub every_space: bool,
}

/// Window options for a panel. It is opened hidden, and shown once placed.
pub fn window_options(background: WindowBackgroundAppearance) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.0), px(0.0)), size(px(64.0), px(64.0))))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: background,
        // Canopy is never the active app, and motion must stay smooth anyway.
        inactive_frame_interval: None,
        low_power_frame_interval: None,
        thermal_frame_interval: None,
        ..Default::default()
    }
}

extern "C" fn no(_: &AnyObject, _: Sel) -> Bool {
    Bool::NO
}

/// Panels can never become key or main: clicking a tab or a gap must leave the
/// keyboard with the app in use. The override goes on GPUI's panel class, which
/// only overlays use. (Not on a subclass swapped in per window: AppKit observes
/// windows through key-value observing, which depends on the object's class.)
fn keep_panels_off_the_keyboard() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for name in [sel!(canBecomeKeyWindow), sel!(canBecomeMainWindow)] {
            // SAFETY: the signature matches NSWindow's, returning BOOL.
            unsafe { class_replaceMethod(class!(GPUIPanel), name, no, c"B@:".as_ptr()) };
        }
    });
}

pub fn ns_window(window: &Window) -> Retained<NSWindow> {
    let handle = HasWindowHandle::window_handle(window).expect("the window is open");
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { unreachable!("Canopy only runs on macOS") };
    // SAFETY: GPUI hands back its live NSView.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window().expect("the view is in its window")
}

/// One of Canopy's panels. Main thread only.
pub struct Panel {
    window: Retained<NSWindow>,
    pub handle: AnyWindowHandle,
}

impl Panel {
    pub fn adopt(window: &Window, handle: AnyWindowHandle, style: PanelStyle) -> Panel {
        let ns = ns_window(window);
        keep_panels_off_the_keyboard();
        queued(&ns, move |ns| {
            ns.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
            ns.setLevel(style.level as NSWindowLevel);
            ns.setOpaque(false);
            ns.setBackgroundColor(Some(&NSColor::clearColor()));
            ns.setHasShadow(false);
            ns.setIgnoresMouseEvents(!style.mouse);
            ns.setHidesOnDeactivate(false);
            ns.setAnimationBehavior(NSWindowAnimationBehavior::None);
            let mut behaviour = NSWindowCollectionBehavior::Stationary | NSWindowCollectionBehavior::IgnoresCycle;
            if style.every_space {
                behaviour |= NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary;
            }
            ns.setCollectionBehavior(behaviour);
        });
        Panel { window: ns, handle }
    }

    fn ns_frame(r: Rect) -> NSRect {
        let f = ns_from_ax(r, primary_height());
        NSRect::new(NSPoint::new(f.origin.x, f.origin.y), NSSize::new(f.size.width, f.size.height))
    }

    pub fn place(&self, r: Rect) {
        let frame = Self::ns_frame(r);
        queued(&self.window, move |w| w.setFrame_display(frame, true));
    }

    /// Moves to `r` over `duration` seconds.
    pub fn glide(&self, r: Rect, duration: f64) {
        let frame = Self::ns_frame(r);
        queued(&self.window, move |w| {
            animate(w, duration, move |animator| animator.setFrame_display(frame, true));
        });
    }

    pub fn order_front(&self) {
        queued(&self.window, |w| w.orderFrontRegardless());
    }

    pub fn order_out(&self) {
        queued(&self.window, |w| w.orderOut(None));
    }

    /// Just above another app's window.
    pub fn order_above(&self, window_number: isize) {
        queued(&self.window, move |w| w.orderWindow_relativeTo(NSWindowOrderingMode::Above, window_number));
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// The sizes of the window, its content view and that view's first
    /// subview, which is GPUI's (for the test harness).
    pub fn sizes(&self) -> [(f64, f64); 3] {
        let size = |r: NSRect| (r.size.width, r.size.height);
        let content = self.window.contentView();
        let inner = content.as_ref().and_then(|c| c.subviews().firstObject());
        [
            size(self.window.frame()),
            content.map_or((0.0, 0.0), |c| size(c.frame())),
            inner.map_or((0.0, 0.0), |v| size(v.frame())),
        ]
    }

    pub fn set_alpha(&self, alpha: f64) {
        queued(&self.window, move |w| w.setAlphaValue(alpha));
    }

    /// Clips the window's content to rounded corners.
    pub fn round_corners(&self, radius: f64) {
        queued(&self.window, move |w| {
            if let Some(view) = w.contentView() {
                view.setWantsLayer(true);
                // SAFETY: a layer-backed view's layer, set up just above.
                unsafe {
                    let layer: *mut AnyObject = msg_send![&*view, layer];
                    if !layer.is_null() {
                        let _: () = msg_send![layer, setCornerRadius: radius];
                        let _: () = msg_send![layer, setMasksToBounds: true];
                    }
                }
            }
        });
    }

    pub fn close(self, cx: &mut App) {
        let _ = self.handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Fades a window to `alpha` over `duration` seconds, drawn by the window server.
pub fn fade(window: &Retained<NSWindow>, alpha: f64, duration: f64) {
    queued(window, move |w| animate(w, duration, move |animator| animator.setAlphaValue(alpha)));
}

/// Takes a window off the screen, after everything already queued for it.
pub fn hide(window: &Retained<NSWindow>) {
    queued(window, |w| w.orderOut(None));
}

fn animate(window: &Retained<NSWindow>, duration: f64, body: impl FnOnce(&NSWindow) + 'static) {
    let window = window.clone();
    let body = std::cell::Cell::new(Some(body));
    NSAnimationContext::runAnimationGroup(&block2::RcBlock::new(move |context: std::ptr::NonNull<NSAnimationContext>| {
        // SAFETY: AppKit passes a valid context.
        unsafe { context.as_ref() }.setDuration(duration);
        if let Some(body) = body.take() {
            body(&window.animator());
        }
    }));
}

/// A closure and window bound for the main thread, which they never leave.
struct OnMain<T>(T);

// SAFETY: the value is created on the main thread and only used back on it.
unsafe impl<T> Send for OnMain<T> {}

impl<T> OnMain<T> {
    fn take(self) -> T {
        self.0
    }
}

/// Changes a window from the main queue, in order. Resizing an NSWindow calls
/// back into GPUI at once, and GPUI quietly drops that if it is on the stack
/// (as it is whenever the coordinator runs): the view would keep drawing at
/// its old size. From the queue, GPUI is never on the stack. Everything that
/// changes a panel comes this way, so the changes can't overtake each other.
fn queued(window: &Retained<NSWindow>, body: impl FnOnce(&Retained<NSWindow>) + 'static) {
    let job = OnMain((window.clone(), body));
    DispatchQueue::main().exec_async(move || {
        let (window, body) = job.take();
        body(&window);
    });
}

/// The main thread's marker, for AppKit calls that want proof.
pub fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("this runs on the main thread")
}
