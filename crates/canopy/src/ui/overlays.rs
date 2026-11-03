//! The small panels that sit among real windows: covers for cramped windows,
//! tab strips, gap handles, the drop preview, the marquee and the tab ghost.

use std::sync::Arc;

use canopy_core::{Axis, NodeId, Rect, Seam, Size, Solver, WindowId};
use gpui::{
    canvas, div, fill, img, point, prelude::FluentBuilder, px, quad, relative, size, AnyElement, App, AppContext, BorderStyle, Bounds, Context, Corners,
    CursorStyle, DispatchPhase, Edges, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Render, RenderImage, Styled, StyledImage, WeakEntity, Window,
    WindowBackgroundAppearance, WindowHandle,
};

use super::theme::accent;
use super::CORNER_RADIUS;
use crate::coordinator::Coordinator;
use crate::imaging::Colour;
use crate::mac::capture::Picture;
use crate::mac::displays::mouse;
use crate::mac::menu::show_tab_menu;
use crate::mac::panel::{window_options, Panel, PanelStyle, LEVEL_FLOATING, LEVEL_NORMAL};

/// A panel and the view it shows.
pub struct Overlay<V: 'static> {
    pub panel: Panel,
    pub view: WindowHandle<V>,
}

impl<V: Render + 'static> Overlay<V> {
    fn open(style: PanelStyle, background: WindowBackgroundAppearance, build: impl FnOnce(&mut Context<V>) -> V, cx: &mut App) -> anyhow::Result<Self> {
        let mut panel = None;
        let view = cx.open_window(window_options(background), |window, cx| {
            let handle = window.window_handle();
            panel = Some(Panel::adopt(window, handle, style));
            cx.new(build)
        })?;
        Ok(Self { panel: panel.expect("the window was built"), view })
    }

    pub fn update(&self, cx: &mut App, body: impl FnOnce(&mut V, &mut Context<V>)) {
        let _ = self.view.update(cx, |view, _, cx| body(view, cx));
    }

    pub fn close(self, cx: &mut App) {
        self.panel.order_out();
        self.panel.close(cx);
    }
}

/// Coordinator work from a click. The window that took the click is in use
/// until the handler returns, and the work may need to update that window.
fn with_coordinator(coordinator: &WeakEntity<Coordinator>, cx: &mut App, body: impl FnOnce(&mut Coordinator, &mut Context<Coordinator>) + 'static) {
    let coordinator = coordinator.clone();
    cx.defer(move |cx| {
        let _ = coordinator.update(cx, body);
    });
}

fn mouse_pixels(p: gpui::Point<Pixels>) -> (f32, f32) {
    (f32::from(p.x), f32::from(p.y))
}

// MARK: - Cover

/// What a cover shows.
#[derive(Clone)]
pub struct CoverContent {
    pub image: Option<Arc<Picture>>,
    pub colour: Colour,
    pub icon: Option<Arc<RenderImage>>,
    pub name: String,
    pub tint: f64,
}

/// Stands in for a window too small for its app: its latest picture tinted
/// with the app's colour, and the app icon. Double-click to go in.
pub struct CoverView {
    content: CoverContent,
    previous: Vec<Arc<RenderImage>>,
    world: String,
    tile: NodeId,
    window: WindowId,
    coordinator: WeakEntity<Coordinator>,
}

impl Render for CoverView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        for image in self.previous.drain(..) {
            let _ = window.drop_image(image);
        }
        let c = self.content.clone();
        let (world, tile, target, coordinator) = (self.world.clone(), self.tile, self.window, self.coordinator.clone());
        let shows_name = {
            let v = window.viewport_size();
            f32::from(v.height) > 120.0 && f32::from(v.width) > 90.0
        };
        let label = shows_name.then(|| {
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(relative(0.5))
                .pt(px(46.0))
                .flex()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(gpui::white())
                .child(div().px(px(6.0)).max_w_full().truncate().child(c.name.clone()))
        });
        div()
            .id("cover")
            .size_full()
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                if event.click_count == 2 {
                    let (world, coordinator2) = (world.clone(), coordinator.clone());
                    with_coordinator(&coordinator2, cx, move |c, cx| c.cover_opened(&world, tile, target, cx));
                }
            })
            .child(canvas(|_, _, _| (), move |bounds, _, window, _| paint_cover(&c, bounds, shows_name, window)).size_full())
            .children(label)
    }
}

fn paint_cover(c: &CoverContent, bounds: Bounds<Pixels>, shows_name: bool, window: &mut Window) {
    let radius = Corners::all(px(CORNER_RADIUS));
    let colour = Hsla::from(c.colour.to_rgba(1.0));
    window.paint_quad(fill(bounds, colour).corner_radii(px(CORNER_RADIUS)));
    if let Some(picture) = &c.image {
        // Aspect fill.
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (iw, ih) = (picture.size.width as f32, picture.size.height as f32);
        let s = (w / iw).max(h / ih);
        let placed = Bounds::new(
            point(bounds.origin.x + px((w - iw * s) / 2.0), bounds.origin.y + px((h - ih * s) / 2.0)),
            size(px(iw * s), px(ih * s)),
        );
        let _ = window.paint_image(bounds, placed, radius, picture.image.clone(), 0, false);
        window.paint_quad(fill(bounds, colour.opacity(c.tint as f32)).corner_radii(px(CORNER_RADIUS)));
    }
    if let Some(icon) = &c.icon {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let side = (w.min(h) * 0.45).clamp(16.0, 72.0);
        let lift = if shows_name { 10.0 } else { 0.0 };
        let frame = Bounds::new(
            point(bounds.origin.x + px((w - side) / 2.0), bounds.origin.y + px((h - side) / 2.0 - lift)),
            size(px(side), px(side)),
        );
        let _ = window.paint_image(frame, frame, Corners::default(), icon.clone(), 0, false);
    }
}

pub type CoverPanel = Overlay<CoverView>;

impl CoverPanel {
    pub fn create(world: String, tile: NodeId, target: WindowId, content: CoverContent, coordinator: WeakEntity<Coordinator>, cx: &mut App) -> anyhow::Result<Self> {
        let style = PanelStyle { level: LEVEL_NORMAL, mouse: true, every_space: false };
        Self::open(
            style,
            WindowBackgroundAppearance::Transparent,
            move |_| CoverView { content, previous: Vec::new(), world, tile, window: target, coordinator },
            cx,
        )
    }

    pub fn set_content(&self, target: WindowId, content: CoverContent, cx: &mut App) {
        self.update(cx, |view, cx| {
            let changed = match (&view.content.image, &content.image) {
                (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                (Some(_), None) => true,
                _ => false,
            };
            if changed {
                view.previous.extend(view.content.image.as_ref().map(|p| p.image.clone()));
            }
            view.window = target;
            view.content = content;
            cx.notify();
        });
    }
}

// MARK: - Tab strip

#[derive(Clone)]
pub struct TabInfo {
    pub window: WindowId,
    pub title: String,
    pub icon: Option<Arc<RenderImage>>,
    pub selected: bool,
}

struct Pressed {
    window: WindowId,
    at: (f32, f32),
    on_close: bool,
}

/// The strip of tabs on top of a tab group: glass, like a title bar.
pub struct TabStripView {
    tabs: Vec<TabInfo>,
    shows_close: bool,
    world: String,
    coordinator: WeakEntity<Coordinator>,
    pressed: Option<Pressed>,
    dragging: bool,
    hovered: Option<usize>,
}

impl TabStripView {
    /// Which tab is at this x, with the strip's insets (4 pt around, 2 pt
    /// between neighbours).
    fn index_at(&self, x: f32, width: f32) -> Option<usize> {
        let each = (width - 8.0) / self.tabs.len().max(1) as f32;
        (0..self.tabs.len()).find(|&i| {
            let start = 4.0 + i as f32 * each + 2.0;
            x >= start && x < start + each - 4.0
        })
    }

    fn close_contains(&self, i: usize, x: f32, width: f32) -> bool {
        let each = (width - 8.0) / self.tabs.len().max(1) as f32;
        let start = 4.0 + i as f32 * each + 2.0 + 6.0;
        x >= start && x < start + 14.0
    }

    fn down(&mut self, event: &MouseDownEvent, width: f32, cx: &mut Context<Self>) {
        let (x, y) = mouse_pixels(event.position);
        let Some(i) = self.index_at(x, width) else { return };
        if event.button == MouseButton::Right {
            show_tab_menu(self.tabs[i].window);
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let on_close = self.shows_close && self.hovered == Some(i) && self.close_contains(i, x, width);
        self.pressed = Some(Pressed { window: self.tabs[i].window, at: (x, y), on_close });
        self.dragging = false;
        cx.notify();
    }

    fn moved(&mut self, event: &MouseMoveEvent, width: f32, cx: &mut Context<Self>) {
        let (x, y) = mouse_pixels(event.position);
        let hovered = self.index_at(x, width);
        if hovered != self.hovered {
            self.hovered = hovered;
            cx.notify();
        }
        let Some(pressed) = self.pressed.as_ref().filter(|p| !p.on_close) else { return };
        if !event.dragging() {
            return;
        }
        if !self.dragging && (x - pressed.at.0).hypot(y - pressed.at.1) > 5.0 {
            self.dragging = true;
            let (window, world) = (pressed.window, self.world.clone());
            with_coordinator(&self.coordinator, cx, move |c, cx| c.begin_tab_drag(window, &world, cx));
        }
        if self.dragging {
            with_coordinator(&self.coordinator, cx, |c, cx| c.update_tab_drag(cx));
        }
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        let dragging = std::mem::take(&mut self.dragging);
        let Some(pressed) = self.pressed.take() else { return };
        let window = pressed.window;
        if dragging {
            with_coordinator(&self.coordinator, cx, |c, cx| c.end_tab_drag(cx));
        } else if pressed.on_close {
            with_coordinator(&self.coordinator, cx, move |c, _| c.close(window));
        } else {
            let world = self.world.clone();
            with_coordinator(&self.coordinator, cx, move |c, cx| c.tab_selected(window, &world, cx));
        }
    }

    fn tab(&self, i: usize, tab: &TabInfo) -> AnyElement {
        let hovered = self.hovered == Some(i);
        let chip = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .mx(px(2.0))
            .px(px(6.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(7.0))
            .when(tab.selected, |d| d.bg(gpui::white().opacity(0.16)))
            .when(!tab.selected && hovered, |d| d.bg(gpui::white().opacity(0.07)));
        let leading: AnyElement = if self.shows_close && hovered {
            div()
                .w(px(14.0))
                .flex_none()
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::white().opacity(0.55))
                .child("✕")
                .into_any_element()
        } else if let Some(icon) = &tab.icon {
            img(icon.clone()).size(px(16.0)).flex_none().object_fit(ObjectFit::Contain).into_any_element()
        } else {
            div().w(px(16.0)).flex_none().into_any_element()
        };
        chip.child(leading)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .font_weight(if tab.selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(gpui::white().opacity(if tab.selected { 0.95 } else { 0.6 }))
                    .child(if tab.title.is_empty() { " ".to_string() } else { tab.title.clone() }),
            )
            .into_any_element()
    }
}

impl Render for TabStripView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let width = f32::from(window.viewport_size().width);
        let chips: Vec<AnyElement> = self.tabs.iter().enumerate().map(|(i, t)| self.tab(i, t)).collect();
        div()
            .size_full()
            .p(px(4.0))
            .flex()
            .bg(gpui::black().opacity(0.28))
            .children(chips)
            .child(
                canvas(|_, _, _| (), move |_, _, window, _| {
                    // The press belongs to this strip wherever the pointer goes.
                    let (down, moved, up, exit) = (this.clone(), this.clone(), this.clone(), this.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let _ = down.update(cx, |v, cx| v.down(e, width, cx));
                        }
                    });
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let _ = moved.update(cx, |v, cx| v.moved(e, width, cx));
                        }
                    });
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                            let _ = up.update(cx, |v, cx| v.up(cx));
                        }
                    });
                    window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let _ = exit.update(cx, |v, cx| {
                                v.hovered = None;
                                cx.notify();
                            });
                        }
                    });
                })
                .absolute()
                .size_full(),
            )
    }
}

pub type TabStripPanel = Overlay<TabStripView>;

impl TabStripPanel {
    pub fn create(world: String, coordinator: WeakEntity<Coordinator>, cx: &mut App) -> anyhow::Result<Self> {
        let style = PanelStyle { level: LEVEL_NORMAL, mouse: true, every_space: false };
        let this = Self::open(
            style,
            WindowBackgroundAppearance::Blurred,
            move |_| TabStripView {
                tabs: Vec::new(),
                shows_close: true,
                world,
                coordinator,
                pressed: None,
                dragging: false,
                hovered: None,
            },
            cx,
        )?;
        this.panel.round_corners(f64::from(CORNER_RADIUS));
        Ok(this)
    }

    pub fn set_tabs(&self, tabs: Vec<TabInfo>, shows_close: bool, cx: &mut App) {
        self.update(cx, |view, cx| {
            view.tabs = tabs;
            view.shows_close = shows_close;
            cx.notify();
        });
    }
}

// MARK: - Gap handle

/// Lives in the gap between two neighbours; drag it to resize both.
pub struct GapHandleView {
    seam: Seam,
    world: String,
    coordinator: WeakEntity<Coordinator>,
    last: Option<canopy_core::Point>,
    hovering: bool,
}

impl GapHandleView {
    fn down(&mut self, cx: &mut Context<Self>) {
        self.last = Some(mouse());
        let world = self.world.clone();
        with_coordinator(&self.coordinator, cx, move |c, cx| c.begin_seam_drag(&world, cx));
        cx.notify();
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.last else {
            if !self.hovering {
                self.hovering = true;
                cx.notify();
            }
            return;
        };
        let p = mouse();
        let delta = if self.seam.axis == Axis::Horizontal { p.x - last.x } else { p.y - last.y };
        if delta != 0.0 {
            let (world, seam) = (self.world.clone(), self.seam);
            with_coordinator(&self.coordinator, cx, move |c, cx| c.drag_seam(&world, seam, delta, cx));
        }
        self.last = Some(p);
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        if self.last.take().is_some() {
            let world = self.world.clone();
            with_coordinator(&self.coordinator, cx, move |c, cx| c.end_seam_drag(&world, cx));
            cx.notify();
        }
    }
}

impl Render for GapHandleView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let axis = self.seam.axis;
        let pill = (self.hovering || self.last.is_some()).then(|| {
            let (w, h) = if axis == Axis::Horizontal { (px(4.0), px(48.0)) } else { (px(48.0), px(4.0)) };
            div().w(w).h(h).rounded(px(2.0)).bg(gpui::white().opacity(0.45))
        });
        let cursor = if axis == Axis::Horizontal { CursorStyle::ResizeLeftRight } else { CursorStyle::ResizeUpDown };
        div()
            .id("gap")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor(cursor)
            .children(pill)
            .child(
                canvas(|_, _, _| (), move |_, _, window, _| {
                    let (down, moved, up, exit) = (this.clone(), this.clone(), this.clone(), this.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                            let _ = down.update(cx, |v, cx| v.down(cx));
                        }
                    });
                    window.on_mouse_event(move |_: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let _ = moved.update(cx, |v, cx| v.moved(cx));
                        }
                    });
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                            let _ = up.update(cx, |v, cx| v.up(cx));
                        }
                    });
                    window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let _ = exit.update(cx, |v, cx| {
                                v.hovering = false;
                                cx.notify();
                            });
                        }
                    });
                })
                .absolute()
                .size_full(),
            )
    }
}

pub type GapHandlePanel = Overlay<GapHandleView>;

impl GapHandlePanel {
    pub fn create(world: String, seam: Seam, coordinator: WeakEntity<Coordinator>, cx: &mut App) -> anyhow::Result<Self> {
        let style = PanelStyle { level: LEVEL_NORMAL, mouse: true, every_space: false };
        Self::open(
            style,
            WindowBackgroundAppearance::Transparent,
            move |_| GapHandleView { seam, world, coordinator, last: None, hovering: false },
            cx,
        )
    }

    pub fn set_seam(&self, seam: Seam, cx: &mut App) {
        self.update(cx, |view, _| view.seam = seam);
    }
}

// MARK: - Drop preview

pub struct HighlightView {
    tab: bool,
}

impl Render for HighlightView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab;
        let accent = accent();
        canvas(|_, _, _| (), move |bounds, _, window, _| {
            let r = bounds.dilate(px(-2.0));
            window.paint_quad(quad(
                r,
                Corners::all(px(CORNER_RADIUS)),
                accent.opacity(0.18),
                Edges::all(px(2.0)),
                accent.opacity(0.8),
                BorderStyle::Solid,
            ));
            if tab {
                let strip = Bounds::new(r.origin, size(r.size.width, px(Solver::TAB_STRIP_HEIGHT as f32)));
                window.paint_quad(fill(strip, accent.opacity(0.35)).corner_radii(px(8.0)));
            }
        })
        .size_full()
    }
}

/// Shows where a dragged window will land.
pub type DropPreviewPanel = Overlay<HighlightView>;

impl DropPreviewPanel {
    pub fn create(cx: &mut App) -> anyhow::Result<Self> {
        let style = PanelStyle { level: LEVEL_FLOATING, mouse: false, every_space: true };
        Self::open(style, WindowBackgroundAppearance::Transparent, |_| HighlightView { tab: false }, cx)
    }

    pub fn show(&self, r: Rect, tab: bool, cx: &mut App) {
        self.update(cx, |view, cx| {
            view.tab = tab;
            cx.notify();
        });
        if self.panel.is_visible() {
            self.panel.glide(r, 0.12);
        } else {
            self.panel.place(r);
            self.panel.order_front();
        }
    }
}

// MARK: - Marquee

pub struct MarqueeView;

impl Render for MarqueeView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let accent = accent();
        canvas(|_, _, _| (), move |bounds, _, window, _| {
            window.paint_quad(quad(
                bounds.dilate(px(-1.0)),
                Corners::default(),
                accent.opacity(0.12),
                Edges::all(px(2.0)),
                accent,
                BorderStyle::Dashed,
            ));
        })
        .size_full()
    }
}

/// The rectangle drawn with ⌘⇧-drag.
pub type MarqueePanel = Overlay<MarqueeView>;

impl MarqueePanel {
    pub fn create(cx: &mut App) -> anyhow::Result<Self> {
        let style = PanelStyle { level: LEVEL_FLOATING, mouse: false, every_space: true };
        Self::open(style, WindowBackgroundAppearance::Transparent, |_| MarqueeView, cx)
    }

    pub fn show(&self, r: Rect) {
        self.panel.place(r);
        if !self.panel.is_visible() {
            self.panel.order_front();
        }
    }
}

// MARK: - Ghost

pub struct GhostView {
    picture: Option<Arc<RenderImage>>,
    title: String,
}

impl Render for GhostView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = super::theme::theme(window);
        div()
            .size_full()
            .rounded(px(CORNER_RADIUS))
            .bg(theme.window.opacity(0.95))
            .p(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .children(self.picture.clone().map(|p| img(p).flex_1().min_h_0().w_full().object_fit(ObjectFit::Contain)))
            .child(
                div()
                    .flex_none()
                    .truncate()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(self.title.clone()),
            )
    }
}

/// Follows the pointer while a tab is dragged out of its strip.
pub type GhostPanel = Overlay<GhostView>;

impl GhostPanel {
    pub const SIZE: Size = Size::new(220.0, 150.0);

    pub fn create(cx: &mut App) -> anyhow::Result<Self> {
        let style = PanelStyle { level: LEVEL_FLOATING, mouse: false, every_space: true };
        let this = Self::open(style, WindowBackgroundAppearance::Transparent, |_| GhostView { picture: None, title: String::new() }, cx)?;
        this.panel.set_alpha(0.9);
        Ok(this)
    }

    pub fn show(&self, picture: Option<Arc<RenderImage>>, title: String, at: canopy_core::Point, cx: &mut App) {
        self.update(cx, |view, cx| {
            view.picture = picture;
            view.title = title;
            cx.notify();
        });
        self.move_to(at);
        self.panel.order_front();
    }

    pub fn move_to(&self, p: canopy_core::Point) {
        let s = Self::SIZE;
        self.panel.place(Rect::new(p.x - s.width / 2.0, p.y - 14.0, s.width, s.height));
    }
}
