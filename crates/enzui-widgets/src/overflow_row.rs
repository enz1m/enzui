//! Horizontal scrolling content between leading and trailing edge slots.
//!
//! Edges remain outside the scrolling band. The higher-priority edge receives
//! the available width first, after reserving gaps; the other edge receives
//! the remainder, and the middle band receives what remains after both edges.
//! Trailing wins by default and on equal priorities. These are layout limits;
//! the row does not add a separate paint clip around each edge child.
//!
//! Observe the horizontal offset through [`EnzOverflowRow::on_scroll`] using
//! `viewport.absolute_offset().x`. Attach [`EnzOverflowRow::content_id`] to drive
//! `iced::widget::operation::scroll_to` or `scroll_by`. [`reveal_offset`] and
//! [`reveal_index_offset`] compute targets from caller-supplied item geometry;
//! the row does not measure individual scrolling items for reveal.
//!
//! Omit an edge with [`EnzOverflowRow::leading`] or [`EnzOverflowRow::trailing`]
//! set to `None` to remove its gap as well. A present zero-sized child still
//! reserves a gap. [`enz_overflow_row_content_only`] omits both edges.

use enzui_core::metrics::FiniteNonNeg;
use enzui_theme::{SpaceToken, SpacingScale};
use iced::widget::Id;
use iced::widget::scrollable::Viewport;
use iced::{Element, Length};

/// Default gap token, resolved against [`SpacingScale::default`] at construction.
///
/// Runtime theme changes do not alter this gap; override it with
/// [`EnzOverflowRow::spacing`].
pub const DEFAULT_OVERFLOW_ROW_SPACING_TOKEN: SpaceToken = SpaceToken::Xs;

fn default_spacing() -> f32 {
    DEFAULT_OVERFLOW_ROW_SPACING_TOKEN.resolve(&SpacingScale::default())
}

/// Build a scrolling middle band with two edge slots.
///
/// A zero-sized edge still reserves its gap. Use [`EnzOverflowRow::leading`]
/// or [`EnzOverflowRow::trailing`] with `None` to omit an edge entirely.
pub fn enz_overflow_row<'a, Message>(
    leading: impl Into<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
    trailing: impl Into<Element<'a, Message>>,
) -> EnzOverflowRow<'a, Message>
where
    Message: 'a,
{
    EnzOverflowRow {
        leading: Some(leading.into()),
        content: content.into(),
        trailing: Some(trailing.into()),
        spacing: default_spacing(),
        height: Length::Shrink,
        content_id: None,
        on_scroll: None,
        leading_priority: DEFAULT_LEADING_PRIORITY,
        trailing_priority: DEFAULT_TRAILING_PRIORITY,
    }
}

/// Variant that starts with no leading / trailing slots (both empty).
pub fn enz_overflow_row_content_only<'a, Message>(
    content: impl Into<Element<'a, Message>>,
) -> EnzOverflowRow<'a, Message>
where
    Message: 'a,
{
    EnzOverflowRow {
        leading: None,
        content: content.into(),
        trailing: None,
        spacing: default_spacing(),
        height: Length::Shrink,
        content_id: None,
        on_scroll: None,
        leading_priority: DEFAULT_LEADING_PRIORITY,
        trailing_priority: DEFAULT_TRAILING_PRIORITY,
    }
}

/// Default leading priority (0), below [`DEFAULT_TRAILING_PRIORITY`].
pub const DEFAULT_LEADING_PRIORITY: i32 = 0;

/// Default trailing priority (1), reserving trailing width before leading width.
pub const DEFAULT_TRAILING_PRIORITY: i32 = 1;

const _: () = assert!(
    DEFAULT_TRAILING_PRIORITY > DEFAULT_LEADING_PRIORITY,
    "the default trailing priority MUST be greater than the leading \
     default so existing callers retain byte-identical layout"
);

/// Builder returned by [`enz_overflow_row`].
#[must_use]
pub struct EnzOverflowRow<'a, Message> {
    leading: Option<Element<'a, Message>>,
    content: Element<'a, Message>,
    trailing: Option<Element<'a, Message>>,
    spacing: f32,
    height: Length,
    content_id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    leading_priority: i32,
    trailing_priority: i32,
}

impl<'a, Message> EnzOverflowRow<'a, Message>
where
    Message: 'a,
{
    /// Set inter-slot spacing in logical pixels (default 4 from the default ladder).
    ///
    /// Non-finite or negative input uses the default; zero is allowed.
    pub fn spacing(mut self, px: f32) -> Self {
        self.spacing = FiniteNonNeg::new_or(px, default_spacing()).get();
        self
    }

    /// Override the row's height (default [`Length::Shrink`] — the row
    /// hugs its tallest child).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Attach a stable `iced::widget::Id` to the scrolling middle so
    /// apps can drive `iced::widget::operation::scroll_to` /
    /// `scroll_by` for tab-into-view behaviour.
    pub fn content_id(mut self, id: Id) -> Self {
        self.content_id = Some(id);
        self
    }

    /// Observe scroll notifications from the middle band.
    ///
    /// The callback receives a [`Viewport`]; `absolute_offset().x` is the
    /// horizontal offset. This forwards to [`EnzScrollView::on_scroll`](crate::scroll_view::EnzScrollView::on_scroll).
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Replace the leading slot (pass `None` to remove it).
    pub fn leading(mut self, slot: Option<Element<'a, Message>>) -> Self {
        self.leading = slot;
        self
    }

    /// Replace the trailing slot (pass `None` to remove it).
    pub fn trailing(mut self, slot: Option<Element<'a, Message>>) -> Self {
        self.trailing = slot;
        self
    }

    /// Set leading-edge reservation priority (default [`DEFAULT_LEADING_PRIORITY`]).
    ///
    /// Leading receives width first only when its priority exceeds trailing
    /// priority; trailing wins ties.
    pub fn leading_priority(mut self, p: i32) -> Self {
        self.leading_priority = p;
        self
    }

    /// Set trailing-edge reservation priority (default [`DEFAULT_TRAILING_PRIORITY`]).
    ///
    /// Trailing receives width first when its priority equals or exceeds leading priority.
    pub fn trailing_priority(mut self, p: i32) -> Self {
        self.trailing_priority = p;
        self
    }

    /// Replace the trailing slot with an intrinsic-width prefix of whole items.
    ///
    /// When items do not fit, omit the remaining suffix and reserve an ellipsis
    /// indicator, removing earlier items as needed to fit it. Omitted items are
    /// excluded from painting and interaction. The indicator retains its
    /// intrinsic width even when its allocation is narrower. An empty list
    /// removes the edge and its gap.
    ///
    /// `item_spacing` is the logical-pixel gap between items and before the indicator.
    pub fn trailing_atomic(mut self, items: Vec<Element<'a, Message>>, item_spacing: f32) -> Self {
        if items.is_empty() {
            self.trailing = None;
        } else {
            let row: Element<'a, Message> = Element::new(AtomicRow::build(items, item_spacing));
            self.trailing = Some(row);
        }
        self
    }

    /// Replace the leading slot with a list of atomic items — the
    /// leading-side counterpart of [`Self::trailing_atomic`]. See that
    /// method for the semantics.
    pub fn leading_atomic(mut self, items: Vec<Element<'a, Message>>, item_spacing: f32) -> Self {
        if items.is_empty() {
            self.leading = None;
        } else {
            let row: Element<'a, Message> = Element::new(AtomicRow::build(items, item_spacing));
            self.leading = Some(row);
        }
        self
    }
}

impl<'a, Message> EnzOverflowRow<'a, Message>
where
    Message: 'a,
{
    /// Build the scrolling band and edge slots.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

mod atomic_row;
mod reveal;
mod widget;

pub use reveal::{clamp_offset, reveal_index_offset, reveal_offset};

use atomic_row::AtomicRow;
