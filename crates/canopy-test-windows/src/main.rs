//! A throwaway app with a few plain windows for the end-to-end test.
//!   CanopyTestWindows W1 W2:slow W3:700x600 W4    (name[:minWxminH][:slow])
//! A slow window takes 45 ms for every frame change, like Finder answering
//! Accessibility.
//! stdin: "frames" prints each window's frame (top-left origin) as JSON;
//! "new <title>" opens another window (on the current desktop); "quit" exits.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, Write};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSFont, NSScreen, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use serde_json::json;

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "CanopySlowWindow"]
    #[ivars = Cell<u64>]
    struct SlowWindow;

    impl SlowWindow {
        #[unsafe(method(setFrame:display:))]
        fn set_frame(&self, frame: NSRect, display: bool) {
            let delay = self.ivars().get();
            if delay > 0 {
                std::thread::sleep(Duration::from_micros(delay));
            }
            // SAFETY: the same signature as the method being overridden.
            let _: () = unsafe { msg_send![super(self), setFrame: frame, display: display] };
        }
    }
);

thread_local! {
    static WINDOWS: RefCell<Vec<Retained<SlowWindow>>> = const { RefCell::new(Vec::new()) };
}

fn open(spec: &str, index: usize, mtm: MainThreadMarker) {
    let mut parts = spec.split(':');
    let title = parts.next().unwrap_or("Window");
    let rect = NSRect::new(
        NSPoint::new(120.0 + index as f64 * 40.0, 160.0 + index as f64 * 30.0),
        NSSize::new(520.0, 380.0),
    );
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
    let this = SlowWindow::alloc(mtm).set_ivars(Cell::new(0));
    // SAFETY: NSWindow's designated initialiser, with valid arguments.
    let w: Retained<SlowWindow> = unsafe {
        msg_send![super(this), initWithContentRect: rect, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
    };
    w.setTitle(&NSString::from_str(title));
    // SAFETY: the window is kept in `WINDOWS`, so it must outlive being closed.
    unsafe { w.setReleasedWhenClosed(false) };
    w.setRestorable(false);
    for p in parts {
        if p == "slow" {
            w.ivars().set(45_000);
            continue;
        }
        let m: Vec<f64> = p.split('x').filter_map(|v| v.parse().ok()).collect();
        if let [width, height] = m[..] {
            w.setMinSize(NSSize::new(width, height));
        }
    }
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(48.0)));
    label.setFrame(NSRect::new(NSPoint::new(20.0, 20.0), NSSize::new(300.0, 60.0)));
    if let Some(content) = w.contentView() {
        content.addSubview(&label);
    }
    w.makeKeyAndOrderFront(None);
    WINDOWS.with_borrow_mut(|list| list.push(w));
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn handle(line: &str) {
    let mtm = MainThreadMarker::new().expect("commands run on the main thread");
    let line = line.trim();
    if line == "frames" {
        let h = NSScreen::screens(mtm).firstObject().map_or(0.0, |s| s.frame().size.height);
        let frames: serde_json::Map<String, serde_json::Value> = WINDOWS.with_borrow(|list| {
            list.iter()
                .map(|w| {
                    let f = w.frame();
                    let entry = json!({
                        "frame": [f.origin.x, h - (f.origin.y + f.size.height), f.size.width, f.size.height],
                        "visible": w.isVisible(),
                        "minimized": w.isMiniaturized(),
                    });
                    (w.title().to_string(), entry)
                })
                .collect()
        });
        say(&serde_json::Value::Object(frames).to_string());
    } else if line == "quit" {
        std::process::exit(0);
    } else if let Some(spec) = line.strip_prefix("new ") {
        open(spec, WINDOWS.with_borrow(Vec::len), mtm);
        NSApplication::sharedApplication(mtm).activate();
        say("ok");
    }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("main runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let specs: Vec<String> = std::env::args().skip(1).collect();
    DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
        for (i, spec) in specs.iter().enumerate() {
            open(spec, i, mtm);
        }
        NSApplication::sharedApplication(mtm).activate();
        std::thread::spawn(|| {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                DispatchQueue::main().exec_async(move || handle(&line));
            }
            std::process::exit(0);
        });
        say("ready");
    });
    app.run();
}
