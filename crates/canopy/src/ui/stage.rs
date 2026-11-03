//! Draws motion for one display: its wallpaper and a picture of every visible
//! window, which slide while the real windows move out of sight underneath.
//! Above ordinary windows, below the menu bar, and never takes the mouse.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use canopy_core::{Point, Rect, Size, Solver, WindowId};
use futures::future::select;
use futures::FutureExt;
use gpui::{
    canvas, fill, point, px, quad, size, App, AppContext, BorderStyle, Bounds, BoxShadow, Context, Corners, Edges, Hsla,
    IntoElement, Pixels, Render, RenderImage, Styled, Window, WindowBackgroundAppearance, WindowHandle,
};

use super::theme::accent;
use super::{local, CORNER_RADIUS};
use crate::clog;
use crate::imaging::{ease, Colour};
use crate::mac::ax_service::WindowWriter;
use crate::mac::capture::Picture;
use crate::mac::displays::Display;
use crate::mac::panel::{fade, hide, ns_window, window_options, Panel, PanelStyle, LEVEL_UTILITY};

fn curve(t: f64) -> f64 {
    ease(t, 0.25, 0.1, 0.1, 1.0)
}

/// One window drawn on the stage.
#[derive(Clone)]
pub struct StageItem {
    pub id: WindowId,
    pub rect: Rect,
    pub image: Option<Arc<Picture>>,
    /// Placeholder colour when there is no picture.
    pub colour: Colour,
    pub icon: Option<Arc<RenderImage>>,
    /// What shows past the picture when the window grows.
    pub edge: Option<Colour>,
}

/// A rect on its way somewhere. The clock starts on the first frame drawn, so
/// motion is never cut short by the moment the stage takes to appear.
#[derive(Clone, Copy)]
struct Slide {
    from: Rect,
    to: Rect,
    start: Option<Instant>,
    duration: f64,
}

impl Slide {
    fn at(rect: Rect) -> Self {
        Self { from: rect, to: rect, start: None, duration: 0.0 }
    }

    fn current(&self, now: Instant) -> Rect {
        let Some(start) = self.start else { return self.from };
        if self.duration <= 0.0 {
            return self.to;
        }
        let t = now.duration_since(start).as_secs_f64() / self.duration;
        if t >= 1.0 {
            self.to
        } else {
            Rect::lerp(&self.from, &self.to, curve(t))
        }
    }

    fn moving(&self, now: Instant) -> bool {
        self.from != self.to
            && self.start.is_none_or(|start| now.duration_since(start).as_secs_f64() < self.duration)
    }

    fn go(&mut self, to: Rect, duration: f64, now: Instant) {
        *self = Self { from: self.current(now), to, start: None, duration };
    }

    fn jump(&mut self, to: Rect) {
        *self = Self::at(to);
    }
}

struct ItemState {
    item: StageItem,
    slide: Slide,
    raised: bool,
}

/// Which way a picture sits in its frame.
fn top_left_fits(picture: &Picture, r: &Rect) -> bool {
    r.width >= picture.size.width * 0.5 && r.height >= picture.size.height * 0.5
}

fn icon_frame(bounds: Rect) -> Rect {
    let side = (bounds.width.min(bounds.height) * 0.4).clamp(16.0, 96.0);
    Rect::new(bounds.mid_x() - side / 2.0, bounds.mid_y() - side / 2.0, side, side)
}

/// The zoom of a pinch: where each point goes, moving back to rest smoothly.
#[derive(Clone, Copy)]
struct Camera {
    from: (f64, f64, f64),
    to: (f64, f64, f64),
    start: Option<Instant>,
    duration: f64,
}

impl Camera {
    const REST: (f64, f64, f64) = (1.0, 0.0, 0.0);

    fn rest() -> Self {
        Self { from: Self::REST, to: Self::REST, start: None, duration: 0.0 }
    }

    fn current(&self, now: Instant) -> (f64, f64, f64) {
        let Some(start) = self.start else { return self.from };
        let t = if self.duration > 0.0 { now.duration_since(start).as_secs_f64() / self.duration } else { 1.0 };
        if t >= 1.0 {
            return self.to;
        }
        let e = curve(t);
        let mix = |a: f64, b: f64| a + (b - a) * e;
        (mix(self.from.0, self.to.0), mix(self.from.1, self.to.1), mix(self.from.2, self.to.2))
    }

    fn moving(&self, now: Instant) -> bool {
        self.from != self.to
            && self.start.is_none_or(|start| now.duration_since(start).as_secs_f64() < self.duration)
    }
}

/// Work that waits for the stage's first frame.
type Waiter = Box<dyn FnOnce(&mut App)>;

pub struct StageView {
    display: Display,
    area: Rect,
    backdrop: Option<Arc<RenderImage>>,
    /// The wallpaper file, rather than the captured desktop: it fills the
    /// display but doesn't match it pixel for pixel.
    backdrop_is_file: bool,
    glass: Option<Arc<RenderImage>>,
    desk: Slide,
    items: Vec<ItemState>,
    slot: Option<(Slide, bool)>,
    camera: Camera,
    /// True once the first frame of this showing is on screen.
    ready: bool,
    ready_scheduled: bool,
    waiters: Vec<Waiter>,
    generation: u64,
    /// Every image painted since the stage last came down.
    painted: Vec<Arc<RenderImage>>,
    shown_at: Instant,
    /// Frames drawn while something moved, this showing: GPUI draws on the
    /// main thread, so this is how smooth the motion really was.
    moving_frames: u32,
}

/// An image, and where the whole of it would sit.
type Placed = (Arc<RenderImage>, Bounds<Pixels>);

/// Everything one frame paints.
struct Frame {
    display: Bounds<Pixels>,
    backdrop: Option<Placed>,
    glass: Option<Arc<RenderImage>>,
    hole: Option<Bounds<Pixels>>,
    items: Vec<PaintedItem>,
    slot: Option<(Bounds<Pixels>, bool)>,
    accent: Hsla,
}

struct PaintedItem {
    bounds: Bounds<Pixels>,
    background: Hsla,
    image: Option<Placed>,
    icon: Option<Placed>,
    raised: bool,
    scale: f32,
}

impl StageView {
    fn empty_desk(&self) -> Rect {
        let f = self.display.frame;
        Rect::new(f.mid_x(), f.mid_y(), 0.0, 0.0)
    }

    fn desk_rect(&self, desk: Option<Rect>) -> Rect {
        // An empty hole keeps the shape the same, so it animates.
        desk.unwrap_or_else(|| self.empty_desk())
    }

    fn start_clocks(&mut self, now: Instant) {
        for item in &mut self.items {
            item.slide.start.get_or_insert(now);
        }
        self.desk.start.get_or_insert(now);
        self.camera.start.get_or_insert(now);
        if let Some((slide, _)) = &mut self.slot {
            slide.start.get_or_insert(now);
        }
    }

    fn is_moving(&self, now: Instant) -> bool {
        self.items.iter().any(|i| i.slide.moving(now))
            || self.desk.moving(now)
            || self.camera.moving(now)
            || self.slot.as_ref().is_some_and(|(s, _)| s.moving(now))
    }

    fn map(&self, now: Instant) -> impl Fn(Rect) -> Rect {
        let (scale, tx, ty) = self.camera.current(now);
        move |r| Rect::new(r.x * scale + tx, r.y * scale + ty, r.width * scale, r.height * scale)
    }

    fn frame(&mut self, now: Instant) -> Frame {
        let origin = (self.area.x, self.area.y);
        let display = local(self.display.frame, origin);
        let map = self.map(now);
        let (scale, _, _) = self.camera.current(now);
        let mut items = Vec::with_capacity(self.items.len());
        for state in &self.items {
            let rect = map(state.slide.current(now));
            let bounds = local(rect, origin);
            let item = &state.item;
            let picture = item.image.as_ref().map(|p| {
                let target = state.slide.to;
                let w = p.size.width * scale;
                let h = p.size.height * scale;
                let placed = if top_left_fits(p, &target) {
                    // A picture keeps its top-left in place like a window being
                    // resized, and only scales down when the window shrinks a
                    // lot (into a cover).
                    Rect::new(rect.x, rect.y, w, h)
                } else {
                    let fill = (rect.width / w).max(rect.height / h);
                    Rect::new(rect.mid_x() - w * fill / 2.0, rect.mid_y() - h * fill / 2.0, w * fill, h * fill)
                };
                (p.image.clone(), local(placed, origin))
            });
            let background = match (&picture, item.edge) {
                (Some(_), Some(edge)) => edge,
                _ => item.colour,
            };
            let icon = if picture.is_none() {
                item.icon.clone().map(|i| (i, local(icon_frame(rect), origin)))
            } else {
                None
            };
            items.push(PaintedItem {
                bounds,
                background: Hsla::from(background.to_rgba(1.0)),
                image: picture,
                icon,
                raised: state.raised,
                scale: scale as f32,
            });
        }
        let hole = self.desk.current(now);
        let hole = (!hole.is_empty()).then(|| local(hole, origin));
        let backdrop = self.backdrop.as_ref().map(|image| {
            let placed = if self.backdrop_is_file {
                let (w, h) = {
                    let s = image.size(0);
                    (f64::from(s.width.0), f64::from(s.height.0))
                };
                let f = self.display.frame;
                let fill = (f.width / w).max(f.height / h);
                Rect::new(f.mid_x() - w * fill / 2.0, f.mid_y() - h * fill / 2.0, w * fill, h * fill)
            } else {
                self.display.frame
            };
            (image.clone(), local(placed, origin))
        });
        let slot = self.slot.as_ref().map(|(s, tab)| (local(map(s.current(now)), origin), *tab));
        for image in items.iter().flat_map(|i| i.image.iter().chain(i.icon.iter())).map(|(i, _)| i) {
            if !self.painted.iter().any(|p| Arc::ptr_eq(p, image)) {
                self.painted.push(image.clone());
            }
        }
        Frame { display, backdrop, glass: self.glass.clone(), hole, items, slot, accent: accent() }
    }

    fn present(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn drop_painted(&mut self, window: &mut Window) {
        for image in self.painted.drain(..) {
            let _ = window.drop_image(image);
        }
    }

    /// Takes the waiters when the stage's first frame is on screen.
    fn take_waiters(&mut self) -> Vec<Waiter> {
        self.ready = true;
        std::mem::take(&mut self.waiters)
    }
}

impl Render for StageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        self.start_clocks(now);
        if self.is_moving(now) {
            self.moving_frames += 1;
            window.request_animation_frame();
        }
        if !self.ready && !self.ready_scheduled {
            self.ready_scheduled = true;
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let Ok(waiters) = this.update(cx, |view, _| view.take_waiters()) else { return };
                // This window is in use until the callback returns.
                cx.defer(move |cx| waiters.into_iter().for_each(|w| w(cx)));
            });
        }
        let frame = self.frame(now);
        canvas(|_, _, _| (), move |_, _, window, _| paint(frame, window)).size_full()
    }
}

fn paint(frame: Frame, window: &mut Window) {
    let everything = Bounds::new(point(px(0.0), px(0.0)), window.viewport_size());
    let radius = Corners::all(px(CORNER_RADIUS));
    window.paint_quad(fill(everything, super::theme::window_background(window)));
    if let Some((image, placed)) = &frame.backdrop {
        let _ = window.paint_image(everything, *placed, Corners::default(), image.clone(), 0, false);
    }
    // Behind arranged windows, and during motion, the desktop is shown blurred
    // and darkened, except the Desktop's own space, which stays clear.
    match &frame.glass {
        Some(glass) => {
            let _ = window.paint_image(everything, frame.display, Corners::default(), glass.clone(), 0, false);
        }
        None => window.paint_quad(fill(everything, gpui::black().opacity(0.3))),
    }
    if let (Some(hole), Some((image, placed))) = (frame.hole, &frame.backdrop) {
        let _ = window.paint_image(hole, *placed, radius, image.clone(), 0, false);
    }
    for item in &frame.items {
        paint_item(item, window);
    }
    if let Some((bounds, tab)) = frame.slot {
        paint_slot(bounds, tab, frame.accent, window);
    }
}

fn paint_item(item: &PaintedItem, window: &mut Window) {
    let r = px(CORNER_RADIUS * item.scale.min(1.0));
    let radius = Corners::all(r);
    if item.raised {
        let shadow = BoxShadow {
            color: gpui::black().opacity(0.35),
            offset: point(px(0.0), px(8.0)),
            blur_radius: px(18.0),
            spread_radius: px(0.0),
            inset: false,
        };
        window.paint_drop_shadows(item.bounds, radius, &[shadow]);
    }
    window.paint_quad(quad(item.bounds, radius, item.background, px(0.0), gpui::transparent_black(), BorderStyle::Solid));
    if let Some((image, placed)) = &item.image {
        let _ = window.paint_image(item.bounds, *placed, radius, image.clone(), 0, false);
    }
    if let Some((icon, frame)) = &item.icon {
        let _ = window.paint_image(*frame, *frame, Corners::default(), icon.clone(), 0, false);
    }
    window.paint_quad(quad(
        item.bounds,
        radius,
        gpui::transparent_black(),
        px(0.5),
        gpui::black().opacity(0.18),
        BorderStyle::Solid,
    ));
}

/// Shows where the dragged window will land. To dock: a filled slot. To join
/// as a tab: the whole target outlined, with the strip it will share.
fn paint_slot(bounds: Bounds<Pixels>, tab: bool, accent: Hsla, window: &mut Window) {
    let grown = bounds.dilate(px(3.0));
    window.paint_quad(quad(
        grown,
        Corners::all(px(12.0)),
        accent.opacity(if tab { 0.08 } else { 0.16 }),
        Edges::all(px(if tab { 3.0 } else { 2.0 })),
        accent.opacity(if tab { 1.0 } else { 0.9 }),
        BorderStyle::Solid,
    ));
    if tab {
        let h = Solver::TAB_STRIP_HEIGHT as f32;
        let strip = Bounds::new(
            point(grown.origin.x + px(6.0), grown.origin.y + px(3.0)),
            size(grown.size.width - px(12.0), px(h - 6.0)),
        );
        window.paint_quad(fill(strip, accent.opacity(0.35)).corner_radii(px(8.0)));
    }
}

/// What the stage stands on.
#[derive(Clone, Default)]
pub struct Ground {
    pub backdrop: Option<Arc<RenderImage>>,
    pub backdrop_is_file: bool,
    pub glass: Option<Arc<RenderImage>>,
}

pub struct StagePanel {
    pub display: Display,
    panel: Panel,
    view: WindowHandle<StageView>,
    area: Rect,
    shown: Rc<Cell<bool>>,
}

impl StagePanel {
    pub fn new(display: &Display, cx: &mut App) -> anyhow::Result<Self> {
        let f = display.frame;
        // The area the stage covers: everything below the menu bar, Dock included.
        let area = Rect::new(f.x, display.visible.y, f.width, f.max_y() - display.visible.y);
        let mut panel = None;
        let shown_display = display.clone();
        let style = PanelStyle { level: LEVEL_UTILITY, mouse: false, every_space: true };
        let view = cx.open_window(window_options(WindowBackgroundAppearance::Transparent), |window, cx| {
            let handle = window.window_handle();
            panel = Some(Panel::adopt(window, handle, style));
            cx.new(|_| StageView {
                display: shown_display,
                area,
                backdrop: None,
                backdrop_is_file: false,
                glass: None,
                desk: Slide::at(Rect::ZERO),
                items: Vec::new(),
                slot: None,
                camera: Camera::rest(),
                ready: true,
                ready_scheduled: true,
                waiters: Vec::new(),
                generation: 0,
                painted: Vec::new(),
                shown_at: Instant::now(),
                moving_frames: 0,
            })
        })?;
        Ok(Self { display: display.clone(), panel: panel.expect("the window was built"), view, area, shown: Rc::default() })
    }

    pub fn is_shown(&self) -> bool {
        self.shown.get()
    }

    fn update(&self, cx: &mut App, body: impl FnOnce(&mut StageView, &mut Window, &mut Context<StageView>)) {
        let _ = self.view.update(cx, body);
    }

    /// Puts the stage up with `items` (back to front) where they are now.
    /// `desk` is the Desktop's clear space now (`None` when it is out of view).
    pub fn show(&self, items: Vec<StageItem>, desk: Option<Rect>, ground: Ground, cx: &mut App) {
        let was_shown = self.shown.replace(true);
        self.update(cx, |view, window, cx| {
            view.generation += 1;
            if !was_shown {
                view.shown_at = Instant::now();
                view.moving_frames = 0;
                let source = if ground.backdrop_is_file { "the wallpaper file (no desktop capture yet)" } else { "the captured desktop" };
                clog!("STAGE up on {source}");
            }
            view.backdrop = ground.backdrop;
            view.backdrop_is_file = ground.backdrop_is_file;
            view.glass = ground.glass;
            let hole = view.desk_rect(desk);
            view.desk.jump(hole);
            view.items = items
                .into_iter()
                .map(|item| ItemState { slide: Slide::at(item.rect), item, raised: false })
                .collect();
            view.slot = None;
            view.camera = Camera::rest();
            view.ready = false;
            view.ready_scheduled = false;
            window.refresh();
            view.present(cx);
        });
        self.panel.set_alpha(1.0);
        self.panel.place(self.area);
        self.panel.order_front();
        // A display that is asleep draws nothing; don't wait for it forever.
        let shown_view = self.view;
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let _ = shown_view.update(cx, |view, _, cx| {
                if !view.ready {
                    let waiters = view.take_waiters();
                    cx.defer(move |cx| waiters.into_iter().for_each(|w| w(cx)));
                }
            });
        })
        .detach();
    }

    /// Whether this showing's first frame is on screen. Real windows must
    /// not move before their pictures cover them.
    pub fn is_ready(&self, cx: &App) -> bool {
        self.view.read(cx).map_or(true, |v| v.ready)
    }

    /// Runs `body` when the first frame is on screen, after anything queued
    /// before it.
    pub fn enqueue(&self, body: Box<dyn FnOnce(&mut App)>, cx: &mut App) {
        self.update(cx, |view, _, _| view.waiters.push(body));
    }

    pub fn contains(&self, id: WindowId, cx: &App) -> bool {
        self.view.read(cx).is_ok_and(|v| v.items.iter().any(|i| i.item.id == id))
    }

    /// Raises one item above the others (the window being dragged).
    pub fn bring_to_front(&self, id: WindowId, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            if let Some(i) = view.items.iter().position(|i| i.item.id == id) {
                let mut item = view.items.remove(i);
                item.raised = true;
                view.items.push(item);
            }
            view.present(cx);
        });
    }

    /// Moves an item at once (following the pointer).
    pub fn move_item(&self, id: WindowId, to: Rect, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            if let Some(i) = view.items.iter_mut().find(|i| i.item.id == id) {
                i.slide.jump(to);
            }
            view.present(cx);
        });
    }

    /// Zooms the whole picture by `scale` around `p`, at once (a pinch in progress).
    pub fn set_camera(&self, scale: f64, around: Point, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            let keep = |c: f64| (1.0 - scale) * c;
            let pose = (scale, keep(around.x), keep(around.y));
            view.camera = Camera { from: pose, to: pose, start: None, duration: 0.0 };
            view.present(cx);
        });
    }

    /// Slides items to new rects (and lets a pinch camera settle back).
    pub fn animate(&self, targets: &HashMap<WindowId, Rect>, duration: f64, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            let now = Instant::now();
            if view.camera.current(now) != Camera::REST {
                view.camera = Camera { from: view.camera.current(now), to: Camera::REST, start: None, duration };
            }
            for item in &mut view.items {
                if let Some(to) = targets.get(&item.item.id) {
                    item.slide.go(*to, duration, now);
                }
            }
            view.present(cx);
        });
    }

    /// Moves items at once (a gap being dragged).
    pub fn place(&self, targets: &HashMap<WindowId, Rect>, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            for item in &mut view.items {
                if let Some(to) = targets.get(&item.item.id) {
                    item.slide.jump(*to);
                }
            }
            view.present(cx);
        });
    }

    /// Slides the Desktop's clear space to `desk` (`None`: glass everywhere).
    pub fn set_desk(&self, desk: Option<Rect>, duration: f64, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            let to = view.desk_rect(desk);
            view.desk.go(to, duration, Instant::now());
            view.present(cx);
        });
    }

    pub fn set_slot(&self, slot: Option<Rect>, tab: bool, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            view.slot = match (slot, view.slot.take()) {
                (None, _) => None,
                (Some(r), None) => Some((Slide::at(r), tab)),
                (Some(r), Some((mut s, _))) => {
                    s.go(r, 0.18, Instant::now());
                    Some((s, tab))
                }
            };
            view.present(cx);
        });
    }

    /// Slides one item to a rect quickly (the dragged window becoming a
    /// thumbnail over a target, and back).
    pub fn reshape(&self, id: WindowId, to: Rect, cx: &mut App) {
        self.animate(&HashMap::from([(id, to)]), 0.18, cx);
    }

    /// What the slot shows now: nothing, "dock" or "tab" (for the test harness).
    pub fn slot_style(&self, cx: &App) -> Option<&'static str> {
        let view = self.view.read(cx).ok()?;
        view.slot.as_ref().map(|(_, tab)| if *tab { "tab" } else { "dock" })
    }

    /// The size of an item's picture, in points.
    pub fn picture_size(&self, id: WindowId, cx: &App) -> Option<Size> {
        let view = self.view.read(cx).ok()?;
        view.items.iter().find(|i| i.item.id == id)?.item.image.as_ref().map(|p| p.size)
    }

    pub fn set_image(&self, id: WindowId, picture: Arc<Picture>, edge: Option<Colour>, cx: &mut App) {
        self.update(cx, |view, _, cx| {
            if let Some(i) = view.items.iter_mut().find(|i| i.item.id == id) {
                i.item.image = Some(picture);
                i.item.edge = edge.or(i.item.edge);
            }
            view.present(cx);
        });
    }

    /// Takes the stage down after `delay`, once `writer` has finished moving
    /// the real windows (or a second has passed) and they have had `redraw`
    /// seconds to draw.
    pub fn land(&self, delay: f64, writer: &WindowWriter, redraw: f64, cx: &mut App) {
        let view = self.view;
        let Ok(generation) = view.read(cx).map(|v| v.generation) else { return };
        let writer = writer.clone();
        let panel = self.fader();
        cx.spawn(async move |cx| {
            let started = Instant::now();
            let current = |cx: &gpui::AsyncApp| view.read_with(cx, |v, _| v.generation == generation).unwrap_or(false);
            cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
            if !current(cx) {
                return;
            }
            let timeout = cx.background_executor().timer(Duration::from_secs(1)).fuse();
            let idle = writer.idle().fuse();
            futures::pin_mut!(timeout);
            select(idle, timeout).await;
            clog!("LANDED writes done {} ms after release", started.elapsed().as_millis());
            cx.background_executor().timer(Duration::from_secs_f64(redraw)).await;
            if current(cx) {
                cx.update(|cx| panel.fade(0.14, cx));
            }
        })
        .detach();
    }

    /// The parts of the panel a task needs to take the stage down later.
    fn fader(&self) -> Fader {
        Fader { view: self.view, shown: self.shown.clone() }
    }

    /// Takes the stage down now (a click during motion ends it).
    pub fn fade(&self, duration: f64, cx: &mut App) {
        self.fader().fade(duration, cx);
    }

    pub fn tear_down(&self, cx: &mut App) {
        self.shown.set(false);
        self.update(cx, |view, window, _| {
            view.generation += 1;
            view.drop_painted(window);
        });
        self.panel.order_out();
    }
}

struct Fader {
    view: WindowHandle<StageView>,
    shown: Rc<Cell<bool>>,
}

impl Fader {
    fn fade(&self, duration: f64, cx: &mut App) {
        if !self.shown.get() {
            return;
        }
        self.shown.set(false);
        let view = self.view;
        let _ = view.update(cx, |view, window, cx| {
            clog!("STAGE down after {} ms, {} frames of motion", view.shown_at.elapsed().as_millis(), view.moving_frames);
            view.generation += 1;
            let generation = view.generation;
            let waiters = view.take_waiters();
            cx.defer(move |cx| waiters.into_iter().for_each(|w| w(cx)));
            let ns = ns_window(window);
            fade(&ns, 0.0, duration);
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs_f64(duration)).await;
                let _ = this.update_in(cx, |view, window, cx| {
                    if view.generation == generation {
                        hide(&ns);
                        view.items.clear();
                        view.drop_painted(window);
                        cx.notify();
                    }
                });
            })
            .detach();
        });
    }
}

/// The stages of all displays.
#[derive(Default)]
pub struct Stage {
    panels: HashMap<String, StagePanel>,
    pub duration: f64,
    pub redraw_wait: f64,
}

impl Stage {
    pub fn panel(&mut self, display: &Display, cx: &mut App) -> Option<&StagePanel> {
        let stale = self.panels.get(&display.uuid).is_some_and(|p| p.display != *display);
        if stale {
            if let Some(p) = self.panels.remove(&display.uuid) {
                p.tear_down(cx);
                p.panel.close(cx);
            }
        }
        if !self.panels.contains_key(&display.uuid) {
            match StagePanel::new(display, cx) {
                Ok(p) => {
                    self.panels.insert(display.uuid.clone(), p);
                }
                Err(e) => {
                    clog!("STAGE could not open a window: {e}");
                    return None;
                }
            }
        }
        self.panels.get(&display.uuid)
    }

    pub fn any_shown(&self) -> bool {
        self.panels.values().any(|p| p.is_shown())
    }

    /// Ends every landing at once.
    pub fn interrupt(&self, cx: &mut App) {
        for p in self.panels.values().filter(|p| p.is_shown()) {
            p.fade(0.08, cx);
        }
    }

    pub fn tear_down(&self, cx: &mut App) {
        for p in self.panels.values() {
            p.tear_down(cx);
        }
    }
}
