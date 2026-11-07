//! EnzCenter / EnzVCenter / EnzHCenter: thin axis-specific
//! centring recipes.
//!
//! # These are convenience recipes, not new widgets
//!
//! `container` already ships `center_x(len)` and `center_y(len)`.
//! These three functions are *thin named aliases* over the native
//! container configuration, kept so consumers can reach for a named
//! layout component. Each returns a plain [`container::Container`]
//! that keeps chaining; prefer the native container recipe
//! directly at call sites that are not reaching for the name:
//!
//! | recipe | native container |
//! |---|---|
//! | [`enz_center`] (both axes) | `container(c).center(Length::Fill)` |
//! | [`enz_vcenter`] (v only) | `container(c).width(Shrink).center_y(Fill)` |
//! | [`enz_hcenter`] (h only) | `container(c).height(Shrink).center_x(Fill)` |
//!
//! `enz_vcenter` / `enz_hcenter` carry the most weight: they are
//! the per-child cross-axis centring cells for a mixed-size
//! `row!` / `column!` (sit a shorter child on the row centre line
//! / a narrower child on the column centre line). `enz_center` is
//! a shorthand for `container.center(Fill)` that returns a
//! chainable [`container::Container`], so callers can
//! `.width(...)`, `.height(...)`, `.padding(...)`, `.style(...)`,
//! `.id(...)` on the result.

use iced::widget::container;
use iced::{Element, Length};

// ---------------------------------------------------------------------------
// EnzCenter — both-axis centre.
// ---------------------------------------------------------------------------

/// Wrap `child` in a container that centres it on both axes. The
/// container fills its parent slot on both axes.
pub fn enz_center<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> container::Container<'a, Message>
where
    Message: 'a,
{
    container(child)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
}

// ---------------------------------------------------------------------------
// EnzVCenter — vertical-only centre (per-child valign primitive).
// ---------------------------------------------------------------------------

/// Wrap `child` in a container that centres it on the vertical
/// axis only, leaving horizontal alone. The container fills its
/// parent slot vertically and shrink-wraps horizontally.
pub fn enz_vcenter<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> container::Container<'a, Message>
where
    Message: 'a,
{
    container(child)
        .width(Length::Shrink)
        .height(Length::Fill)
        .center_y(Length::Fill)
}

// ---------------------------------------------------------------------------
// EnzHCenter — horizontal-only centre (per-child halign primitive).
// ---------------------------------------------------------------------------

/// Wrap `child` in a container that centres it on the horizontal
/// axis only, leaving vertical alone. The container fills its
/// parent slot horizontally and shrink-wraps vertically.
pub fn enz_hcenter<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> container::Container<'a, Message>
where
    Message: 'a,
{
    container(child)
        .width(Length::Fill)
        .height(Length::Shrink)
        .center_x(Length::Fill)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
