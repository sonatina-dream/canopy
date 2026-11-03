//! End-to-end test driver: `Canopy --harness <bundle-id>` manages only that
//! app's windows and takes one command per line on stdin, answering with one
//! line of JSON. Used by Tests/E2E/e2e.py; not part of normal use.
//!
//!   dump                                  the arrangement and every managed window
//!   dock <title> screen <edge>            dock at a screen edge
//!   dock <title> <other> <edge>           dock beside another window's tile
//!   tab <title> <other>                   join another window's tab group
//!   float <title>                         drop on the Desktop
//!   toggle <title>                        the green button
//!   frame <title>                         double-click the title bar
//!   select <title>                        click its tab
//!   raise <title>                         bring it to the front
//!   close <title> | minimize <title> | restore <title>
//!   url <canopy://…>                      a command by URL
//!   overlays                              each overlay's size as asked and as drawn
//!   With --tap, real mouse input through the event tap (each press first
//!   checks that a test window, or Canopy's own handle, is under the pointer):
//!   drag <title> <x> <y> [hold]           drag the title bar to a point
//!   green <title>                         click the green button
//!   dblclick <title>                      double-click the title bar
//!   seam <n> <delta>                      drag gap handle n along its axis
//!   quit

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::time::Duration;

use canopy_core::{Axis, DropTarget, Edge, Point, Rect, WindowId};
use core_foundation::base::CFRelease;
use gpui::{AsyncApp, Context};
use serde_json::{json, Value};

use crate::bus::{self, Msg};
use crate::coordinator::input::InputMsg;
use crate::coordinator::{Coordinator, LayoutOptions};
use crate::mac::ax::{self, attr};
use crate::mac::event_tap::{self, CGEventRef};
use crate::mac::tracker::TrackedWindow;
use crate::mac::{window_server, CGPoint};

const MOUSE_MOVED: u32 = 5;
const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const LEFT_DRAGGED: u32 = 6;
const FIELD_CLICK_STATE: u32 = 1;
const HID_TAP: u32 = 0;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const c_void) -> CGEventRef;
    fn CGEventCreateMouseEvent(source: *const c_void, kind: u32, position: CGPoint, button: u32) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventPost(tap: u32, event: CGEventRef);
}

fn reply(value: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn ok() {
    reply(json!({ "ok": true }));
}

fn fail(line: &str) {
    reply(json!({ "ok": false, "error": format!("bad command: {line}") }));
}

/// Reads commands from stdin on a thread of its own.
pub fn start() {
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) => bus::send(Msg::Harness(line)),
                Err(_) => break,
            }
        }
        bus::send(Msg::HarnessClosed);
    });
    reply(json!({ "ready": true }));
}

fn window_id(c: &Coordinator, title: &str) -> Option<WindowId> {
    let windows = || c.tracker.windows.values().filter(|w| w.title == title);
    windows().find(|w| c.manageable(w)).or_else(|| windows().next()).map(TrackedWindow::id)
}

/// The topmost window at `p` must be the test window we mean to touch.
fn safe(p: Point, window: WindowId) -> bool {
    window_server::window_at(p).is_some_and(|(id, _)| id == window)
}

/// Posts real mouse events, paced like a hand, then puts the pointer back.
mod mouse {
    use super::*;

    pub fn post(kind: u32, p: Point, clicks: i64, flags: u64) {
        // SAFETY: the event is created, posted and released here.
        unsafe {
            let e = CGEventCreateMouseEvent(std::ptr::null(), kind, CGPoint::new(p.x, p.y), 0);
            if e.is_null() {
                return;
            }
            CGEventSetIntegerValueField(e, FIELD_CLICK_STATE, clicks);
            if flags != 0 {
                CGEventSetFlags(e, flags);
            }
            CGEventPost(HID_TAP, e);
            CFRelease(e);
        }
    }

    pub fn home() -> Point {
        // SAFETY: the event is created, read and released here.
        unsafe {
            let e = CGEventCreate(std::ptr::null());
            let p = CGEventGetLocation(e);
            CFRelease(e);
            Point::new(p.x, p.y)
        }
    }

    pub async fn wait(cx: &AsyncApp, seconds: f64) {
        cx.background_executor().timer(Duration::from_secs_f64(seconds)).await;
    }

    pub async fn drag(cx: &AsyncApp, a: Point, b: Point, hold: f64, flags: u64) {
        let back = home();
        post(MOUSE_MOVED, a, 1, 0);
        wait(cx, 0.15).await;
        post(LEFT_DOWN, a, 1, flags);
        wait(cx, 0.15).await;
        let steps = 24;
        for i in 1..=steps {
            wait(cx, 0.025).await;
            let t = f64::from(i) / f64::from(steps);
            post(LEFT_DRAGGED, Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t), 1, flags);
        }
        if hold > 0.0 {
            // Keep a tiny movement going so the hold timer sees a live drag.
            wait(cx, 0.1 + hold).await;
            post(LEFT_DRAGGED, b, 1, 0);
            wait(cx, 0.1).await;
        } else {
            wait(cx, 0.2).await;
        }
        post(LEFT_UP, b, 1, flags);
        wait(cx, 0.1).await;
        post(MOUSE_MOVED, back, 1, 0);
    }

    pub async fn click(cx: &AsyncApp, p: Point, count: i64) {
        let back = home();
        post(MOUSE_MOVED, p, 1, 0);
        wait(cx, 0.15).await;
        for n in 1..=count {
            post(LEFT_DOWN, p, n, 0);
            wait(cx, 0.04).await;
            post(LEFT_UP, p, n, 0);
            wait(cx, 0.08).await;
        }
        wait(cx, 0.1).await;
        post(MOUSE_MOVED, back, 1, 0);
    }
}

pub fn handle(c: &mut Coordinator, line: &str, cx: &mut Context<Coordinator>) {
    let line = line.trim();
    let parts: Vec<&str> = line.split_whitespace().collect();
    let Some(&verb) = parts.first() else { return };
    let window = |n: usize| parts.get(n).and_then(|t| window_id(c, t));
    match (verb, parts.len()) {
        ("dump", _) => return reply(dump(c)),
        ("session", _) => {
            let Some(s) = c.press.as_ref().and_then(|p| p.session.as_ref()) else {
                return reply(json!({ "ok": true, "dragging": false }));
            };
            let (target, thumbnail, display) = (s.target, s.thumbnail, s.display.clone());
            let slot = c.stage.panel(&display, cx).and_then(|p| p.slot_style(cx));
            return reply(json!({
                "ok": true, "dragging": true, "target": target_name(target), "thumbnail": thumbnail,
                "slot": slot.unwrap_or("none"),
            }));
        }
        ("log", _) => return reply(json!({ "ok": true, "log": crate::log::take() })),
        ("overlays", _) => {
            let overlays = c.overlay_report(cx);
            return reply(json!({ "ok": true, "overlays": overlays }));
        }
        ("stageShown", _) => return reply(json!({ "ok": true, "shown": c.stage.any_shown() })),
        ("stall", _) => {
            let worst = (c.reset_stall_meter() * 1000.0) as i64;
            return reply(json!({ "ok": true, "worstStallMs": worst, "tap": c.tap_running }));
        }
        ("dock", 4) => {
            let (Some(w), Some(e)) = (window(1), Edge::parse(parts[3])) else { return fail(line) };
            if parts[2] == "screen" {
                let Some(key) = c.current_world_key() else { return fail(line) };
                let world = &c.worlds[&key];
                let target = DropTarget::ScreenEdge(e).in_view(w, false, &world.navigator, &world.arrangement);
                c.drop_window(w, target, &key, false, cx);
            } else {
                let Some((key, tile)) = window(2).and_then(|other| tile_of(c, other)) else { return fail(line) };
                c.drop_window(w, DropTarget::Beside(tile, e), &key, false, cx);
            }
        }
        // A whole drag without the mouse: `hover` as often as it takes, then `release`.
        ("hover", 4) => {
            let (Some(w), Ok(x), Ok(y)) = (window(1), parts[2].parse::<f64>(), parts[3].parse::<f64>()) else { return fail(line) };
            c.hover(w, Point::new(x, y), cx);
        }
        ("release", 3) => {
            let (Ok(x), Ok(y)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) else { return fail(line) };
            c.on_input(InputMsg::PressEnded(Point::new(x, y)), cx);
        }
        ("lose-mouse-up", _) => c.lose_mouse_up(),
        ("tab", 3) => {
            let Some(w) = window(1) else { return fail(line) };
            let Some((key, tile)) = window(2).and_then(|other| tile_of(c, other)) else { return fail(line) };
            c.drop_window(w, DropTarget::Tab(tile), &key, false, cx);
        }
        ("float", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            let Some(key) = c.world_key_of(w) else { return fail(line) };
            c.drop_window(w, DropTarget::Float, &key, false, cx);
        }
        ("toggle", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            c.toggle_docked(w, cx);
        }
        ("frame", 2) => {
            let Some((key, tile)) = window(1).and_then(|w| tile_of(c, w)) else { return fail(line) };
            let world = c.worlds.get_mut(&key).expect("the key was just found");
            if world.navigator.toggle_frame(tile, &world.arrangement) {
                c.layout(&key, true, LayoutOptions::default(), cx);
            }
        }
        ("raise", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            c.focus(w);
        }
        ("select", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            let Some(key) = c.world_key_of(w) else { return fail(line) };
            c.tab_selected(w, &key, cx);
        }
        ("close", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            c.close(w);
        }
        ("minimize", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            c.minimize(w);
        }
        ("restore", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            c.set_window_minimized(w, false);
        }
        ("drag", n) if n >= 4 => {
            let (Some(w), Ok(x), Ok(y)) = (window(1), parts[2].parse::<f64>(), parts[3].parse::<f64>()) else { return fail(line) };
            let Some(f) = window_server::frame(w) else { return fail(line) };
            let start = Point::new(f.mid_x(), f.min_y() + 12.0);
            if !safe(start, w) {
                return fail(&format!("unsafe: {line}"));
            }
            let hold = parts.get(4).and_then(|h| h.parse().ok()).unwrap_or(0.0);
            cx.spawn(async move |_, cx| {
                mouse::drag(cx, start, Point::new(x, y), hold, 0).await;
                ok();
            })
            .detach();
            return;
        }
        ("green", 2) => {
            let Some(tw) = window(1).and_then(|w| c.tracker.windows.get(&w)) else { return fail(line) };
            let Some(button) = ax::element(&tw.ax.element, attr::FULL_SCREEN_BUTTON) else { return fail(line) };
            let (Some(o), Some(s)) = (ax::point(&button, attr::POSITION), ax::size(&button, attr::SIZE)) else { return fail(line) };
            let p = Point::new(o.x + s.width / 2.0, o.y + s.height / 2.0);
            if !safe(p, tw.id()) {
                return fail(&format!("unsafe: {line}"));
            }
            cx.spawn(async move |_, cx| {
                mouse::click(cx, p, 1).await;
                ok();
            })
            .detach();
            return;
        }
        ("dblclick", 2) => {
            let Some(w) = window(1) else { return fail(line) };
            let Some(f) = window_server::frame(w) else { return fail(line) };
            let p = Point::new(f.mid_x(), f.min_y() + 12.0);
            if !safe(p, w) {
                return fail(&format!("unsafe: {line}"));
            }
            cx.spawn(async move |_, cx| {
                mouse::click(cx, p, 2).await;
                ok();
            })
            .detach();
            return;
        }
        ("marquee", 5) => {
            // ⌘⇧-drag a rectangle that starts inside a test window, so nothing else is ever clicked.
            let values: Vec<f64> = parts[1..].iter().filter_map(|v| v.parse().ok()).collect();
            let [x0, y0, x1, y1] = values[..] else { return fail(line) };
            let a = Point::new(x0, y0);
            let managed = window_server::window_at(a).filter(|(id, _)| c.tracker.windows.contains_key(id));
            if !managed.is_some_and(|(id, _)| safe(a, id)) {
                return fail(&format!("unsafe: {line}"));
            }
            let flags = event_tap::FLAG_COMMAND | event_tap::FLAG_SHIFT;
            cx.spawn(async move |_, cx| {
                mouse::drag(cx, a, Point::new(x1, y1), 0.0, flags).await;
                ok();
            })
            .detach();
            return;
        }
        ("pinch", 4) => {
            // The pinch path after the event tap: a stream of totals, then the lift.
            let values: Vec<f64> = parts[1..].iter().filter_map(|v| v.parse().ok()).collect();
            let [x, y, total] = values[..] else { return fail(line) };
            let p = Point::new(x, y);
            cx.spawn(async move |this, cx| {
                for i in 1..=12 {
                    mouse::wait(cx, 0.03).await;
                    let _ = this.update(cx, |c, cx| c.pinch(total * f64::from(i) / 12.0, p, false, cx));
                }
                mouse::wait(cx, 0.09).await;
                let _ = this.update(cx, |c, cx| c.pinch(total, p, true, cx));
                ok();
            })
            .detach();
            return;
        }
        ("seam", 3) => {
            let (Ok(n), Ok(delta)) = (parts[1].parse::<usize>(), parts[2].parse::<f64>()) else { return fail(line) };
            let seam = c.active_keys().first().and_then(|k| c.worlds[k].plan.as_ref()?.layout.seams.get(n).copied());
            let Some(seam) = seam else { return fail(line) };
            let start = seam.rect.center();
            let end = match seam.axis {
                Axis::Horizontal => Point::new(start.x + delta, start.y),
                Axis::Vertical => Point::new(start.x, start.y + delta),
            };
            cx.spawn(async move |_, cx| {
                mouse::drag(cx, start, end, 0.0, 0).await;
                ok();
            })
            .detach();
            return;
        }
        ("key", 3) => {
            // A key press with modifiers, e.g. "key 124 ctrl" (next desktop).
            let Ok(code) = parts[1].parse::<u16>() else { return fail(line) };
            let mut flags = 0;
            for m in parts[2].split(',') {
                flags |= match m {
                    "ctrl" => event_tap::FLAG_CONTROL,
                    "opt" => event_tap::FLAG_ALTERNATE,
                    "shift" => event_tap::FLAG_SHIFT,
                    "cmd" => event_tap::FLAG_COMMAND,
                    _ => 0,
                };
            }
            // Arrow keys from a real keyboard carry these too; shortcuts expect them.
            if (123..=126).contains(&code) {
                flags |= event_tap::FLAG_NUMERIC_PAD | event_tap::FLAG_SECONDARY_FN;
            }
            for down in [true, false] {
                // SAFETY: the event is created, posted and released here.
                unsafe {
                    let e = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
                    CGEventSetFlags(e, flags);
                    CGEventPost(HID_TAP, e);
                    CFRelease(e);
                }
            }
        }
        #[cfg(feature = "snapshots")]
        ("fake-pictures", _) => snapshots::fake_pictures(c),
        #[cfg(feature = "snapshots")]
        ("snapshot", 2) => return snapshots::save_all(parts[1].to_string(), cx),
        #[cfg(feature = "snapshots")]
        ("welcome", 2) => {
            let step = parts[1].parse().unwrap_or(0);
            snapshots::open_welcome(c, step, cx);
        }
        ("url", 2) => c.on_msg(Msg::Urls(vec![parts[1].to_string()]), cx),
        ("quit", _) => {
            ok();
            c.on_msg(Msg::Quit, cx);
            return;
        }
        _ => return fail(line),
    }
    ok();
}

fn tile_of(c: &Coordinator, window: WindowId) -> Option<(String, canopy_core::NodeId)> {
    let world = c.world_of(window)?;
    Some((world.key(), world.arrangement.tile_of(window)?))
}

/// The drop target as the Swift harness printed it: `tab(5)` and so on.
fn target_name(t: DropTarget) -> String {
    match t {
        DropTarget::ScreenEdge(e) => format!("screenEdge({})", e.name()),
        DropTarget::Beside(id, e) => format!("beside({id}, {})", e.name()),
        DropTarget::Tab(id) => format!("tab({id})"),
        DropTarget::Floating { edge: Some(e), .. } => format!("floating({})", e.name()),
        DropTarget::Floating { edge: None, .. } => "floating(tab)".to_string(),
        DropTarget::Float => "float".to_string(),
        DropTarget::None => "none".to_string(),
    }
}

fn rect_list(r: Rect) -> Value {
    json!([r.x, r.y, r.width, r.height])
}

fn dump(c: &Coordinator) -> Value {
    let mut names: Vec<(WindowId, String)> = c.tracker.windows.values().map(|w| (w.id(), w.title.clone())).collect();
    names.sort_by_key(|(id, _)| std::cmp::Reverse(id.to_string().len()));
    let name = |id: WindowId| names.iter().find(|(w, _)| *w == id).map_or(id.to_string(), |(_, n)| n.clone());
    // Replace window ids in a tree description with titles.
    let named = |s: String| names.iter().fold(s, |s, (id, title)| s.replace(&id.to_string(), title));

    let mut keys: Vec<&String> = c.worlds.keys().collect();
    keys.sort_by_key(|k| !c.is_active(k));
    let mut worlds = Vec::new();
    for key in keys {
        let w = &c.worlds[key];
        let a = &w.arrangement;
        let framed = match w.navigator.framed_range(a) {
            Some(run) => format!("range:{}", run.iter().map(|&n| a.describe(n)).collect::<Vec<_>>().join(",")),
            None => a.describe(w.navigator.framed_node(a)),
        };
        let mut entry = json!({
            "active": c.is_active(key),
            "space": w.space,
            "visible": rect_list(w.display.visible),
            "tree": named(a.describe(a.root())),
            "framed": named(framed),
            "desktopHidden": a.is_desktop_hidden(),
            "savedViews": w.navigator.saved_slots(),
            "canGoBack": w.navigator.can_go_back(),
        });
        if let Some(p) = &w.plan {
            let visible: serde_json::Map<String, Value> = p.visible.iter().map(|(id, r)| (name(*id), rect_list(*r))).collect();
            let mut covers: Vec<String> = p
                .covers
                .keys()
                .filter_map(|t| a.node(*t).and_then(|t| a.selected_window(t)))
                .map(name)
                .collect();
            covers.sort();
            entry["visibleWindows"] = Value::Object(visible);
            entry["covers"] = json!(covers);
            entry["seams"] = json!(p.layout.seams.len());
        }
        worlds.push(entry);
    }
    // Front to back: a window's place here says what is in front of what.
    let on_screen: Vec<WindowId> = window_server::on_screen().iter().map(|o| o.id).collect();
    let depth = |id: WindowId| on_screen.iter().position(|w| *w == id);
    let mut windows: Vec<&TrackedWindow> = c
        .tracker
        .windows
        .values()
        .filter(|tw| c.only_bundle_id.as_ref().is_none_or(|only| tw.bundle_id == *only))
        .collect();
    windows.sort_by(|a, b| a.title.cmp(&b.title));
    let windows: Vec<Value> = windows
        .into_iter()
        .map(|tw| {
            let f = window_server::frame(tw.id()).or(tw.frame).unwrap_or(Rect::ZERO);
            json!({
                "id": tw.id(),
                "title": tw.title,
                "onScreen": on_screen.contains(&tw.id()),
                "depth": depth(tw.id()),
                "frame": rect_list(f),
                "docked": c.world_of(tw.id()).is_some(),
                "parked": c.parked.contains(&tw.id()),
                "minimized": tw.minimized,
                "greenButton": tw.green_button.map(rect_list),
            })
        })
        .collect();
    json!({ "worlds": worlds, "windows": windows })
}

/// Offscreen pictures of Canopy's own windows, for looking at what it draws
/// without capturing the screen.
#[cfg(feature = "snapshots")]
mod snapshots {
    use std::sync::Arc;
    use std::time::Duration;

    use canopy_core::Size;
    use gpui::Context;

    use super::ok;
    use crate::coordinator::Coordinator;
    use crate::imaging::{Colour, Pixels};
    use crate::mac::capture::{make_glass, render_image, DeskShot, Picture, WindowShot};
    use crate::mac::displays::Display;
    use crate::mac::panel::ns_window;

    fn gradient(width: usize, height: usize, tint: [u8; 3]) -> Pixels {
        let mut data = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
                let band = if (x / 64 + y / 64) % 2 == 0 { 1.0 } else { 0.86 };
                for (c, t) in tint.iter().enumerate() {
                    data.push((f32::from(*t) * (0.45 + 0.55 * if c == 2 { u } else { v }) * band) as u8);
                }
                data.push(255);
            }
        }
        Pixels { width, height, data }
    }

    /// Gives every window a picture, and each display a desktop and its glass.
    pub fn fake_pictures(c: &mut Coordinator) {
        const TINTS: [[u8; 3]; 4] = [[220, 120, 60], [70, 170, 90], [70, 90, 220], [190, 70, 170]];
        let shots = c
            .tracker
            .windows
            .values()
            .enumerate()
            .filter_map(|(i, w)| {
                let f = w.frame?;
                let (width, height) = (f.width as usize, f.height as usize);
                let image = render_image(gradient(width, height, TINTS[i % 4]))?;
                let picture = Arc::new(Picture { image, size: Size::new(f.width, f.height) });
                Some(WindowShot { id: w.id(), picture, edge: Some(Colour::new(0.3, 0.3, 0.32)) })
            })
            .collect();
        c.pictures.windows_arrived(shots, 1.0);
        let desks = Display::current()
            .into_iter()
            .filter_map(|d| {
                let (width, height) = ((d.frame.width * d.scale) as usize, (d.frame.height * d.scale) as usize);
                let desktop = gradient(width, height, [90, 200, 240]);
                let glass = render_image(make_glass(&desktop));
                Some(DeskShot { display: d.id, backdrop: render_image(desktop)?, glass })
            })
            .collect();
        c.pictures.desks_arrived(desks);
    }

    /// Saves each of Canopy's windows as `<dir>/<width>x<height>-<n>.png`,
    /// from the frame it last drew. Rendering needs the window's display link
    /// detached, which hiding it does, so this hides them all first.
    pub fn save_all(dir: String, cx: &mut Context<Coordinator>) {
        for handle in cx.windows() {
            let _ = handle.update(cx, |_, window, _| ns_window(window).orderOut(None));
        }
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(900)).await;
            let _ = cx.update(|cx| {
                for (n, handle) in cx.windows().into_iter().enumerate() {
                    let _ = handle.update(cx, |_, window, _| {
                        if let Ok(image) = window.render_to_image() {
                            let _ = image.save(format!("{dir}/{}x{}-{n}.png", image.width(), image.height()));
                        }
                    });
                }
            });
            ok();
        })
        .detach();
    }

    pub fn open_welcome(c: &mut Coordinator, step: usize, cx: &mut Context<Coordinator>) {
        c.open_welcome(false, cx);
        let this = cx.entity();
        cx.defer(move |cx| {
            cx.defer(move |cx| {
                if let Some(w) = this.read(cx).welcome {
                    let _ = w.update(cx, |view, _, cx| view.show_step(step, cx));
                }
            });
        });
    }
}
