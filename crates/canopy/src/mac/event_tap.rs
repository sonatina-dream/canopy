//! A session event tap for mouse buttons, drags, pinches and (while a marquee
//! is active) Escape. It runs on its own thread; the handler returns true to
//! swallow an event and must return quickly.

use std::ffi::c_void;
use std::ptr;

use canopy_core::Point;
use core_foundation_sys::mach_port::CFMachPortRef;
use core_foundation_sys::runloop::CFRunLoopSourceRef;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSEventPhase};

use super::run_loop::{self, RunLoop};
use super::CGPoint;

pub type CGEventRef = *mut c_void;

pub const LEFT_MOUSE_DOWN: u32 = 1;
pub const LEFT_MOUSE_UP: u32 = 2;
pub const LEFT_MOUSE_DRAGGED: u32 = 6;
pub const KEY_DOWN: u32 = 10;
/// Raw event types: NSEventTypeGesture and NSEventTypeMagnify.
pub const GESTURE: u32 = 29;
pub const MAGNIFY: u32 = 30;
const DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

const FIELD_MOUSE_CLICK_STATE: u32 = 1;
const FIELD_KEYCODE: u32 = 9;

pub const FLAG_SHIFT: u64 = 0x2_0000;
pub const FLAG_CONTROL: u64 = 0x4_0000;
pub const FLAG_ALTERNATE: u64 = 0x8_0000;
pub const FLAG_COMMAND: u64 = 0x10_0000;
pub const FLAG_NUMERIC_PAD: u64 = 0x20_0000;
pub const FLAG_SECONDARY_FN: u64 = 0x80_0000;

type TapCallback = unsafe extern "C" fn(proxy: *mut c_void, kind: u32, event: CGEventRef, refcon: *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, callback: TapCallback, refcon: *mut c_void) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CFMachPortCreateRunLoopSource(allocator: *const c_void, port: CFMachPortRef, order: isize) -> CFRunLoopSourceRef;
    fn CFMachPortInvalidate(port: CFMachPortRef);
}

/// An event in the tap, valid for the callback.
#[derive(Clone, Copy)]
pub struct Event(CGEventRef);

impl Event {
    pub fn location(self) -> Point {
        // SAFETY: the event is valid for the callback.
        let p = unsafe { CGEventGetLocation(self.0) };
        Point::new(p.x, p.y)
    }

    pub fn flags(self) -> u64 {
        // SAFETY: the event is valid for the callback.
        unsafe { CGEventGetFlags(self.0) }
    }

    pub fn keycode(self) -> i64 {
        // SAFETY: the event is valid for the callback.
        unsafe { CGEventGetIntegerValueField(self.0, FIELD_KEYCODE) }
    }

    pub fn click_count(self) -> i64 {
        // SAFETY: the event is valid for the callback.
        unsafe { CGEventGetIntegerValueField(self.0, FIELD_MOUSE_CLICK_STATE) }
    }

    /// The pinch in this magnify event: its change, and whether it ends or starts.
    pub fn pinch(self) -> Option<(f64, Phase)> {
        let cg = self.0 as *const objc2_core_graphics::CGEvent;
        // SAFETY: this event is a CGEvent, valid for the callback.
        let ns: Retained<NSEvent> = NSEvent::eventWithCGEvent(unsafe { &*cg })?;
        let phase = ns.phase();
        let kind = if phase.contains(NSEventPhase::Ended) || phase.contains(NSEventPhase::Cancelled) {
            Phase::Ended
        } else if phase.contains(NSEventPhase::Began) {
            Phase::Began
        } else {
            Phase::Changed
        };
        Some((ns.magnification(), kind))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Began,
    Changed,
    Ended,
}

type Handler = Box<dyn FnMut(u32, Event) -> bool + Send>;

struct Context {
    handler: Handler,
    tap: CFMachPortRef,
}

unsafe extern "C" fn callback(_: *mut c_void, kind: u32, event: CGEventRef, refcon: *mut c_void) -> CGEventRef {
    // SAFETY: `refcon` is the leaked `Context`, used only on the tap's thread.
    let context = unsafe { &mut *refcon.cast::<Context>() };
    if kind == DISABLED_BY_TIMEOUT || kind == DISABLED_BY_USER_INPUT {
        crate::clog!("INPUT macOS turned the event tap off ({}); turning it back on", if kind == DISABLED_BY_TIMEOUT { "too slow" } else { "user input" });
        // SAFETY: the tap outlives its callback.
        unsafe { CGEventTapEnable(context.tap, true) };
        return event;
    }
    if (context.handler)(kind, Event(event)) {
        ptr::null_mut()
    } else {
        event
    }
}

pub struct EventTap {
    tap: usize,
    source: usize,
    run_loop: RunLoop,
}

impl EventTap {
    /// Starts the tap on a thread of its own. `None` when macOS refuses it.
    pub fn start(handler: Handler) -> Option<EventTap> {
        let (run_loop, created) = run_loop::start("canopy.input", move |run_loop| {
            let types = [LEFT_MOUSE_DOWN, LEFT_MOUSE_UP, LEFT_MOUSE_DRAGGED, KEY_DOWN, GESTURE, MAGNIFY];
            let mask = types.iter().fold(0u64, |m, t| m | (1 << t));
            let context = Box::into_raw(Box::new(Context { handler, tap: ptr::null_mut() }));
            // SAFETY: `context` is leaked for the life of the tap; session tap at the head.
            unsafe {
                let tap = CGEventTapCreate(1, 0, 0, mask, callback, context.cast());
                if tap.is_null() {
                    drop(Box::from_raw(context));
                    return None;
                }
                (*context).tap = tap;
                let source = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
                run_loop.add_source(source);
                CGEventTapEnable(tap, true);
                Some((tap as usize, source as usize))
            }
        });
        let (tap, source) = created?;
        Some(EventTap { tap, source, run_loop })
    }

    pub fn stop(self) {
        // SAFETY: the tap and its source are still alive; invalidating ends them.
        unsafe {
            CGEventTapEnable(self.tap as CFMachPortRef, false);
            CFMachPortInvalidate(self.tap as CFMachPortRef);
        }
        self.run_loop.remove_source(self.source as CFRunLoopSourceRef);
    }
}
