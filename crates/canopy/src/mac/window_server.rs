//! Fast, thread-safe questions for the window server, including the private
//! calls window managers rely on.

use std::ffi::c_void;
use std::ptr;

use canopy_core::{Point, Rect, WindowId};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};

use super::CGRect;

pub type ConnectionId = i32;

const LIST_ON_SCREEN_ONLY: u32 = 1 << 0;
const LIST_INCLUDING_WINDOW: u32 = 1 << 3;
const LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    pub fn _CGSDefaultConnection() -> ConnectionId;
    fn CGSSetConnectionProperty(cid: ConnectionId, target: ConnectionId, key: CFStringRef, value: CFTypeRef) -> i32;
    static kCGWindowNumber: CFStringRef;
    static kCGWindowLayer: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowBounds: CFStringRef;
}

/// Lets Canopy set the pointer (resize arrows over gaps) while another app
/// is active, which macOS otherwise ignores.
pub fn set_cursor_in_background() {
    let key = CFString::new("SetsCursorInBackground");
    // SAFETY: the key and value outlive the call.
    unsafe {
        let cid = _CGSDefaultConnection();
        CGSSetConnectionProperty(cid, cid, key.as_concrete_TypeRef(), CFBoolean::true_value().as_CFTypeRef());
    }
}

pub fn screen_capture_allowed() -> bool {
    // SAFETY: takes no arguments.
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn request_screen_capture() -> bool {
    // SAFETY: takes no arguments.
    unsafe { CGRequestScreenCaptureAccess() }
}

#[derive(Clone, Copy, Debug)]
pub struct OnScreen {
    pub id: WindowId,
    pub pid: i32,
    pub frame: Rect,
}

fn number(dict: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
    // SAFETY: `dict` is a window info dictionary; the value is read as a CFNumber.
    unsafe {
        let mut value: *const c_void = ptr::null();
        if core_foundation_sys::dictionary::CFDictionaryGetValueIfPresent(dict, key as *const c_void, &mut value) == 0 {
            return None;
        }
        let mut out = 0i64;
        (CFNumberGetValue(value as CFNumberRef, kCFNumberSInt64Type, (&mut out as *mut i64).cast()))
            .then_some(out)
    }
}

fn bounds(dict: CFDictionaryRef) -> Option<Rect> {
    // SAFETY: `dict` is a window info dictionary; the bounds value is a CFDictionary.
    unsafe {
        let mut value: *const c_void = ptr::null();
        if core_foundation_sys::dictionary::CFDictionaryGetValueIfPresent(dict, kCGWindowBounds as *const c_void, &mut value) == 0 {
            return None;
        }
        let mut r = CGRect::default();
        CGRectMakeWithDictionaryRepresentation(value as CFDictionaryRef, &mut r)
            .then(|| Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height))
    }
}

fn each_window(list: CFArrayRef, mut body: impl FnMut(CFDictionaryRef) -> bool) {
    if list.is_null() {
        return;
    }
    // SAFETY: the list was returned under the create rule, and holds dictionaries.
    unsafe {
        let list = CFArray::<CFType>::wrap_under_create_rule(list);
        let raw = list.as_concrete_TypeRef();
        for i in 0..CFArrayGetCount(raw) {
            if !body(CFArrayGetValueAtIndex(raw, i) as CFDictionaryRef) {
                break;
            }
        }
    }
}

/// Ordinary on-screen windows, front to back. One call to the window server,
/// well under a millisecond, safe from any thread.
pub fn on_screen() -> Vec<OnScreen> {
    let mut out = Vec::with_capacity(32);
    // SAFETY: plain query; the result is owned by `each_window`.
    let list = unsafe { CGWindowListCopyWindowInfo(LIST_ON_SCREEN_ONLY | LIST_EXCLUDE_DESKTOP_ELEMENTS, 0) };
    each_window(list, |info| {
        // SAFETY: the key constants are valid for the process's lifetime.
        let (layer, id, pid) = unsafe {
            (number(info, kCGWindowLayer), number(info, kCGWindowNumber), number(info, kCGWindowOwnerPID))
        };
        if let (Some(0), Some(id), Some(pid), Some(frame)) = (layer, id, pid, bounds(info)) {
            out.push(OnScreen { id: id as WindowId, pid: pid as i32, frame });
        }
        true
    });
    out
}

/// One window's current bounds, from the window server.
pub fn frame(id: WindowId) -> Option<Rect> {
    let mut out = None;
    // SAFETY: plain query; the result is owned by `each_window`.
    let list = unsafe { CGWindowListCopyWindowInfo(LIST_INCLUDING_WINDOW, id as u32) };
    each_window(list, |info| {
        out = bounds(info);
        false
    });
    out
}

/// The frontmost ordinary window under the pointer that isn't Canopy's.
pub fn window_at(p: Point) -> Option<(WindowId, Rect)> {
    let own = std::process::id() as i32;
    on_screen().into_iter().find(|o| o.pid != own && o.frame.contains(p)).map(|o| (o.id, o.frame))
}

/// A value out of a dictionary with string keys.
pub fn dict_value(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<CFType> {
    dict.find(CFString::new(key)).map(|v| v.clone())
}

pub fn as_u64(v: &CFType) -> Option<u64> {
    v.downcast::<CFNumber>().and_then(|n| n.to_i64()).map(|n| n as u64)
}
