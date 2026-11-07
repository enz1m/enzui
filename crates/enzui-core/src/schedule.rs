//! Renderer-neutral layout, interaction, and accessibility primitives for day and week schedules.
//!
//! Events use caller-defined minute offsets and stable IDs. Timed events are laid out per column with half-open overlap semantics, while all-day events use a separate lane. Drag and keyboard operations return proposals without mutating the caller’s model.

mod all_day;
mod axis;
mod drag;
mod event;
mod format;
mod keyboard;
mod overlap;
mod semantics;

pub use all_day::*;
pub use axis::*;
pub use drag::*;
pub use event::*;
pub use format::*;
pub use keyboard::*;
pub use overlap::*;
pub use semantics::*;
