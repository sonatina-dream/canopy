//! The Accessibility API. Calls block until the app answers, so everything
//! here runs on an app's own queue (see `ax_service`) or the input thread,
//! never on the main thread.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use canopy_core::{Point, Rect, Size};
use core_foundation::base::{CFGetTypeID, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::runloop::CFRunLoopSourceRef;

use super::{CGPoint, CGSize};

pub type AXUIElementRef = *const c_void;
pub type AXObserverRef = *const c_void;
type AXValueRef = *const c_void;
pub type AXObserverCallback =
    unsafe extern "C" fn(observer: AXObserverRef, element: AXUIElementRef, notification: CFStringRef, refcon: *mut c_void);

const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: CFTypeRef) -> i32;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> i32;
    fn AXUIElementCopyElementAtPosition(application: AXUIElementRef, x: f32, y: f32, element: *mut AXUIElementRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> i32;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXValueCreate(value_type: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, value_type: u32, out: *mut c_void) -> u8;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> u8;
    fn AXObserverCreate(pid: i32, callback: AXObserverCallback, observer: *mut AXObserverRef) -> i32;
    fn AXObserverAddNotification(observer: AXObserverRef, element: AXUIElementRef, notification: CFStringRef, refcon: *mut c_void) -> i32;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
    /// Maps an AX window element to its CGWindowID. Private but long-stable;
    /// used by most macOS window managers.
    fn _AXUIElementGetWindow(element: AXUIElementRef, id: *mut u32) -> i32;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// A retained AXUIElement. AX elements may be used from any thread.
#[derive(Clone, PartialEq)]
pub struct Element(CFType);

// SAFETY: AXUIElement is an immutable, thread-safe handle to a remote element.
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

impl Element {
    pub fn application(pid: i32) -> Self {
        // SAFETY: returns a new element under the create rule.
        let e = unsafe { Self::created(AXUIElementCreateApplication(pid)) };
        e.set_timeout(1.0);
        e
    }

    /// # Safety
    /// `ptr` is a valid AXUIElement the caller owns a reference to.
    unsafe fn created(ptr: AXUIElementRef) -> Self {
        Self(CFType::wrap_under_create_rule(ptr))
    }

    /// # Safety
    /// `ptr` is a valid AXUIElement.
    pub unsafe fn retained(ptr: AXUIElementRef) -> Self {
        Self(CFType::wrap_under_get_rule(ptr))
    }

    pub fn as_ptr(&self) -> AXUIElementRef {
        self.0.as_CFTypeRef()
    }

    fn set_timeout(&self, seconds: f32) {
        // SAFETY: the element is valid for the call.
        unsafe { AXUIElementSetMessagingTimeout(self.as_ptr(), seconds) };
    }
}

static SYSTEM_WIDE: LazyLock<Element> = LazyLock::new(|| {
    // SAFETY: returns a new element under the create rule.
    let e = unsafe { Element::created(AXUIElementCreateSystemWide()) };
    e.set_timeout(0.3);
    e
});

pub fn value(el: &Element, attribute: &str) -> Option<CFType> {
    let name = CFString::new(attribute);
    let mut out: CFTypeRef = ptr::null();
    // SAFETY: `out` receives a value under the create rule when the call succeeds.
    let status = unsafe { AXUIElementCopyAttributeValue(el.as_ptr(), name.as_concrete_TypeRef(), &mut out) };
    (status == 0 && !out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) })
}

pub fn string(el: &Element, attribute: &str) -> Option<String> {
    value(el, attribute)?.downcast_into::<CFString>().map(|s| s.to_string())
}

pub fn boolean(el: &Element, attribute: &str) -> Option<bool> {
    let v = value(el, attribute)?;
    if let Some(b) = v.downcast::<CFBoolean>() {
        return Some(bool::from(b));
    }
    v.downcast::<CFNumber>().and_then(|n| n.to_i64()).map(|n| n != 0)
}

pub fn element(el: &Element, attribute: &str) -> Option<Element> {
    let v = value(el, attribute)?;
    // SAFETY: the type id is checked before treating the value as an element.
    unsafe {
        (CFGetTypeID(v.as_CFTypeRef()) == AXUIElementGetTypeID()).then(|| Element::retained(v.as_CFTypeRef()))
    }
}

pub fn elements(el: &Element, attribute: &str) -> Vec<Element> {
    let Some(v) = value(el, attribute) else { return Vec::new() };
    let array = v.as_CFTypeRef() as CFArrayRef;
    // SAFETY: AX list attributes are CFArrays; each item is checked before use.
    unsafe {
        if CFGetTypeID(v.as_CFTypeRef()) != core_foundation_sys::array::CFArrayGetTypeID() {
            return Vec::new();
        }
        (0..CFArrayGetCount(array))
            .map(|i| CFArrayGetValueAtIndex(array, i))
            .filter(|item| CFGetTypeID(*item) == AXUIElementGetTypeID())
            .map(|item| Element::retained(item))
            .collect()
    }
}

fn typed_value<T: Default>(el: &Element, attribute: &str, value_type: u32) -> Option<T> {
    let v = value(el, attribute)?;
    let mut out = T::default();
    // SAFETY: the type id is checked, and `T` is the CG struct `value_type` names.
    let ok = unsafe {
        CFGetTypeID(v.as_CFTypeRef()) == AXValueGetTypeID()
            && AXValueGetValue(v.as_CFTypeRef(), value_type, (&mut out as *mut T).cast()) != 0
    };
    ok.then_some(out)
}

pub fn point(el: &Element, attribute: &str) -> Option<CGPoint> {
    typed_value(el, attribute, AX_VALUE_CG_POINT)
}

pub fn size(el: &Element, attribute: &str) -> Option<CGSize> {
    typed_value(el, attribute, AX_VALUE_CG_SIZE)
}

pub fn set(el: &Element, attribute: &str, value: CFTypeRef) -> bool {
    let name = CFString::new(attribute);
    // SAFETY: the element and value are valid for the call.
    unsafe { AXUIElementSetAttributeValue(el.as_ptr(), name.as_concrete_TypeRef(), value) == 0 }
}

pub fn set_bool(el: &Element, attribute: &str, on: bool) -> bool {
    set(el, attribute, CFBoolean::from(on).as_CFTypeRef())
}

fn set_typed<T>(el: &Element, attribute: &str, value_type: u32, value: &T) -> bool {
    // SAFETY: `T` is the CG struct `value_type` names; the created value is released here.
    unsafe {
        let v = AXValueCreate(value_type, (value as *const T).cast());
        if v.is_null() {
            return false;
        }
        let v = CFType::wrap_under_create_rule(v);
        set(el, attribute, v.as_CFTypeRef())
    }
}

pub fn set_point(el: &Element, p: CGPoint) -> bool {
    set_typed(el, attr::POSITION, AX_VALUE_CG_POINT, &p)
}

pub fn set_size(el: &Element, s: CGSize) -> bool {
    set_typed(el, attr::SIZE, AX_VALUE_CG_SIZE, &s)
}

/// A window element's id in the window server. `_AXUIElementGetWindow` is
/// private, and macOS has been seen refusing it for every app at once, which
/// would leave new windows unmanageable. Then the id comes from the window
/// server's list, by owner and frame. Two windows of one app stacked exactly
/// alike can't be told apart that way; the later one waits until they differ.
pub fn window_id(el: &Element) -> Option<u32> {
    let mut id = 0u32;
    // SAFETY: `id` outlives the call.
    let status = unsafe { _AXUIElementGetWindow(el.as_ptr(), &mut id) };
    if status == 0 && id != 0 {
        return Some(id);
    }
    let mut pid = 0;
    // SAFETY: `pid` outlives the call.
    if unsafe { AXUIElementGetPid(el.as_ptr(), &mut pid) } != 0 {
        return None;
    }
    let (at, extent) = (point(el, attr::POSITION)?, size(el, attr::SIZE)?);
    let matches = |f: Rect| {
        (f.x - at.x).abs() <= 2.0 && (f.y - at.y).abs() <= 2.0 && (f.width - extent.width).abs() <= 2.0 && (f.height - extent.height).abs() <= 2.0
    };
    super::window_server::on_screen().into_iter().find(|o| o.pid == pid && matches(o.frame)).map(|o| o.id as u32)
}

pub fn element_at(p: Point) -> Option<Element> {
    let mut out: AXUIElementRef = ptr::null();
    // SAFETY: `out` receives an element under the create rule when the call succeeds.
    let status = unsafe { AXUIElementCopyElementAtPosition(SYSTEM_WIDE.as_ptr(), p.x as f32, p.y as f32, &mut out) };
    (status == 0 && !out.is_null()).then(|| unsafe { Element::created(out) })
}

pub fn press(el: &Element) {
    perform(el, "AXPress");
}

pub fn perform(el: &Element, action: &str) {
    let name = CFString::new(action);
    // SAFETY: the element is valid for the call.
    unsafe { AXUIElementPerformAction(el.as_ptr(), name.as_concrete_TypeRef()) };
}

pub fn is_trusted() -> bool {
    // SAFETY: takes no arguments.
    unsafe { AXIsProcessTrusted() != 0 }
}

pub fn request_trust(prompt: bool) -> bool {
    // SAFETY: the option key is a constant, and the dictionary outlives the call.
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) != 0
    }
}

/// An app's AXObserver.
pub struct Observer(CFType);

// SAFETY: the observer is only registered and read; AX serialises its use.
unsafe impl Send for Observer {}
unsafe impl Sync for Observer {}

impl Observer {
    pub fn new(pid: i32, callback: AXObserverCallback) -> Option<Self> {
        let mut out: AXObserverRef = ptr::null();
        // SAFETY: `out` receives an observer under the create rule when the call succeeds.
        let status = unsafe { AXObserverCreate(pid, callback, &mut out) };
        (status == 0 && !out.is_null()).then(|| Self(unsafe { CFType::wrap_under_create_rule(out) }))
    }

    pub fn run_loop_source(&self) -> CFRunLoopSourceRef {
        // SAFETY: the source belongs to the observer, which outlives it.
        unsafe { AXObserverGetRunLoopSource(self.0.as_CFTypeRef()) }
    }

    pub fn add(&self, element: &Element, notification: &str, refcon: *mut c_void) {
        let name = CFString::new(notification);
        // SAFETY: `refcon` stays valid for the app's lifetime (see `tracker`).
        unsafe { AXObserverAddNotification(self.0.as_CFTypeRef(), element.as_ptr(), name.as_concrete_TypeRef(), refcon) };
    }
}

pub mod attr {
    pub const ROLE: &str = "AXRole";
    pub const SUBROLE: &str = "AXSubrole";
    pub const TITLE: &str = "AXTitle";
    pub const POSITION: &str = "AXPosition";
    pub const SIZE: &str = "AXSize";
    pub const MINIMIZED: &str = "AXMinimized";
    pub const FULL_SCREEN: &str = "AXFullScreen";
    pub const MAIN: &str = "AXMain";
    pub const WINDOWS: &str = "AXWindows";
    pub const FOCUSED_WINDOW: &str = "AXFocusedWindow";
    pub const CLOSE_BUTTON: &str = "AXCloseButton";
    pub const ENHANCED_UI: &str = "AXEnhancedUserInterface";
    pub const FULL_SCREEN_BUTTON: &str = "AXFullScreenButton";
}

pub const WINDOW_ROLE: &str = "AXWindow";
pub const STANDARD_WINDOW_SUBROLE: &str = "AXStandardWindow";

/// Whether an app has AXEnhancedUserInterface on, asked at most every 10 s.
static ENHANCED: LazyLock<Mutex<HashMap<i32, (bool, Instant)>>> = LazyLock::new(Default::default);

/// One app window, controlled through Accessibility.
pub struct AxWindow {
    pub element: Element,
    pub app: Element,
    pub pid: i32,
    pub id: u32,
}

impl AxWindow {
    /// Sets the frame: size, position, size again, so a window crossing
    /// displays isn't clamped by its old display. Browsers with
    /// AXEnhancedUserInterface on animate or ignore moves, so it is switched
    /// off around the move.
    pub fn set_frame(&self, r: Rect) {
        self.with_enhanced_ui_off(|| {
            let (origin, size) = (CGPoint::new(r.x, r.y), CGSize::new(r.width, r.height));
            set_size(&self.element, size);
            set_point(&self.element, origin);
            set_size(&self.element, size);
        });
    }

    /// The green button, relative to the window's top-left corner.
    pub fn green_button(&self) -> Option<Rect> {
        let button = element(&self.element, attr::FULL_SCREEN_BUTTON)?;
        let (at, extent) = (point(&button, attr::POSITION)?, size(&button, attr::SIZE)?);
        let origin = point(&self.element, attr::POSITION)?;
        Some(Rect::new(at.x - origin.x, at.y - origin.y, extent.width, extent.height))
    }

    pub fn current_size(&self) -> Option<Size> {
        size(&self.element, attr::SIZE).map(|s| Size::new(s.width, s.height))
    }

    fn app_is_enhanced(&self) -> bool {
        let now = Instant::now();
        let cached = ENHANCED.lock().unwrap().get(&self.pid).copied();
        if let Some((on, at)) = cached {
            if now - at < Duration::from_secs(10) {
                return on;
            }
        }
        let on = boolean(&self.app, attr::ENHANCED_UI).unwrap_or(false);
        ENHANCED.lock().unwrap().insert(self.pid, (on, now));
        on
    }

    fn with_enhanced_ui_off(&self, body: impl FnOnce()) {
        let enhanced = self.app_is_enhanced();
        if enhanced {
            set_bool(&self.app, attr::ENHANCED_UI, false);
        }
        body();
        if enhanced {
            set_bool(&self.app, attr::ENHANCED_UI, true);
        }
    }

    pub fn raise(&self) {
        perform(&self.element, "AXRaise");
    }

    pub fn close(&self) {
        if let Some(b) = element(&self.element, attr::CLOSE_BUTTON) {
            press(&b);
        }
    }

    pub fn minimize(&self) {
        set_bool(&self.element, attr::MINIMIZED, true);
    }
}
