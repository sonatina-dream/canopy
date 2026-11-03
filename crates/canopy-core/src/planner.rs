use std::collections::{HashMap, HashSet};

use crate::arrangement::{Arrangement, NodeId, WindowId};
use crate::geometry::{AffineMap, Rect, Size};
use crate::navigator::Navigator;
use crate::solver::{Layout, Solver};

/// A hidden tab waiting behind the selected tab of its group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackedTab {
    pub rect: Rect,
    /// The selected tab, which has to stay in front of it.
    pub behind: WindowId,
}

/// Where every window of one display goes for the current camera.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub framed: NodeId,
    /// Layout of the framed subtree.
    pub layout: Layout,
    /// Real windows shown on screen (the selected tab of each roomy tile).
    pub visible: HashMap<WindowId, Rect>,
    /// Tiles too small for their window: drawn as a cover, window parked.
    pub covers: HashMap<NodeId, Rect>,
    /// Docked windows moved out of view: outside the framed node, hidden tabs, cramped.
    pub parked: HashSet<WindowId>,
    /// The parked windows that wait behind the selected tab, at its frame, so
    /// that switching tabs moves nothing. The rest wait at a corner of the display.
    pub stacked: HashMap<WindowId, StackedTab>,
    /// The desktop's rect when it is in view and not hidden.
    pub desktop_rect: Option<Rect>,
    /// Every node's rect as the camera sees it, including nodes outside the
    /// framed subtree (off screen). Used to animate windows into and out of view.
    pub projected: HashMap<NodeId, Rect>,
}

pub struct Planner;

impl Planner {
    /// Below this, a window is not usable and shows as a cover.
    pub const MINIMUM_USABLE: Size = Size::new(160.0, 100.0);

    /// A window may be a little bigger than its tile and reach into the gap;
    /// only a real shortfall makes it a cover.
    pub fn is_cramped(rect: Rect, minimum: Option<Size>) -> bool {
        let tolerance = 12.0;
        let m = minimum.unwrap_or(Size::ZERO);
        let w = (m.width - tolerance).max(Self::MINIMUM_USABLE.width);
        let h = (m.height - tolerance).max(Self::MINIMUM_USABLE.height);
        rect.width + 0.5 < w || rect.height + 0.5 < h
    }

    /// Whether a hidden tab can wait behind a window at `rect` without
    /// sticking out from behind it.
    fn fits_behind(rect: Rect, minimum: Option<Size>) -> bool {
        minimum.is_none_or(|m| m.width <= rect.width + 0.5 && m.height <= rect.height + 0.5)
    }

    pub fn plan(
        a: &Arrangement,
        nav: &Navigator,
        visible_frame: Rect,
        gap: f64,
        minimum_size: impl Fn(WindowId) -> Option<Size>,
    ) -> Plan {
        let bounds = Solver::bounds(visible_frame, gap);
        let framed = nav.framed_node(a);
        let range = nav.framed_range(a);
        let run_parent = range.as_ref().and_then(|r| a.parent(r[0]));

        let mut layout = match (&range, run_parent) {
            (Some(run), Some(p)) => {
                // Lay out just the run of children, as if it were the whole split.
                let weights: Vec<f64> =
                    run.iter().map(|&c| a.get(p).weights[a.index_in_parent(c).unwrap()]).collect();
                Solver::solve_run(a, a.get(p).axis, run, &weights, bounds, gap)
            }
            _ => Solver::solve(a, framed, bounds, gap),
        };
        if let (Some(run), Some(p)) = (&range, run_parent) {
            // Seams of the run belong to the real split. Its weights cover more
            // children than the run, so scale the length a drag is measured in.
            let parent = a.get(p);
            let run_total: f64 = run.iter().map(|&c| parent.weights[a.index_in_parent(c).unwrap()]).sum();
            let total: f64 = parent.weights.iter().sum();
            for seam in &mut layout.seams {
                if seam.split != Solver::RUN_ID {
                    continue;
                }
                seam.split = p;
                seam.available = if run_total > 0.0 { seam.available * total / run_total } else { seam.available };
                seam.index = a.index_in_parent(run[seam.index]).unwrap();
                seam.next = a.index_in_parent(run[seam.next]).unwrap();
            }
        }

        // Project the whole tree so the framed node (or run) fills the bounds.
        let world = Solver::solve(a, a.root(), bounds, gap);
        let mut projected = HashMap::new();
        let framed_world = match &range {
            Some(run) => Some(
                run.iter()
                    .filter_map(|c| world.node_rects.get(c))
                    .fold(Rect::ZERO, |acc, r| acc.union(r)),
            ),
            None => world.node_rects.get(&framed).copied(),
        };
        if let Some(framed_world) = framed_world {
            let map = AffineMap::new(&framed_world, &bounds);
            for (id, r) in &world.node_rects {
                projected.insert(*id, map.apply(r));
            }
        }
        for (id, r) in &layout.node_rects {
            if *id >= 0 {
                projected.insert(*id, *r);
            }
        }

        let mut visible = HashMap::new();
        let mut covers = HashMap::new();
        let mut parked = HashSet::new();
        let mut stacked = HashMap::new();
        for tile in a.tiles() {
            let node = a.get(tile);
            let (Some(window_rect), Some(selected)) = (layout.window_rects.get(&tile), node.selected_window()) else {
                parked.extend(node.windows.iter().copied());
                continue;
            };
            parked.extend(node.windows.iter().copied().filter(|&w| w != selected));
            if Self::is_cramped(*window_rect, minimum_size(selected)) {
                covers.insert(tile, layout.node_rects[&tile]);
                parked.insert(selected);
            } else {
                visible.insert(selected, *window_rect);
                for &w in node.windows.iter().filter(|w| **w != selected) {
                    if Self::fits_behind(*window_rect, minimum_size(w)) {
                        stacked.insert(w, StackedTab { rect: *window_rect, behind: selected });
                    }
                }
            }
        }
        let desktop = a.get(a.desktop());
        let desktop_rect = if desktop.hidden { None } else { layout.node_rects.get(&desktop.id).copied() };
        Plan { framed, layout, visible, covers, parked, stacked, desktop_rect, projected }
    }

    /// Whether `tile`'s window would be usable with `framing` framed.
    pub fn fits(a: &Arrangement, tile: NodeId, framing: NodeId, visible_frame: Rect, gap: f64, minimum: Option<Size>) -> bool {
        if !a.node_contains(framing, tile) {
            return false;
        }
        let layout = Solver::solve(a, framing, Solver::bounds(visible_frame, gap), gap);
        match layout.window_rects.get(&tile) {
            Some(r) => !Self::is_cramped(*r, minimum),
            None => false,
        }
    }
}
