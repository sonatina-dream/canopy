//! Covers, tab strips, gap handles and the glass, kept in step with each plan.

use std::collections::{HashMap, HashSet};

use canopy_core::{Axis, NodeId, Plan, Rect, Seam, SeamLimits, Size, WindowId};
use gpui::{App, Context};

use super::{Coordinator, LayoutOptions};
use crate::mac::tracker::TrackedWindow;
use crate::mac::{app_look, ax, ax_service, workspace};
use crate::ui::desk::DeskPanel;
use crate::ui::overlays::{CoverContent, CoverPanel, GapHandlePanel, Overlay, TabInfo, TabStripPanel};

fn overlay_key(world: &str, id: NodeId) -> String {
    format!("{world}:{id}")
}

fn remove_except<V: gpui::Render + 'static>(dict: &mut HashMap<String, Overlay<V>>, world: &str, keep: &HashSet<String>, cx: &mut App) {
    let prefix = format!("{world}:");
    let stale: Vec<String> = dict.keys().filter(|k| k.starts_with(&prefix) && !keep.contains(*k)).cloned().collect();
    for k in stale {
        if let Some(overlay) = dict.remove(&k) {
            overlay.close(cx);
        }
    }
}

impl Coordinator {
    pub(super) fn remove_overlays_of(&mut self, world: &str, cx: &mut App) {
        let keep = HashSet::new();
        remove_except(&mut self.covers, world, &keep, cx);
        remove_except(&mut self.strips, world, &keep, cx);
        remove_except(&mut self.handles, world, &keep, cx);
    }

    /// Every overlay's frame as asked and as drawn, for the test harness.
    pub fn overlay_report(&self, cx: &mut App) -> serde_json::Value {
        fn report<V: gpui::Render + 'static>(dict: &HashMap<String, Overlay<V>>, kind: &str, cx: &mut App, out: &mut Vec<serde_json::Value>) {
            for overlay in dict.values() {
                let [window, content, view] = overlay.panel.sizes();
                let viewport = overlay.view.update(cx, |_, w, _| w.viewport_size()).ok();
                let viewport = viewport.map(|v| (f32::from(v.width), f32::from(v.height)));
                out.push(serde_json::json!({ "kind": kind, "window": window, "content": content, "view": view, "viewport": viewport }));
            }
        }
        let mut out = Vec::new();
        report(&self.covers, "cover", cx, &mut out);
        report(&self.strips, "strip", cx, &mut out);
        report(&self.handles, "handle", cx, &mut out);
        serde_json::Value::Array(out)
    }

    pub(super) fn update_desk(&mut self, key: &str, plan: &Plan, cx: &mut Context<Self>) {
        let Some(world) = self.worlds.get(key) else { return };
        let display = world.display.clone();
        let harness_without_glass = self.only_bundle_id.is_some() && !self.show_glass_in_harness;
        let any_docked = !world.arrangement.is_empty() && !harness_without_glass;
        if !self.desks.contains_key(key) {
            match DeskPanel::new(&display, cx) {
                Ok(desk) => {
                    self.desks.insert(key.to_string(), desk);
                }
                Err(_) => return,
            }
        }
        let glass = self.pictures.glass.get(&display.id).cloned();
        let backdrop = self.pictures.backdrops.get(&display.id).cloned();
        self.desks[key].update(&display, glass, backdrop, plan.desktop_rect, any_docked, cx);
    }

    fn cover_content(&self, tw: &TrackedWindow, sel: WindowId) -> CoverContent {
        CoverContent {
            image: self.pictures.images.get(&sel).cloned(),
            colour: app_look::placeholder_colour(&tw.bundle_id, tw.pid()),
            icon: app_look::icon(&tw.bundle_id, tw.pid()),
            name: tw.app_name.clone(),
            tint: self.prefs.cover_tint,
        }
    }

    fn tabs(&self, key: &str, tile: NodeId) -> Vec<TabInfo> {
        let a = &self.worlds[key].arrangement;
        let selected = a.selected_window(tile);
        a.get(tile)
            .windows
            .iter()
            .filter_map(|&id| {
                let tw = self.tracker.windows.get(&id)?;
                Some(TabInfo {
                    window: id,
                    title: if tw.title.is_empty() { tw.app_name.clone() } else { tw.title.clone() },
                    icon: app_look::icon(&tw.bundle_id, tw.pid()),
                    selected: Some(id) == selected,
                })
            })
            .collect()
    }

    pub(super) fn update_overlays(&mut self, key: &str, plan: &Plan, cx: &mut Context<Self>) {
        self.update_desk(key, plan, cx);
        let weak = cx.weak_entity();

        // Covers for cramped tiles.
        let mut keep_covers = HashSet::new();
        for (&tile, &rect) in &plan.covers {
            let Some(world) = self.worlds.get(key) else { return };
            let Some(tile_id) = world.arrangement.node(tile) else { continue };
            let Some(sel) = world.arrangement.selected_window(tile_id) else { continue };
            let Some(tw) = self.tracker.windows.get(&sel) else { continue };
            let k = overlay_key(key, tile);
            let content = self.cover_content(tw, sel);
            keep_covers.insert(k.clone());
            if let Some(panel) = self.covers.get(&k) {
                panel.set_content(sel, content, cx);
            } else if let Ok(panel) = CoverPanel::create(key.to_string(), tile, sel, content, weak.clone(), cx) {
                self.covers.insert(k.clone(), panel);
            }
            if let Some(panel) = self.covers.get(&k) {
                panel.panel.place(rect);
                panel.panel.order_front();
            }
        }
        remove_except(&mut self.covers, key, &keep_covers, cx);

        // Tab strips for visible groups.
        let mut keep_strips = HashSet::new();
        for (&tile, &rect) in &plan.layout.strip_rects {
            if plan.covers.contains_key(&tile) {
                continue;
            }
            let Some(world) = self.worlds.get(key) else { return };
            let Some(tile_id) = world.arrangement.node(tile) else { continue };
            let Some(sel) = world.arrangement.selected_window(tile_id) else { continue };
            let tabs = self.tabs(key, tile_id);
            let k = overlay_key(key, tile);
            keep_strips.insert(k.clone());
            if !self.strips.contains_key(&k) {
                if let Ok(panel) = TabStripPanel::create(key.to_string(), weak.clone(), cx) {
                    self.strips.insert(k.clone(), panel);
                }
            }
            if let Some(panel) = self.strips.get(&k) {
                panel.set_tabs(tabs, self.prefs.tab_close_buttons, cx);
                panel.panel.place(rect);
                panel.panel.order_above(sel as isize);
                if !panel.panel.is_visible() {
                    panel.panel.order_front();
                }
            }
        }
        remove_except(&mut self.strips, key, &keep_strips, cx);

        // Gap handles, wider than the gap so they're easy to catch.
        let mut keep_handles = HashSet::new();
        if self.prefs.gap >= 4.0 {
            for seam in &plan.layout.seams {
                let k = format!("{key}:{}:{}", seam.split, seam.index);
                keep_handles.insert(k.clone());
                if let Some(panel) = self.handles.get(&k) {
                    panel.set_seam(*seam, cx);
                } else if let Ok(panel) = GapHandlePanel::create(key.to_string(), *seam, weak.clone(), cx) {
                    self.handles.insert(k.clone(), panel);
                }
                let grow = 5.0;
                let s = seam.rect;
                let r = match seam.axis {
                    Axis::Horizontal => Rect::new(s.x - grow, s.y, s.width + 2.0 * grow, s.height),
                    Axis::Vertical => Rect::new(s.x, s.y - grow, s.width, s.height + 2.0 * grow),
                };
                if let Some(panel) = self.handles.get(&k) {
                    panel.panel.place(r);
                    panel.panel.order_front();
                }
            }
        }
        remove_except(&mut self.handles, key, &keep_handles, cx);
    }

    pub fn cover_opened(&mut self, key: &str, tile: NodeId, window: WindowId, cx: &mut Context<Self>) {
        let Some(world) = self.worlds.get_mut(key) else { return };
        if let Some(t) = world.arrangement.node(tile) {
            if world.navigator.frame(t, &world.arrangement) {
                self.layout(key, true, LayoutOptions::default(), cx);
            }
            self.focus(window);
        }
    }

    pub fn tab_selected(&mut self, id: WindowId, key: &str, cx: &mut Context<Self>) {
        let Some(world) = self.worlds.get_mut(key) else { return };
        world.arrangement.select(id);
        self.layout(key, false, LayoutOptions::default(), cx);
        self.focus(id);
    }

    pub fn focus(&self, id: WindowId) {
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        workspace::activate(tw.pid());
        let w = tw.ax.clone();
        ax_service::run(tw.pid(), move || {
            ax::set_bool(&w.element, ax::attr::MAIN, true);
            w.raise();
        });
    }

    /// Brings a window in front of the others in its group, without activating its app.
    pub(super) fn raise(&self, id: WindowId) {
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let w = tw.ax.clone();
        ax_service::run(tw.pid(), move || w.raise());
    }

    pub fn close(&self, id: WindowId) {
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let w = tw.ax.clone();
        ax_service::run(tw.pid(), move || w.close());
    }

    pub fn minimize(&self, id: WindowId) {
        let Some(tw) = self.tracker.windows.get(&id) else { return };
        let w = tw.ax.clone();
        ax_service::run(tw.pid(), move || w.minimize());
    }

    // MARK: Gap drags

    /// Gaps are dragged on the stage: the pictures resize at once under the
    /// pointer, and the real windows are written once, when the gap lets go.
    pub fn begin_seam_drag(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.prefs.animate {
            return;
        }
        self.refresh_frames();
        let Some(world) = self.worlds.get(key) else { return };
        let display = world.display.clone();
        let desk = world.plan.as_ref().and_then(|p| p.desktop_rect);
        let mut included = HashSet::new();
        let items = self.on_screen_items(&display, &mut included);
        let ground = self.ground(&display);
        if let Some(p) = self.stage.panel(&display, cx) {
            p.show(items, desk, ground, cx);
        }
        self.seam_world = Some(key.to_string());
        self.publish_input();
    }

    pub fn drag_seam(&mut self, key: &str, seam: Seam, delta: f64, cx: &mut Context<Self>) {
        let Some(world) = self.worlds.get(key) else { return };
        let Some(split) = world.arrangement.node(seam.split) else { return };
        let current = world
            .plan
            .as_ref()
            .and_then(|p| p.layout.seams.iter().find(|s| s.split == seam.split && s.index == seam.index))
            .copied()
            .unwrap_or(seam);
        // Stop at the window's own minimum, or its app's when it hasn't refused anything yet.
        let minimum = |id: WindowId| -> Option<Size> {
            let tw = self.tracker.windows.get(&id)?;
            tw.minimum_size.or_else(|| self.parking_floor(tw))
        };
        let a = &world.arrangement;
        let node = a.get(split);
        let (Some(&before_node), Some(&after_node)) = (node.children.get(current.index), node.children.get(current.next)) else { return };
        let before = a.minimum_length(before_node, node.axis, self.prefs.gap, 60.0, &minimum);
        let after = a.minimum_length(after_node, node.axis, self.prefs.gap, 60.0, &minimum);
        let limits = SeamLimits { min_before: before, min_after: after, ..SeamLimits::default() };
        let world = self.worlds.get_mut(key).expect("the world was just read");
        world.arrangement.resize_seam(split, current.index, current.next, delta, current.available, limits);
        if self.seam_world.as_deref() != Some(key) {
            self.layout(key, false, LayoutOptions::default(), cx);
            return;
        }
        let Some(plan) = self.plan_for(key) else { return };
        let world = self.worlds.get_mut(key).expect("the world was just read");
        let mut targets = plan.visible.clone();
        for (tile, r) in &plan.covers {
            if let Some(sel) = world.arrangement.node(*tile).and_then(|t| world.arrangement.selected_window(t)) {
                targets.insert(sel, *r);
            }
        }
        world.plan = Some(plan);
        let display = world.display.clone();
        if let Some(p) = self.stage.panel(&display, cx) {
            p.place(&targets, cx);
        }
    }

    pub fn end_seam_drag(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.seam_world.as_deref() == Some(key) {
            self.seam_world = None;
            self.layout(key, true, LayoutOptions { quick: true, ..LayoutOptions::default() }, cx);
        }
        self.schedule_save(cx);
    }
}
