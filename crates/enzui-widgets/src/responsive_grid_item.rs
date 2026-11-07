//! EnzResponsiveGridItem: fixed-width, fixed-height cell host for a
//! responsive grid.
//!
//! It pins a fixed cell width and row height so width-dependent children stay
//! out of flex measurement. Callers pass the cell width in explicitly.
//!
//! # Prefer [`crate::responsive_grid`]
//!
//! This fixed-`Container` cell requires the app to compute the "responsive"
//! width itself. The parent
//! [`crate::responsive_grid::enz_responsive_grid`] provides the complete API
//! on top of
//! iced's native `iced::widget::responsive` primitive: it measures its own
//! width, picks the size-class column count, and computes each cell's even
//! width internally — so callers do not need this item at all. Prefer
//! `enz_responsive_grid(cells)`; feed it plain children (as builder closures)
//! and it owns the cell sizing.
//!
//! This item is kept only as a thin fixed-cell escape hatch for the rare case
//! where a caller *already* has a computed cell width (e.g. from its own
//! bespoke measurement) and wants the fixed-height cell host without the grid.

use iced::widget::container;
use iced::{Element, Length};

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default item height .
pub const DEFAULT_ITEM_HEIGHT: f32 = 120.0;

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_responsive_grid_item(child, cell_width)`.
pub fn enz_responsive_grid_item<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    cell_width: f32,
) -> EnzResponsiveGridItem<'a, Message>
where
    Message: 'a,
{
    EnzResponsiveGridItem {
        child: child.into(),
        cell_width: if cell_width.is_finite() {
            cell_width.max(0.0)
        } else {
            0.0
        },
        item_height: DEFAULT_ITEM_HEIGHT,
    }
}

/// Builder returned by [`enz_responsive_grid_item`].
#[must_use]
pub struct EnzResponsiveGridItem<'a, Message> {
    child: Element<'a, Message>,
    cell_width: f32,
    item_height: f32,
}

impl<'a, Message> EnzResponsiveGridItem<'a, Message>
where
    Message: 'a,
{
    /// Set the item height (default `120.0`). Non-finite values normalise to 0.
    pub fn item_height(mut self, h: f32) -> Self {
        self.item_height = if h.is_finite() { h.max(0.0) } else { 0.0 };
        self
    }

    /// Set the cell width on the builder (chainable). This is a build-time
    /// value like every other iced view property — the widget is rebuilt each
    /// frame from the app's `view`, so a caller computing widths from a resize
    /// message simply passes the new value on the next build. (Prefer
    /// [`crate::responsive_grid::enz_responsive_grid`], which measures the
    /// width itself and needs no external computation at all.)
    pub fn cell_width(mut self, w: f32) -> Self {
        self.cell_width = if w.is_finite() { w.max(0.0) } else { 0.0 };
        self
    }
}

impl<'a, Message> EnzResponsiveGridItem<'a, Message>
where
    Message: 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzResponsiveGridItem<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzResponsiveGridItem<'a, Message>) -> Self {
        container(w.child)
            .width(Length::Fixed(w.cell_width))
            .height(Length::Fixed(w.item_height))
            .into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
