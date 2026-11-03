//! Docking and floating, commands, and what arrives on the bus.

use std::time::{Duration, Instant};

use canopy_core::{Command, NodeId, Planner, Rect, RestoreTarget, Size, Solver, WindowId};
use gpui::{AppContext, Bounds, Context, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use super::{Coordinator, LayoutOptions};
use crate::bus::Msg;
use crate::clog;
use crate::mac::displays::mouse;
use crate::mac::tracker::{TrackedWindow, TrackerEvent};
use crate::mac::{app_look, ax, ax_service, conflicts, window_server, workspace};
use crate::store;
use crate::ui::settings::SettingsView;
use crate::ui::welcome::WelcomeView;

impl Coordinator {
    pub fn on_msg(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Tracker(m) => {
                let events = self.tracker.handle(m);
                self.handle_tracker_events(events, cx);
            }
            Msg::Written(result) => self.written(result, cx),
            Msg::Input(m) => self.on_input(m, cx),
            Msg::Run(c) => self.run(c, cx),
            Msg::Urls(urls) => {
                for url in urls {
                    match Command::from_url(&url) {
                        Some(c) => self.run(c, cx),
                        None => clog!("URL ignored: {url}"),
                    }
                }
            }
            Msg::Shots(shots) => {
                let scale = self.picture_scale();
                for id in self.pictures.windows_arrived(shots, scale) {
                    self.picture_arrived(id, cx);
                }
            }
            Msg::Desks(shots) => {
                for id in self.pictures.desks_arrived(shots) {
                    let key = self.active_keys().into_iter().find(|k| self.worlds[k].display.id == id);
                    if let Some(key) = key {
                        if let Some(plan) = self.worlds[&key].plan.clone() {
                            self.update_desk(&key, &plan, cx);
                        }
                    }
                }
            }
            Msg::Wallpaper(key, image) => app_look::wallpaper_arrived(key, image),
            Msg::AppLook { bundle_id, icon, colour } => app_look::look_arrived(bundle_id, icon, colour),
            Msg::SpacesChecked(answers) => self.spaces_checked(answers, cx),
            Msg::ScreensChanged => {
                self.rebuild_displays(cx);
                self.after(Duration::from_millis(600), |c, _| c.refresh_desks(), cx);
            }
            Msg::SpaceChanged => self.spaces_changed(cx),
            Msg::Quit => {
                self.prepare_for_quit(cx);
                cx.quit();
            }
            Msg::ForgetView(n) => {
                if let Some(key) = self.current_world_key() {
                    self.worlds.get_mut(&key).expect("current worlds exist").navigator.forget(n);
                    self.schedule_save(cx);
                    self.state_changed();
                }
            }
            Msg::SetIgnored(bundle, ignored) => self.set_ignored(&bundle, ignored, cx),
            Msg::IgnoreApp(bundle) => self.set_ignored(&bundle, true, cx),
            Msg::AskScreenRecording => self.ask_screen_recording(cx),
            Msg::CloseWindow(id) => self.close(id),
            Msg::FloatWindow(id) => self.float(id, true, cx),
            Msg::MinimizeWindow(id) => self.minimize(id),
            Msg::RecordedKey { key_code, modifiers } => {
                let view = self.settings_view.borrow().clone();
                if let Some(view) = view {
                    let _ = view.update(cx, |v, cx| v.key_recorded(key_code, modifiers, cx));
                }
            }
            Msg::Harness(line) => crate::harness::handle(self, &line, cx),
            Msg::HarnessClosed => {
                self.prepare_for_quit(cx);
                cx.quit();
            }
        }
    }

    pub(super) fn handle_tracker_events(&mut self, events: Vec<TrackerEvent>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                TrackerEvent::Appeared(id) => self.window_appeared(id, cx),
                TrackerEvent::Disappeared(id) => self.window_disappeared(id, cx),
                TrackerEvent::Minimized(id, on) | TrackerEvent::FullScreen(id, on) => {
                    self.publish_input();
                    if self.started {
                        if on {
                            self.go_away(id, cx);
                        } else {
                            self.come_back(id, cx);
                        }
                    }
                }
                TrackerEvent::AppHidden(pid, hidden) => {
                    self.publish_input();
                    if self.started {
                        let ids: Vec<WindowId> =
                            self.tracker.windows.values().filter(|w| w.pid() == pid).map(TrackedWindow::id).collect();
                        for id in ids {
                            if hidden {
                                self.go_away(id, cx);
                            } else {
                                self.come_back(id, cx);
                            }
                        }
                    }
                }
                TrackerEvent::MovedOrResized(id) => self.window_moved_or_resized(id, cx),
                TrackerEvent::TitleChanged(id) => self.window_title_changed(id, cx),
                TrackerEvent::FocusChanged(id) => self.focus_changed(id, cx),
                TrackerEvent::GreenButtonFound => self.publish_input(),
            }
        }
    }

    // MARK: - Docking and floating

    /// Docks a floating window or floats a docked one (the green button).
    pub fn toggle_docked(&mut self, id: WindowId, cx: &mut Context<Self>) {
        if self.world_of(id).is_some() {
            self.float(id, true, cx);
            return;
        }
        self.refresh_frames();
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        if !self.manageable(tw) {
            return;
        }
        let Some(key) = self.display_world_key(tw).or_else(|| self.current_world_key()) else { return };
        if let Some(f) = tw.frame {
            self.float_frames.insert(id, f);
        }
        let options = || LayoutOptions { confine_floating: true, ..LayoutOptions::default() };
        if let Some((back, target)) = self.float_restore.remove(&id) {
            if let Some(w) = self.worlds.get_mut(&back) {
                if w.arrangement.restore(id, target).is_some() {
                    self.reveal(id, &back);
                    self.layout(&back, true, options(), cx);
                    return;
                }
            }
        }
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let rects = world.plan.as_ref().map(|p| p.layout.node_rects.clone()).unwrap_or_default();
        world.arrangement.auto_dock(id, &rects);
        self.reveal(id, &key);
        self.layout(&key, true, options(), cx);
    }

    /// Takes a window out of the layout and floats it on its Desktop.
    pub fn float(&mut self, id: WindowId, animated: bool, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_of(id) else { return };
        let Some(frame) = self.tracker.windows.get(&id).map(|t| t.frame) else { return };
        let share = self.prefs.share_removed_space;
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let target = world.arrangement.remove(id, share);
        let bounds = world.plan.as_ref().and_then(|p| p.desktop_rect).unwrap_or(world.display.visible);
        let live = world.arrangement.windows().into_iter().collect();
        world.navigator.prune(&live);
        self.float_restore.insert(id, (key.clone(), target.unwrap_or(RestoreTarget::Floating)));
        let current = self.applied.get(&id).copied().or(frame).unwrap_or(bounds);
        let mut r = self.float_frames.get(&id).copied().unwrap_or(Rect::new(
            bounds.mid_x() - current.width * 0.4,
            bounds.mid_y() - current.height * 0.4,
            current.width * 0.8,
            current.height * 0.8,
        ));
        r.width = r.width.min(bounds.width);
        r.height = r.height.min(bounds.height);
        let r = r.clamped(&bounds);
        self.unpark(id);
        self.applied.remove(&id);
        let options = LayoutOptions { float_to: [(id, r)].into(), ..LayoutOptions::default() };
        self.layout(&key, animated, options, cx);
    }

    pub(super) fn reveal(&mut self, id: WindowId, key: &str) {
        let Some(world) = self.worlds.get_mut(key) else { return };
        if let Some(t) = world.arrangement.tile_of(id) {
            world.navigator.reveal(t, &world.arrangement);
        }
    }

    /// Docks every floating window of a display (Arrange Windows).
    fn arrange(&mut self, key: &str, cx: &mut Context<Self>) {
        self.refresh_frames();
        let mut floating: Vec<(WindowId, Option<Rect>)> = self
            .floating_windows(key)
            .into_iter()
            .filter(|id| !self.parked_floating.contains_key(id))
            .map(|id| (id, self.tracker.windows.get(&id).and_then(|w| w.frame)))
            .collect();
        if floating.is_empty() {
            return;
        }
        floating.sort_by(|a, b| a.1.map_or(0.0, |r| r.min_x()).total_cmp(&b.1.map_or(0.0, |r| r.min_x())));
        let gap = self.prefs.gap;
        let world = self.worlds.get_mut(key).expect("the key is a live world");
        for (id, frame) in floating {
            if let Some(f) = frame {
                self.float_frames.insert(id, f);
            }
            let bounds = Solver::bounds(world.display.visible, gap);
            let rects = Solver::solve(&world.arrangement, world.arrangement.root(), bounds, gap).node_rects;
            world.arrangement.auto_dock(id, &rects);
        }
        world.navigator.everything(&world.arrangement);
        self.layout(key, true, LayoutOptions::default(), cx);
    }

    /// Floats every window on its Desktop: back to a normal Mac.
    fn float_all(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.worlds.keys().cloned().collect();
        for key in keys {
            let world = self.worlds.get_mut(&key).expect("the key came from the map");
            let v = world.display.visible;
            let mut moves = std::collections::HashMap::new();
            let mut offset = 0.0;
            for id in world.arrangement.windows() {
                world.arrangement.remove(id, true);
                self.parked.remove(&id);
                self.stacked.remove(&id);
                let size = self
                    .float_frames
                    .get(&id)
                    .map(Rect::size)
                    .or(self.applied.get(&id).map(Rect::size))
                    .unwrap_or(Size::new(900.0, 650.0));
                self.applied.remove(&id);
                let r = Rect::new(
                    v.x + 40.0 + offset,
                    v.y + 40.0 + offset,
                    size.width.max(400.0).min(v.width * 0.8),
                    size.height.max(300.0).min(v.height * 0.8),
                );
                moves.insert(id, r.clamped(&v));
                offset = if offset >= 240.0 { 0.0 } else { offset + 30.0 };
            }
            world.arrangement.set_desktop_hidden(false);
            world.navigator.everything(&world.arrangement);
            self.layout(&key, true, LayoutOptions { float_to: moves, ..LayoutOptions::default() }, cx);
        }
    }

    pub fn set_ignored(&mut self, bundle_id: &str, ignored: bool, cx: &mut Context<Self>) {
        let mut p = self.prefs.clone();
        p.ignored_bundle_ids.retain(|b| b != bundle_id);
        if ignored {
            p.ignored_bundle_ids.push(bundle_id.to_string());
        }
        self.set_prefs(p, cx);
    }

    // MARK: - Commands

    pub fn run(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Command::Settings(pane) = command {
            self.open_settings(pane, cx);
            return;
        }
        if !self.started {
            return;
        }
        if command == Command::FloatAll {
            self.float_all(cx);
            return;
        }
        let Some(key) = self.current_world_key() else { return };
        let focused = self.tracker.focused_window();
        let (gap, prefer_pointer) = (self.prefs.gap, self.prefs.step_toward_pointer);
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let (a, nav) = (&mut world.arrangement, &mut world.navigator);
        let mut moved = false;
        match command {
            Command::Everything => moved = nav.everything(a),
            Command::StepOut => moved = nav.step_out(a),
            Command::StepIn => {
                let rects = world.plan.as_ref().map(|p| p.layout.node_rects.clone()).unwrap_or_default();
                moved = nav.step_in(a, focused, Some(mouse()), &rects, prefer_pointer);
            }
            Command::Back => moved = nav.back(a),
            Command::Forward => moved = nav.forward(a),
            Command::Neighbour(direction) => {
                let bounds = Solver::bounds(world.display.visible, gap);
                let rects = Solver::solve(a, a.root(), bounds, gap).node_rects;
                moved = nav.neighbour(direction, a, &rects);
            }
            Command::FrameWindow => {
                if let Some(t) = focused.and_then(|f| a.tile_of(f)) {
                    moved = nav.toggle_frame(t, a);
                }
            }
            Command::FrameDesktop => {
                if a.is_desktop_hidden() {
                    a.set_desktop_hidden(false);
                }
                nav.toggle_frame(a.desktop(), a);
                moved = true;
            }
            Command::ToggleDesktop => {
                let hide = !a.is_desktop_hidden();
                if hide && a.get(nav.framed_node(a)).is_desktop() {
                    nav.everything(a);
                }
                a.set_desktop_hidden(hide);
                moved = true;
            }
            Command::GoToView(n) => moved = nav.go_to(n, a),
            Command::SaveView(n) => {
                nav.save(n, a);
                self.schedule_save(cx);
                self.state_changed();
            }
            Command::SplitFinder => self.split_finder(&key),
            Command::Arrange => self.arrange(&key, cx),
            Command::FloatAll | Command::Settings(_) => {}
        }
        if moved {
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
    }

    fn split_finder(&mut self, key: &str) {
        let tile: Option<NodeId> = self.tracker.focused_window().and_then(|f| self.worlds[key].arrangement.tile_of(f));
        self.pending_split = Some((tile, key.to_string(), Instant::now() + Duration::from_secs(6)));
        std::thread::spawn(|| {
            let opened = workspace::run_apple_script(
                "tell application \"Finder\"\n    make new Finder window to home\n    activate\nend tell",
            );
            if !opened {
                workspace::on_main(|| {
                    let home = std::env::var("HOME").unwrap_or_default();
                    workspace::open_url(&format!("file://{home}/"));
                });
            }
        });
    }

    fn open_settings(&mut self, pane: Option<canopy_core::SettingsPane>, cx: &mut Context<Self>) {
        self.pending_pane = pane;
        if let Some(handle) = self.settings {
            let pane = self.pending_pane.take();
            let opened = handle.update(cx, |view, window, cx| {
                if let Some(pane) = pane {
                    view.show_pane(pane, cx);
                }
                window.activate_window();
            });
            if opened.is_ok() {
                self.come_forward(cx);
                return;
            }
        }
        let this = cx.entity();
        let slot = self.settings_view.clone();
        // Opening draws the window, and its view reads this coordinator.
        cx.defer(move |cx| {
            let pane = this.update(cx, |c, _| c.pending_pane.take());
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(620.0), px(520.0)), cx))),
                titlebar: Some(TitlebarOptions { title: Some("Canopy Settings".into()), ..Default::default() }),
                is_resizable: false,
                ..Default::default()
            };
            let view = this.clone();
            let opened = cx.open_window(options, |_, cx| {
                let settings = cx.new(|cx| SettingsView::new(view, pane, cx));
                *slot.borrow_mut() = Some(settings.downgrade());
                settings
            });
            if let Ok(handle) = opened {
                this.update(cx, |c, _| c.settings = Some(handle));
            }
            if this.read(cx).only_bundle_id.is_none() {
                cx.activate(true);
            }
        });
    }

    /// Canopy has come forward, unless under test: the harness never takes
    /// the keyboard from whatever is in use.
    fn come_forward(&self, cx: &mut Context<Self>) {
        if self.only_bundle_id.is_none() {
            cx.activate(true);
        }
    }

    /// Opens first-run setup, or just the Screen Recording step.
    pub fn open_welcome(&mut self, screen_only: bool, cx: &mut Context<Self>) {
        let this = cx.entity();
        cx.defer(move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(520.0), px(400.0)), cx))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Welcome to Canopy".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            };
            let view = this.clone();
            let opened = cx.open_window(options, |_, cx| cx.new(|cx| WelcomeView::new(view, screen_only, cx)));
            if let Ok(handle) = opened {
                this.update(cx, |c, _| c.welcome = Some(handle));
            }
            if this.read(cx).only_bundle_id.is_none() {
                cx.activate(true);
            }
        });
    }

    fn welcome_is_open(&self, cx: &gpui::App) -> bool {
        self.welcome.is_some_and(|w| w.read(cx).is_ok())
    }

    fn ask_screen_recording(&mut self, cx: &mut Context<Self>) {
        if self.welcome_is_open(cx) {
            if let Some(w) = self.welcome {
                let _ = w.update(cx, |view, _, cx| view.go_to_screen_recording(cx));
                self.come_forward(cx);
            }
            return;
        }
        self.open_welcome(true, cx);
    }

    /// Waits for Accessibility, then starts; shows setup when it's needed.
    pub fn launch(&mut self, no_prompt: bool, cx: &mut Context<Self>) {
        if crate::ui::welcome::should_show() && !no_prompt {
            self.open_welcome(false, cx);
        }
        if ax::is_trusted() {
            self.start_after_trust(cx);
            return;
        }
        self.trust_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if ax::is_trusted() {
                let _ = this.update(cx, |c, cx| c.start_after_trust(cx));
                break;
            }
        }));
    }

    pub fn start_after_trust(&mut self, cx: &mut Context<Self>) {
        if self.launched {
            return;
        }
        self.start(cx);
        self.state_changed();
        self.after(Duration::from_secs(1), |c, cx| {
            // Starts a modal loop, which can't run from inside GPUI.
            workspace::on_main(conflicts::warn_if_needed);
            // Until Screen Recording is allowed, ask on every launch.
            if !crate::ui::welcome::screen_recording_allowed() && !c.welcome_is_open(cx) {
                c.ask_screen_recording(cx);
            }
        }, cx);
    }

    // MARK: - Tracker events

    fn window_appeared(&mut self, id: WindowId, cx: &mut Context<Self>) {
        self.publish_input();
        if !self.started {
            return;
        }
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let is_finder = tw.bundle_id == "com.apple.finder";
        let frame = tw.frame;
        let pending = self.pending_split.as_ref().filter(|p| Instant::now() < p.2 && is_finder);
        if let Some((tile, key, _)) = pending.cloned() {
            let Some(world) = self.worlds.get_mut(&key) else { return };
            self.pending_split = None;
            if let Some(f) = frame {
                self.float_frames.insert(id, f);
            }
            let rects = world.plan.as_ref().map(|p| p.layout.node_rects.clone()).unwrap_or_default();
            match tile.and_then(|t| world.arrangement.node(t)).zip(tile.and_then(|t| rects.get(&t))) {
                Some((t, r)) => {
                    let edge = if r.width >= r.height { canopy_core::Edge::Right } else { canopy_core::Edge::Bottom };
                    world.arrangement.dock(id, t, edge, None);
                }
                None => {
                    world.arrangement.auto_dock(id, &rects);
                }
            }
            self.reveal(id, &key);
            self.layout(&key, true, LayoutOptions::default(), cx);
        }
        // Otherwise new windows float where they open.
    }

    fn window_disappeared(&mut self, id: WindowId, cx: &mut Context<Self>) {
        self.pictures.forget(id);
        self.unpark(id);
        self.applied.remove(&id);
        self.parked_floating.remove(&id);
        self.away.remove(&id);
        self.float_restore.remove(&id);
        self.float_frames.remove(&id);
        self.last_write.remove(&id);
        self.publish_input();
        if !self.started {
            return;
        }
        let Some(key) = self.world_key_of(id) else { return };
        let share = self.prefs.share_removed_space;
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        world.arrangement.remove(id, share);
        let live = world.arrangement.windows().into_iter().collect();
        world.navigator.prune(&live);
        self.layout(&key, true, LayoutOptions::default(), cx);
    }

    fn go_away(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_of(id) else { return };
        let share = self.prefs.share_removed_space;
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let target = world.arrangement.remove(id, share);
        self.away.insert(id, (key.clone(), target.unwrap_or(RestoreTarget::Floating)));
        self.unpark(id);
        self.applied.remove(&id);
        self.layout(&key, true, LayoutOptions::default(), cx);
    }

    fn come_back(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let Some((key, target)) = self.away.remove(&id) else { return };
        let Some(world) = self.worlds.get_mut(&key) else { return };
        if target == RestoreTarget::Floating {
            return;
        }
        if world.arrangement.restore(id, target).is_none() {
            let rects = world.plan.as_ref().map(|p| p.layout.node_rects.clone()).unwrap_or_default();
            world.arrangement.auto_dock(id, &rects);
        }
        self.reveal(id, &key);
        self.layout(&key, true, LayoutOptions::default(), cx);
    }

    fn window_moved_or_resized(&mut self, id: WindowId, cx: &mut Context<Self>) {
        if !self.started || !self.tracker.windows.contains_key(&id) {
            return;
        }
        // Our own writes echo back as notifications; ignore them.
        let echo = self.last_write.get(&id).is_some_and(|t| t.elapsed() < Duration::from_millis(400));
        if self.writer.is_busy(id) || echo || self.press.as_ref().is_some_and(|p| p.window == id) {
            return;
        }
        let frame = window_server::frame(id);
        let tw = self.tracker.windows.get_mut(&id).expect("checked above");
        if frame.is_some() {
            tw.frame = frame;
        }
        let f = tw.frame;
        if objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0 {
            self.moved_while_mouse_down.insert(id);
            return;
        }
        let (Some(key), Some(want), Some(f)) = (self.world_key_of(id), self.applied.get(&id).copied(), f) else { return };
        if super::near(&f, &want) {
            return;
        }
        // Something else moved a docked window: put it back, a few times at most.
        let now = Instant::now();
        let recent = self.snap_backs.entry(id).or_default();
        recent.retain(|t| now - *t < Duration::from_secs(5));
        if recent.len() >= 3 {
            return;
        }
        recent.push(now);
        self.applied.remove(&id);
        self.schedule_relayout(key, cx);
    }

    fn window_title_changed(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let Some(key) = self.world_key_of(id) else { return };
        let world = &self.worlds[&key];
        let (Some(plan), Some(tile)) = (&world.plan, world.arrangement.tile_of(id)) else { return };
        if plan.layout.strip_rects.contains_key(&tile) {
            let plan = plan.clone();
            self.update_overlays(&key, &plan, cx);
        }
    }

    fn focus_changed(&mut self, id: Option<WindowId>, cx: &mut Context<Self>) {
        let Some(id) = id.filter(|_| self.started) else { return };
        let Some(key) = self.world_key_of(id) else {
            // A floating window parked with its hidden Desktop comes back with it.
            if let Some((key, _)) = self.parked_floating.get(&id).cloned() {
                if let Some(w) = self.worlds.get_mut(&key) {
                    w.arrangement.set_desktop_hidden(false);
                    let desktop = w.arrangement.desktop();
                    w.navigator.reveal(desktop, &w.arrangement);
                    self.layout(&key, true, LayoutOptions::default(), cx);
                }
            }
            return;
        };
        let min = self.minimum(id);
        let gap = self.prefs.gap;
        let world = self.worlds.get_mut(&key).expect("the key was just found");
        let Some(tile) = world.arrangement.tile_of(id) else { return };
        let tab_changed = world.arrangement.selected_window(tile) != Some(id);
        if tab_changed {
            world.arrangement.select(id);
        }
        let visible = world.display.visible;
        let a = &world.arrangement;
        let view_changed = world.navigator.bring_into_view(tile, a, |framing| Planner::fits(a, tile, framing, visible, gap, min));
        if view_changed {
            self.layout(&key, true, LayoutOptions::default(), cx);
        } else if tab_changed {
            // Nothing moves when only the tab changes, so there's no motion to show.
            self.layout(&key, false, LayoutOptions::default(), cx);
        } else if let Some(panel) = self.strips.get(&format!("{key}:{tile}")) {
            panel.panel.order_above(id as isize);
        }
    }

    pub(super) fn process_moved_while_mouse_down(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<WindowId> = self.moved_while_mouse_down.drain().collect();
        let mut touched = std::collections::HashSet::new();
        for id in ids {
            if let Some(key) = self.world_key_of(id) {
                self.applied.remove(&id);
                touched.insert(key);
            }
        }
        for key in touched {
            self.layout(&key, false, LayoutOptions::default(), cx);
        }
    }

    /// Windows the harness and menus ask about.
    pub fn set_window_minimized(&self, id: WindowId, minimized: bool) {
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let w = tw.ax.clone();
        ax_service::run(tw.pid(), move || {
            ax::set_bool(&w.element, ax::attr::MINIMIZED, minimized);
        });
    }

    pub fn first_launch_done() {
        store::set_bool("welcomed", true);
    }
}
