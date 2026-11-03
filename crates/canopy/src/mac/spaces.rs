//! macOS Spaces (the desktops in Mission Control), from the window server.
//! Canopy keeps one arrangement per display per Space.

use std::collections::HashMap;

use canopy_core::WindowId;
use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, GlobalQueueIdentifier};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation_sys::array::CFArrayRef;

use super::displays::Display;
use super::window_server::{_CGSDefaultConnection, as_u64, dict_value, ConnectionId};
use crate::bus::{self, Msg};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSCopyManagedDisplaySpaces(cid: ConnectionId) -> CFArrayRef;
    fn CGSCopySpacesForWindows(cid: ConnectionId, mask: i32, windows: CFArrayRef) -> CFArrayRef;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    /// The window server's id for the Space; changes across restarts.
    pub id: u64,
    /// A stable name: the Space's uuid, or "main" for the first desktop.
    pub key: String,
    /// A full-screen app's Space: Canopy leaves it alone.
    pub full_screen: bool,
}

impl Space {
    pub fn main() -> Self {
        Self { id: 0, key: "main".into(), full_screen: false }
    }
}

/// The current Space of each display, by display UUID. When displays share
/// Spaces, every display gets the same one. `None` if the lookup is unavailable.
pub fn current(displays: &[Display]) -> Option<HashMap<String, Space>> {
    // SAFETY: returns an array under the create rule, or null.
    let raw = unsafe { CGSCopyManagedDisplaySpaces(_CGSDefaultConnection()) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: the array holds string-keyed dictionaries.
    let entries = unsafe { CFArray::<CFDictionary<CFString, CFType>>::wrap_under_create_rule(raw) };
    let mut out = HashMap::new();
    for entry in entries.iter() {
        let Some(current) = dict_value(&entry, "Current Space").and_then(as_dictionary) else { continue };
        let id = dict_value(&current, "ManagedSpaceID")
            .or_else(|| dict_value(&current, "id64"))
            .and_then(|v| as_u64(&v))
            .unwrap_or(0);
        let uuid = dict_value(&current, "uuid").and_then(|v| v.downcast_into::<CFString>()).map(|s| s.to_string());
        let uuid = uuid.unwrap_or_default();
        let kind = dict_value(&current, "type").and_then(|v| as_u64(&v)).unwrap_or(0);
        let space = Space { id, key: if uuid.is_empty() { "main".into() } else { uuid }, full_screen: kind == 4 };
        let identifier = dict_value(&entry, "Display Identifier")
            .and_then(|v| v.downcast_into::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Main".into());
        match displays.iter().find(|d| d.uuid.eq_ignore_ascii_case(&identifier)) {
            Some(d) => {
                out.insert(d.uuid.clone(), space);
            }
            None => {
                // "Main": displays share one set of Spaces.
                for d in displays {
                    out.entry(d.uuid.clone()).or_insert_with(|| space.clone());
                }
            }
        }
    }
    Some(out)
}

fn as_dictionary(v: CFType) -> Option<CFDictionary<CFString, CFType>> {
    // SAFETY: the type id is checked, and these dictionaries have string keys.
    unsafe {
        (core_foundation_sys::base::CFGetTypeID(v.as_CFTypeRef()) == core_foundation_sys::dictionary::CFDictionaryGetTypeID())
            .then(|| CFDictionary::wrap_under_get_rule(v.as_CFTypeRef() as _))
    }
}

/// Finds which windows have left their world's Space, off the main thread, and
/// reports on the bus. Each ask is a world, its Space, and the windows to check.
pub fn find_moved(asks: Vec<(String, u64, Vec<WindowId>)>) {
    let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Default));
    queue.exec_async(move || {
        let answers = asks
            .into_iter()
            .map(|(key, here, windows)| {
                let moved = windows.into_iter().filter(|id| {
                    let spaces = of(*id);
                    !spaces.is_empty() && !spaces.contains(&here)
                });
                (key, moved.collect())
            })
            .collect();
        bus::send(Msg::SpacesChecked(answers));
    });
}

/// The Spaces a window is on (more than one for windows on all desktops).
pub fn of(window: WindowId) -> Vec<u64> {
    let ids = CFArray::from_CFTypes(&[CFNumber::from(window as u32 as i64)]);
    // SAFETY: returns an array of numbers under the create rule, or null.
    unsafe {
        let raw = CGSCopySpacesForWindows(_CGSDefaultConnection(), 0x7, ids.as_concrete_TypeRef());
        if raw.is_null() {
            return Vec::new();
        }
        let list = CFArray::<CFNumber>::wrap_under_create_rule(raw);
        let found = list.iter().filter_map(|n| n.to_i64()).map(|n| n as u64).collect();
        found
    }
}
