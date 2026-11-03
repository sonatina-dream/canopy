//! Global shortcuts through Carbon's RegisterEventHotKey, which needs no
//! Input Monitoring permission, and the keys' labels on the current layout.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{LazyLock, Mutex};

use canopy_core::shortcuts::key;
use canopy_core::{Command, KeyCombo};
use core_foundation::base::{CFType, TCFType};
use core_foundation_sys::data::{CFDataGetBytePtr, CFDataRef};
use core_foundation_sys::string::CFStringRef;

use crate::bus::{self, Msg};

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

const KEYBOARD_CLASS: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const SIGNATURE: u32 = u32::from_be_bytes(*b"CNPY");
const NOT_HANDLED: i32 = -9874;
const KEY_ACTION_DISPLAY: u16 = 3;

type EventHandler = unsafe extern "C" fn(next: *mut c_void, event: *mut c_void, user: *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandler,
        count: u32,
        list: *const EventTypeSpec,
        user: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired: u32,
        actual: *mut u32,
        size: usize,
        out_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(code: u32, modifiers: u32, id: EventHotKeyId, target: *mut c_void, options: u32, out: *mut *mut c_void) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(source: *const c_void, key: CFStringRef) -> *const c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        code: u16,
        action: u16,
        modifiers: u32,
        keyboard: u32,
        options: u32,
        dead: *mut u32,
        max: usize,
        length: *mut usize,
        chars: *mut u16,
    ) -> i32;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
}

static COMMANDS: LazyLock<Mutex<HashMap<u32, Command>>> = LazyLock::new(Default::default);

unsafe extern "C" fn pressed(_: *mut c_void, event: *mut c_void, _: *mut c_void) -> i32 {
    let mut hot_key = EventHotKeyId::default();
    // SAFETY: `hot_key` matches the parameter's type and size.
    let status = unsafe {
        GetEventParameter(
            event,
            PARAM_DIRECT_OBJECT,
            TYPE_HOT_KEY_ID,
            ptr::null_mut(),
            size_of::<EventHotKeyId>(),
            ptr::null_mut(),
            (&mut hot_key as *mut EventHotKeyId).cast(),
        )
    };
    match (status, COMMANDS.lock().unwrap().get(&hot_key.id)) {
        (0, Some(&c)) => {
            bus::send(Msg::Run(c));
            0
        }
        _ => NOT_HANDLED,
    }
}

pub struct HotkeyCenter {
    refs: Vec<usize>,
}

impl HotkeyCenter {
    pub fn new() -> Self {
        let spec = EventTypeSpec { event_class: KEYBOARD_CLASS, event_kind: HOT_KEY_PRESSED };
        let mut handler = ptr::null_mut();
        // SAFETY: `spec` outlives the call and the handler is a plain function.
        unsafe { InstallEventHandler(GetApplicationEventTarget(), pressed, 1, &spec, ptr::null_mut(), &mut handler) };
        Self { refs: Vec::new() }
    }

    pub fn register(&mut self, bindings: &HashMap<Command, KeyCombo>) {
        self.unregister_all();
        let mut sorted: Vec<(&Command, &KeyCombo)> = bindings.iter().collect();
        sorted.sort_by_key(|(c, _)| c.url());
        let mut commands = COMMANDS.lock().unwrap();
        for (next, (command, combo)) in (1u32..).zip(sorted) {
            let mut hot_key = ptr::null_mut();
            let id = EventHotKeyId { signature: SIGNATURE, id: next };
            // SAFETY: `hot_key` receives a reference the center keeps until unregistered.
            let status = unsafe {
                RegisterEventHotKey(combo.key_code, combo.modifiers, id, GetApplicationEventTarget(), 0, &mut hot_key)
            };
            if status == 0 {
                self.refs.push(hot_key as usize);
                commands.insert(next, *command);
            }
        }
    }

    pub fn unregister_all(&mut self) {
        for r in self.refs.drain(..) {
            // SAFETY: each reference came from `RegisterEventHotKey`.
            unsafe { UnregisterEventHotKey(r as *mut c_void) };
        }
        COMMANDS.lock().unwrap().clear();
    }
}

/// A shortcut as it reads on the keyboard in use, like `⌃⌥Space`.
pub fn display(combo: &KeyCombo) -> String {
    combo.display(&key_name(combo.key_code))
}

pub fn key_name(code: u32) -> String {
    match code {
        key::SPACE => "Space".into(),
        key::RETURN => "↩".into(),
        key::ESCAPE => "⎋".into(),
        key::TAB => "⇥".into(),
        key::DELETE => "⌫".into(),
        key::LEFT_ARROW => "←".into(),
        key::RIGHT_ARROW => "→".into(),
        key::UP_ARROW => "↑".into(),
        key::DOWN_ARROW => "↓".into(),
        _ => translate(code).unwrap_or_else(|| format!("#{code}")),
    }
}

/// The key's character on the current keyboard layout.
fn translate(code: u32) -> Option<String> {
    // SAFETY: the source is owned here and released below, and the layout data
    // it holds stays valid until then.
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let source = CFType::wrap_under_create_rule(source);
        let raw = TISGetInputSourceProperty(source.as_CFTypeRef(), kTISPropertyUnicodeKeyLayoutData);
        if raw.is_null() {
            return None;
        }
        let layout = CFDataGetBytePtr(raw as CFDataRef).cast();
        let mut dead = 0u32;
        let mut chars = [0u16; 4];
        let mut length = 0usize;
        let status = UCKeyTranslate(
            layout,
            code as u16,
            KEY_ACTION_DISPLAY,
            0,
            u32::from(LMGetKbdType()),
            0,
            &mut dead,
            chars.len(),
            &mut length,
            chars.as_mut_ptr(),
        );
        (status == 0 && length > 0).then(|| String::from_utf16_lossy(&chars[..length]).to_uppercase())
    }
}
