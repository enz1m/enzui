//! Day or week time grids with overlapping events and caller-owned model updates.
//!
//! Supply preformatted columns and uniquely identified blocks; one column makes
//! a day view and seven make a week. All-day spans occupy one band rectangle,
//! while timed spans split at midnight and retain one accessible event identity.
//! The application applies drag proposals and selection to its own model; the
//! widget retains scrolling and the live gesture. Current time is caller-supplied.
//!
//! Empty-grid creation crosses a 2 logical-pixel deadband; moving and resizing
//! existing events use 4 pixels. Creation paints its own preview, while the host
//! owns previews for move and resize proposals. Creation's minimum duration is
//! limited by the visible minute interval.
//!
//! The widget publishes a named group with event children, but registers no Tab
//! focus stop. Enter activates exactly one selected block when no drag is live;
//! the host must scope key delivery if other controls can receive Enter.

mod geometry;
mod tokens;
mod widget;

pub use geometry::*;
pub use tokens::*;
pub use widget::*;
