//! Canopy's platform-neutral model: the layout tree, solver, drop resolution,
//! camera and history, URL commands and persistence. Nothing here touches the
//! operating system, so a port to another platform could reuse all of it.

mod arrangement;
mod commands;
mod drop_resolver;
mod geometry;
mod navigator;
mod persistence;
mod planner;
pub mod shortcuts;
mod solver;

pub use arrangement::{Arrangement, Kind, Node, NodeId, RestoreTarget, SeamLimits, WindowId};
pub use commands::{Command, SettingsPane};
pub use drop_resolver::{DropResolver, DropTarget, Zone};
pub use geometry::{AffineMap, Axis, Direction, Edge, Point, Rect, Size};
pub use navigator::{Navigator, Visit, SAVED_VIEW_SLOTS};
pub use persistence::{DisplayState, Preferences, SavedState, WindowMatcher, WindowRef};
pub use planner::{Plan, Planner, StackedTab};
pub use shortcuts::{KeyCombo, Shortcuts};
pub use solver::{Layout, Seam, Solver};
