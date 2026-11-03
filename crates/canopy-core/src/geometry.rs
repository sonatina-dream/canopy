use std::fmt;

use serde::{Deserialize, Serialize};

/// A point in top-left-origin screen coordinates (y grows downward), the same
/// space the Accessibility API uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Size = Size { width: 0.0, height: 0.0 };

    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in top-left-origin coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };

    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub fn min_x(&self) -> f64 {
        self.x
    }
    pub fn min_y(&self) -> f64 {
        self.y
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
    pub fn center(&self) -> Point {
        Point::new(self.mid_x(), self.mid_y())
    }
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
    pub fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.min_x() && p.x < self.max_x() && p.y >= self.min_y() && p.y < self.max_y()
    }

    pub fn contains_rect(&self, r: &Rect) -> bool {
        r.min_x() >= self.min_x()
            && r.max_x() <= self.max_x()
            && r.min_y() >= self.min_y()
            && r.max_y() <= self.max_y()
    }

    pub fn intersection(&self, r: &Rect) -> Rect {
        let (x0, y0) = (self.min_x().max(r.min_x()), self.min_y().max(r.min_y()));
        let (x1, y1) = (self.max_x().min(r.max_x()), self.max_y().min(r.max_y()));
        if x1 <= x0 || y1 <= y0 {
            return Rect::ZERO;
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn intersects(&self, r: &Rect) -> bool {
        !self.intersection(r).is_empty()
    }

    pub fn union(&self, r: &Rect) -> Rect {
        if self.is_empty() {
            return *r;
        }
        if r.is_empty() {
            return *self;
        }
        let (x0, y0) = (self.min_x().min(r.min_x()), self.min_y().min(r.min_y()));
        let (x1, y1) = (self.max_x().max(r.max_x()), self.max_y().max(r.max_y()));
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn inset_by(&self, d: f64) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.width - 2.0 * d).max(0.0),
            (self.height - 2.0 * d).max(0.0),
        )
    }

    /// The extent along an axis: width for horizontal, height for vertical.
    pub fn length(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Horizontal => self.width,
            Axis::Vertical => self.height,
        }
    }

    pub fn start(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        }
    }

    /// Linear interpolation between two rects, `t` in 0...1.
    pub fn lerp(a: &Rect, b: &Rect, t: f64) -> Rect {
        Rect::new(
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
            a.width + (b.width - a.width) * t,
            a.height + (b.height - a.height) * t,
        )
    }

    /// The rect moved so it lies inside `bounds` where possible.
    pub fn clamped(&self, bounds: &Rect) -> Rect {
        let mut r = *self;
        r.width = r.width.min(bounds.width);
        r.height = r.height.min(bounds.height);
        r.x = r.x.max(bounds.min_x()).min(bounds.max_x() - r.width);
        r.y = r.y.max(bounds.min_y()).min(bounds.max_y() - r.height);
        r
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}, {}, {}×{})",
            self.x as i64, self.y as i64, self.width as i64, self.height as i64
        )
    }
}

/// How a split lays out its children: `Horizontal` places them left to right,
/// `Vertical` top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    pub fn other(self) -> Axis {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
}

/// A side of a rectangle; also used for directions (left/right/up/down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    /// The split axis that docking on this edge creates.
    pub fn axis(self) -> Axis {
        match self {
            Edge::Left | Edge::Right => Axis::Horizontal,
            Edge::Top | Edge::Bottom => Axis::Vertical,
        }
    }

    /// Left and top come first in their split.
    pub fn is_leading(self) -> bool {
        matches!(self, Edge::Left | Edge::Top)
    }

    pub fn opposite(self) -> Edge {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }

    pub fn parse(s: &str) -> Option<Edge> {
        match s {
            "left" => Some(Edge::Left),
            "right" => Some(Edge::Right),
            "top" => Some(Edge::Top),
            "bottom" => Some(Edge::Bottom),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Right => "right",
            Edge::Top => "top",
            Edge::Bottom => "bottom",
        }
    }
}

/// Navigation direction for the neighbouring space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Direction::Left, Direction::Right, Direction::Up, Direction::Down];

    pub fn edge(self) -> Edge {
        match self {
            Direction::Left => Edge::Left,
            Direction::Right => Edge::Right,
            Direction::Up => Edge::Top,
            Direction::Down => Edge::Bottom,
        }
    }

    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "left" => Some(Direction::Left),
            "right" => Some(Direction::Right),
            "up" => Some(Direction::Up),
            "down" => Some(Direction::Down),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

/// Maps rect `from` onto rect `to` (independent x and y scale). Used to place
/// windows that lie outside the framed node during camera motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffineMap {
    pub sx: f64,
    pub sy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl AffineMap {
    pub fn new(from: &Rect, to: &Rect) -> Self {
        let sx = if from.width > 0.0 { to.width / from.width } else { 1.0 };
        let sy = if from.height > 0.0 { to.height / from.height } else { 1.0 };
        Self { sx, sy, tx: to.x - from.x * sx, ty: to.y - from.y * sy }
    }

    pub fn apply(&self, r: &Rect) -> Rect {
        Rect::new(
            r.x * self.sx + self.tx,
            r.y * self.sy + self.ty,
            r.width * self.sx,
            r.height * self.sy,
        )
    }

    pub fn apply_point(&self, p: Point) -> Point {
        Point::new(p.x * self.sx + self.tx, p.y * self.sy + self.ty)
    }
}
