//! The glass behind arranged windows while nothing moves: the desktop, blurred
//! and darkened, everywhere except the Desktop's own space. It sits just above
//! the desktop icons and below every window, and never takes the mouse.

use std::sync::Arc;

use canopy_core::Rect;
use gpui::{
    canvas, fill, point, px, quad, size, App, AppContext, BorderStyle, Bounds, Context, Corners, Edges, IntoElement, Pixels, Render,
    RenderImage, Styled, Window, WindowBackgroundAppearance, WindowHandle,
};

use super::{local, CORNER_RADIUS};
use crate::mac::displays::Display;
use crate::mac::panel::{level_above_desktop_icons, window_options, Panel, PanelStyle};

pub struct DeskView {
    frame: Rect,
    glass: Option<Arc<RenderImage>>,
    backdrop: Option<Arc<RenderImage>>,
    desk: Option<Rect>,
    previous: Vec<Arc<RenderImage>>,
}

impl Render for DeskView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        for image in self.previous.drain(..) {
            let _ = window.drop_image(image);
        }
        let origin = (self.frame.x, self.frame.y);
        let hole = self.desk.map(|d| local(d, origin));
        let (glass, backdrop) = (self.glass.clone(), self.backdrop.clone());
        canvas(|_, _, _| (), move |_, _, window, _| paint(glass, backdrop, hole, window)).size_full()
    }
}

fn paint(glass: Option<Arc<RenderImage>>, backdrop: Option<Arc<RenderImage>>, hole: Option<Bounds<Pixels>>, window: &mut Window) {
    let everything = Bounds::new(point(px(0.0), px(0.0)), window.viewport_size());
    let Some(hole) = hole else {
        match glass {
            Some(glass) => {
                let _ = window.paint_image(everything, everything, Corners::default(), glass, 0, false);
            }
            None => window.paint_quad(fill(everything, gpui::black().opacity(0.3))),
        }
        return;
    };
    let radius = px(CORNER_RADIUS);
    match glass {
        Some(glass) => {
            // The hole stays truly clear, so the desktop shows through live:
            // the glass is painted around it, not over it.
            let (left, top) = (hole.origin.x, hole.origin.y);
            let (right, bottom) = (hole.origin.x + hole.size.width, hole.origin.y + hole.size.height);
            let (w, h) = (everything.size.width, everything.size.height);
            let pieces = [
                Bounds::new(point(px(0.0), px(0.0)), size(w, top)),
                Bounds::new(point(px(0.0), bottom), size(w, h - bottom)),
                Bounds::new(point(px(0.0), top), size(left, hole.size.height)),
                Bounds::new(point(right, top), size(w - right, hole.size.height)),
            ];
            for piece in pieces {
                let _ = window.paint_image(piece, everything, Corners::default(), glass.clone(), 0, false);
            }
            // Its rounded corners: glass in each corner of the hole, with a
            // quarter circle of the desktop on top.
            let corners = [
                (point(left, top), Corners { top_left: radius, ..Corners::default() }),
                (point(right - radius, top), Corners { top_right: radius, ..Corners::default() }),
                (point(left, bottom - radius), Corners { bottom_left: radius, ..Corners::default() }),
                (point(right - radius, bottom - radius), Corners { bottom_right: radius, ..Corners::default() }),
            ];
            for (origin, shape) in corners {
                let square = Bounds::new(origin, size(radius, radius));
                let _ = window.paint_image(square, everything, Corners::default(), glass.clone(), 0, false);
                if let Some(backdrop) = &backdrop {
                    let _ = window.paint_image(square, everything, shape, backdrop.clone(), 0, false);
                }
            }
        }
        None => {
            // A border as wide as the display, around the hole: a solid colour
            // with a rounded hole in it.
            let reach = (everything.size.width + everything.size.height) * 2.0;
            window.paint_quad(quad(
                hole.dilate(reach),
                Corners::all(reach + radius),
                gpui::transparent_black(),
                Edges::all(reach),
                gpui::black().opacity(0.3),
                BorderStyle::Solid,
            ));
        }
    }
}

pub struct DeskPanel {
    panel: Panel,
    view: WindowHandle<DeskView>,
}

impl DeskPanel {
    pub fn new(display: &Display, cx: &mut App) -> anyhow::Result<Self> {
        let mut panel = None;
        let frame = display.frame;
        // The glass belongs to its Space's arrangement, so it stays on that Space.
        let style = PanelStyle { level: level_above_desktop_icons(), mouse: false, every_space: false };
        let view = cx.open_window(window_options(WindowBackgroundAppearance::Transparent), |window, cx| {
            let handle = window.window_handle();
            panel = Some(Panel::adopt(window, handle, style));
            cx.new(|_| DeskView { frame, glass: None, backdrop: None, desk: None, previous: Vec::new() })
        })?;
        Ok(Self { panel: panel.expect("the window was built"), view })
    }

    /// Shows glass with the Desktop's space (`desk`) clear. Hidden when
    /// nothing is docked, so a plain desktop stays plain.
    pub fn update(
        &self,
        display: &Display,
        glass: Option<Arc<RenderImage>>,
        backdrop: Option<Arc<RenderImage>>,
        desk: Option<Rect>,
        any_docked: bool,
        cx: &mut App,
    ) {
        if !any_docked {
            self.panel.order_out();
            return;
        }
        let frame = display.frame;
        let _ = self.view.update(cx, |view, _, cx| {
            let same = |a: &Option<Arc<RenderImage>>, b: &Option<Arc<RenderImage>>| match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same(&view.glass, &glass) {
                view.previous.extend(view.glass.take());
            }
            if !same(&view.backdrop, &backdrop) {
                view.previous.extend(view.backdrop.take());
            }
            view.frame = frame;
            view.glass = glass;
            view.backdrop = backdrop;
            view.desk = desk;
            cx.notify();
        });
        self.panel.place(frame);
        if !self.panel.is_visible() {
            self.panel.order_front();
        }
    }

    pub fn hide(&self) {
        self.panel.order_out();
    }
}
