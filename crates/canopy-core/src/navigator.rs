use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::arrangement::{Arrangement, NodeId, WindowId};
use crate::geometry::{Axis, Direction, Edge, Point, Rect};

pub const SAVED_VIEW_SLOTS: std::ops::RangeInclusive<i64> = 1..=9;
const HISTORY_LIMIT: usize = 64;

/// A place the camera has been. Stores the node id plus the windows inside it,
/// so it still resolves after the tree changes shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Visit {
    pub node: NodeId,
    pub windows: Vec<WindowId>,
    pub includes_desktop: bool,
    /// "Everything": always resolves to the current root.
    pub is_root: bool,
    /// A run of neighbouring children of `node` (a split), framed together
    /// without the rest of the split. `None` frames all of `node`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Vec<NodeId>>,
}

impl Visit {
    pub fn root() -> Self {
        Self { node: 0, windows: Vec::new(), includes_desktop: true, is_root: true, range: None }
    }
}

/// The camera of one display: what is framed, back/forward history, saved views.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Navigator {
    framed: Visit,
    back_stack: Vec<Visit>,
    forward_stack: Vec<Visit>,
    saved_views: BTreeMap<i64, Visit>,
}

impl Default for Navigator {
    fn default() -> Self {
        Self::new()
    }
}

impl Navigator {
    pub fn new() -> Self {
        Self {
            framed: Visit::root(),
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            saved_views: BTreeMap::new(),
        }
    }

    pub fn framed(&self) -> &Visit {
        &self.framed
    }

    pub fn saved_views(&self) -> &BTreeMap<i64, Visit> {
        &self.saved_views
    }

    pub fn saved_slots(&self) -> Vec<i64> {
        self.saved_views.keys().copied().collect()
    }

    // MARK: Resolution

    pub fn visit_for(&self, node: NodeId, a: &Arrangement) -> Visit {
        if node == a.root() {
            return Visit::root();
        }
        Visit {
            node,
            windows: a.windows_in(node),
            includes_desktop: a.desktop_in(node).is_some(),
            is_root: false,
            range: None,
        }
    }

    pub fn visit_for_range(&self, nodes: &[NodeId], a: &Arrangement) -> Visit {
        let parent = nodes.first().and_then(|&n| a.parent(n));
        match parent {
            Some(p) if nodes.len() > 1 && nodes.len() < a.get(p).children.len() => Visit {
                node: p,
                windows: nodes.iter().flat_map(|&n| a.windows_in(n)).collect(),
                includes_desktop: nodes.iter().any(|&n| a.desktop_in(n).is_some()),
                is_root: false,
                range: Some(nodes.to_vec()),
            },
            _ => self.visit_for(nodes.first().copied().unwrap_or(a.root()), a),
        }
    }

    /// The children a range visit frames, in order, if they are still
    /// neighbouring children of one split (and not all of them).
    pub fn resolve_range(&self, v: &Visit, a: &Arrangement) -> Option<Vec<NodeId>> {
        let ids = v.range.as_ref().filter(|_| !v.is_root)?;
        let nodes: Vec<NodeId> = ids.iter().filter_map(|&i| a.node(i)).collect();
        if nodes.len() != ids.len() || nodes.len() <= 1 {
            return None;
        }
        let p = a.parent(nodes[0])?;
        if !nodes.iter().all(|&n| a.parent(n) == Some(p)) {
            return None;
        }
        let mut idx: Vec<usize> = nodes.iter().filter_map(|&n| a.index_in_parent(n)).collect();
        idx.sort_unstable();
        let parent = a.get(p);
        if idx.len() != nodes.len() || idx[idx.len() - 1] - idx[0] != idx.len() - 1 || idx.len() >= parent.children.len() {
            return None;
        }
        Some(idx.iter().map(|&i| parent.children[i]).collect())
    }

    /// The framed run of children, when a range is framed.
    pub fn framed_range(&self, a: &Arrangement) -> Option<Vec<NodeId>> {
        self.resolve_range(&self.framed, a)
    }

    fn current_visit(&self, a: &Arrangement) -> Visit {
        match self.framed_range(a) {
            Some(r) => self.visit_for_range(&r, a),
            None => self.visit_for(self.framed_node(a), a),
        }
    }

    fn same(&self, v: &Visit, w: &Visit, a: &Arrangement) -> bool {
        let (r1, r2) = (self.resolve_range(v, a), self.resolve_range(w, a));
        if r1.is_some() || r2.is_some() {
            return r1 == r2;
        }
        self.resolve(v, a) == self.resolve(w, a)
    }

    pub fn resolve(&self, v: &Visit, a: &Arrangement) -> NodeId {
        if v.is_root {
            return a.root();
        }
        if let Some(n) = a.node(v.node) {
            return n;
        }
        let mut nodes: Vec<NodeId> = v.windows.iter().filter_map(|&w| a.tile_of(w)).collect();
        if v.includes_desktop {
            nodes.push(a.desktop());
        }
        a.lowest_common_ancestor(&nodes).unwrap_or(a.root())
    }

    pub fn framed_node(&self, a: &Arrangement) -> NodeId {
        self.resolve(&self.framed, a)
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    // MARK: Moving the camera

    /// Frames `node`, pushing history. Returns false when it was already framed.
    pub fn frame(&mut self, node: NodeId, a: &Arrangement) -> bool {
        let v = self.visit_for(node, a);
        self.push(v, a)
    }

    /// Frames a run of neighbouring children of one split.
    pub fn frame_range(&mut self, nodes: &[NodeId], a: &Arrangement) -> bool {
        let v = self.visit_for_range(nodes, a);
        self.push(v, a)
    }

    fn push(&mut self, v: Visit, a: &Arrangement) -> bool {
        if self.same(&v, &self.framed, a) {
            return false;
        }
        let current = self.current_visit(a);
        self.back_stack.push(current);
        if self.back_stack.len() > HISTORY_LIMIT {
            let extra = self.back_stack.len() - HISTORY_LIMIT;
            self.back_stack.drain(..extra);
        }
        self.forward_stack.clear();
        self.framed = v;
        true
    }

    /// Frames `node`, or goes back if it is already framed ("again to go back").
    pub fn toggle_frame(&mut self, node: NodeId, a: &Arrangement) -> bool {
        if self.framed_range(a).is_none() && self.framed_node(a) == node {
            return self.back(a);
        }
        self.frame(node, a)
    }

    pub fn everything(&mut self, a: &Arrangement) -> bool {
        self.frame(a.root(), a)
    }

    pub fn step_out(&mut self, a: &Arrangement) -> bool {
        // Out of a range: the whole split it is part of.
        if self.framed_range(a).is_some() {
            return self.frame(self.framed_node(a), a);
        }
        match a.parent(self.framed_node(a)) {
            Some(p) => self.frame(p, a),
            None => false,
        }
    }

    /// Steps into the child of the framed split toward the focused window, or
    /// toward the pointer, or the first child.
    pub fn step_in(
        &mut self,
        a: &Arrangement,
        focused: Option<WindowId>,
        pointer: Option<Point>,
        rects: &HashMap<NodeId, Rect>,
        prefer_pointer: bool,
    ) -> bool {
        let f = self.framed_node(a);
        if !a.get(f).is_split() {
            return false;
        }
        let visible: Vec<NodeId> = self
            .framed_range(a)
            .unwrap_or_else(|| a.get(f).children.clone())
            .into_iter()
            .filter(|&c| !(a.get(c).is_desktop() && a.get(c).hidden))
            .collect();
        let by_focus = focused
            .and_then(|w| a.tile_of(w))
            .and_then(|t| visible.iter().copied().find(|&c| a.node_contains(c, t)));
        let by_pointer = pointer
            .and_then(|p| visible.iter().copied().find(|c| rects.get(c).is_some_and(|r| r.contains(p))));
        let chosen = if prefer_pointer { by_pointer.or(by_focus) } else { by_focus.or(by_pointer) };
        match chosen.or(visible.first().copied()) {
            Some(child) => self.frame(child, a),
            None => false,
        }
    }

    /// Frames the neighbouring space in `direction`, at the same depth where possible.
    pub fn neighbour(&mut self, direction: Direction, a: &Arrangement, rects: &HashMap<NodeId, Rect>) -> bool {
        match self.neighbour_node(direction, a, rects) {
            Some(target) => self.frame(target, a),
            None => false,
        }
    }

    pub fn neighbour_node(&self, direction: Direction, a: &Arrangement, rects: &HashMap<NodeId, Rect>) -> Option<NodeId> {
        let edge = direction.edge();
        // From a range, look past its child on that side.
        let f = match self.framed_range(a) {
            Some(r) => {
                if edge.is_leading() {
                    r[0]
                } else {
                    r[r.len() - 1]
                }
            }
            None => self.framed_node(a),
        };
        let reference = rects.get(&f).map(|r| r.center());
        let mut c = f;
        while let Some(p) = a.parent(c) {
            let parent = a.get(p);
            if parent.axis == edge.axis() {
                if let Some(i) = a.index_in_parent(c) {
                    let step: i64 = if edge.is_leading() { -1 } else { 1 };
                    let child = |j: i64| usize::try_from(j).ok().and_then(|j| parent.children.get(j).copied());
                    let mut j = i as i64 + step;
                    while child(j).is_some_and(|n| a.get(n).is_desktop() && a.get(n).hidden) {
                        j += step;
                    }
                    if let Some(n) = child(j) {
                        return Some(descend(a, n, a.depth(f), edge, reference, rects));
                    }
                }
            }
            c = p;
        }
        None
    }

    pub fn back(&mut self, a: &Arrangement) -> bool {
        let Some(v) = self.back_stack.pop() else { return false };
        let current = self.current_visit(a);
        self.forward_stack.push(current);
        self.framed = v;
        true
    }

    pub fn forward(&mut self, a: &Arrangement) -> bool {
        let Some(v) = self.forward_stack.pop() else { return false };
        let current = self.current_visit(a);
        self.back_stack.push(current);
        self.framed = v;
        true
    }

    // MARK: Saved views

    pub fn save(&mut self, slot: i64, a: &Arrangement) {
        if SAVED_VIEW_SLOTS.contains(&slot) {
            let current = self.current_visit(a);
            self.saved_views.insert(slot, current);
        }
    }

    pub fn go_to(&mut self, slot: i64, a: &Arrangement) -> bool {
        let Some(v) = self.saved_views.get(&slot).cloned() else { return false };
        if let Some(r) = self.resolve_range(&v, a) {
            return self.frame_range(&r, a);
        }
        let node = self.resolve(&v, a);
        self.frame(node, a)
    }

    pub fn forget(&mut self, slot: i64) {
        self.saved_views.remove(&slot);
    }

    // MARK: Following the layout

    /// Widens the view just enough to include `node`, without touching history.
    /// Used after a dock so the new window never lands out of sight. Next to
    /// the view it joins the run, so the rest of the split stays out of it.
    pub fn reveal(&mut self, node: NodeId, a: &Arrangement) -> bool {
        let f = self.framed_node(a);
        let range = self.framed_range(a);
        match &range {
            Some(r) if r.iter().any(|&c| a.node_contains(c, node)) => return false,
            None if a.node_contains(f, node) => return false,
            _ => {}
        }
        let shown = range.unwrap_or_else(|| vec![f]);
        self.framed = match longer_run(&shown, node, a) {
            Some(run) => self.visit_for_range(&run, a),
            None => self.visit_for(a.lowest_common_ancestor(&[f, node]).unwrap_or(a.root()), a),
        };
        true
    }

    /// Brings `tile` into view at a size where it `fits`: from the common
    /// ancestor of the current view and the tile, step down toward the tile.
    pub fn bring_into_view(&mut self, tile: NodeId, a: &Arrangement, fits: impl Fn(NodeId) -> bool) -> bool {
        let f = self.framed_node(a);
        match self.framed_range(a) {
            Some(r) if r.iter().any(|&c| a.node_contains(c, tile)) => return false,
            None if a.node_contains(f, tile) && fits(f) => return false,
            _ => {}
        }
        let mut n = a.lowest_common_ancestor(&[f, tile]).unwrap_or(a.root());
        while !fits(n) && n != tile {
            match a.get(n).children.iter().copied().find(|&c| a.node_contains(c, tile)) {
                Some(next) => n = next,
                None => break,
            }
        }
        self.frame(n, a)
    }

    /// Frames what a rectangle drawn on screen covers: the tiles (and the
    /// desktop) it covers a quarter of, that hold a fifth of it, or that hold
    /// its centre. When those are some of a split's children, just that run of
    /// children is framed.
    pub fn frame_rect(&mut self, rect: Rect, a: &Arrangement, rects: &HashMap<NodeId, Rect>) -> bool {
        let f = self.framed_node(a);
        let mut hits = Vec::new();
        for n in a.all_nodes(f) {
            let node = a.get(n);
            if !(node.is_tile() || (node.is_desktop() && !node.hidden)) {
                continue;
            }
            let Some(r) = rects.get(&n).filter(|r| !r.is_empty()) else { continue };
            let overlap = r.intersection(&rect).area();
            if overlap >= 0.25 * r.area() || overlap >= 0.2 * rect.area() || r.contains(rect.center()) {
                hits.push(n);
            }
        }
        let Some(target) = a.lowest_common_ancestor(&hits) else { return false };
        let node = a.get(target);
        if node.is_split() {
            let idx: HashSet<usize> = hits
                .iter()
                .filter_map(|&h| node.children.iter().position(|&c| a.node_contains(c, h)))
                .collect();
            if let (Some(&lo), Some(&hi)) = (idx.iter().min(), idx.iter().max()) {
                if hi > lo && hi - lo + 1 < node.children.len() {
                    let run = node.children[lo..=hi].to_vec();
                    return self.frame_range(&run, a);
                }
            }
        }
        self.frame(target, a)
    }

    /// Where the camera settles after a pinch that scaled the view by `scale`
    /// (above 1 zooms in) around `pointer`. Candidates are the nodes under the
    /// pointer from the root down, and the current view's ancestors; the one
    /// whose own zoom to fill `screen` is nearest the pinch wins. `rects` are
    /// every node's rect under the current camera (`Plan::projected`).
    pub fn settle_target(&self, scale: f64, pointer: Point, a: &Arrangement, rects: &HashMap<NodeId, Rect>, screen: Rect) -> NodeId {
        let f = self.framed_node(a);
        if scale <= 0.0 || scale.ln().abs() <= 0.12 {
            return f;
        }
        let mut candidates = vec![f];
        candidates.extend(a.ancestors(f));
        let mut n = Some(a.root());
        while let Some(node) = n {
            candidates.push(node);
            n = a.get(node).children.iter().copied().find(|&c| {
                let child = a.get(c);
                !(child.is_desktop() && child.hidden) && rects.get(&c).is_some_and(|r| r.contains(pointer))
            });
        }
        let zoom = |node: NodeId| -> Option<f64> {
            let r = rects.get(&node).filter(|r| r.width > 1.0 && r.height > 1.0)?;
            // How much the node grows to fill the screen, by area.
            Some(((screen.width / r.width) * (screen.height / r.height)).sqrt())
        };
        let target = scale.ln();
        let mut best: Option<(NodeId, f64)> = None;
        for c in candidates {
            let Some(z) = zoom(c) else { continue };
            let distance = (z.ln() - target).abs();
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((c, distance));
            }
        }
        best.map_or(f, |(c, _)| c)
    }

    /// Drops saved views and history entries whose windows are all gone.
    pub fn prune(&mut self, live_windows: &HashSet<WindowId>) {
        let alive = |v: &Visit| v.is_root || v.includes_desktop || v.windows.iter().any(|w| live_windows.contains(w));
        self.back_stack.retain(alive);
        self.forward_stack.retain(alive);
        self.saved_views.retain(|_, v| alive(v));
        if !alive(&self.framed) {
            self.framed = Visit::root();
        }
    }

    /// Replaces window ids, matching `Arrangement::remap`.
    pub fn remap(&mut self, mapping: &HashMap<WindowId, WindowId>) {
        let map = |v: &mut Visit| {
            for w in &mut v.windows {
                *w = mapping.get(w).copied().unwrap_or(*w);
            }
        };
        map(&mut self.framed);
        self.back_stack.iter_mut().for_each(map);
        self.forward_stack.iter_mut().for_each(map);
        self.saved_views.values_mut().for_each(map);
    }
}

fn descend(a: &Arrangement, start: NodeId, depth: usize, edge: Edge, reference: Option<Point>, rects: &HashMap<NodeId, Rect>) -> NodeId {
    let mut s = start;
    while a.depth(s) < depth && a.get(s).is_split() {
        let node = a.get(s);
        let visible: Vec<NodeId> = node
            .children
            .iter()
            .copied()
            .filter(|&c| !(a.get(c).is_desktop() && a.get(c).hidden))
            .collect();
        let (Some(&first), Some(&last)) = (visible.first(), visible.last()) else { break };
        if node.axis == edge.axis() {
            // Moving left enters from the right side, and so on.
            s = if edge.is_leading() { last } else { first };
        } else if let Some(reference) = reference {
            let mut best = first;
            let mut distance = cross_distance(rects.get(&first), reference, node.axis);
            for &c in &visible[1..] {
                let d = cross_distance(rects.get(&c), reference, node.axis);
                if d < distance {
                    best = c;
                    distance = d;
                }
            }
            s = best;
        } else {
            s = first;
        }
    }
    s
}

/// `run` with `node` added, when `node` is next to it in the same split and
/// the result still leaves some of that split out.
fn longer_run(run: &[NodeId], node: NodeId, a: &Arrangement) -> Option<Vec<NodeId>> {
    let (first, last) = (*run.first()?, *run.last()?);
    let parent = a.get(a.parent(first).filter(|&p| a.parent(node) == Some(p))?);
    let (start, end, at) = (a.index_in_parent(first)?, a.index_in_parent(last)?, a.index_in_parent(node)?);
    if (at + 1 != start && at != end + 1) || run.len() + 1 >= parent.children.len() {
        return None;
    }
    Some(parent.children[start.min(at)..=end.max(at)].to_vec())
}

fn cross_distance(r: Option<&Rect>, p: Point, axis: Axis) -> f64 {
    match r {
        None => f64::INFINITY,
        Some(r) if axis == Axis::Horizontal => (r.mid_x() - p.x).abs(),
        Some(r) => (r.mid_y() - p.y).abs(),
    }
}
