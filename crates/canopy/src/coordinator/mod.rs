//! Ties windows, input, layout and motion together. Main thread only, and it
//! never waits on an app: every Accessibility call goes through `ax_service`
//! and `WindowWriter`, frames come from the window server.

pub mod input;
mod messages;
mod overlays;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use canopy_core::{
    Arrangement, Command, DisplayState, DropResolver, KeyCombo, Navigator, NodeId, Plan, Planner, Preferences, Rect, RestoreTarget,
    SavedState, SettingsPane, Size, WindowId, WindowRef,
};
use gpui::{App, AsyncApp, Context, Task, WeakEntity, WindowHandle};
use objc2::rc::Retained;

use crate::clog;
use crate::mac::ax_service::{WindowWriter, WriteResult};
use crate::mac::capture::PictureStore;
use crate::mac::displays::{mouse, Display};
use crate::mac::event_tap::EventTap;
use crate::mac::hotkeys::HotkeyCenter;
use crate::mac::menu::{MenuBar, MenuState};
use crate::mac::spaces::{self, Space};
use crate::mac::tracker::{TrackedWindow, WindowTracker};
use crate::mac::window_server;
use crate::mac::{app_look, workspace};
use crate::store;
use crate::ui::desk::DeskPanel;
use crate::ui::overlays::{CoverPanel, DropPreviewPanel, GapHandlePanel, GhostPanel, MarqueePanel, TabStripPanel};
use crate::ui::settings::SettingsView;
use crate::ui::stage::{Ground, Stage, StageItem};
use crate::ui::welcome::WelcomeView;
use input::{InputRouter, InputState, Press, TabDrag, ZoomSession};

pub const WINDOW_ID_E2E_BUNDLE: &str = "app.canopy.e2e.windows";

/// The arrangement and camera of one display on one Space (desktop).
pub struct World {
    pub display: Display,
    /// The Space's stable key ("main" for the first desktop).
    pub space: String,
    pub arrangement: Arrangement,
    pub navigator: Navigator,
    pub plan: Option<Plan>,
}

impl World {
    fn new(display: Display, space: String) -> Self {
        Self { display, space, arrangement: Arrangement::new(), navigator: Navigator::new(), plan: None }
    }

    /// This world's identity: display and Space.
    pub fn key(&self) -> String {
        world_key(&self.display.uuid, &self.space)
    }
}

fn world_key(display: &str, space: &str) -> String {
    format!("{display}|{space}")
}

fn space_of_key(key: &str) -> String {
    key.split_once('|').map_or("main", |(_, space)| space).to_string()
}

/// How a layout change reaches the screen.
#[derive(Default)]
pub struct LayoutOptions {
    pub confine_floating: bool,
    pub float_to: HashMap<WindowId, Rect>,
    pub dragged: Option<WindowId>,
    pub quick: bool,
}

/// The real moves a layout asks for. On a stage they wait until it is on screen.
struct Apply {
    world: String,
    plan: Plan,
    float_moves: HashMap<WindowId, Rect>,
    float_park: Vec<WindowId>,
    float_return: HashMap<WindowId, Rect>,
}

pub struct Coordinator {
    pub tracker: WindowTracker,
    pub writer: WindowWriter,
    pub pictures: PictureStore,
    pub stage: Stage,
    hotkeys: HotkeyCenter,
    pub resolver: DropResolver,
    pub input: Arc<InputRouter>,
    tap: Option<EventTap>,

    pub prefs: Preferences,
    pub bindings: HashMap<Command, KeyCombo>,

    pub menu_bar: Option<Retained<MenuBar>>,
    settings: Option<WindowHandle<SettingsView>>,
    settings_view: Rc<RefCell<Option<WeakEntity<SettingsView>>>>,
    pub welcome: Option<WindowHandle<WelcomeView>>,
    trust_task: Option<Task<()>>,
    /// The Settings pane to show, set by whatever asks for it.
    pending_pane: Option<SettingsPane>,

    /// Every display's arrangement on every Space, by world key.
    pub worlds: HashMap<String, World>,
    /// The world each display is showing now (its current Space), by display
    /// UUID. Absent on a full-screen app's Space.
    active: HashMap<String, String>,
    /// The window server's id of each world's Space.
    space_ids: HashMap<String, u64>,
    pub started: bool,
    /// Test harness: manage only this app's windows, without the tap or hotkeys.
    pub only_bundle_id: Option<String>,
    pub use_event_tap: bool,
    pub use_hotkeys: bool,
    pub show_glass_in_harness: bool,
    pub capture_desks: bool,
    pub tap_running: bool,

    detached: HashMap<String, DisplayState>,
    /// Minimised, hidden or full-screen docked windows, and where they go back.
    away: HashMap<WindowId, (String, RestoreTarget)>,
    /// Windows floated with the green button, and where they were docked.
    float_restore: HashMap<WindowId, (String, RestoreTarget)>,
    /// A window's floating frame from before it was docked.
    float_frames: HashMap<WindowId, Rect>,
    /// Floating windows parked while their Desktop is hidden.
    parked_floating: HashMap<WindowId, (String, Rect)>,
    pub parked: HashSet<WindowId>,
    /// The parked windows that are hidden tabs behind their selected tab,
    /// rather than away at a corner of the display.
    stacked: HashSet<WindowId>,
    /// The rect each docked window was last given.
    applied: HashMap<WindowId, Rect>,
    last_write: HashMap<WindowId, Instant>,
    snap_backs: HashMap<WindowId, Vec<Instant>>,
    moved_while_mouse_down: HashSet<WindowId>,
    app_minimums: HashMap<String, Size>,
    relayout_tasks: HashMap<String, Task<()>>,

    desks: HashMap<String, DeskPanel>,
    covers: HashMap<String, CoverPanel>,
    strips: HashMap<String, TabStripPanel>,
    handles: HashMap<String, GapHandlePanel>,
    preview: Option<DropPreviewPanel>,
    marquee: Option<MarqueePanel>,
    ghost: Option<GhostPanel>,

    pub press: Option<Press>,
    pub tab_drag: Option<TabDrag>,
    zoom: Option<ZoomSession>,
    /// The world whose gap is being dragged on the stage.
    seam_world: Option<String>,
    hold_task: Option<Task<()>>,
    /// What was on screen when the drag began, front to back.
    under_drag: Vec<window_server::OnScreen>,
    /// When a drag first found the mouse button up.
    button_up_since: Option<Instant>,
    /// When a stage with nothing moving it was first seen.
    stage_idle: Option<Instant>,
    /// Each display's scale, the main one first, as of the last display change.
    scales: Vec<(u32, f64)>,
    spaces_checking: bool,
    pending_split: Option<(Option<NodeId>, String, Instant)>,
    save_task: Option<Task<()>>,

    /// Main-thread responsiveness, for the harness: the longest gap between ticks.
    worst_stall: f64,
    last_tick: Instant,
    launched: bool,
    quitting: bool,
}

impl Coordinator {
    pub fn new() -> Self {
        Self {
            tracker: WindowTracker::new(),
            writer: WindowWriter::default(),
            pictures: PictureStore::default(),
            stage: Stage::default(),
            hotkeys: HotkeyCenter::new(),
            resolver: DropResolver::default(),
            input: Arc::new(InputRouter::default()),
            tap: None,
            prefs: Preferences::default(),
            bindings: canopy_core::Shortcuts::defaults(),
            menu_bar: None,
            settings: None,
            settings_view: Rc::default(),
            welcome: None,
            trust_task: None,
            pending_pane: None,
            worlds: HashMap::new(),
            active: HashMap::new(),
            space_ids: HashMap::new(),
            started: false,
            only_bundle_id: None,
            use_event_tap: true,
            use_hotkeys: true,
            show_glass_in_harness: false,
            capture_desks: true,
            tap_running: false,
            detached: HashMap::new(),
            away: HashMap::new(),
            float_restore: HashMap::new(),
            float_frames: HashMap::new(),
            parked_floating: HashMap::new(),
            parked: HashSet::new(),
            stacked: HashSet::new(),
            applied: HashMap::new(),
            last_write: HashMap::new(),
            snap_backs: HashMap::new(),
            moved_while_mouse_down: HashSet::new(),
            app_minimums: HashMap::new(),
            relayout_tasks: HashMap::new(),
            desks: HashMap::new(),
            covers: HashMap::new(),
            strips: HashMap::new(),
            handles: HashMap::new(),
            preview: None,
            marquee: None,
            ghost: None,
            press: None,
            tab_drag: None,
            zoom: None,
            seam_world: None,
            hold_task: None,
            under_drag: Vec::new(),
            button_up_since: None,
            stage_idle: None,
            scales: Vec::new(),
            spaces_checking: false,
            pending_split: None,
            save_task: None,
            worst_stall: 0.0,
            last_tick: Instant::now(),
            launched: false,
            quitting: false,
        }
    }

    // MARK: - Lifecycle

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.launched, true) {
            return;
        }
        crate::log::open();
        crate::mac::stay_out_of_app_nap();
        clog!(
            "LAUNCH pid {} accessibility={} screen={}",
            std::process::id(),
            crate::mac::ax::is_trusted(),
            crate::ui::welcome::screen_recording_allowed()
        );
        self.prefs = store::load_preferences();
        self.bindings = store::load_bindings();
        self.app_minimums = store::load_app_minimums();
        self.apply_preferences_to_parts();
        window_server::set_cursor_in_background();
        app_look::prepare_wallpapers();
        for app in workspace::running_apps().into_iter().filter(|a| a.regular) {
            if let Some(bundle_id) = app.bundle_id {
                app_look::request(&bundle_id, app.pid);
            }
        }
        self.rebuild_displays(cx);
        workspace::start_observing();

        let events = self.tracker.start();
        self.handle_tracker_events(events, cx);

        if self.use_hotkeys {
            self.hotkeys.register(&self.bindings);
        } else {
            self.hotkeys.unregister_all();
        }

        self.repeat("tick", Duration::from_millis(20), Self::tick, cx);
        self.repeat("picture timer", Duration::from_millis(1500), |c, _| c.refresh_pictures(), cx);
        self.repeat("desk timer", Duration::from_secs(15), |c, _| c.refresh_desks(), cx);
        self.repeat("rescan", Duration::from_secs(3), |c, _| c.tracker.rescan_all(), cx);
        self.refresh_desks();

        // Give the first scan a moment, then restore saved arrangements.
        self.after(Duration::from_millis(600), |c, cx| {
            c.restore_saved();
            c.started = true;
            c.start_input();
            for key in c.active_keys() {
                c.layout(&key, false, LayoutOptions::default(), cx);
            }
            c.repeat("space check", Duration::from_secs(3), Self::check_window_spaces, cx);
            c.refresh_pictures();
            c.state_changed();
        }, cx);
    }

    /// Runs `body` every `interval`, as long as the coordinator lives.
    fn repeat(&self, name: &'static str, interval: Duration, body: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(interval).await;
            let done = this.update(cx, |c, cx| {
                let started = Instant::now();
                body(c, cx);
                crate::log::slow(name, started);
            });
            if done.is_err() {
                break;
            }
        })
        .detach();
    }

    /// Runs `body` once, after `delay`.
    fn after(&self, delay: Duration, body: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) {
        self.later(delay, body, cx).detach();
    }

    fn later(&self, delay: Duration, body: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, body);
        })
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let gap = (now - self.last_tick).as_secs_f64();
        self.worst_stall = self.worst_stall.max(gap);
        if gap > 0.06 {
            clog!("STALL main thread {} ms", (gap * 1000.0) as i64);
        }
        self.last_tick = now;
        self.watch_input(cx);
    }

    fn picture_scale(&self) -> f64 {
        self.scales.first().map_or(2.0, |(_, scale)| *scale) * 0.5
    }

    fn refresh_pictures(&mut self) {
        let scale = self.picture_scale();
        self.pictures.refresh_on_screen(self.parked.clone(), scale);
    }

    pub fn refresh_desks(&mut self) {
        if !self.capture_desks {
            return;
        }
        self.pictures.refresh_desks(self.scales.iter().copied().collect());
    }

    fn start_input(&mut self) {
        self.publish_input();
        if !self.use_event_tap {
            return;
        }
        let router = self.input.clone();
        let mut thread = input::InputThread::default();
        self.tap = EventTap::start(Box::new(move |kind, event| thread.handle(&router, kind, event)));
        self.tap_running = self.tap.is_some();
        clog!(
            "{}",
            if self.tap_running {
                "INPUT event tap running on its own thread"
            } else {
                "INPUT event tap unavailable; drag-to-dock and gestures are off"
            }
        );
    }

    pub fn reset_stall_meter(&mut self) -> f64 {
        std::mem::take(&mut self.worst_stall)
    }

    /// Brings every parked window back into view and saves (quitting returns
    /// all windows to visible positions).
    pub fn prepare_for_quit(&mut self, cx: &mut Context<Self>) {
        if !self.started || self.quitting {
            return;
        }
        self.quitting = true;
        self.save_now();
        if let Some(tap) = self.tap.take() {
            tap.stop();
        }
        self.hotkeys.unregister_all();
        let mut jobs = Vec::new();
        let mut cascade = 0.0;
        for world in self.worlds.values() {
            let v = world.display.visible;
            for id in world.arrangement.windows() {
                if !self.parked.contains(&id) {
                    continue;
                }
                let Some(tw) = self.tracker.windows.get(&id) else { continue };
                let size = self
                    .applied
                    .get(&id)
                    .map(Rect::size)
                    .or(self.float_frames.get(&id).map(Rect::size))
                    .unwrap_or(Size::new(900.0, 650.0));
                let w = size.width.max(480.0).min(v.width * 0.8);
                let h = size.height.max(320.0).min(v.height * 0.8);
                let r = Rect::new(v.mid_x() - w / 2.0 + cascade, v.mid_y() - h / 2.0 + cascade, w, h);
                jobs.push((tw.ax.clone(), r.clamped(&v)));
                cascade = if cascade >= 180.0 { 0.0 } else { cascade + 30.0 };
            }
        }
        for (id, (_, frame)) in &self.parked_floating {
            if let Some(tw) = self.tracker.windows.get(id) {
                jobs.push((tw.ax.clone(), *frame));
            }
        }
        self.writer.flush_synchronously(jobs, Duration::from_millis(2500));
        for p in self.covers.values() {
            p.panel.order_out();
        }
        for p in self.strips.values() {
            p.panel.order_out();
        }
        for p in self.handles.values() {
            p.panel.order_out();
        }
        for d in self.desks.values() {
            d.hide();
        }
        self.stage.tear_down(cx);
    }

    fn apply_preferences_to_parts(&mut self) {
        self.stage.duration = self.prefs.animation_duration;
        self.stage.redraw_wait = self.prefs.redraw_wait;
        self.pictures.enabled = self.prefs.live_previews;
    }

    /// Takes new preferences, saving them and bringing everything in line.
    pub fn set_prefs(&mut self, prefs: Preferences, cx: &mut Context<Self>) {
        if prefs == self.prefs {
            return;
        }
        let old = std::mem::replace(&mut self.prefs, prefs);
        store::save_preferences(&self.prefs);
        self.apply_preferences_to_parts();
        if !self.started {
            return;
        }
        if self.prefs.ignored_bundle_ids != old.ignored_bundle_ids {
            let ignored: Vec<WindowId> = self
                .tracker
                .windows
                .values()
                .filter(|w| self.prefs.ignored_bundle_ids.contains(&w.bundle_id) && !old.ignored_bundle_ids.contains(&w.bundle_id))
                .map(TrackedWindow::id)
                .collect();
            for id in ignored {
                self.float(id, false, cx);
            }
        }
        let keys: Vec<String> = self.worlds.keys().cloned().collect();
        for key in keys {
            self.layout(&key, false, LayoutOptions::default(), cx);
        }
        self.state_changed();
    }

    pub fn set_bindings(&mut self, bindings: HashMap<Command, KeyCombo>) {
        self.bindings = bindings;
        store::save_bindings(&self.bindings);
        if self.use_hotkeys {
            self.hotkeys.register(&self.bindings);
        }
        self.state_changed();
    }

    /// Stops shortcuts while one is being recorded.
    pub fn pause_hotkeys(&mut self) {
        self.hotkeys.unregister_all();
    }

    pub fn resume_hotkeys(&mut self) {
        if self.use_hotkeys {
            self.hotkeys.register(&self.bindings);
        }
    }

    /// What the input thread may read.
    pub fn publish_input(&self) {
        let managed = self.tracker.windows.values().filter(|w| self.manageable(w)).map(TrackedWindow::id).collect();
        self.input.publish(InputState {
            started: self.started,
            rect_zoom: self.prefs.rect_zoom,
            green_button: self.prefs.green_button_toggles,
            title_double_click: self.prefs.title_double_click_frames,
            option_pinch: self.prefs.option_pinch,
            managed,
            docked: self.worlds.values().flat_map(|w| w.arrangement.windows()).collect(),
            green_buttons: self.tracker.windows.values().filter_map(|w| Some((w.id(), w.green_button?))).collect(),
            landing: self.stage.any_shown(),
        });
    }

    /// Menu bar refresh.
    pub fn state_changed(&self) {
        let Some(bar) = &self.menu_bar else { return };
        let world = if self.started { self.current_world_key().and_then(|k| self.worlds.get(&k)) } else { None };
        bar.refresh(MenuState {
            started: self.started,
            screen_recording: crate::ui::welcome::screen_recording_last_known(),
            controls: self.prefs.menu_bar_controls,
            saved_views: world.map(|w| w.navigator.saved_slots()).unwrap_or_default(),
            can_go_back: world.is_some_and(|w| w.navigator.can_go_back()),
            can_go_forward: world.is_some_and(|w| w.navigator.can_go_forward()),
            desktop_hidden: world.is_some_and(|w| w.arrangement.is_desktop_hidden()),
            ignored: self.prefs.ignored_bundle_ids.clone(),
            bindings: self.bindings.clone(),
        });
    }

    // MARK: - Spaces

    pub fn active_keys(&self) -> Vec<String> {
        self.active.values().filter(|k| self.worlds.contains_key(*k)).cloned().collect()
    }

    pub fn is_active(&self, key: &str) -> bool {
        self.worlds.get(key).is_some_and(|w| self.active.get(&w.display.uuid).map(String::as_str) == Some(key))
    }

    fn active_key_for(&self, display: &Display) -> Option<String> {
        self.active.get(&display.uuid).filter(|k| self.worlds.contains_key(*k)).cloned()
    }

    /// Points `display` at its world for `space`, creating it if new. A saved
    /// arrangement for that Space is restored separately (`restore_space`),
    /// once its windows have shown up. Returns true when the display changed world.
    fn activate(&mut self, display: &Display, space: Option<&Space>) -> bool {
        let main = Space::main();
        let space = space.unwrap_or(&main);
        if space.full_screen {
            // A full-screen app's Space: nothing to arrange.
            return self.active.remove(&display.uuid).is_some();
        }
        let key = world_key(&display.uuid, &space.key);
        self.space_ids.insert(key.clone(), space.id);
        if self.active.get(&display.uuid) == Some(&key) {
            return false;
        }
        if !self.worlds.contains_key(&key) && (!self.detached.contains_key(&key) || !self.started) {
            self.worlds.insert(key.clone(), World::new(display.clone(), space.key.clone()));
        }
        self.active.insert(display.uuid.clone(), key);
        true
    }

    /// Back on a Space Canopy arranged before (since launch, or saved): its
    /// windows return to their places.
    fn restore_space(&mut self, display: &Display) {
        let Some(key) = self.active.get(&display.uuid).cloned() else { return };
        if self.worlds.contains_key(&key) {
            return;
        }
        let mut world = World::new(display.clone(), space_of_key(&key));
        if let Some(mut saved) = self.detached.remove(&key) {
            let live = self.live_refs(true, self.space_ids.get(&key).copied());
            saved.reattach(&live, self.prefs.share_removed_space);
            clog!("SPACE restored {} windows", saved.arrangement.windows().len());
            world.arrangement = saved.arrangement;
            world.navigator = saved.navigator;
        }
        self.worlds.insert(key, world);
    }

    /// The user switched desktops: each display shows its world for the new
    /// Space. Windows on other Spaces are never touched.
    pub fn spaces_changed(&mut self, cx: &mut Context<Self>) {
        self.after(Duration::from_millis(600), |c, _| c.refresh_desks(), cx);
        if !self.started {
            return;
        }
        let displays = Display::current();
        let Some(spaces) = spaces::current(&displays) else { return };
        // A drag or pinch in progress belongs to the Space that just left.
        self.press = None;
        self.zoom = None;
        self.tab_drag = None;
        self.seam_world = None;
        for d in displays {
            let before = self.active.get(&d.uuid).cloned();
            if !self.activate(&d, spaces.get(&d.uuid)) {
                continue;
            }
            if let Some(p) = self.stage.panel(&d, cx) {
                p.tear_down(cx);
            }
            clog!(
                "SPACE {} leaves {} for {}",
                d.name,
                before.as_deref().unwrap_or("a full-screen Space"),
                self.active.get(&d.uuid).map_or("a full-screen Space", String::as_str)
            );
            // The new Space's windows take a moment to show up; then arrange them.
            for pid in self.tracker.tracked_pids() {
                self.tracker.scan(pid);
            }
            self.after(Duration::from_millis(500), move |c, cx| {
                c.restore_space(&d);
                if let Some(key) = c.active_key_for(&d) {
                    c.layout(&key, false, LayoutOptions::default(), cx);
                }
                c.publish_input();
            }, cx);
        }
        self.publish_input();
        self.state_changed();
    }

    /// A docked window moved to another Space (Mission Control, or an app's
    /// own "move to desktop"): it leaves this Space's arrangement.
    fn check_window_spaces(&mut self, _: &mut Context<Self>) {
        if !self.started || self.spaces_checking {
            return;
        }
        let mut asks = Vec::new();
        for key in self.active_keys() {
            let Some(&here) = self.space_ids.get(&key).filter(|h| **h != 0) else { continue };
            let world = &self.worlds[&key];
            let shown = world.arrangement.windows().into_iter().filter(|id| {
                !self.parked.contains(id)
                    || world.arrangement.tile_of(*id).and_then(|t| world.arrangement.selected_window(t)) == Some(*id)
            });
            asks.push((key, here, shown.collect()));
        }
        self.spaces_checking = true;
        spaces::find_moved(asks);
    }

    pub(super) fn spaces_checked(&mut self, answers: Vec<(String, Vec<WindowId>)>, cx: &mut Context<Self>) {
        self.spaces_checking = false;
        for (key, moved) in answers {
            // The user may have changed desktops since the question was asked.
            if moved.is_empty() || !self.is_active(&key) {
                continue;
            }
            let share = self.prefs.share_removed_space;
            for &id in &moved {
                clog!("SPACE w{id} moved to another desktop; it leaves this arrangement");
                let world = self.worlds.get_mut(&key).expect("the world is active");
                world.arrangement.remove(id, share);
                self.unpark(id);
                self.applied.remove(&id);
            }
            let world = self.worlds.get_mut(&key).expect("the world is active");
            let live = world.arrangement.windows().into_iter().collect();
            world.navigator.prune(&live);
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
    }

    pub fn rebuild_displays(&mut self, cx: &mut Context<Self>) {
        let now = Display::current();
        self.scales = now.iter().map(|d| (d.id, d.scale)).collect();
        let spaces = spaces::current(&now);
        for d in &now {
            for w in self.worlds.values_mut().filter(|w| w.display.uuid == d.uuid) {
                w.display = d.clone();
            }
            self.activate(d, spaces.as_ref().and_then(|s| s.get(&d.uuid)));
            // Open the stage ahead of the first motion.
            self.stage.panel(d, cx);
        }
        let main = now.first().cloned();
        let gone: Vec<String> = self
            .worlds
            .iter()
            .filter(|(_, w)| !now.iter().any(|d| d.uuid == w.display.uuid))
            .map(|(k, _)| k.clone())
            .collect();
        for key in gone {
            let world = self.worlds.remove(&key).expect("the key came from the map");
            self.active.remove(&world.display.uuid);
            let windows = world.arrangement.windows();
            self.detached.insert(
                key.clone(),
                DisplayState::new(world.arrangement, world.navigator, self.refs(&windows)),
            );
            // Its windows float onto a remaining display until it returns.
            if let Some(main) = &main {
                let mut offset = 0.0;
                for id in windows {
                    let v = main.visible;
                    let r = Rect::new(v.x + 60.0 + offset, v.y + 60.0 + offset, v.width.min(1500.0) * 0.6, v.height * 0.6);
                    if self.write(id, r) {
                        self.unpark(id);
                        offset += 30.0;
                    }
                }
            }
            let floating: Vec<(WindowId, Rect)> =
                self.parked_floating.iter().filter(|(_, (w, _))| *w == key).map(|(id, (_, f))| (*id, *f)).collect();
            for (id, frame) in floating {
                self.write(id, frame);
                self.parked_floating.remove(&id);
            }
            self.remove_overlays_of(&key, cx);
        }
        if self.started {
            for key in self.active_keys() {
                self.layout(&key, false, LayoutOptions::default(), cx);
            }
            self.schedule_save(cx);
        }
    }

    /// Saved arrangements come back for the Spaces on screen now; the others
    /// wait until their Space is opened. (Before Spaces were kept apart,
    /// arrangements were saved per display: those become the current Space's.)
    fn restore_saved(&mut self) {
        let Some(state) = store::load_state() else { return };
        let mut live = self.live_refs(false, None);
        for (saved_key, mut saved) in state.displays {
            let key = if saved_key.contains('|') {
                saved_key
            } else {
                match self.active.get(&saved_key) {
                    Some(current) => current.clone(),
                    None => continue,
                }
            };
            if self.is_active(&key) {
                saved.reattach(&live, self.prefs.share_removed_space);
                for id in saved.arrangement.windows() {
                    live.remove(&id);
                }
                let world = self.worlds.get_mut(&key).expect("active worlds exist");
                world.arrangement = saved.arrangement;
                world.navigator = saved.navigator;
            } else {
                self.detached.insert(key, saved);
            }
        }
    }

    /// Windows that can be arranged, optionally only floating ones and only
    /// those on one Space.
    fn live_refs(&self, floating_only: bool, on_space: Option<u64>) -> HashMap<WindowId, WindowRef> {
        let mut out = HashMap::new();
        for w in self.tracker.windows.values().filter(|w| self.manageable(w)) {
            if floating_only && self.world_of(w.id()).is_some() {
                continue;
            }
            if let Some(space) = on_space.filter(|s| *s != 0) {
                let spaces = spaces::of(w.id());
                if !spaces.is_empty() && !spaces.contains(&space) {
                    continue;
                }
            }
            out.insert(w.id(), w.window_ref());
        }
        out
    }

    fn refs(&self, ids: &[WindowId]) -> HashMap<WindowId, WindowRef> {
        ids.iter().filter_map(|id| Some((*id, self.tracker.windows.get(id)?.window_ref()))).collect()
    }

    pub fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(self.later(Duration::from_secs(1), |c, _| c.save_now(), cx));
    }

    fn save_now(&self) {
        let mut displays: std::collections::BTreeMap<String, DisplayState> =
            self.detached.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (key, w) in &self.worlds {
            let state = DisplayState::new(w.arrangement.clone(), w.navigator.clone(), self.refs(&w.arrangement.windows()));
            displays.insert(key.clone(), state);
        }
        store::save_state(&SavedState::new(displays));
    }

    // MARK: - Lookups

    pub fn world_of(&self, id: WindowId) -> Option<&World> {
        self.worlds.values().find(|w| w.arrangement.contains(id))
    }

    pub fn world_key_of(&self, id: WindowId) -> Option<String> {
        self.world_of(id).map(World::key)
    }

    /// The world shown on the display under `p`.
    pub fn world_key_at(&self, p: canopy_core::Point) -> Option<String> {
        self.active_keys().into_iter().find(|k| self.worlds[k].display.frame.contains(p))
    }

    fn primary_world_key(&self) -> Option<String> {
        Display::current().first().and_then(|main| self.active_key_for(main)).or_else(|| self.active_keys().into_iter().next())
    }

    /// Commands act on the focused window's display, else the pointer's.
    pub fn current_world_key(&self) -> Option<String> {
        if let Some(f) = self.tracker.focused_window() {
            if let Some(key) = self.world_key_of(f).filter(|k| self.is_active(k)) {
                return Some(key);
            }
            if let Some(frame) = self.tracker.windows.get(&f).and_then(|w| w.frame) {
                if let Some(key) = self.world_key_at(frame.center()) {
                    return Some(key);
                }
            }
        }
        self.world_key_at(mouse()).or_else(|| self.primary_world_key())
    }

    fn display_world_key(&self, w: &TrackedWindow) -> Option<String> {
        if let Some((key, _)) = self.parked_floating.get(&w.id()) {
            return Some(key.clone()).filter(|k| self.worlds.contains_key(k));
        }
        self.world_key_at(w.frame?.center())
    }

    pub fn manageable(&self, w: &TrackedWindow) -> bool {
        if self.only_bundle_id.as_ref().is_some_and(|only| w.bundle_id != *only) {
            return false;
        }
        // The end-to-end test's windows belong to the test harness alone.
        if self.only_bundle_id.is_none() && w.bundle_id == WINDOW_ID_E2E_BUNDLE {
            return false;
        }
        !self.prefs.ignored_bundle_ids.contains(&w.bundle_id) && !w.minimized && !w.full_screen && !w.hidden
    }

    /// Floating windows that live on a world's Desktop: on its Space now, or
    /// parked by it (excluded apps are left alone).
    fn floating_windows(&self, key: &str) -> Vec<WindowId> {
        let here: HashSet<WindowId> = window_server::on_screen().iter().map(|o| o.id).collect();
        self.tracker
            .windows
            .values()
            .filter(|w| {
                if !self.manageable(w) || self.world_of(w.id()).is_some() {
                    return false;
                }
                if let Some((world, _)) = self.parked_floating.get(&w.id()) {
                    return world == key;
                }
                here.contains(&w.id()) && self.display_world_key(w).as_deref() == Some(key)
            })
            .map(TrackedWindow::id)
            .collect()
    }

    /// The smallest size this window has accepted. Windows of one app can
    /// differ, so the app's minimum is only a floor for parking.
    fn minimum(&self, id: WindowId) -> Option<Size> {
        self.tracker.windows.get(&id)?.minimum_size
    }

    fn parking_floor(&self, tw: &TrackedWindow) -> Option<Size> {
        let (a, b) = (tw.minimum_size, self.app_minimums.get(&tw.bundle_id));
        if a.is_none() && b.is_none() {
            return None;
        }
        Some(Size::new(
            a.map_or(0.0, |s| s.width).max(b.map_or(0.0, |s| s.width)),
            a.map_or(0.0, |s| s.height).max(b.map_or(0.0, |s| s.height)),
        ))
    }

    /// Updates cached frames from the window server (well under a millisecond).
    pub fn refresh_frames(&mut self) {
        for o in window_server::on_screen() {
            if let Some(tw) = self.tracker.windows.get_mut(&o.id) {
                if !self.writer.is_busy(o.id) {
                    tw.frame = Some(o.frame);
                }
            }
        }
    }

    // MARK: - Writing frames

    /// Asks a window to move. False if Canopy doesn't know it.
    fn write(&mut self, id: WindowId, r: Rect) -> bool {
        let Some(tw) = self.tracker.windows.get_mut(&id) else { return false };
        clog!("WRITE w{id} {} {r}", tw.app_name);
        self.writer.set_frame(&tw.ax, r);
        tw.frame = Some(r);
        true
    }

    fn written(&mut self, result: WriteResult, cx: &mut Context<Self>) {
        self.last_write.insert(result.id, Instant::now());
        let (Some(tw), Some(actual)) = (self.tracker.windows.get_mut(&result.id), result.actual_size) else { return };
        tw.frame = Some(Rect::new(result.asked.x, result.asked.y, actual.width, actual.height));
        // A window that refused to shrink tells us its minimum; remember it for the app too.
        let wider = actual.width > result.asked.width + 2.0;
        let taller = actual.height > result.asked.height + 2.0;
        if !wider && !taller {
            return;
        }
        let learned = Size::new(
            (if wider { actual.width } else { 0.0 }).max(tw.minimum_size.map_or(0.0, |s| s.width)),
            (if taller { actual.height } else { 0.0 }).max(tw.minimum_size.map_or(0.0, |s| s.height)),
        );
        if Some(learned) == tw.minimum_size {
            return;
        }
        let over_w = if wider { actual.width - result.asked.width } else { 0.0 };
        let over_h = if taller { actual.height - result.asked.height } else { 0.0 };
        let small = over_w <= self.prefs.gap && over_h <= self.prefs.gap;
        clog!(
            "MINIMUM w{} {} refused {} → {}×{}",
            result.id,
            tw.app_name,
            result.asked,
            actual.width as i64,
            actual.height as i64
        );
        tw.minimum_size = Some(learned);
        let entry = self.app_minimums.entry(tw.bundle_id.clone()).or_default();
        *entry = Size::new(entry.width.max(learned.width), entry.height.max(learned.height));
        store::save_app_minimums(&self.app_minimums);
        // Reaching a little into the gap changes nothing; a real shortfall makes
        // a cover. A hidden tab that won't fit would stick out from behind its group.
        if self.stacked.contains(&result.id) || (!small && !self.parked.contains(&result.id)) {
            if let Some(key) = self.world_key_of(result.id) {
                self.schedule_relayout(key, cx);
            }
        }
    }

    fn schedule_relayout(&mut self, key: String, cx: &mut Context<Self>) {
        let k = key.clone();
        let task = self.later(Duration::from_millis(50), move |c, cx| c.layout(&k, false, LayoutOptions::default(), cx), cx);
        self.relayout_tasks.insert(key, task);
    }

    fn unpark(&mut self, id: WindowId) {
        self.parked.remove(&id);
        self.stacked.remove(&id);
    }

    /// Moves a window out of view: shrunk, at a corner of its display no other
    /// display touches, leaving the sliver on screen that macOS requires.
    fn park(&mut self, id: WindowId, key: &str) {
        if self.parked.contains(&id) && !self.stacked.contains(&id) {
            return;
        }
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let current = tw.frame.or(self.applied.get(&id).copied()).unwrap_or(Rect::new(0.0, 0.0, 800.0, 600.0));
        let floor = self.parking_floor(tw);
        let size = Size::new(
            floor.map_or(0.0, |s| s.width).max(current.width.min(480.0)),
            floor.map_or(0.0, |s| s.height).max(128.0),
        );
        let Some(world) = self.worlds.get(key) else { return };
        let f = world.display.frame;
        let others: Vec<Rect> = self
            .active_keys()
            .iter()
            .map(|k| &self.worlds[k].display)
            .filter(|d| d.uuid != world.display.uuid)
            .map(|d| d.frame)
            .collect();
        let candidates = [(f.max_x() - 1.0, f.max_y() - 1.0), (f.min_x() - size.width + 1.0, f.max_y() - 1.0), (f.max_x() - 1.0, f.min_y() + 1.0)];
        let spot = candidates
            .iter()
            .copied()
            .find(|&(x, y)| {
                let r = Rect::new(x, y, size.width, size.height);
                !others.iter().any(|o| o.intersects(&r))
            })
            .unwrap_or(candidates[0]);
        let r = Rect::new(spot.0, spot.1, size.width, size.height);
        clog!("PARK w{id} {}", tw.app_name);
        self.writer.set_frame(&tw.ax, r);
        if let Some(tw) = self.tracker.windows.get_mut(&id) {
            tw.frame = Some(r);
        }
        self.stacked.remove(&id);
        self.parked.insert(id);
        self.applied.remove(&id);
    }

    // MARK: - Layout

    fn plan_for(&self, key: &str) -> Option<Plan> {
        let world = self.worlds.get(key)?;
        Some(Planner::plan(&world.arrangement, &world.navigator, world.display.visible, self.prefs.gap, |id| self.minimum(id)))
    }

    /// Solves a world for its camera and moves every window there. Animated
    /// changes play on the stage; the real windows move underneath.
    pub fn layout(&mut self, key: &str, animated: bool, options: LayoutOptions, cx: &mut Context<Self>) {
        // A world on another Space keeps its arrangement but moves nothing:
        // its windows are out of sight on that Space. It is laid out on return.
        if !self.started || !self.is_active(key) {
            self.schedule_save(cx);
            return;
        }
        self.refresh_frames();
        let Some(plan) = self.plan_for(key) else { return };
        let world = self.worlds.get_mut(key).expect("the plan came from this world");
        let old = world.plan.replace(plan.clone());
        let display = world.display.clone();
        clog!(
            "LAYOUT {} framed {} visible {} covers {} parked {}{}",
            world.arrangement.describe(world.arrangement.root()),
            world.navigator.framed_node(&world.arrangement),
            plan.visible.len(),
            plan.covers.len(),
            plan.parked.len(),
            if animated { " animated" } else { "" }
        );

        // Floating windows: hidden with the Desktop, kept on it otherwise.
        let mut float_moves = options.float_to;
        let mut float_park = Vec::new();
        let mut float_return = HashMap::new();
        let floating = self.floating_windows(key);
        if let Some(desk) = plan.desktop_rect {
            for (id, (w, frame)) in &self.parked_floating {
                if w == key {
                    float_return.insert(*id, *frame);
                }
            }
            if options.confine_floating {
                for &w in &floating {
                    if self.parked_floating.contains_key(&w) || float_moves.contains_key(&w) || Some(w) == options.dragged {
                        continue;
                    }
                    let Some(f) = self.tracker.windows.get(&w).and_then(|t| t.frame) else { continue };
                    if f.intersection(&desk).area() < 0.6 * f.area() {
                        let r = Rect::new(f.x, f.y, f.width.min(desk.width), f.height.min(desk.height));
                        float_moves.insert(w, r.clamped(&desk));
                    }
                }
            }
        } else {
            float_park = floating.iter().copied().filter(|w| !self.parked_floating.contains_key(w)).collect();
        }

        let apply = Apply {
            world: key.to_string(),
            plan: plan.clone(),
            float_moves: float_moves.clone(),
            float_park: float_park.clone(),
            float_return: float_return.clone(),
        };

        let shown = self.stage.panel(&display, cx).is_some_and(|p| p.is_shown());
        if animated && self.prefs.animate {
            let (items, targets, moving) = self.motion(key, &plan, old.as_ref(), &float_moves, &float_park, &float_return);
            if shown {
                // Continue on the stage a drag put up.
                let duration = if options.quick { 0.12 } else { self.stage.duration };
                if let Some(p) = self.stage.panel(&display, cx) {
                    p.set_slot(None, false, cx);
                    p.animate(&targets, duration, cx);
                    p.set_desk(plan.desktop_rect, duration, cx);
                }
                self.apply_when_ready(apply, duration, &display, cx);
            } else if moving {
                let ground = self.ground(&display);
                let duration = self.stage.duration;
                if let Some(p) = self.stage.panel(&display, cx) {
                    p.show(items, old.as_ref().and_then(|o| o.desktop_rect), ground, cx);
                    p.animate(&targets, duration, cx);
                    p.set_desk(plan.desktop_rect, duration, cx);
                }
                self.apply_when_ready(apply, duration, &display, cx);
            } else {
                self.run_apply(apply, cx);
            }
        } else {
            // A landing in progress finishes on its own; it waits for these writes too.
            if shown && !self.prefs.animate {
                if let Some(p) = self.stage.panel(&display, cx) {
                    p.fade(0.14, cx);
                }
            }
            self.run_apply(apply, cx);
        }
        self.publish_input();
        self.schedule_save(cx);
        self.state_changed();
    }

    /// Runs `apply` once the stage covers the real windows, then lands.
    fn apply_when_ready(&mut self, apply: Apply, land_after: f64, display: &Display, cx: &mut Context<Self>) {
        if self.stage.panel(display, cx).is_none_or(|p| p.is_ready(cx)) {
            self.run_apply(apply, cx);
            self.land(display, land_after, cx);
            return;
        }
        let weak = cx.weak_entity();
        let target = display.clone();
        let body: Box<dyn FnOnce(&mut App)> = Box::new(move |cx| {
            let _ = weak.update(cx, |c, cx| {
                c.run_apply(apply, cx);
                c.land(&target, land_after, cx);
            });
        });
        if let Some(p) = self.stage.panel(display, cx) {
            p.enqueue(body, cx);
        }
    }

    fn land(&mut self, display: &Display, delay: f64, cx: &mut Context<Self>) {
        let (redraw, writer) = (self.stage.redraw_wait, self.writer.clone());
        if let Some(p) = self.stage.panel(display, cx) {
            p.land(delay, &writer, redraw, cx);
        }
    }

    /// Stage items for everything on the display, back to front, where it is
    /// now; and where each one goes.
    fn motion(
        &self,
        key: &str,
        plan: &Plan,
        old: Option<&Plan>,
        float_moves: &HashMap<WindowId, Rect>,
        float_park: &[WindowId],
        float_return: &HashMap<WindowId, Rect>,
    ) -> (Vec<StageItem>, HashMap<WindowId, Rect>, bool) {
        let world = &self.worlds[key];
        let v = world.display.visible;
        let mut items = Vec::new();
        let mut targets = HashMap::new();
        let mut included = HashSet::new();
        let a = &world.arrangement;

        // Docked windows coming out of parking start from where the old camera had them.
        for tile in a.tiles() {
            let Some(sel) = a.selected_window(tile).filter(|s| self.parked.contains(s)) else { continue };
            let Some(from) = old.and_then(|o| o.covers.get(&tile).or(o.projected.get(&tile))) else { continue };
            if let Some(item) = self.stage_item(sel, *from, None) {
                items.push(item);
                included.insert(sel);
            }
        }
        // Everything on screen, back to front.
        items.extend(self.on_screen_items(&world.display, &mut included));
        // Floating windows returning with the Desktop come from below.
        for (id, back) in float_return {
            if included.contains(id) {
                continue;
            }
            if let Some(item) = self.stage_item(*id, Rect::new(back.x, v.max_y() + 40.0, back.width, back.height), None) {
                items.push(item);
                included.insert(*id);
            }
        }

        for tile in a.tiles() {
            let Some(sel) = a.selected_window(tile) else { continue };
            let to = plan.visible.get(&sel).or(plan.covers.get(&tile)).or(plan.projected.get(&tile));
            if let Some(to) = to {
                // Every tab: one that just joined the group may still be on the stage.
                targets.extend(a.get(tile).windows.iter().map(|w| (*w, *to)));
            }
        }
        targets.extend(float_moves.iter().map(|(k, v)| (*k, *v)));
        for id in float_park {
            if let Some(f) = self.tracker.windows.get(id).and_then(|w| w.frame) {
                targets.insert(*id, Rect::new(f.x, v.max_y() + 40.0, f.width, f.height));
            }
        }
        targets.extend(float_return.iter().map(|(k, v)| (*k, *v)));

        let moving = items.iter().any(|item| {
            let Some(to) = targets.get(&item.id) else { return false };
            let from = item.rect;
            (from.x - to.x).abs() + (from.y - to.y).abs() + (from.width - to.width).abs() + (from.height - to.height).abs() > 2.0
        });
        (items, targets, moving)
    }

    /// Every ordinary window on a display as it is now, back to front, plus
    /// the covers Canopy draws for cramped tiles.
    pub fn on_screen_items(&self, display: &Display, included: &mut HashSet<WindowId>) -> Vec<StageItem> {
        let own = std::process::id() as i32;
        let mut items = Vec::new();
        for o in window_server::on_screen().into_iter().rev() {
            if o.pid == own || self.parked.contains(&o.id) || included.contains(&o.id) || !o.frame.intersects(&display.frame) {
                continue;
            }
            let at = self.tracker.windows.get(&o.id).and_then(|w| w.frame).unwrap_or(o.frame);
            if let Some(item) = self.stage_item(o.id, at, Some(o.pid)) {
                items.push(item);
                included.insert(o.id);
            }
        }
        if let Some(world) = self.active_key_for(display).map(|k| &self.worlds[&k]) {
            for (tile, r) in world.plan.iter().flat_map(|p| p.covers.iter()) {
                let sel = world.arrangement.node(*tile).and_then(|t| world.arrangement.selected_window(t));
                let Some(sel) = sel.filter(|s| !included.contains(s)) else { continue };
                if let Some(item) = self.stage_item(sel, *r, None) {
                    items.insert(0, item);
                    included.insert(sel);
                }
            }
        }
        items
    }

    pub fn stage_item(&self, id: WindowId, rect: Rect, pid: Option<i32>) -> Option<StageItem> {
        let tw = self.tracker.windows.get(&id);
        let pid = tw.map(TrackedWindow::pid).or(pid)?;
        let bundle = tw
            .map(|w| w.bundle_id.clone())
            .or_else(|| workspace::app(pid).and_then(|a| a.bundle_id))
            .unwrap_or_else(|| format!("pid.{pid}"));
        Some(StageItem {
            id,
            rect,
            image: self.pictures.images.get(&id).cloned(),
            colour: app_look::placeholder_colour(&bundle, pid),
            icon: app_look::icon(&bundle, pid),
            edge: self.pictures.edges.get(&id).copied(),
        })
    }

    /// What a display's stage stands on: the captured desktop, or else the
    /// wallpaper file.
    pub fn ground(&self, display: &Display) -> Ground {
        let backdrop = self.pictures.backdrops.get(&display.id).cloned();
        let backdrop_is_file = backdrop.is_none();
        Ground {
            backdrop: backdrop.or_else(|| app_look::wallpaper(display)),
            backdrop_is_file,
            glass: self.pictures.glass.get(&display.id).cloned(),
        }
    }

    fn picture_arrived(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let (Some(picture), edge) = (self.pictures.images.get(&id).cloned(), self.pictures.edges.get(&id).copied()) else { return };
        for key in self.active_keys() {
            let display = self.worlds[&key].display.clone();
            if let Some(p) = self.stage.panel(&display, cx) {
                if p.is_shown() && p.contains(id, cx) {
                    p.set_image(id, picture.clone(), edge, cx);
                }
            }
        }
    }

    fn run_apply(&mut self, job: Apply, cx: &mut Context<Self>) {
        let key = job.world;
        if !self.worlds.contains_key(&key) {
            return;
        }
        // Selected tabs that may have a hidden tab in front of them.
        let mut raise = Vec::new();
        let frame_of = |c: &Self, id: &WindowId| c.tracker.windows.get(id).and_then(|w| w.frame);
        for (id, r) in &job.plan.visible {
            let in_place = frame_of(self, id).is_some_and(|f| near(&f, r));
            if self.stacked.contains(id) {
                raise.push(*id);
                if in_place {
                    // A hidden tab is already there: nothing moves, so nothing redraws.
                    self.unpark(*id);
                    self.applied.insert(*id, *r);
                    continue;
                }
            }
            if !self.parked.contains(id) && self.applied.get(id) == Some(r) && in_place {
                continue;
            }
            if self.write(*id, *r) {
                self.applied.insert(*id, *r);
                self.unpark(*id);
            }
        }
        for (id, tab) in &job.plan.stacked {
            if !frame_of(self, id).is_some_and(|f| near(&f, &tab.rect)) && self.write(*id, tab.rect) {
                // Moving doesn't change the order: it could be in front.
                raise.push(tab.behind);
            }
            if self.tracker.windows.contains_key(id) {
                self.parked.insert(*id);
                self.stacked.insert(*id);
                self.applied.insert(*id, tab.rect);
            }
        }
        for id in &job.plan.parked {
            if !job.plan.stacked.contains_key(id) {
                self.park(*id, &key);
            }
        }
        raise.sort_unstable();
        raise.dedup();
        for id in raise {
            self.raise(id);
        }
        for (id, r) in job.float_moves {
            self.write(id, r);
        }
        for id in job.float_park {
            if let Some(f) = self.tracker.windows.get(&id).and_then(|w| w.frame) {
                self.parked_floating.insert(id, (key.clone(), f));
            }
            self.park(id, &key);
        }
        for (id, r) in job.float_return {
            self.parked_floating.remove(&id);
            self.unpark(id);
            self.write(id, r);
        }
        self.update_overlays(&key, &job.plan, cx);
    }
}

fn near(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() < 1.0 && (a.y - b.y).abs() < 1.0 && (a.width - b.width).abs() < 1.0 && (a.height - b.height).abs() < 1.0
}
