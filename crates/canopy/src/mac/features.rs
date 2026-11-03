//! macOS window features that move windows on their own and fight Canopy.
//! They live in the com.apple.WindowManager preferences domain.

use std::process::Command;

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;

use crate::store;

const DOMAIN: &str = "com.apple.WindowManager";

extern "C" {
    fn CFPreferencesCopyAppValue(key: CFStringRef, application: CFStringRef) -> core_foundation::base::CFTypeRef;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MacFeature {
    EdgeTiling,
    ClickToShowDesktop,
    StageManager,
}

impl MacFeature {
    pub const ALL: [MacFeature; 3] = [MacFeature::EdgeTiling, MacFeature::ClickToShowDesktop, MacFeature::StageManager];

    fn name(self) -> &'static str {
        match self {
            MacFeature::EdgeTiling => "edgeTiling",
            MacFeature::ClickToShowDesktop => "clickToShowDesktop",
            MacFeature::StageManager => "stageManager",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            MacFeature::EdgeTiling => "Tiling by dragging to screen edges",
            MacFeature::ClickToShowDesktop => "Click wallpaper to reveal desktop",
            MacFeature::StageManager => "Stage Manager",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            MacFeature::EdgeTiling => "snaps a window to half the screen when you drag it to an edge",
            MacFeature::ClickToShowDesktop => "sweeps every window aside when you click the desktop",
            MacFeature::StageManager => "moves windows into a strip at the side of the screen",
        }
    }

    fn keys(self) -> &'static [&'static str] {
        match self {
            MacFeature::EdgeTiling => &["EnableTilingByEdgeDrag", "EnableTopTilingByEdgeDrag"],
            MacFeature::ClickToShowDesktop => &["EnableStandardClickToShowDesktop"],
            MacFeature::StageManager => &["GloballyEnabled"],
        }
    }

    /// What macOS assumes when the key is absent.
    fn default_on(self) -> bool {
        self != MacFeature::StageManager
    }

    pub fn is_on(self) -> bool {
        self.keys().iter().any(|key| {
            let (key, domain) = (CFString::new(key), CFString::new(DOMAIN));
            // SAFETY: returns a value under the create rule, or null.
            let value = unsafe { CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), domain.as_concrete_TypeRef()) };
            if value.is_null() {
                return self.default_on();
            }
            // SAFETY: `value` is non-null and ours to release.
            let value = unsafe { CFType::wrap_under_create_rule(value) };
            match value.downcast::<CFBoolean>() {
                Some(b) => bool::from(b),
                None => value.downcast::<CFNumber>().and_then(|n| n.to_i64()).map_or(self.default_on(), |n| n != 0),
            }
        })
    }

    fn remembered(self) -> String {
        format!("previous.{}", self.name())
    }

    /// Writes through `defaults` so cfprefsd and WindowManager see the change.
    pub fn set(self, on: bool) {
        let remembered = self.remembered();
        if !on && !store::has_key(&remembered) {
            store::set_bool(&remembered, self.is_on());
        }
        for key in self.keys() {
            let value = if on { "true" } else { "false" };
            let _ = Command::new("/usr/bin/defaults").args(["write", DOMAIN, key, "-bool", value]).status();
        }
        if on {
            store::remove_key(&remembered);
        }
    }

    /// Whether Canopy turned this off and can turn it back on.
    pub fn turned_off_by_canopy(self) -> bool {
        store::has_key(&self.remembered())
    }
}

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

/// Open at login, through SMAppService.
pub mod login_item {
    use super::*;

    const STATUS_ENABLED: isize = 1;

    fn main_app() -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"SMAppService")?;
        // SAFETY: +[SMAppService mainAppService] returns an object.
        unsafe { msg_send![class, mainAppService] }
    }

    pub fn is_enabled() -> bool {
        main_app().is_some_and(|s| {
            // SAFETY: -status takes nothing and returns an NSInteger.
            let status: isize = unsafe { msg_send![&*s, status] };
            status == STATUS_ENABLED
        })
    }

    pub fn set(on: bool) -> Result<(), String> {
        let service = main_app().ok_or("Login items aren't available.")?;
        // SAFETY: both methods take an error out-parameter and return BOOL.
        let result: Result<(), Retained<objc2_foundation::NSError>> = unsafe {
            if on {
                msg_send![&*service, registerAndReturnError: _]
            } else {
                msg_send![&*service, unregisterAndReturnError: _]
            }
        };
        result.map_err(|e| e.localizedDescription().to_string())
    }
}
