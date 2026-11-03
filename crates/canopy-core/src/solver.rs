use std::collections::HashMap;

use crate::arrangement::{Arrangement, Kind, NodeId};
use crate::geometry::{Axis, Rect};

/// The gap between two neighbouring children of a split; drag it to resize both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    pub split: NodeId,
    /// The seam sits between child `index` and child `next` (the next visible one).
    pub index: usize,
    pub next: usize,
    pub axis: Axis,
    pub rect: Rect,
    /// The split's length available to children (for converting drags to weights).
    pub available: f64,
}

/// The solved geometry of one framed subtree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// A rect for every visible node of the framed subtree.
    pub node_rects: HashMap<NodeId, Rect>,
    /// Where each tile's selected window goes (below the tab strip for groups).
    pub window_rects: HashMap<NodeId, Rect>,
    /// Tab strip rects for tiles with more than one window.
    pub strip_rects: HashMap<NodeId, Rect>,
    pub seams: Vec<Seam>,
}

pub struct Solver;

impl Solver {
    pub const TAB_STRIP_HEIGHT: f64 = 36.0;

    /// The id the run of children in `solve_run` is laid out under.
    pub const RUN_ID: NodeId = -1;

    /// Lays out `node` to fill `bounds` (already inset for the outer margin).
    pub fn solve(a: &Arrangement, node: NodeId, bounds: Rect, gap: f64) -> Layout {
        let mut layout = Layout::default();
        place(a, node, bounds, gap, &mut layout);
        layout
    }

    /// Lays out a run of children as if they were a whole split of their own,
    /// under the id `RUN_ID`. Its seams' indices count within the run.
    pub fn solve_run(a: &Arrangement, axis: Axis, children: &[NodeId], weights: &[f64], bounds: Rect, gap: f64) -> Layout {
        let mut layout = Layout::default();
        layout.node_rects.insert(Self::RUN_ID, bounds);
        place_children(a, Self::RUN_ID, axis, children, weights, bounds, gap, &mut layout);
        layout
    }

    /// The bounds for a display: its visible frame inset by the gap.
    pub fn bounds(visible_frame: Rect, gap: f64) -> Rect {
        visible_frame.inset_by(gap)
    }
}

fn place(a: &Arrangement, id: NodeId, rect: Rect, gap: f64, layout: &mut Layout) {
    layout.node_rects.insert(id, rect);
    let node = a.get(id);
    match node.kind {
        Kind::Desktop => {}
        Kind::Tile => {
            if node.windows.len() > 1 {
                let strip = Rect::new(rect.x, rect.y, rect.width, Solver::TAB_STRIP_HEIGHT.min(rect.height));
                layout.strip_rects.insert(id, strip);
                layout.window_rects.insert(
                    id,
                    Rect::new(rect.x, rect.y + strip.height, rect.width, (rect.height - strip.height).max(0.0)),
                );
            } else {
                layout.window_rects.insert(id, rect);
            }
        }
        Kind::Split => place_children(a, id, node.axis, &node.children, &node.weights, rect, gap, layout),
    }
}

#[allow(clippy::too_many_arguments)]
fn place_children(
    a: &Arrangement,
    split: NodeId,
    axis: Axis,
    children: &[NodeId],
    weights: &[f64],
    rect: Rect,
    gap: f64,
    layout: &mut Layout,
) {
    let visible: Vec<usize> = (0..children.len())
        .filter(|&i| {
            let c = a.get(children[i]);
            !(c.is_desktop() && c.hidden)
        })
        .collect();
    if visible.is_empty() {
        return;
    }
    let length = rect.length(axis);
    let available = (length - gap * (visible.len() - 1) as f64).max(0.0);
    let total: f64 = visible.iter().map(|&i| weights[i].max(0.0)).sum();
    let mut cursor = rect.start(axis);
    let mut exact = cursor;
    for (k, &i) in visible.iter().enumerate() {
        let share = if total > 0.0 { weights[i].max(0.0) / total } else { 1.0 / visible.len() as f64 };
        exact += available * share;
        let is_last = k == visible.len() - 1;
        let end = if is_last { rect.start(axis) + length } else { exact.round() };
        let size = (end - cursor).max(0.0);
        let child_rect = match axis {
            Axis::Horizontal => Rect::new(cursor, rect.y, size, rect.height),
            Axis::Vertical => Rect::new(rect.x, cursor, rect.width, size),
        };
        place(a, children[i], child_rect, gap, layout);
        if !is_last {
            let seam_rect = match axis {
                Axis::Horizontal => Rect::new(end, rect.y, gap, rect.height),
                Axis::Vertical => Rect::new(rect.x, end, rect.width, gap),
            };
            layout.seams.push(Seam { split, index: i, next: visible[k + 1], axis, rect: seam_rect, available });
        }
        cursor = end + gap;
        exact += gap;
    }
}
