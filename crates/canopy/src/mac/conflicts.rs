//! Other window managers that move windows too. Two at once fight over every
//! window, so Canopy says so instead of quietly losing.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication};
use objc2_foundation::NSString;

use super::workspace;
use crate::clog;

/// Bundle id and name; a blank name is known but doesn't conflict.
const KNOWN: [(&str, &str); 8] = [
    ("app.trellisui.trelliswm", "TrellisWM"),
    ("bobko.aerospace", "AeroSpace"),
    ("com.knollsoft.Rectangle", "Rectangle"),
    ("com.knollsoft.Hookshot", "Hookshot"),
    ("com.amethyst.Amethyst", "Amethyst"),
    ("com.mizage.Divvy", "Divvy"),
    ("com.crowdcafe.windowmagnet", "Magnet"),
    ("com.lwouis.alt-tab-macos", ""),
];

fn name_of(bundle_id: &str) -> Option<&'static str> {
    KNOWN.iter().find(|(id, name)| *id == bundle_id && !name.is_empty()).map(|(_, name)| *name)
}

/// Asks about other window managers. Starts a modal loop, so it runs from
/// `workspace::on_main`.
pub fn warn_if_needed() {
    let running: Vec<(i32, &str)> = workspace::running_apps()
        .iter()
        .filter_map(|a| Some((a.pid, name_of(a.bundle_id.as_deref()?)?)))
        .collect();
    if running.is_empty() {
        return;
    }
    let names: Vec<&str> = running.iter().map(|(_, n)| *n).collect();
    let list = names.join(" and ");
    clog!("CONFLICT {} is running", names.join(", "));
    let mtm = MainThreadMarker::new().expect("alerts run on the main thread");
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!("{list} is also arranging windows")));
    alert.setInformativeText(&NSString::from_str(&format!(
        "Two window managers fight over every window: windows jump back, refuse to dock and feel slow. Quit {list} while you use Canopy."
    )));
    let quit = if names.len() == 1 { format!("Quit {}", names[0]) } else { "Quit Them".to_string() };
    alert.addButtonWithTitle(&NSString::from_str(&quit));
    alert.addButtonWithTitle(&NSString::from_str("Keep Both"));
    NSApplication::sharedApplication(mtm).activate();
    if alert.runModal() == NSAlertFirstButtonReturn {
        for (pid, _) in running {
            workspace::terminate(pid);
        }
    }
}
