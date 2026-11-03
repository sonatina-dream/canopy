use crate::arrangement::{Arrangement, NodeId, WindowId};
use crate::geometry::{Edge, Point, Rect};
use crate::navigator::Navigator;

/// What letting go of a dragged window would do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// Dock beside everything at a screen edge.
    ScreenEdge(Edge),
    /// Dock beside a node on one of its edges.
    Beside(NodeId, Edge),
    /// Join a tile as a tab.
    Tab(NodeId),
    /// Join a floating window, which docks too: it goes beside `beside` on
    /// `side`, and the dragged window beside it on `edge`, or into it as a tab
    /// when there is no `edge`.
    Floating { window: WindowId, beside: NodeId, side: Edge, edge: Option<Edge> },
    /// Float on the Desktop.
    Float,
    /// No change: a floating window stays put, a docked one snaps back.
    None,
}

impl DropTarget {
    /// Whether dropping here docks the window (rather than floating it or
    /// leaving it alone).
    pub fn docks(self) -> bool {
        !matches!(self, DropTarget::None | DropTarget::Float)
    }

    /// A screen edge is the edge of what's in view: a window docked there goes
    /// beside the framed node, or the near end of a framed run. This is how a
    /// window gets into a space from the inside.
    pub fn in_view(self, window: WindowId, whole_tile: bool, nav: &Navigator, a: &Arrangement) -> DropTarget {
        let DropTarget::ScreenEdge(edge) = self else { return self };
        let framed = nav.framed_node(a);
        // A window that is the whole view has nothing there to dock beside.
        if whole_tile && a.tile_of(window) == Some(framed) {
            return self;
        }
        let beside = match nav.framed_range(a) {
            Some(run) if a.get(framed).axis == edge.axis() => {
                if edge.is_leading() {
                    run[0]
                } else {
                    run[run.len() - 1]
                }
            }
            _ => framed,
        };
        if beside == a.root() {
            self
        } else {
            DropTarget::Beside(beside, edge)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Edge(Edge),
    Centre,
}

#[derive(Clone, Copy, Debug)]
pub struct DropResolver {
    /// Outer share of a tile that counts as its edge, at most `max_tile_edge`
    /// points: on a big window most of it means "make a tab".
    pub tile_edge_fraction: f64,
    pub max_tile_edge: f64,
    /// Fraction of the screen's shorter side used as the screen-edge band.
    pub screen_edge_fraction: f64,
    pub min_screen_edge_band: f64,
    /// Distance from an inner desktop edge that docks beside the desktop.
    pub desktop_edge_band: f64,
}

impl Default for DropResolver {
    fn default() -> Self {
        Self {
            tile_edge_fraction: 0.22,
            max_tile_edge: 200.0,
            screen_edge_fraction: 0.05,
            min_screen_edge_band: 24.0,
            desktop_edge_band: 40.0,
        }
    }
}

impl DropResolver {
    pub fn screen_edge_band(&self, screen: &Rect) -> f64 {
        self.min_screen_edge_band.max(self.screen_edge_fraction * screen.width.min(screen.height))
    }

    /// - `pointer`: the cursor, in screen coordinates.
    /// - `screen`: the display's visible frame.
    /// - `display`: the display's whole frame. The menu bar and the Dock lie
    ///   outside the visible frame; a pointer pushed into them is at that edge.
    /// - `tiles`: rects of tiles that may be dropped on (exclude the dragged tile).
    /// - `desktop`: the desktop's id and rect, when it is visible.
    /// - `held`: the tile id the pointer has rested on in its centre long enough.
    pub fn resolve(
        &self,
        pointer: Point,
        screen: Rect,
        display: Option<Rect>,
        tiles: &[(NodeId, Rect)],
        desktop: Option<(NodeId, Rect)>,
        held: Option<NodeId>,
    ) -> DropTarget {
        if !display.unwrap_or(screen).contains(pointer) && !screen.contains(pointer) {
            return DropTarget::None;
        }

        let band = self.screen_edge_band(&screen);
        // Negative when the pointer is past the visible frame (menu bar, Dock).
        let distances = [
            (Edge::Left, pointer.x - screen.min_x()),
            (Edge::Right, screen.max_x() - pointer.x),
            (Edge::Top, pointer.y - screen.min_y()),
            (Edge::Bottom, screen.max_y() - pointer.y),
        ];
        if let Some(&(edge, distance)) = nearest(&distances) {
            if distance < band {
                return DropTarget::ScreenEdge(edge);
            }
        }

        if let Some(&(id, r)) = tiles.iter().find(|(_, r)| r.contains(pointer)) {
            return match self.zone(pointer, r) {
                Zone::Edge(e) => DropTarget::Beside(id, e),
                Zone::Centre if held == Some(id) => DropTarget::Tab(id),
                Zone::Centre => DropTarget::None,
            };
        }

        if let Some((id, r)) = desktop.filter(|(_, r)| r.contains(pointer)) {
            // Inner edges of the desktop border docked tiles; dock beside it there.
            let inner: Vec<(Edge, f64)> = [
                (Edge::Left, pointer.x - r.min_x()),
                (Edge::Right, r.max_x() - pointer.x),
                (Edge::Top, pointer.y - r.min_y()),
                (Edge::Bottom, r.max_y() - pointer.y),
            ]
            .into_iter()
            .filter(|&(edge, _)| !touches_screen(&r, edge, &screen, band + 16.0))
            .collect();
            if let Some(&(edge, distance)) = nearest(&inner) {
                if distance < self.desktop_edge_band {
                    return DropTarget::Beside(id, edge);
                }
            }
            return DropTarget::Float;
        }
        DropTarget::None
    }

    /// A drop on floating window `window` at `r`, which the pointer has paused
    /// over. It docks along the longer side of `home`, on the side it is
    /// nearer. The dragged window joins it by the same zones as a tile.
    pub fn over_floating(&self, pointer: Point, window: WindowId, r: Rect, home: (NodeId, Rect)) -> DropTarget {
        let (beside, area) = home;
        let side = if area.width >= area.height {
            if r.mid_x() <= area.mid_x() {
                Edge::Left
            } else {
                Edge::Right
            }
        } else if r.mid_y() <= area.mid_y() {
            Edge::Top
        } else {
            Edge::Bottom
        };
        let edge = match self.zone(pointer, r) {
            Zone::Edge(e) => Some(e),
            Zone::Centre => None,
        };
        DropTarget::Floating { window, beside, side, edge }
    }

    /// Which part of a tile the pointer is over.
    pub fn zone(&self, p: Point, r: Rect) -> Zone {
        if r.width <= 0.0 || r.height <= 0.0 {
            return Zone::Centre;
        }
        let band_x = (self.tile_edge_fraction * r.width).min(self.max_tile_edge);
        let band_y = (self.tile_edge_fraction * r.height).min(self.max_tile_edge);
        // How far into each edge band the pointer is, 0 at the edge, 1 at its inner side.
        let candidates: Vec<(Edge, f64)> = [
            (Edge::Left, (p.x - r.min_x()) / band_x),
            (Edge::Right, (r.max_x() - p.x) / band_x),
            (Edge::Top, (p.y - r.min_y()) / band_y),
            (Edge::Bottom, (r.max_y() - p.y) / band_y),
        ]
        .into_iter()
        .filter(|&(_, depth)| depth < 1.0)
        .collect();
        match nearest(&candidates) {
            Some(&(edge, _)) => Zone::Edge(edge),
            None => Zone::Centre,
        }
    }
}

/// The smallest entry, the first of equals.
fn nearest(list: &[(Edge, f64)]) -> Option<&(Edge, f64)> {
    list.iter().reduce(|best, e| if e.1 < best.1 { e } else { best })
}

fn touches_screen(r: &Rect, edge: Edge, screen: &Rect, slack: f64) -> bool {
    match edge {
        Edge::Left => r.min_x() - screen.min_x() <= slack,
        Edge::Right => screen.max_x() - r.max_x() <= slack,
        Edge::Top => r.min_y() - screen.min_y() <= slack,
        Edge::Bottom => screen.max_y() - r.max_y() <= slack,
    }
}

impl Arrangement {
    /// Applies a drop of `window` (or its whole tile when `whole_tile`).
    /// Returns false when the target means "no change" or "float".
    pub fn apply(&mut self, target: DropTarget, window: WindowId, whole_tile: bool) -> bool {
        let moving = if whole_tile { self.tile_of(window) } else { None };
        match target {
            DropTarget::None | DropTarget::Float => false,
            DropTarget::ScreenEdge(edge) => {
                match moving {
                    Some(t) => self.move_node(t, self.root(), edge, None),
                    None => {
                        self.dock_at_screen_edge(window, edge);
                    }
                }
                true
            }
            DropTarget::Beside(id, edge) => {
                let Some(n) = self.node(id) else { return false };
                match moving {
                    Some(t) => self.move_node(t, n, edge, None),
                    None => {
                        self.dock(window, n, edge, None);
                    }
                }
                true
            }
            DropTarget::Tab(id) => {
                let Some(n) = self.node(id).filter(|&n| self.get(n).is_tile()) else { return false };
                match moving {
                    Some(t) if t != n => self.merge(t, n),
                    _ => self.add_tab(window, n, None),
                }
                true
            }
            DropTarget::Floating { window: other, beside, side, edge } => {
                let Some(n) = self.node(beside) else { return false };
                let tile = self.dock(other, n, side, None);
                let join = match edge {
                    Some(e) => DropTarget::Beside(tile, e),
                    None => DropTarget::Tab(tile),
                };
                self.apply(join, window, whole_tile)
            }
        }
    }
}
