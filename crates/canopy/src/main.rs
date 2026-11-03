//! Canopy: dock, nest and zoom real windows. Menu-bar-only app.

mod bus;
mod coordinator;
mod harness;
mod imaging;
mod log;
mod mac;
mod store;
mod ui;

use std::path::PathBuf;

use futures::StreamExt;
use gpui::{ActivationPolicy, App, AppContext, Entity, QuitMode};
use serde_json::json;

use bus::Msg;
use coordinator::Coordinator;
use mac::displays::Display;
use mac::menu::MenuBar;
use mac::{ax, window_server};

const HARNESS_SUITE: &str = "app.canopy.wm.harness";

fn main() {
    log::capture_library_logs();
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    ui::welcome::set_pretend_no_screen_recording(has("--pretend-no-screen-recording"));
    let mut messages = bus::init();
    let app = gpui_platform::application();
    // canopy:// commands (SPEC §11).
    app.on_open_urls(|urls| bus::send(Msg::Urls(urls)));
    app.run(move |cx: &mut App| {
        cx.set_activation_policy(ActivationPolicy::Accessory);
        cx.set_quit_mode(QuitMode::Explicit);
        handle_signals();
        let has = |flag: &str| args.iter().any(|a| a == flag);
        if has("--smoke-test") {
            run_smoke_test();
            return;
        }
        let coordinator = cx.new(|_| Coordinator::new());
        let c = coordinator.clone();
        cx.spawn(async move |cx| {
            while let Some(msg) = messages.next().await {
                c.update(cx, |c, cx| {
                    let (what, started) = (msg.kind(), std::time::Instant::now());
                    c.on_msg(msg, cx);
                    log::slow(what, started);
                });
            }
        })
        .detach();
        let c = coordinator.clone();
        cx.on_app_quit(move |cx| {
            c.update(cx, |c, cx| c.prepare_for_quit(cx));
            async {}
        })
        .detach();

        let harness = args.iter().position(|a| a == "--harness").and_then(|i| args.get(i + 1));
        match harness {
            Some(bundle_id) => start_harness(&coordinator, bundle_id.clone(), &args, cx),
            None => coordinator.update(cx, |c, cx| {
                c.menu_bar = Some(MenuBar::new());
                c.state_changed();
                c.launch(has("--no-prompt"), cx);
            }),
        }
    });
}

/// SIGTERM and SIGINT quit cleanly, so windows come back into view.
fn handle_signals() {
    use signal_hook::consts::{SIGINT, SIGTERM};
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGINT]) {
        std::thread::spawn(move || {
            for _ in signals.forever() {
                bus::send(Msg::Quit);
            }
        });
    }
}

/// Manages only one app's windows, with disposable state, driven over stdin.
fn start_harness(coordinator: &Entity<Coordinator>, bundle_id: String, args: &[String], cx: &mut App) {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let dir = std::env::var_os("CANOPY_HARNESS_DIR")
        .map_or_else(|| std::env::temp_dir().join("canopy-harness"), PathBuf::from);
    store::use_folder(dir.clone());
    log::set_folder(dir);
    store::use_suite(HARNESS_SUITE);
    if !has("--keep-state") {
        let _ = std::fs::remove_file(store::state_path());
        store::clear_suite(HARNESS_SUITE);
    }
    let mut p = store::load_preferences();
    p.animate = has("--animate");
    // Captures can raise a macOS consent prompt for whatever launched the
    // test, and no check needs them.
    p.live_previews = has("--pictures");
    store::save_preferences(&p);
    if !ax::is_trusted() {
        println!(r#"{{"ready":false,"error":"Accessibility not allowed"}}"#);
        std::process::exit(2);
    }
    let (tap, glass) = (has("--tap"), has("--glass"));
    let c = coordinator.clone();
    coordinator.update(cx, |co, cx| {
        co.only_bundle_id = Some(bundle_id);
        co.use_event_tap = tap;
        co.use_hotkeys = false;
        co.show_glass_in_harness = glass;
        co.capture_desks = has("--pictures");
        co.start(cx);
    });
    // Answer only once the coordinator has scanned and started.
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
        if c.read_with(cx, |c, _| c.started) {
            harness::start();
            break;
        }
    })
    .detach();
}

/// Launches every part that doesn't need permissions, reports, and quits.
fn run_smoke_test() {
    let displays: Vec<_> = Display::current().iter().map(|d| json!({ "uuid": d.uuid, "visible": d.visible.to_string() })).collect();
    let home = std::env::var("HOME").unwrap_or_default();
    let info = objc2_foundation::NSBundle::mainBundle();
    let menu = mac::menu::menu_titles(mac::menu::MenuState {
        started: true,
        screen_recording: true,
        controls: true,
        saved_views: vec![1, 2],
        can_go_back: true,
        bindings: canopy_core::Shortcuts::defaults(),
        ..Default::default()
    });
    let report = json!({
        "menu": menu,
        "accessibilityTrusted": ax::is_trusted(),
        "screenRecordingAllowed": window_server::screen_capture_allowed(),
        "displays": displays,
        "urlScheme": info.objectForInfoDictionaryKey(objc2_foundation::ns_string!("CFBundleURLTypes")).is_some(),
        "defaultShortcuts": canopy_core::Shortcuts::defaults().len(),
        "statePath": store::state_path().to_string_lossy().replace(&home, "~"),
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(0);
    });
}
