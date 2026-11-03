//! The input thread, and what presses and drags do once they reach the main
//! thread.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use canopy_core::{Arrangement, DropTarget, Edge, Navigator, NodeId, Plan, Planner, Point, Rect, Size, Solver, WindowId, Zone};
use dispatch2::{DispatchQueue, DispatchRetained};
use gpui::Context;

use super::{Coordinator, LayoutOptions};
use crate::bus::{self, Msg};
use crate::clog;
use crate::mac::ax::{self, attr};
use crate::mac::displays::{left_button_held, mouse, Display};
use crate::mac::event_tap::{self, Event, Phase, FLAG_ALTERNATE, FLAG_COMMAND, FLAG_CONTROL, FLAG_SECONDARY_FN, FLAG_SHIFT};
use crate::mac::window_server;
use crate::ui::overlays::{DropPreviewPanel, GhostPanel, MarqueePanel};

/// What the input thread may read. The main thread publishes a new copy
/// whenever it changes.
#[derive(Clone, Default)]
pub struct InputState {
    pub started: bool,
    pub rect_zoom: bool,
    pub green_button: bool,
    pub title_double_click: bool,
    pub option_pinch: bool,
    pub managed: HashSet<WindowId>,
    pub docked: HashSet<WindowId>,
    /// Each window's green button, relative to its top-left corner.
    pub green_buttons: HashMap<WindowId, Rect>,
    /// A stage is up after a change; a press ends it.
    pub landing: bool,
}

/// What the input thread tells the main thread.
#[derive(Clone, Copy, Debug)]
pub enum InputMsg {
    Interrupt,
    MarqueeBegan(Point),
    MarqueeMoved(Point, Point),
    MarqueeCancelled,
    MarqueeEnded(Point, Point),
    ToggleDocked(WindowId),
    TitleDoubleClicked(WindowId),
    DesktopDoubleClicked(Point),
    PressBegan(WindowId, Rect),
    /// The newest drag point is in the router.
    PressDragged,
    PressEnded(Point),
    /// The newest pinch total is in the router.
    Pinched,
    PinchEnded(f64, Point),
}

#[derive(Default)]
struct Latest {
    drag: Option<Point>,
    drag_scheduled: bool,
    pinch: Option<(f64, Point)>,
    pinch_scheduled: bool,
}

/// Decides which events Canopy takes, and hands drags to the main thread
/// coalesced: the main thread takes the newest point.
#[derive(Default)]
pub struct InputRouter {
    state: Mutex<Arc<InputState>>,
    latest: Mutex<Latest>,
}

fn main(msg: InputMsg) {
    bus::send(Msg::Input(msg));
}

impl InputRouter {
    pub fn publish(&self, s: InputState) {
        *self.state.lock().unwrap() = Arc::new(s);
    }

    fn snapshot(&self) -> Arc<InputState> {
        self.state.lock().unwrap().clone()
    }

    pub fn take_drag(&self) -> Option<Point> {
        let mut l = self.latest.lock().unwrap();
        l.drag_scheduled = false;
        l.drag.take()
    }

    pub fn take_pinch(&self) -> Option<(f64, Point)> {
        let mut l = self.latest.lock().unwrap();
        l.pinch_scheduled = false;
        l.pinch
    }
}

/// Input-thread state. macOS holds every mouse event on the Mac until this
/// answers, so it only asks the window server (fast). Everything else is handed
/// to the main thread or `hit` without waiting, so the pointer never stalls.
#[derive(Default)]
pub struct InputThread {
    marquee_start: Option<Point>,
    swallow_up: bool,
    pinch: f64,
    pinching: bool,
    tracking: bool,
}

impl InputThread {
    pub fn handle(&mut self, router: &InputRouter, kind: u32, e: Event) -> bool {
        let s = router.snapshot();
        if !s.started {
            return false;
        }
        match kind {
            event_tap::MAGNIFY => self.handle_pinch(router, e, &s),
            event_tap::GESTURE => false,
            event_tap::KEY_DOWN => {
                if self.marquee_start.is_some() && e.keycode() == 53 {
                    self.marquee_start = None;
                    main(InputMsg::MarqueeCancelled);
                    return true;
                }
                false
            }
            event_tap::LEFT_MOUSE_DOWN => self.down(e.location(), e, &s),
            event_tap::LEFT_MOUSE_DRAGGED => self.dragged(router, e.location()),
            event_tap::LEFT_MOUSE_UP => self.up(e.location()),
            _ => false,
        }
    }

    fn down(&mut self, p: Point, e: Event, s: &InputState) -> bool {
        if s.landing {
            main(InputMsg::Interrupt);
        }
        let flags = e.flags();
        let option = flags & FLAG_ALTERNATE != 0;
        if s.rect_zoom && flags & FLAG_COMMAND != 0 && flags & FLAG_SHIFT != 0 && !option && flags & FLAG_CONTROL == 0 {
            self.marquee_start = Some(p);
            main(InputMsg::MarqueeBegan(p));
            return true;
        }
        let clicks = e.click_count();
        let top = window_server::window_at(p);

        // The green button docks and floats; ⌥ and fn keep their usual meaning.
        if let Some((id, frame)) = top {
            if s.green_button
                && clicks == 1
                && !option
                && flags & FLAG_SECONDARY_FN == 0
                && s.managed.contains(&id)
                && s.green_buttons.get(&id).is_some_and(|b| hit::on_button(*b, frame, p))
            {
                self.swallow_up = true;
                main(InputMsg::ToggleDocked(id));
                return true;
            }
        }
        let in_title_bar = top.is_some_and(|(id, frame)| s.title_double_click && s.docked.contains(&id) && p.y - frame.min_y() < 32.0);
        if clicks == 2 && !option {
            // A docked window's title bar: that window on its own, and back.
            if let Some((id, _)) = top.filter(|_| in_title_bar && !hit::over_control(p)) {
                self.swallow_up = true;
                main(InputMsg::TitleDoubleClicked(id));
                return true;
            }
            // The bare Desktop: the Desktop on its own, and back.
            if top.is_none() {
                hit::if_bare_desktop(p);
                return false;
            }
        }
        if clicks == 1 {
            if in_title_bar {
                hit::look_ahead(p);
            }
            if let Some((id, frame)) = top.filter(|(id, _)| s.managed.contains(id)) {
                self.tracking = true;
                main(InputMsg::PressBegan(id, frame));
            }
        }
        false
    }

    fn dragged(&mut self, router: &InputRouter, p: Point) -> bool {
        if let Some(start) = self.marquee_start {
            main(InputMsg::MarqueeMoved(start, p));
            return true;
        }
        if !self.tracking {
            return false;
        }
        let mut l = router.latest.lock().unwrap();
        l.drag = Some(p);
        if !l.drag_scheduled {
            l.drag_scheduled = true;
            main(InputMsg::PressDragged);
        }
        false
    }

    fn up(&mut self, p: Point) -> bool {
        if self.swallow_up {
            self.swallow_up = false;
            return true;
        }
        if let Some(start) = self.marquee_start.take() {
            main(InputMsg::MarqueeEnded(start, p));
            return true;
        }
        if self.tracking {
            self.tracking = false;
            main(InputMsg::PressEnded(p));
        }
        false
    }

    /// ⌥-pinch drives the camera continuously; the main thread gets the
    /// newest total, coalesced, and a final call when the fingers lift.
    fn handle_pinch(&mut self, router: &InputRouter, e: Event, s: &InputState) -> bool {
        let Some((magnification, phase)) = e.pinch() else { return false };
        let ending = phase == Phase::Ended;
        let p = e.location();
        if !(s.option_pinch && (e.flags() & FLAG_ALTERNATE != 0 || (self.pinching && ending))) {
            if self.pinching {
                self.finish_pinch(p);
            }
            return false;
        }
        if phase == Phase::Began || !self.pinching {
            self.pinching = true;
            self.pinch = 0.0;
        }
        if ending {
            self.finish_pinch(p);
            return true;
        }
        self.pinch += magnification;
        let mut l = router.latest.lock().unwrap();
        l.pinch = Some((self.pinch, p));
        if !l.pinch_scheduled {
            l.pinch_scheduled = true;
            main(InputMsg::Pinched);
        }
        true
    }

    fn finish_pinch(&mut self, p: Point) {
        let total = self.pinch;
        self.pinching = false;
        self.pinch = 0.0;
        main(InputMsg::PinchEnded(total, p));
    }
}

/// Hit testing. Accessibility answers at the pace of the app under the pointer,
/// so it never runs on the input thread: `hit` looks ahead on a queue of its
/// own, and the input thread only reads what has come back.
mod hit {
    use super::*;

    static QUEUE: LazyLock<DispatchRetained<DispatchQueue>> = LazyLock::new(|| DispatchQueue::new("canopy.hit", None));
    /// Where the last look ahead looked, and whether it found a control.
    static LOOKED: Mutex<Option<(Point, bool)>> = Mutex::new(None);

    pub fn on_button(button: Rect, frame: Rect, p: Point) -> bool {
        Rect::new(frame.x + button.x, frame.y + button.y, button.width, button.height).inset_by(-2.0).contains(p)
    }

    /// A click in a title bar may be the first of two. By the second, this has
    /// found out whether they are on a control.
    pub fn look_ahead(p: Point) {
        *LOOKED.lock().unwrap() = None;
        QUEUE.exec_async(move || {
            let control = is_control(p);
            *LOOKED.lock().unwrap() = Some((p, control));
        });
    }

    /// What the look ahead has found near `p`. An app too slow to have answered
    /// yet counts as no control: a double-click on a title bar is what's likely.
    pub fn over_control(p: Point) -> bool {
        LOOKED.lock().unwrap().is_some_and(|(at, control)| control && (at.x - p.x).abs() <= 4.0 && (at.y - p.y).abs() <= 4.0)
    }

    fn is_control(p: Point) -> bool {
        const CONTROLS: [&str; 14] = [
            "AXButton", "AXTextField", "AXPopUpButton", "AXMenuButton", "AXCheckBox", "AXRadioButton", "AXSlider", "AXComboBox",
            "AXLink", "AXSegmentedControl", "AXTabGroup", "AXDisclosureTriangle", "AXIncrementor", "AXTextArea",
        ];
        ax::element_at(p).and_then(|el| ax::string(&el, attr::ROLE)).is_some_and(|role| CONTROLS.contains(&role.as_str()))
    }

    /// Tells the main thread about a double-click, if it was on the bare Desktop.
    pub fn if_bare_desktop(p: Point) {
        QUEUE.exec_async(move || {
            if is_bare_desktop(p) {
                main(InputMsg::DesktopDoubleClicked(p));
            }
        });
    }

    fn is_bare_desktop(p: Point) -> bool {
        const ITEMS: [&str; 5] = ["AXImage", "AXTextField", "AXStaticText", "AXButton", "AXMenuBarItem"];
        let Some(el) = ax::element_at(p) else { return true };
        !ITEMS.contains(&ax::string(&el, attr::ROLE).unwrap_or_default().as_str())
    }
}

// MARK: - Presses and drags (main thread)

/// A left-button press on a managed window, which may become a move or a resize.
pub struct Press {
    pub window: WindowId,
    start_frame: Rect,
    moving: bool,
    resizing: bool,
    pub session: Option<DragSession>,
    /// The test harness's drag has no mouse button behind it.
    pretend: bool,
}

/// A window being dragged: its picture follows the pointer on the stage, and
/// the other windows make room live for wherever it would land.
pub struct DragSession {
    pub display: Display,
    grab: Point,
    size: Size,
    /// Where the other windows are on the stage when nothing would change.
    baseline: std::collections::HashMap<WindowId, Rect>,
    stage_up: bool,
    /// Over a target, the dragged window shrinks to a thumbnail under the
    /// pointer, so the target stays in view.
    pub thumbnail: bool,
    pub target: DropTarget,
    target_world: Option<String>,
    hold: Hold,
    pointer: Point,
}

/// The pause over a tile's middle that turns a drop into a tab.
#[derive(Clone, Copy, Default)]
pub struct Hold {
    tile: Option<NodeId>,
    start: Option<Instant>,
    held: Option<NodeId>,
    rest: Option<Rest>,
}

/// The pause that makes a floating window a drop target. Windows are moved over
/// each other on the Desktop all the time, so only a real stop counts: a window
/// passing over another, however slowly, never docks anything.
#[derive(Clone, Copy)]
struct Rest {
    window: WindowId,
    at: Point,
    start: Instant,
    reached: bool,
}

/// How far the pointer can drift and still be resting.
const REST_DRIFT: f64 = 8.0;

/// A pinch in progress.
pub struct ZoomSession {
    world: String,
    anchor: Point,
    scale: f64,
    last: Instant,
}

pub struct TabDrag {
    window: WindowId,
    target: DropTarget,
    target_world: Option<String>,
    hold: Hold,
}

/// What a drop would produce, worked out on a copy.
struct Simulation {
    arrangement: Arrangement,
    plan: Plan,
    slot: Rect,
}

fn marquee_rect(a: Point, b: Point) -> Rect {
    Rect::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs().max(1.0), (a.y - b.y).abs().max(1.0))
}

impl Coordinator {
    pub fn on_input(&mut self, msg: InputMsg, cx: &mut Context<Self>) {
        match msg {
            InputMsg::Interrupt => self.stage.interrupt(cx),
            InputMsg::MarqueeBegan(p) => self.marquee_began(p, cx),
            InputMsg::MarqueeMoved(a, b) => {
                if let Some(m) = &self.marquee {
                    m.show(marquee_rect(a, b));
                }
            }
            InputMsg::MarqueeCancelled => {
                if let Some(m) = &self.marquee {
                    m.panel.order_out();
                }
            }
            InputMsg::MarqueeEnded(a, b) => self.marquee_ended(a, b, cx),
            InputMsg::ToggleDocked(id) => self.toggle_docked(id, cx),
            InputMsg::TitleDoubleClicked(id) => self.title_double_clicked(id, cx),
            InputMsg::DesktopDoubleClicked(p) => self.desktop_double_clicked(p, cx),
            InputMsg::PressBegan(window, frame) => {
                self.press = Some(Press { window, start_frame: frame, moving: false, resizing: false, session: None, pretend: false });
            }
            InputMsg::PressDragged => {
                if let Some(p) = self.input.take_drag() {
                    self.press_dragged(p, cx);
                }
            }
            InputMsg::PressEnded(p) => self.press_ended(p, cx),
            InputMsg::Pinched => {
                if let Some((total, at)) = self.input.take_pinch() {
                    self.pinch(total, at, false, cx);
                }
            }
            InputMsg::PinchEnded(total, at) => self.pinch(total, at, true, cx),
        }
    }

    fn press_dragged(&mut self, p: Point, cx: &mut Context<Self>) {
        let Some(mut pr) = self.press.take() else { return };
        if !pr.moving && !pr.resizing {
            let Some(f) = window_server::frame(pr.window) else {
                self.press = Some(pr);
                return;
            };
            if (f.width - pr.start_frame.width).abs() > 1.0 || (f.height - pr.start_frame.height).abs() > 1.0 {
                pr.resizing = true;
            } else if (f.x - pr.start_frame.x).abs() > 2.0 || (f.y - pr.start_frame.y).abs() > 2.0 {
                pr.moving = true;
                clog!("DRAG w{} begins", pr.window);
                pr.session = self.begin_session(pr.window, f, p, cx);
            }
        }
        if pr.moving {
            if let Some(s) = &mut pr.session {
                self.update_session(s, pr.window, p, cx);
            }
        }
        self.press = Some(pr);
    }

    fn press_ended(&mut self, p: Point, cx: &mut Context<Self>) {
        if let Some(mut pr) = self.press.take() {
            self.hold_task = None;
            if pr.moving {
                if let Some(mut s) = pr.session.take() {
                    self.update_session(&mut s, pr.window, p, cx);
                    self.finish_session(s, pr.window, cx);
                }
            } else if pr.resizing {
                if let Some(key) = self.world_key_of(pr.window) {
                    self.adopt_resize(pr.window, &key, cx);
                }
            }
            self.moved_while_mouse_down.remove(&pr.window);
        }
        self.process_moved_while_mouse_down(cx);
    }

    /// A drag for the test harness: all of a real one but the events.
    pub fn hover(&mut self, window: WindowId, p: Point, cx: &mut Context<Self>) {
        if self.press.is_none() {
            let Some(f) = window_server::frame(window) else { return };
            let session = self.begin_session(window, f, p, cx);
            self.press = Some(Press { window, start_frame: f, moving: true, resizing: false, session, pretend: true });
        }
        self.press_dragged(p, cx);
    }

    /// The harness's way to lose a mouse-up: the button is up, and no event says so.
    pub fn lose_mouse_up(&mut self) {
        if let Some(p) = &mut self.press {
            p.pretend = false;
        }
    }

    /// The backstop for an event that never came. macOS can lose a mouse-up
    /// (a tap it disabled, a gesture that took over), and a stage left up is a
    /// picture over the whole screen, which looks like a frozen Mac.
    pub(super) fn watch_input(&mut self, cx: &mut Context<Self>) {
        const BUTTON_GRACE: Duration = Duration::from_millis(200);
        const PINCH_LIMIT: Duration = Duration::from_secs(3);
        const STAGE_LIMIT: Duration = Duration::from_secs(4);

        let dragging = self.press.as_ref().is_some_and(|p| p.session.is_some() && !p.pretend);
        if (dragging || self.tab_drag.is_some()) && !left_button_held() {
            let up = *self.button_up_since.get_or_insert_with(Instant::now);
            if up.elapsed() > BUTTON_GRACE {
                clog!("INPUT the mouse-up never came; ending the drag");
                self.button_up_since = None;
                if dragging {
                    self.press_ended(mouse(), cx);
                } else {
                    self.end_tab_drag(cx);
                }
            }
        } else {
            self.button_up_since = None;
        }

        if let Some((scale, anchor)) = self.zoom.as_ref().filter(|z| z.last.elapsed() > PINCH_LIMIT).map(|z| (z.scale, z.anchor)) {
            clog!("INPUT the pinch never ended; ending it");
            self.pinch(scale.ln(), anchor, true, cx);
        }

        let busy = self.zoom.is_some() || self.seam_world.is_some() || self.press.as_ref().is_some_and(|p| p.session.is_some());
        if busy || !self.stage.any_shown() {
            self.stage_idle = None;
        } else if self.stage_idle.get_or_insert_with(Instant::now).elapsed() > STAGE_LIMIT {
            clog!("STAGE left up with nothing moving; taking it down");
            self.stage_idle = None;
            self.stage.interrupt(cx);
            self.publish_input();
        }
    }

    fn begin_session(&mut self, id: WindowId, f: Rect, p: Point, cx: &mut Context<Self>) -> Option<DragSession> {
        self.refresh_frames();
        self.under_drag = window_server::on_screen();
        let key = self.world_key_at(p).or_else(|| self.world_key_of(id))?;
        let world = &self.worlds[&key];
        let display = world.display.clone();
        let desk = world.plan.as_ref().and_then(|p| p.desktop_rect);
        let mut session = DragSession {
            display: display.clone(),
            grab: Point::new(p.x - f.x, p.y - f.y),
            size: f.size(),
            baseline: Default::default(),
            stage_up: false,
            thumbnail: false,
            target: DropTarget::None,
            target_world: None,
            hold: Hold::default(),
            pointer: p,
        };
        if !self.prefs.animate {
            return Some(session);
        }
        let started = Instant::now();
        let mut included = HashSet::new();
        let mut items = self.on_screen_items(&display, &mut included);
        match items.iter_mut().find(|i| i.id == id) {
            Some(item) => item.rect = f,
            None => items.extend(self.stage_item(id, f, None)),
        }
        let ids: Vec<WindowId> = items.iter().map(|i| i.id).collect();
        session.baseline = items.iter().filter(|i| i.id != id).map(|i| (i.id, i.rect)).collect();
        let count = items.len();
        let ground = self.ground(&display);
        let panel = self.stage.panel(&display, cx)?;
        panel.show(items, desk, ground, cx);
        panel.bring_to_front(id, cx);
        session.stage_up = true;
        let scale = self.picture_scale();
        self.pictures.refresh(ids, scale);
        self.publish_input();
        clog!("DRAG stage up in {} ms ({count} windows)", started.elapsed().as_millis());
        Some(session)
    }

    fn update_session(&mut self, s: &mut DragSession, window: WindowId, p: Point, cx: &mut Context<Self>) {
        s.pointer = p;
        let mut hold = s.hold;
        let (target, world) = self.resolve_drop(window, p, true, &mut hold, cx);
        s.hold = hold;
        let changed = target != s.target || world != s.target_world;
        s.target = target;
        s.target_world = world.clone();
        if s.stage_up {
            let over_target = target.docks();
            let rect = if over_target {
                self.thumbnail_rect(window, s.size, p, &s.display, cx)
            } else {
                Rect::new(p.x - s.grab.x, p.y - s.grab.y, s.size.width, s.size.height)
            };
            if let Some(panel) = self.stage.panel(&s.display, cx) {
                if over_target != s.thumbnail {
                    s.thumbnail = over_target;
                    panel.reshape(window, rect, cx);
                } else {
                    panel.move_item(window, rect, cx);
                }
            }
        }
        if changed {
            self.show_target(s, window, world.as_deref(), cx);
        }
    }

    /// A small copy of the window with its title bar under the pointer.
    fn thumbnail_rect(&mut self, id: WindowId, size: Size, p: Point, display: &Display, cx: &mut Context<Self>) -> Rect {
        let picture = self.stage.panel(display, cx).and_then(|panel| panel.picture_size(id, cx)).unwrap_or(size);
        let scale = (240.0 / picture.width.max(1.0)).min(170.0 / picture.height.max(1.0)).min(1.0);
        let (w, h) = (picture.width * scale, picture.height * scale);
        Rect::new(p.x - w / 2.0, p.y - 12.0, w, h)
    }

    /// Shows what the drop would do: on the stage, the other windows make
    /// room and a slot marks the landing place; without it, a highlight.
    fn show_target(&mut self, s: &DragSession, window: WindowId, world: Option<&str>, cx: &mut Context<Self>) {
        if !s.stage_up {
            let whole = self.world_of(window).is_some();
            self.show_preview(s.target, window, world, whole, cx);
            return;
        }
        let same_display = world.is_some_and(|k| self.worlds[k].display.uuid == s.display.uuid);
        let sim = world.filter(|_| same_display).and_then(|k| self.simulate(s.target, window, k, true));
        let Some(sim) = sim else {
            if let Some(panel) = self.stage.panel(&s.display, cx) {
                panel.set_slot(None, false, cx);
                panel.animate(&s.baseline, 0.25, cx);
            }
            return;
        };
        let mut targets = s.baseline.clone();
        for tile in sim.arrangement.tiles() {
            let selected = sim.arrangement.selected_window(tile);
            let r = selected.and_then(|sel| sim.plan.visible.get(&sel)).or(sim.plan.covers.get(&tile)).or(sim.plan.projected.get(&tile));
            let Some(r) = r else { continue };
            for w in sim.arrangement.get(tile).windows.iter().filter(|w| **w != window) {
                if targets.contains_key(w) {
                    targets.insert(*w, *r);
                }
            }
        }
        if let Some(panel) = self.stage.panel(&s.display, cx) {
            panel.animate(&targets, 0.25, cx);
            panel.set_slot(Some(sim.slot), matches!(s.target, DropTarget::Tab(_)), cx);
        }
    }

    /// The arrangement and plan a drop would produce, worked out on a copy.
    fn simulate(&self, target: DropTarget, window: WindowId, key: &str, whole_tile: bool) -> Option<Simulation> {
        if !target.docks() {
            return None;
        }
        let world = self.worlds.get(key)?;
        let mut a = world.arrangement.clone();
        let mut nav: Navigator = world.navigator.clone();
        let whole = whole_tile && a.contains(window);
        if !a.apply(target, window, whole) {
            return None;
        }
        let tile = a.tile_of(window)?;
        nav.reveal(tile, &a);
        let plan = Planner::plan(&a, &nav, world.display.visible, self.prefs.gap, |id| self.minimum(id));
        let slot = *plan.layout.node_rects.get(&tile)?;
        Some(Simulation { arrangement: a, plan, slot })
    }

    fn finish_session(&mut self, s: DragSession, window: WindowId, cx: &mut Context<Self>) {
        if let Some(p) = &self.preview {
            p.panel.order_out();
        }
        let target = s.target;
        let target_world = s
            .target_world
            .clone()
            .filter(|k| self.worlds.contains_key(k))
            .or_else(|| self.world_key_of(window))
            .or_else(|| self.world_key_at(s.pointer));
        let source = self.world_key_of(window);
        if s.stage_up && source.is_none() && !target.docks() {
            // A floating window lands where it was let go; the real one is already there.
            if let Some(f) = window_server::frame(window) {
                if let Some(tw) = self.tracker.windows.get_mut(&window) {
                    tw.frame = Some(f);
                }
                if let Some(panel) = self.stage.panel(&s.display, cx) {
                    panel.animate(&std::collections::HashMap::from([(window, f)]), 0.1, cx);
                }
            }
            let writer = self.writer.clone();
            if let Some(panel) = self.stage.panel(&s.display, cx) {
                panel.set_slot(None, false, cx);
                panel.animate(&s.baseline, 0.1, cx);
                panel.land(0.1, &writer, 0.02, cx);
            }
            return;
        }
        let Some(target_world) = target_world else {
            if let Some(panel) = self.stage.panel(&s.display, cx) {
                panel.fade(0.14, cx);
            }
            return;
        };
        self.drop_window(window, target, &target_world, true, cx);
        let other_display = self.worlds.get(&target_world).is_some_and(|w| w.display.uuid != s.display.uuid);
        let source_elsewhere = source.as_ref().and_then(|k| self.worlds.get(k)).is_none_or(|w| w.display.uuid != s.display.uuid);
        if let Some(panel) = self.stage.panel(&s.display, cx) {
            if panel.is_shown() && other_display && source_elsewhere {
                panel.fade(0.14, cx);
            }
        }
    }

    // MARK: Dropping

    pub fn drop_window(&mut self, id: WindowId, target: DropTarget, key: &str, whole_tile: bool, cx: &mut Context<Self>) {
        let source = self.world_key_of(id);
        clog!("DROP w{id} {target:?} from {}", if source.is_none() { "floating" } else { "docked" });
        let share = self.prefs.share_removed_space;
        let dragged = LayoutOptions { dragged: Some(id), ..LayoutOptions::default() };
        match target {
            DropTarget::None => {
                if let Some(s) = source {
                    self.applied.remove(&id);
                    self.layout(&s, true, dragged, cx);
                }
            }
            DropTarget::Float => {
                let Some(s) = source else { return };
                let world = self.worlds.get_mut(&s).expect("the window is in this world");
                let restore = world.arrangement.remove(id, share);
                let live = world.arrangement.windows().into_iter().collect();
                world.navigator.prune(&live);
                self.float_restore.insert(id, (s.clone(), restore.unwrap_or(canopy_core::RestoreTarget::Floating)));
                self.applied.remove(&id);
                self.layout(&s, true, dragged, cx);
            }
            _ => {
                let other = if let DropTarget::Floating { window, .. } = target { Some(window) } else { None };
                for w in other.into_iter().chain(source.is_none().then_some(id)) {
                    if let Some(f) = self.tracker.windows.get(&w).and_then(|w| w.frame) {
                        self.float_frames.insert(w, f);
                    }
                }
                if let Some(s) = source.as_ref().filter(|s| s.as_str() != key) {
                    if let Some(world) = self.worlds.get_mut(s) {
                        world.arrangement.remove(id, share);
                    }
                    let s = s.clone();
                    self.layout(&s, true, LayoutOptions::default(), cx);
                }
                let whole = whole_tile && source.as_deref() == Some(key);
                let Some(world) = self.worlds.get_mut(key) else { return };
                if !world.arrangement.apply(target, id, whole) {
                    if let Some(s) = source {
                        self.layout(&s, true, dragged, cx);
                    }
                    return;
                }
                self.applied.remove(&id);
                if let Some(other) = other {
                    self.applied.remove(&other);
                }
                self.reveal(id, key);
                self.layout(key, true, LayoutOptions { confine_floating: true, ..dragged }, cx);
            }
        }
    }

    /// Resolves where a drop at `p` would go, tracking the pauses that turn a
    /// drop into a tab or make a floating window a target.
    fn resolve_drop(&mut self, window: WindowId, p: Point, exclude_own_tile: bool, hold: &mut Hold, cx: &mut Context<Self>) -> (DropTarget, Option<String>) {
        let Some(key) = self.world_key_at(p) else { return (DropTarget::None, None) };
        let world = &self.worlds[&key];
        let Some(plan) = &world.plan else { return (DropTarget::None, None) };
        let a = &world.arrangement;
        let own = a.tile_of(window);
        let own_is_single = own.map_or(0, |t| a.get(t).windows.len()) <= 1;
        let mut tiles = Vec::new();
        for t in a.tiles() {
            let Some(r) = plan.covers.get(&t).or(plan.layout.node_rects.get(&t)) else { continue };
            if Some(t) == own && (exclude_own_tile || own_is_single) {
                continue;
            }
            tiles.push((t, *r));
        }
        let desktop = plan.desktop_rect.map(|r| (a.desktop(), r));
        let (visible, frame) = (world.display.visible, world.display.frame);
        // A floating window docks on the Desktop, or beside the view when the
        // Desktop isn't in it.
        let home = desktop.unwrap_or((plan.framed, visible));
        let now = Instant::now();
        let centre = tiles.iter().find(|(_, r)| r.contains(p) && self.resolver.zone(p, *r) == Zone::Centre).map(|(t, _)| *t);
        if centre != hold.tile {
            hold.tile = centre;
            hold.start = Some(now);
            hold.held = None;
            if centre.is_some() {
                let wait = Duration::from_secs_f64(self.prefs.tab_hold + 0.02);
                self.hold_task = Some(self.later(wait, |c, cx| c.hold_elapsed(cx), cx));
            }
        } else if let (Some(c), Some(start)) = (centre, hold.start) {
            if (now - start).as_secs_f64() >= self.prefs.tab_hold {
                hold.held = Some(c);
            }
        }
        let rested = self.rested_on(window, p, &key, hold, cx);
        let target = match (self.resolver.resolve(p, visible, Some(frame), &tiles, desktop, hold.held), rested) {
            (edge @ DropTarget::ScreenEdge(_), _) => edge,
            (_, Some((w, r))) => self.resolver.over_floating(p, w, r, home),
            (target, None) => target,
        };
        let world = &self.worlds[&key];
        (target.in_view(window, exclude_own_tile, &world.navigator, &world.arrangement), Some(key))
    }

    /// The floating window the pointer has come to rest on, if any.
    fn rested_on(&mut self, window: WindowId, p: Point, key: &str, hold: &mut Hold, cx: &mut Context<Self>) -> Option<(WindowId, Rect)> {
        let Some((over, frame)) = self.floating_under(window, p, key) else {
            hold.rest = None;
            return None;
        };
        let now = Instant::now();
        let resting = hold.rest.filter(|r| r.window == over && (r.reached || (r.at.x - p.x).hypot(r.at.y - p.y) <= REST_DRIFT));
        let Some(mut rest) = resting else {
            hold.rest = Some(Rest { window: over, at: p, start: now, reached: false });
            let wait = Duration::from_secs_f64(self.prefs.tab_hold + 0.02);
            self.hold_task = Some(self.later(wait, |c, cx| c.hold_elapsed(cx), cx));
            return None;
        };
        rest.reached |= (now - rest.start).as_secs_f64() >= self.prefs.tab_hold;
        hold.rest = Some(rest);
        rest.reached.then_some((over, frame))
    }

    /// The floating window in front of everything else under `p`, from the
    /// list taken when the drag began: a drag asks about every pointer move.
    fn floating_under(&self, dragged: WindowId, p: Point, key: &str) -> Option<(WindowId, Rect)> {
        let own = std::process::id() as i32;
        let top = self.under_drag.iter().find(|o| o.id != dragged && o.pid != own && !self.parked.contains(&o.id) && o.frame.contains(p))?;
        let tw = self.tracker.windows.get(&top.id).filter(|w| self.manageable(w))?;
        let floating = self.world_of(top.id).is_none() && !self.parked_floating.contains_key(&top.id);
        (floating && self.display_world_key(tw).as_deref() == Some(key)).then_some((top.id, top.frame))
    }

    /// The pause is over: look again, even if the pointer hasn't moved.
    fn hold_elapsed(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.press.as_ref().and_then(|p| p.session.as_ref()).map(|s| s.pointer) {
            self.press_dragged(p, cx);
        } else if self.tab_drag.is_some() {
            self.update_tab_drag(cx);
        }
    }

    /// The plain highlight, for tab drags and when motion is off.
    fn show_preview(&mut self, target: DropTarget, window: WindowId, world: Option<&str>, whole_tile: bool, cx: &mut Context<Self>) {
        let sim = world.and_then(|k| self.simulate(target, window, k, whole_tile));
        let Some(sim) = sim else {
            if let Some(p) = &self.preview {
                p.panel.order_out();
            }
            return;
        };
        if self.preview.is_none() {
            self.preview = DropPreviewPanel::create(cx).ok();
        }
        if let Some(p) = &self.preview {
            p.show(sim.slot, matches!(target, DropTarget::Tab(_)), cx);
        }
    }

    /// The user dragged a docked window's edge: move the matching seam.
    fn adopt_resize(&mut self, id: WindowId, key: &str, cx: &mut Context<Self>) {
        let frame = window_server::frame(id);
        let old = self.applied.get(&id).copied();
        let world = self.worlds.get_mut(key).expect("the world holds this window");
        let (Some(f), Some(old), Some(plan), Some(tile)) = (frame, old, world.plan.clone(), world.arrangement.tile_of(id)) else {
            self.layout(key, false, LayoutOptions::default(), cx);
            return;
        };
        if let Some(tw) = self.tracker.windows.get_mut(&id) {
            tw.frame = Some(f);
        }
        for edge in Edge::ALL {
            let delta = match edge {
                Edge::Left => f.min_x() - old.min_x(),
                Edge::Right => f.max_x() - old.max_x(),
                Edge::Top => f.min_y() - old.min_y(),
                Edge::Bottom => f.max_y() - old.max_y(),
            };
            if delta.abs() <= 2.0 {
                continue;
            }
            let a = &mut world.arrangement;
            let mut c = tile;
            while let Some(p) = a.parent(c) {
                let split = a.get(p);
                if split.axis == edge.axis() {
                    if let Some(i) = a.index_in_parent(c) {
                        let visible: Vec<usize> = (0..split.children.len())
                            .filter(|&k| {
                                let n = a.get(split.children[k]);
                                !(n.is_desktop() && n.hidden)
                            })
                            .collect();
                        if let Some(k) = visible.iter().position(|&v| v == i) {
                            let j = if edge.is_leading() {
                                k.checked_sub(1).map(|k| visible[k])
                            } else {
                                visible.get(k + 1).copied()
                            };
                            let seam = j.and_then(|j| {
                                plan.layout.seams.iter().find(|s| s.split == p && s.index == i.min(j) && s.next == i.max(j))
                            });
                            if let Some(seam) = seam {
                                a.resize_seam(p, seam.index, seam.next, delta, seam.available, Default::default());
                                break;
                            }
                        }
                    }
                }
                c = p;
            }
        }
        self.applied.remove(&id);
        self.layout(key, false, LayoutOptions::default(), cx);
    }

    // MARK: Tab drags

    pub fn begin_tab_drag(&mut self, id: WindowId, _world: &str, cx: &mut Context<Self>) {
        self.under_drag = window_server::on_screen();
        self.tab_drag = Some(TabDrag { window: id, target: DropTarget::None, target_world: None, hold: Hold::default() });
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let image = self
            .pictures
            .images
            .get(&id)
            .map(|p| p.image.clone())
            .or_else(|| crate::mac::app_look::icon(&tw.bundle_id, tw.pid()));
        let title = if tw.title.is_empty() { tw.app_name.clone() } else { tw.title.clone() };
        if self.ghost.is_none() {
            self.ghost = GhostPanel::create(cx).ok();
        }
        if let Some(g) = &self.ghost {
            g.show(image, title, mouse(), cx);
        }
    }

    pub fn update_tab_drag(&mut self, cx: &mut Context<Self>) {
        let Some(mut t) = self.tab_drag.take() else { return };
        let p = mouse();
        if let Some(g) = &self.ghost {
            g.move_to(p);
        }
        let mut hold = t.hold;
        let (mut target, world) = self.resolve_drop(t.window, p, false, &mut hold, cx);
        t.hold = hold;
        if let DropTarget::Tab(id) = target {
            let own = self.world_of(t.window).and_then(|w| w.arrangement.tile_of(t.window));
            if own == Some(id) {
                target = DropTarget::None;
            }
        }
        t.target = target;
        t.target_world = world.clone();
        let window = t.window;
        self.tab_drag = Some(t);
        self.show_preview(target, window, world.as_deref(), false, cx);
    }

    pub fn end_tab_drag(&mut self, cx: &mut Context<Self>) {
        self.update_tab_drag(cx);
        let drag = self.tab_drag.take();
        if let Some(g) = &self.ghost {
            g.panel.order_out();
        }
        if let Some(p) = &self.preview {
            p.panel.order_out();
        }
        self.hold_task = None;
        let Some(t) = drag else { return };
        if t.target.docks() {
            if let Some(key) = t.target_world.filter(|k| self.worlds.contains_key(k)) {
                self.drop_window(t.window, t.target, &key, false, cx);
            }
            return;
        }
        // Torn off: float it where it was dropped.
        let Some(source) = self.world_key_of(t.window).filter(|_| self.tracker.windows.contains_key(&t.window)) else { return };
        let p = mouse();
        let size = self
            .float_frames
            .get(&t.window)
            .map(Rect::size)
            .or(self.applied.get(&t.window).map(Rect::size))
            .unwrap_or(Size::new(800.0, 600.0));
        let bounds = self.worlds[&self.world_key_at(p).unwrap_or_else(|| source.clone())].display.visible;
        let r = Rect::new(p.x - size.width / 2.0, p.y - 14.0, size.width.min(bounds.width), size.height.min(bounds.height)).clamped(&bounds);
        let share = self.prefs.share_removed_space;
        let world = self.worlds.get_mut(&source).expect("the window is in this world");
        let target = world.arrangement.remove(t.window, share);
        self.float_restore.insert(t.window, (source.clone(), target.unwrap_or(canopy_core::RestoreTarget::Floating)));
        self.applied.remove(&t.window);
        let options = LayoutOptions { float_to: [(t.window, r)].into(), ..LayoutOptions::default() };
        self.layout(&source, true, options, cx);
        self.focus(t.window);
    }

    // MARK: Clicks, marquee and gestures

    fn title_double_clicked(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_of(id) else { return };
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let Some(tile) = world.arrangement.tile_of(id) else { return };
        if world.navigator.toggle_frame(tile, &world.arrangement) {
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
    }

    fn desktop_double_clicked(&mut self, p: Point, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_at(p) else { return };
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        if !world.plan.as_ref().and_then(|p| p.desktop_rect).is_some_and(|d| d.contains(p)) {
            return;
        }
        let desktop = world.arrangement.desktop();
        if world.navigator.toggle_frame(desktop, &world.arrangement) {
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
    }

    fn marquee_began(&mut self, p: Point, cx: &mut Context<Self>) {
        clog!("MARQUEE begins at {},{}", p.x as i64, p.y as i64);
        if self.marquee.is_none() {
            self.marquee = MarqueePanel::create(cx).ok();
        }
        if let Some(m) = &self.marquee {
            m.show(Rect::new(p.x, p.y, 1.0, 1.0));
        }
    }

    fn marquee_ended(&mut self, a: Point, b: Point, cx: &mut Context<Self>) {
        if let Some(m) = &self.marquee {
            m.panel.order_out();
        }
        let r = marquee_rect(a, b);
        clog!("MARQUEE {r}");
        let Some(key) = self.world_key_at(a) else { return };
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let Some(plan) = &world.plan else { return };
        if r.width <= 8.0 || r.height <= 8.0 {
            return;
        }
        let mut rects = plan.layout.node_rects.clone();
        rects.extend(plan.covers.iter().map(|(k, v)| (*k, *v)));
        if world.navigator.frame_rect(r, &world.arrangement, &rects) {
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
    }

    /// A pinch: the stage zooms live around where it began, then the camera
    /// settles on the level nearest how far you pinched.
    pub fn pinch(&mut self, total: f64, p: Point, ended: bool, cx: &mut Context<Self>) {
        if !self.prefs.animate {
            if ended && total.abs() >= 0.25 {
                self.zoom_step(total > 0.0, p, cx);
            }
            return;
        }
        if self.zoom.is_none() {
            if ended {
                return;
            }
            self.begin_zoom(p, cx);
        }
        let Some(z) = &mut self.zoom else { return };
        z.scale = total.exp().clamp(0.05, 20.0);
        z.last = Instant::now();
        let (key, anchor, scale) = (z.world.clone(), z.anchor, z.scale);
        let Some(display) = self.worlds.get(&key).map(|w| w.display.clone()) else { return };
        if let Some(panel) = self.stage.panel(&display, cx) {
            let local = Point::new(anchor.x, anchor.y);
            panel.set_camera(scale, local, cx);
        }
        if !ended {
            return;
        }
        self.zoom = None;
        let gap = self.prefs.gap;
        let world = self.worlds.get_mut(&key).expect("the world was just read");
        let Some(plan) = &world.plan else {
            if let Some(panel) = self.stage.panel(&display, cx) {
                panel.fade(0.14, cx);
            }
            return;
        };
        let mut rects = plan.projected.clone();
        rects.extend(plan.covers.iter().map(|(k, v)| (*k, *v)));
        let screen = Solver::bounds(display.visible, gap);
        let target = world.navigator.settle_target(scale, anchor, &world.arrangement, &rects, screen);
        clog!("PINCH ×{scale:.2} settles on {target}");
        if world.navigator.frame(target, &world.arrangement) {
            self.layout(&key, true, LayoutOptions::default(), cx);
        } else {
            let writer = self.writer.clone();
            if let Some(panel) = self.stage.panel(&display, cx) {
                panel.animate(&Default::default(), 0.3, cx);
                panel.land(0.3, &writer, 0.02, cx);
            }
        }
    }

    fn begin_zoom(&mut self, p: Point, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_at(p).or_else(|| self.current_world_key()) else { return };
        self.refresh_frames();
        let world = &self.worlds[&key];
        let Some(plan) = &world.plan else { return };
        let display = world.display.clone();
        let desk = plan.desktop_rect;
        let mut included = HashSet::new();
        // Windows outside the view sit where the camera would see them.
        let mut items = Vec::new();
        for tile in world.arrangement.tiles() {
            let Some(sel) = world.arrangement.selected_window(tile).filter(|s| self.parked.contains(s)) else { continue };
            let Some(r) = plan.projected.get(&tile) else { continue };
            if let Some(item) = self.stage_item(sel, *r, None) {
                items.push(item);
                included.insert(sel);
            }
        }
        items.extend(self.on_screen_items(&display, &mut included));
        let ground = self.ground(&display);
        if let Some(panel) = self.stage.panel(&display, cx) {
            panel.show(items, desk, ground, cx);
        }
        self.zoom = Some(ZoomSession { world: key, anchor: p, scale: 1.0, last: Instant::now() });
        self.publish_input();
    }

    fn zoom_step(&mut self, inward: bool, pointer: Point, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_at(pointer) else { return };
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let rects = world.plan.as_ref().map(|p| p.layout.node_rects.clone()).unwrap_or_default();
        let moved = if inward {
            world.navigator.step_in(&world.arrangement, None, Some(pointer), &rects, true)
        } else {
            world.navigator.step_out(&world.arrangement)
        };
        if moved {
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
    }
}
