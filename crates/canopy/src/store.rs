//! Arrangements in Application Support; preferences and shortcuts in user
//! defaults, under the same keys the app has always used.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};

use canopy_core::{Command, KeyCombo, Preferences, SavedState, Shortcuts, Size};
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSData, NSString, NSUserDefaults};

use crate::clog;

/// The test harness points these somewhere disposable.
static FOLDER: LazyLock<Mutex<PathBuf>> = LazyLock::new(|| {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    Mutex::new(home.join("Library/Application Support/Canopy"))
});
static SUITE: OnceLock<String> = OnceLock::new();

pub fn use_folder(folder: PathBuf) {
    *FOLDER.lock().unwrap() = folder;
}

pub fn use_suite(name: &str) {
    let _ = SUITE.set(name.to_string());
}

pub fn state_path() -> PathBuf {
    FOLDER.lock().unwrap().join("arrangements.json")
}

fn defaults() -> Retained<NSUserDefaults> {
    match SUITE.get() {
        Some(name) => {
            let suite = NSString::from_str(name);
            NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&suite)).expect("suite name is valid")
        }
        None => NSUserDefaults::standardUserDefaults(),
    }
}

/// Forgets everything in the harness's suite.
pub fn clear_suite(name: &str) {
    defaults().removePersistentDomainForName(&NSString::from_str(name));
}

fn data(key: &str) -> Option<Vec<u8>> {
    defaults().dataForKey(&NSString::from_str(key)).map(|d| d.to_vec())
}

fn set_data(key: &str, bytes: &[u8]) {
    let value = NSData::with_bytes(bytes);
    // SAFETY: NSData is a property-list type.
    unsafe { defaults().setObject_forKey(Some(&value), &NSString::from_str(key)) };
}

pub fn has_key(key: &str) -> bool {
    defaults().objectForKey(&NSString::from_str(key)).is_some()
}

pub fn get_bool(key: &str) -> bool {
    defaults().boolForKey(&NSString::from_str(key))
}

pub fn set_bool(key: &str, value: bool) {
    defaults().setBool_forKey(value, &NSString::from_str(key));
}

pub fn get_int(key: &str) -> Option<i64> {
    has_key(key).then(|| defaults().integerForKey(&NSString::from_str(key)) as i64)
}

pub fn set_int(key: &str, value: i64) {
    defaults().setInteger_forKey(value as isize, &NSString::from_str(key));
}

pub fn remove_key(key: &str) {
    defaults().removeObjectForKey(&NSString::from_str(key));
}

pub fn load_state() -> Option<SavedState> {
    let bytes = fs::read(state_path()).ok()?;
    match SavedState::decode(&bytes) {
        Ok(state) => Some(state),
        Err(e) => {
            clog!("could not read saved arrangements: {e}");
            None
        }
    }
}

pub fn save_state(state: &SavedState) {
    let path = state_path();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(path.parent().expect("the state file is in a folder"))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, state.encoded()?)?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        clog!("could not save arrangements: {e}");
    }
}

/// Smallest sizes apps have refused to go below, by bundle id.
pub fn load_app_minimums() -> HashMap<String, Size> {
    data("appMinimums").and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

pub fn save_app_minimums(m: &HashMap<String, Size>) {
    if let Ok(bytes) = serde_json::to_vec(m) {
        set_data("appMinimums", &bytes);
    }
}

pub fn load_preferences() -> Preferences {
    data("preferences").and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

pub fn save_preferences(p: &Preferences) {
    if let Ok(bytes) = serde_json::to_vec(p) {
        set_data("preferences", &bytes);
    }
}

pub fn load_bindings() -> HashMap<Command, KeyCombo> {
    data("shortcuts")
        .and_then(|d| serde_json::from_slice::<HashMap<String, Option<KeyCombo>>>(&d).ok())
        .map_or_else(Shortcuts::defaults, Shortcuts::from_stored)
}

pub fn save_bindings(b: &HashMap<Command, KeyCombo>) {
    if let Ok(bytes) = serde_json::to_vec(&Shortcuts::to_stored(b)) {
        set_data("shortcuts", &bytes);
    }
}
