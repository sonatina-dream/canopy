use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::geometry::{Axis, Edge, Rect, Size};
use crate::solver::Solver;

/// A window's identity. On macOS this is the CGWindowID.
pub type WindowId = u64;
pub type NodeId = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Split,
    Tile,
    Desktop,
}

/// One node of a display's arrangement tree.
///
/// - `Split`: children laid out along `axis`, sized by `weights` (sum 1).
/// - `Tile`: one or more windows; more than one makes a tab group.
/// - `Desktop`: the free-floating Desktop region. Exactly one per tree.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: Kind,
    pub axis: Axis,
    pub children: Vec<NodeId>,
    pub weights: Vec<f64>,
    pub windows: Vec<WindowId>,
    pub selected: i64,
    /// Only meaningful for the desktop node.
    pub hidden: bool,
    pub parent: Option<NodeId>,
}

impl Node {
    fn new(id: NodeId, kind: Kind) -> Self {
        Self {
            id,
            kind,
            axis: Axis::Horizontal,
            children: Vec::new(),
            weights: Vec::new(),
            windows: Vec::new(),
            selected: 0,
            hidden: false,
            parent: None,
        }
    }

    pub fn is_split(&self) -> bool {
        self.kind == Kind::Split
    }
    pub fn is_tile(&self) -> bool {
        self.kind == Kind::Tile
    }
    pub fn is_desktop(&self) -> bool {
        self.kind == Kind::Desktop
    }

    pub fn selected_window(&self) -> Option<WindowId> {
        if !self.is_tile() || self.windows.is_empty() {
            return None;
        }
        let i = self.selected.clamp(0, self.windows.len() as i64 - 1);
        Some(self.windows[i as usize])
    }
}

/// Where a window goes back to when it is restored (after minimise, or when a
/// floating window is docked again with the green button).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RestoreTarget {
    /// Beside node `beside` on `edge`, taking `fraction` of that split.
    /// `fallback` is a window that was in that neighbour, used if the node is gone.
    Docked { beside: NodeId, edge: Edge, fraction: f64, fallback: Option<WindowId> },
    /// A tab at `index` in the group that contains `window`.
    Tab { window: WindowId, index: i64 },
    Floating,
}

/// One display's tree of docked windows, plus every operation that changes it.
///
/// Nodes live in an arena keyed by id and refer to each other by id, so a copy
/// (used to preview the outcome of a drop) is an ordinary `clone`.
#[derive(Clone, Debug)]
pub struct Arrangement {
    nodes: HashMap<NodeId, Node>,
    root: NodeId,
    next_id: NodeId,
    /// When a split collapses, its id points at the node that took its place,
    /// so visits and restore targets that named it still resolve.
    aliases: HashMap<NodeId, NodeId>,
}

impl Default for Arrangement {
    fn default() -> Self {
        Self::new()
    }
}

impl Arrangement {
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(1, Node::new(1, Kind::Desktop));
        Self { nodes, root: 1, next_id: 2, aliases: HashMap::new() }
    }

    fn make_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // MARK: Queries

    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The node with exactly this id. Panics if it isn't in the tree, like
    /// following a stale reference would.
    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[&id]
    }

    pub fn try_get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    fn get_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes.get_mut(&id).expect("node is in the tree")
    }

    /// Preorder traversal of the subtree at `id`.
    pub fn all_nodes(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect(id, &mut out);
        out
    }

    fn collect(&self, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        for &c in &self.get(id).children {
            self.collect(c, out);
        }
    }

    pub fn tiles_in(&self, id: NodeId) -> Vec<NodeId> {
        self.all_nodes(id).into_iter().filter(|&n| self.get(n).is_tile()).collect()
    }

    pub fn windows_in(&self, id: NodeId) -> Vec<WindowId> {
        self.tiles_in(id).into_iter().flat_map(|t| self.get(t).windows.clone()).collect()
    }

    pub fn desktop_in(&self, id: NodeId) -> Option<NodeId> {
        self.all_nodes(id).into_iter().find(|&n| self.get(n).is_desktop())
    }

    pub fn desktop(&self) -> NodeId {
        self.desktop_in(self.root).expect("every tree has a desktop")
    }

    pub fn is_desktop_hidden(&self) -> bool {
        self.get(self.desktop()).hidden
    }

    pub fn windows(&self) -> Vec<WindowId> {
        self.windows_in(self.root)
    }

    pub fn tiles(&self) -> Vec<NodeId> {
        self.tiles_in(self.root)
    }

    pub fn is_empty(&self) -> bool {
        self.tiles().is_empty()
    }

    pub fn contains(&self, window: WindowId) -> bool {
        self.tile_of(window).is_some()
    }

    pub fn tile_of(&self, window: WindowId) -> Option<NodeId> {
        self.tiles().into_iter().find(|&t| self.get(t).windows.contains(&window))
    }

    pub fn selected_window(&self, id: NodeId) -> Option<WindowId> {
        self.get(id).selected_window()
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.get(id).parent
    }

    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let p = self.get(id).parent?;
        self.get(p).children.iter().position(|&c| c == id)
    }

    pub fn depth(&self, id: NodeId) -> usize {
        self.ancestors(id).len()
    }

    /// Ancestors from the parent up to the root.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut n = self.get(id).parent;
        while let Some(p) = n {
            out.push(p);
            n = self.get(p).parent;
        }
        out
    }

    pub fn is_ancestor(&self, id: NodeId, of: NodeId) -> bool {
        self.ancestors(of).contains(&id)
    }

    /// True if `inner` is `outer` or inside it.
    pub fn node_contains(&self, outer: NodeId, inner: NodeId) -> bool {
        outer == inner || self.is_ancestor(outer, inner)
    }

    /// The deepest node containing every node in `nodes`.
    pub fn lowest_common_ancestor(&self, nodes: &[NodeId]) -> Option<NodeId> {
        let first = *nodes.first()?;
        let mut candidate = Some(first);
        while let Some(c) = candidate {
            if nodes.iter().all(|&n| self.node_contains(c, n)) {
                return Some(c);
            }
            candidate = self.get(c).parent;
        }
        None
    }

    /// The node with `id`, or the node that took its place after a collapse.
    pub fn node(&self, id: NodeId) -> Option<NodeId> {
        let mut id = id;
        let mut seen = HashSet::new();
        loop {
            if self.nodes.contains_key(&id) {
                return Some(id);
            }
            let next = *self.aliases.get(&id)?;
            if !seen.insert(id) {
                return None;
            }
            id = next;
        }
    }

    /// The least length along `axis` this subtree can take, given each
    /// window's minimum size (unknown minimums count as `fallback`).
    pub fn minimum_length(
        &self,
        id: NodeId,
        axis: Axis,
        gap: f64,
        fallback: f64,
        minimum: &impl Fn(WindowId) -> Option<Size>,
    ) -> f64 {
        let node = self.get(id);
        match node.kind {
            Kind::Desktop => {
                if node.hidden {
                    0.0
                } else {
                    fallback
                }
            }
            Kind::Tile => {
                let Some(m) = node.selected_window().and_then(minimum) else {
                    return fallback;
                };
                let strip = if axis == Axis::Vertical && node.windows.len() > 1 {
                    Solver::TAB_STRIP_HEIGHT
                } else {
                    0.0
                };
                let length = if axis == Axis::Horizontal { m.width } else { m.height };
                fallback.max(length) + strip
            }
            Kind::Split => {
                let kids: Vec<NodeId> = node
                    .children
                    .iter()
                    .copied()
                    .filter(|&c| !(self.get(c).is_desktop() && self.get(c).hidden))
                    .collect();
                let lengths = kids.iter().map(|&k| self.minimum_length(k, axis, gap, fallback, minimum));
                if node.axis == axis {
                    let count = kids.len().saturating_sub(1) as f64;
                    lengths.sum::<f64>() + gap * count
                } else {
                    lengths.fold(0.0, f64::max)
                }
            }
        }
    }

    /// `H(1 V(2 [3,4]) D)`: the tree as a short string.
    pub fn describe(&self, id: NodeId) -> String {
        let node = self.get(id);
        match node.kind {
            Kind::Desktop => (if node.hidden { "D(hidden)" } else { "D" }).to_string(),
            Kind::Tile => {
                if node.windows.len() == 1 {
                    node.windows[0].to_string()
                } else {
                    let list: Vec<String> = node.windows.iter().map(|w| w.to_string()).collect();
                    format!("[{}]", list.join(","))
                }
            }
            Kind::Split => {
                let sym = if node.axis == Axis::Horizontal { "H" } else { "V" };
                let kids: Vec<String> = node.children.iter().map(|&c| self.describe(c)).collect();
                format!("{sym}({})", kids.join(" "))
            }
        }
    }

    // MARK: Docking

    /// Docks `window` in a new tile beside `target` on `edge`. Returns the new tile.
    pub fn dock(&mut self, window: WindowId, beside: NodeId, edge: Edge, fraction: Option<f64>) -> NodeId {
        if let Some(existing) = self.tile_of(window) {
            // Already docked: move it rather than duplicate it.
            if self.get(existing).windows.len() == 1 {
                self.move_node(existing, beside, edge, fraction);
                return existing;
            }
            self.remove(window, true);
        }
        let id = self.make_id();
        let mut tile = Node::new(id, Kind::Tile);
        tile.windows = vec![window];
        self.nodes.insert(id, tile);
        self.insert(id, beside, edge, fraction);
        id
    }

    /// Docks beside everything at a screen edge.
    pub fn dock_at_screen_edge(&mut self, window: WindowId, edge: Edge) -> NodeId {
        let root = self.root;
        self.dock(window, root, edge, None)
    }

    /// Inserts a detached node beside `target`. Neighbours make room.
    fn insert(&mut self, node: NodeId, target: NodeId, edge: Edge, fraction: Option<f64>) {
        assert!(self.get(node).parent.is_none(), "insert expects a detached node");
        let axis = edge.axis();

        // Same axis as the target's split: become a sibling.
        if let Some(p) = self.get(target).parent {
            let parent = self.get(p);
            if parent.is_split() && parent.axis == axis {
                let i = self.index_in_parent(target).expect("target is its parent's child");
                let at = if edge.is_leading() { i } else { i + 1 };
                self.join(p, node, at, fraction);
                return;
            }
        }

        // Target is itself a split on this axis: join it at the near end.
        let t = self.get(target);
        if t.is_split() && t.axis == axis {
            let at = if edge.is_leading() { 0 } else { t.children.len() };
            self.join(target, node, at, fraction);
            return;
        }

        // Otherwise wrap the target in a new split.
        let f = clamp_fraction(fraction.unwrap_or(0.5));
        let parent = self.get(target).parent;
        let index = self.index_in_parent(target);
        let id = self.make_id();
        let mut split = Node::new(id, Kind::Split);
        split.axis = axis;
        (split.children, split.weights) = if edge.is_leading() {
            (vec![node, target], vec![f, 1.0 - f])
        } else {
            (vec![target, node], vec![1.0 - f, f])
        };
        split.parent = parent;
        self.nodes.insert(id, split);
        self.get_mut(node).parent = Some(id);
        self.get_mut(target).parent = Some(id);
        match (parent, index) {
            (Some(parent), Some(index)) => self.get_mut(parent).children[index] = id,
            _ => self.root = id,
        }
    }

    /// Adds `node` as child `at` of `split`; the others scale down to make room.
    fn join(&mut self, split: NodeId, node: NodeId, at: usize, fraction: Option<f64>) {
        let s = self.get_mut(split);
        let f = clamp_fraction(fraction.unwrap_or(1.0 / (s.children.len() + 1) as f64));
        let total: f64 = s.weights.iter().sum();
        for w in &mut s.weights {
            *w = *w / total * (1.0 - f);
        }
        s.children.insert(at, node);
        s.weights.insert(at, f);
        self.get_mut(node).parent = Some(split);
    }

    /// Moves an existing node (tile or subtree) beside `target`.
    pub fn move_node(&mut self, node: NodeId, target: NodeId, edge: Edge, fraction: Option<f64>) {
        if node == target || self.node_contains(node, target) {
            return;
        }
        let mut target = target;
        if let Some((replaced, by)) = self.detach(node, true) {
            if replaced == target {
                target = by;
            }
        }
        self.insert(node, target, edge, fraction);
    }

    // MARK: Tabs

    /// Adds `window` as a selected tab of `tile`.
    pub fn add_tab(&mut self, window: WindowId, tile: NodeId, index: Option<i64>) {
        if !self.get(tile).is_tile() {
            return;
        }
        if let Some(current) = self.tile_of(window) {
            if current == tile {
                return;
            }
            self.remove(window, true);
        }
        let t = self.get_mut(tile);
        let count = t.windows.len() as i64;
        let at = index.unwrap_or(count).clamp(0, count);
        t.windows.insert(at as usize, window);
        t.selected = at;
    }

    /// Merges every window of `source` into `target` as tabs.
    pub fn merge(&mut self, source: NodeId, target: NodeId) {
        if !self.get(source).is_tile() || !self.get(target).is_tile() || source == target {
            return;
        }
        let moving = self.get(source).windows.clone();
        let selected = self.get(source).selected_window();
        self.detach(source, true);
        self.nodes.remove(&source);
        let t = self.get_mut(target);
        t.windows.extend(moving);
        if let Some(i) = selected.and_then(|s| t.windows.iter().position(|&w| w == s)) {
            t.selected = i as i64;
        }
    }

    pub fn select(&mut self, window: WindowId) {
        let Some(t) = self.tile_of(window) else { return };
        let tile = self.get_mut(t);
        if let Some(i) = tile.windows.iter().position(|&w| w == window) {
            tile.selected = i as i64;
        }
    }

    // MARK: Removal

    /// Removes `window` from the tree and returns where it could go back to.
    pub fn remove(&mut self, window: WindowId, share_space: bool) -> Option<RestoreTarget> {
        let t = self.tile_of(window)?;
        let i = self.get(t).windows.iter().position(|&w| w == window)?;
        if self.get(t).windows.len() > 1 {
            let was_selected = self.get(t).selected_window();
            let tile = self.get_mut(t);
            tile.windows.remove(i);
            let last = tile.windows.len() - 1;
            if was_selected == Some(window) {
                tile.selected = i.min(last) as i64;
            } else if let Some(j) = was_selected.and_then(|w| tile.windows.iter().position(|&x| x == w)) {
                tile.selected = j as i64;
            }
            return Some(RestoreTarget::Tab { window: tile.windows[i.min(last)], index: i as i64 });
        }
        let target = self.restore_target_for_detaching(t);
        self.detach(t, share_space);
        self.nodes.remove(&t);
        target
    }

    /// Removes every window that is not in `live`. Returns the removed windows.
    pub fn retain_only(&mut self, live: &HashSet<WindowId>, share_space: bool) -> Vec<WindowId> {
        let gone: Vec<WindowId> = self.windows().into_iter().filter(|w| !live.contains(w)).collect();
        for &w in &gone {
            self.remove(w, share_space);
        }
        gone
    }

    /// Replaces window ids (after a restart the same window can have a new id).
    pub fn remap(&mut self, mapping: &HashMap<WindowId, WindowId>) {
        for t in self.tiles() {
            for w in &mut self.get_mut(t).windows {
                *w = mapping.get(w).copied().unwrap_or(*w);
            }
        }
    }

    fn restore_target_for_detaching(&self, node: NodeId) -> Option<RestoreTarget> {
        let n = self.get(node);
        let p = self.get(n.parent?);
        let i = self.index_in_parent(node)?;
        if p.children.len() <= 1 {
            return None;
        }
        let total: f64 = p.weights.iter().sum();
        let fraction = if total > 0.0 { p.weights[i] / total } else { 0.5 };
        let (neighbour, edge) = if i > 0 {
            (p.children[i - 1], if p.axis == Axis::Horizontal { Edge::Right } else { Edge::Bottom })
        } else {
            (p.children[i + 1], if p.axis == Axis::Horizontal { Edge::Left } else { Edge::Top })
        };
        Some(RestoreTarget::Docked {
            beside: neighbour,
            edge,
            fraction,
            fallback: self.windows_in(neighbour).first().copied(),
        })
    }

    /// Detaches `node` from its parent. A split left with one child collapses;
    /// returns that collapse (replaced, by) so callers can follow the survivor.
    fn detach(&mut self, node: NodeId, share_space: bool) -> Option<(NodeId, NodeId)> {
        let p = self.get(node).parent?;
        let i = self.index_in_parent(node)?;
        let parent = self.get_mut(p);
        let w = parent.weights[i];
        parent.children.remove(i);
        parent.weights.remove(i);
        if !share_space && !parent.children.is_empty() {
            let j = if i < parent.children.len() { i } else { i - 1 };
            parent.weights[j] += w;
        }
        let total: f64 = parent.weights.iter().sum();
        if total > 0.0 {
            for weight in &mut parent.weights {
                *weight /= total;
            }
        }
        let collapses = parent.children.len() == 1;
        self.get_mut(node).parent = None;
        if !collapses {
            return None;
        }
        let only = self.get(p).children[0];
        match (self.get(p).parent, self.index_in_parent(p)) {
            (Some(g), Some(k)) => {
                self.get_mut(g).children[k] = only;
                self.get_mut(only).parent = Some(g);
            }
            _ => {
                self.get_mut(only).parent = None;
                self.root = only;
            }
        }
        self.nodes.remove(&p);
        self.record_alias(p, only);
        Some((p, only))
    }

    /// Puts `window` back where `target` says. Returns `None` if it could not,
    /// in which case the caller auto-docks it.
    pub fn restore(&mut self, window: WindowId, target: RestoreTarget) -> Option<NodeId> {
        match target {
            RestoreTarget::Tab { window: other, index } => {
                let t = self.tile_of(other)?;
                self.add_tab(window, t, Some(index));
                Some(t)
            }
            RestoreTarget::Docked { beside, edge, fraction, fallback } => {
                if let Some(n) = self.node(beside) {
                    return Some(self.dock(window, n, edge, Some(fraction)));
                }
                let t = self.tile_of(fallback?)?;
                Some(self.dock(window, t, edge, Some(fraction)))
            }
            RestoreTarget::Floating => None,
        }
    }

    /// Docks `window` by splitting the largest visible tile along its longer
    /// side, or at the left screen edge when nothing is docked yet.
    pub fn auto_dock(&mut self, window: WindowId, rects: &HashMap<NodeId, Rect>) -> NodeId {
        let mut largest: Option<(NodeId, Rect)> = None;
        for t in self.tiles() {
            let Some(r) = rects.get(&t).filter(|r| !r.is_empty()) else { continue };
            if largest.is_none_or(|(_, best)| best.area() < r.area()) {
                largest = Some((t, *r));
            }
        }
        let Some((tile, r)) = largest else {
            return self.dock_at_screen_edge(window, Edge::Left);
        };
        let edge = if r.width >= r.height { Edge::Right } else { Edge::Bottom };
        self.dock(window, tile, edge, None)
    }

    // MARK: Resizing and the desktop

    /// Moves the seam between child `index` and child `next` of `split` by
    /// `delta` points, given the split's available length in points. Neither
    /// side goes below `min_points`, or below its own minimum (`min_before`,
    /// `min_after`) when known: the seam stops where a window would refuse.
    pub fn resize_seam(&mut self, split: NodeId, index: usize, next: usize, delta: f64, available: f64, limits: SeamLimits) {
        let s = self.get_mut(split);
        if !s.is_split() || next <= index || next >= s.children.len() || available <= 0.0 {
            return;
        }
        let total: f64 = s.weights.iter().sum();
        let low_a = (limits.min_fraction * total).max(limits.min_points.max(limits.min_before) / available * total);
        let low_b = (limits.min_fraction * total).max(limits.min_points.max(limits.min_after) / available * total);
        let (a, b) = (s.weights[index], s.weights[next]);
        let dw = delta / available * total;
        let mut na = a + dw;
        // Never squeeze a side that already fits; never push one that doesn't further.
        if dw > 0.0 {
            na = na.min(a.max(a + b - low_b));
        } else {
            na = na.max(a.min(low_a));
        }
        if na <= 0.0 || a + b - na <= 0.0 {
            return;
        }
        s.weights[index] = na;
        s.weights[next] = a + b - na;
    }

    pub fn set_desktop_hidden(&mut self, hidden: bool) {
        let d = self.desktop();
        self.get_mut(d).hidden = hidden;
    }

    fn record_alias(&mut self, from: NodeId, to: NodeId) {
        self.aliases.insert(from, to);
        if self.aliases.len() > 512 {
            let live: HashMap<NodeId, NodeId> =
                self.aliases.iter().filter(|(_, v)| self.node(**v).is_some()).map(|(k, v)| (*k, *v)).collect();
            self.aliases = live;
        }
    }
}

/// The floors `Arrangement::resize_seam` holds a seam to.
#[derive(Clone, Copy, Debug)]
pub struct SeamLimits {
    pub min_fraction: f64,
    pub min_points: f64,
    pub min_before: f64,
    pub min_after: f64,
}

impl Default for SeamLimits {
    fn default() -> Self {
        Self { min_fraction: 0.05, min_points: 60.0, min_before: 0.0, min_after: 0.0 }
    }
}

fn clamp_fraction(f: f64) -> f64 {
    f.clamp(0.05, 0.95)
}

// MARK: JSON
//
// On disk nodes nest, with only the fields their kind uses; parent links are
// rebuilt on decode. This is the format arrangements.json has always had.

#[derive(Serialize, Deserialize)]
struct NodeWire {
    id: NodeId,
    kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    axis: Option<Axis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<NodeWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weights: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    windows: Option<Vec<WindowId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,
}

#[derive(Serialize, Deserialize)]
struct ArrangementWire {
    root: NodeWire,
    #[serde(default)]
    aliases: BTreeMap<String, NodeId>,
}

impl Arrangement {
    fn wire(&self, id: NodeId) -> NodeWire {
        let n = self.get(id);
        let mut w = NodeWire {
            id,
            kind: n.kind,
            axis: None,
            children: None,
            weights: None,
            windows: None,
            selected: None,
            hidden: None,
        };
        match n.kind {
            Kind::Split => {
                w.axis = Some(n.axis);
                w.children = Some(n.children.iter().map(|&c| self.wire(c)).collect());
                w.weights = Some(n.weights.clone());
            }
            Kind::Tile => {
                w.windows = Some(n.windows.clone());
                w.selected = Some(n.selected);
            }
            Kind::Desktop => w.hidden = Some(n.hidden),
        }
        w
    }

    fn adopt(&mut self, wire: NodeWire, parent: Option<NodeId>) -> NodeId {
        let id = wire.id;
        let children = wire.children.unwrap_or_default();
        let count = children.len();
        let mut node = Node::new(id, wire.kind);
        node.axis = wire.axis.unwrap_or(Axis::Horizontal);
        node.windows = wire.windows.unwrap_or_default();
        node.selected = wire.selected.unwrap_or(0);
        node.hidden = wire.hidden.unwrap_or(false);
        node.parent = parent;
        node.weights = wire.weights.unwrap_or_default();
        if node.weights.len() != count {
            node.weights = vec![1.0 / count.max(1) as f64; count];
        }
        node.children = children.into_iter().map(|c| self.adopt(c, Some(id))).collect();
        self.nodes.insert(id, node);
        id
    }
}

impl Serialize for Arrangement {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let aliases = self.aliases.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        ArrangementWire { root: self.wire(self.root), aliases }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Arrangement {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ArrangementWire::deserialize(deserializer)?;
        let mut a = Arrangement { nodes: HashMap::new(), root: 0, next_id: 0, aliases: HashMap::new() };
        a.root = a.adopt(wire.root, None);
        a.aliases = wire.aliases.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect();
        a.next_id = a.nodes.keys().max().copied().unwrap_or(0) + 1;
        if a.desktop_in(a.root).is_none() {
            // A tree without a desktop is invalid; give it one on the right.
            let id = a.make_id();
            a.nodes.insert(id, Node::new(id, Kind::Desktop));
            let root = a.root;
            a.insert(id, root, Edge::Right, None);
        }
        Ok(a)
    }
}
