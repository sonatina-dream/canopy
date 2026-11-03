//! The few controls the settings and setup windows use. GPUI draws everything
//! itself, so these are built from its elements, in the system's style.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, fill, img, point, prelude::FluentBuilder, px, size, AnyElement, App, Bounds, Corners, CursorStyle, DispatchPhase, Div,
    ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage, Window,
};

use super::theme::Theme;
use crate::imaging::Colour;
use crate::mac::app_look;

pub fn text(s: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_size(px(13.0)).text_color(theme.text).child(s.into())
}

pub fn caption(s: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_size(px(11.0)).line_height(px(15.0)).text_color(theme.secondary).child(s.into())
}

/// A rounded group of rows with an optional heading.
pub fn section(title: Option<&str>, rows: Vec<AnyElement>, theme: &Theme) -> Div {
    let last = rows.len().saturating_sub(1);
    let body = div().flex().flex_col().rounded(px(10.0)).bg(theme.group).border_1().border_color(theme.separator);
    let body = rows.into_iter().enumerate().fold(body, |body, (i, row)| {
        body.child(row).when(i != last, |b| b.child(div().h(px(1.0)).mx(px(12.0)).bg(theme.separator)))
    });
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when_some(title, |d, title| {
            d.child(
                div()
                    .px(px(4.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.secondary)
                    .child(title.to_string()),
            )
        })
        .child(body)
}

/// One line of a section: what it is on the left, the control on the right.
pub fn row(label: impl IntoElement, control: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .min_h(px(36.0))
        .px(px(12.0))
        .py(px(6.0))
        .child(div().flex_1().min_w_0().child(label))
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// A line of explanation under a row, inside a section.
pub fn note(s: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    div().px(px(12.0)).py(px(6.0)).child(caption(s, theme)).into_any_element()
}

pub fn titled(title: impl Into<SharedString>, detail: impl Into<SharedString>, theme: &Theme) -> Div {
    div().flex().flex_col().gap(px(2.0)).child(text(title, theme)).child(caption(detail, theme))
}

pub fn symbol(name: &str, points: f32, tint: Hsla) -> Option<impl IntoElement> {
    let rgba = gpui::Rgba::from(tint);
    let image = app_look::symbol(name, f64::from(points), Colour::new(rgba.r, rgba.g, rgba.b), rgba.a)?;
    Some(img(image).flex_none().size(px(points)).object_fit(ObjectFit::Contain))
}

pub fn toggle(id: impl Into<ElementId>, on: bool, theme: &Theme, change: impl Fn(bool, &mut App) + 'static) -> impl IntoElement {
    div()
        .id(id)
        .w(px(32.0))
        .h(px(19.0))
        .flex_none()
        .rounded(px(9.5))
        .p(px(1.5))
        .flex()
        .when(on, |d| d.justify_end())
        .bg(if on { theme.accent } else { theme.track })
        .cursor(CursorStyle::PointingHand)
        .on_click(move |_, _, cx| change(!on, cx))
        .child(div().size(px(16.0)).rounded(px(8.0)).bg(gpui::white()).shadow_sm())
}

/// A switch with its label, as a row.
pub fn toggle_row(id: impl Into<ElementId>, label: &str, on: bool, theme: &Theme, change: impl Fn(bool, &mut App) + 'static) -> AnyElement {
    row(text(label.to_string(), theme), toggle(id, on, theme, change))
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, primary: bool, theme: &Theme, click: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    let (background, foreground) = if primary { (theme.accent, gpui::white()) } else { (theme.control, theme.text) };
    div()
        .id(id)
        .flex_none()
        .h(px(24.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .bg(background)
        .border_1()
        .border_color(if primary { theme.accent } else { theme.control_border })
        .text_size(px(13.0))
        .text_color(foreground)
        .cursor(CursorStyle::PointingHand)
        .active(|s| s.opacity(0.75))
        .on_click(move |_, window, cx| click(window, cx))
        .child(label.into())
}

/// Two or more choices side by side.
pub fn segmented(id: &'static str, options: &[&str], selected: usize, theme: &Theme, choose: impl Fn(usize, &mut App) + 'static) -> impl IntoElement {
    let choose = Rc::new(choose);
    let mut bar = div().flex().flex_none().rounded(px(6.0)).p(px(2.0)).bg(theme.track);
    for (i, option) in options.iter().enumerate() {
        let choose = choose.clone();
        let chosen = i == selected;
        bar = bar.child(
            div()
                .id((id, i))
                .px(px(10.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(if chosen { gpui::white() } else { theme.text })
                .when(chosen, |d| d.bg(theme.accent))
                .cursor(CursorStyle::PointingHand)
                .on_click(move |_, _, cx| choose(i, cx))
                .child(option.to_string()),
        );
    }
    bar
}

/// A slider. `dragging` keeps the press across frames.
pub fn slider(value: f64, range: (f64, f64), step: Option<f64>, dragging: Rc<Cell<bool>>, theme: &Theme, change: impl Fn(f64, &mut App) + 'static) -> impl IntoElement {
    let (track, accent) = (theme.track, theme.accent);
    let change = Rc::new(change);
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        let (min, max) = range;
        let t = ((value - min) / (max - min)).clamp(0.0, 1.0) as f32;
        let width = bounds.size.width - px(16.0);
        let mid = bounds.origin.y + bounds.size.height / 2.0;
        let left = bounds.origin.x + px(8.0);
        let bar = Bounds::new(point(left, mid - px(2.0)), size(width, px(4.0)));
        window.paint_quad(fill(bar, track).corner_radii(px(2.0)));
        window.paint_quad(fill(Bounds::new(bar.origin, size(width * t, px(4.0))), accent).corner_radii(px(2.0)));
        let thumb = Bounds::new(point(left + width * t - px(8.0), mid - px(8.0)), size(px(16.0), px(16.0)));
        window.paint_quad(fill(thumb, gpui::white()).corner_radii(Corners::all(px(8.0))).border_widths(px(0.5)).border_color(gpui::black().opacity(0.2)));
        let hit = bounds;
        let pick = move |x: gpui::Pixels| -> f64 {
            let t = (f32::from(x - left) / f32::from(width)).clamp(0.0, 1.0);
            let v = min + (max - min) * f64::from(t);
            match step {
                Some(step) => (v / step).round() * step,
                None => v,
            }
        };
        let (d, c) = (dragging.clone(), change.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left && hit.contains(&e.position) {
                d.set(true);
                c(pick(e.position.x), cx);
            }
        });
        let (d, c) = (dragging.clone(), change.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && d.get() && e.dragging() {
                c(pick(e.position.x), cx);
            }
        });
        let d = dragging.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
            if phase == DispatchPhase::Bubble {
                d.set(false);
            }
        });
    })
    .h(px(20.0))
    .w(px(220.0))
}
