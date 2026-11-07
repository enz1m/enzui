//! EnzFlowItem: fixed-height child for a flow / wrap layout.
//!
//! The primitive is a tiny wrapper that pins the child's height
//! ([`DEFAULT_ITEM_HEIGHT`] px by default) so a wrapping row of
//! variable-content cards stays on a single row rhythm. The same effect comes
//! from `container.height(Length::Fixed)`; the wrapper exists for
//! discoverability.
//!
//! The container does _not_ clip oversized children. Content taller than
//! `item_height` will overflow visually.
//!
//! # Deferred
//!
//! - Per-axis flex properties (flex-grow / flex-basis /
//!   flex-shrink / align-self / order): iced's `Row::wrap()` doesn't expose
//!   per-child flex flags. Fixed height is the primary use.

use iced::widget::container;
use iced::{Element, Length};

/// Default item height in px, `EnzFlowItem` default.
pub const DEFAULT_ITEM_HEIGHT: f32 = 72.0;

/// Builder for a fixed-height flow item (a sized wrapper for a flow child).
/// See [`enz_flow_item`].
#[must_use]
pub struct EnzFlowItem<'a, Message> {
    child: Element<'a, Message>,
    item_height: f32,
    width: Length,
}

/// Wrap `child` as a flow item. Defaults: [`DEFAULT_ITEM_HEIGHT`] px fixed
/// height, shrink-to-content width.
pub fn enz_flow_item<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
) -> EnzFlowItem<'a, Message> {
    EnzFlowItem {
        child: child.into(),
        item_height: DEFAULT_ITEM_HEIGHT,
        width: Length::Shrink,
    }
}

impl<'a, Message: 'a> EnzFlowItem<'a, Message> {
    /// Fixed item height in px. Non-finite or non-positive input falls back
    /// to [`DEFAULT_ITEM_HEIGHT`].
    pub fn item_height(mut self, h: f32) -> Self {
        self.item_height =
            enzui_core::metrics::FinitePositive::new_or(h, DEFAULT_ITEM_HEIGHT).get();
        self
    }

    /// Item width. Defaults to [`Length::Shrink`] (hug content).
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// The configured fixed height in px.
    pub fn item_height_value(&self) -> f32 {
        self.item_height
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        container(self.child)
            .height(Length::Fixed(self.item_height))
            .width(self.width)
            .into()
    }
}

impl<'a, Message: 'a> From<EnzFlowItem<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzFlowItem<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
