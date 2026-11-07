//! Population gate over every raw `scrollable(..)` construction in this crate.
//!
//! `enz_scroll_view` defines the framework's scrollbar policy — a 5 px painted
//! thumb inside a 15 px proximity band, hidden at
//! rest, revealed only near the trailing edge, never reserving a layout lane
//! (`scroll_view.rs`). Any widget
//! that builds a raw `iced::widget::scrollable(..)` (or `Scrollable::new(..)`,
//! or a bare `scrollable::Scrollbar::default()` direction) instead of routing
//! through the shim [`enz_scrollable`](crate::scroll_view::enz_scrollable) is a
//! chance for the policy to silently disagree with itself inside one window —
//! otherwise the search popup, combo-box popup, chat list, tab strip, and
//! shortcut list can silently diverge.
//!
//! # What the gate enforces
//!
//! - Every scrollable-construction site in this crate's sources and examples
//!   (the examples are what apps copy) routes through [`enz_scrollable`],
//!   [`enz_scroll_view`](crate::scroll_view::enz_scroll_view) or
//!   [`overlay_scrollbar_with_tokens`](crate::scroll_view::overlay_scrollbar_with_tokens),
//!   or is one of the named exceptions below.
//! - Every exception must still construct a raw scrollable, so the list
//!   cannot outlive the reason it exists.
//! - The scan has a floor: finding no source files is a scanner bug, not a
//!   clean crate.
//!
//! # Runtime gate rather than compile-time
//!
//! The `iced::widget::scrollable` function is public and always will be — this
//! crate cannot make it uncallable and does not want to (widgets that fully
//! opt out of the policy, like [`line_number_gutter`](crate::line_number_gutter),
//! still need to build a `Scrollable`). The gate is what turns "please use
//! `enz_scrollable`" into a check that can fail.
//!
//! [`overlay_scrollbar`]: crate::scroll_view::overlay_scrollbar
