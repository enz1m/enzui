//! Horizontal scrolling strips with hidden scrollbar chrome by default.
//!
//! Uses [`crate::scroll_view::EnzScrollView`] with a preferred 100 px height,
//! fill width and horizontal-only scrolling. Parent constraints can limit its
//! height. Callbacks, programmatic operations and translation publication use
//! the shared scroll view. [`strip_overflow`] reads live, laid-out geometry.

//! Assign an ID to target scroll operations from the host.

use crate::scroll_view::{Axis, QueryScrollGeometry, enz_scroll_view};
use enzui_core::metrics::FiniteNonNeg;
use iced::widget::Id;
use iced::widget::scrollable::Viewport;
use iced::{Element, Length};

/// Default fixed strip height in logical pixels.
pub const DEFAULT_STRIP_HEIGHT: f32 = 100.0;

/// Create a horizontal strip with default geometry and hidden scrollbar chrome.
pub fn enz_horizontal_scroll_view<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> EnzHorizontalScrollView<'a, Message>
where
    Message: 'a,
{
    EnzHorizontalScrollView {
        child: child.into(),
        strip_height: DEFAULT_STRIP_HEIGHT,
        width: Length::Fill,
        id: None,
        on_scroll: None,
        show_scrollbar: false,
        publish_translation: None,
    }
}

/// Builder returned by [`enz_horizontal_scroll_view`].
#[must_use]
pub struct EnzHorizontalScrollView<'a, Message> {
    child: Element<'a, Message>,
    strip_height: f32,
    width: Length,
    id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    show_scrollbar: bool,
    publish_translation: Option<crate::scroll_view::ScrollTranslation>,
}

impl<'a, Message> EnzHorizontalScrollView<'a, Message>
where
    Message: 'a,
{
    /// Publish live translation into the shared cell.
    /// See [`crate::scroll_view::EnzScrollView::publish_translation`] for observation timing.
    pub fn publish_translation(mut self, cell: crate::scroll_view::ScrollTranslation) -> Self {
        self.publish_translation = Some(cell);
        self
    }
    /// Set preferred strip height in logical pixels, subject to parent constraints.
    /// Non-finite or negative input resets it to 100 px; zero is accepted.
    pub fn strip_height(mut self, h: f32) -> Self {
        self.strip_height = FiniteNonNeg::new_or(h, DEFAULT_STRIP_HEIGHT).get();
        self
    }

    /// Set strip width; fills its parent by default.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Assign an identity for targeted scrolling and geometry queries.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Observe scroll offset changes (see
    /// [`EnzScrollView::on_scroll`](crate::scroll_view::EnzScrollView::on_scroll)).
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Set scrollbar visibility; hidden by default.
    pub fn show_scrollbar(mut self, show: bool) -> Self {
        self.show_scrollbar = show;
        self
    }
}

impl<'a, Message> EnzHorizontalScrollView<'a, Message>
where
    Message: 'a,
{
    /// Build the horizontal strip as an iced element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzHorizontalScrollView<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzHorizontalScrollView<'a, Message>) -> Self {
        let mut sv = enz_scroll_view(w.child)
            .axis(Axis::Horizontal)
            .width(w.width)
            .height(Length::Fixed(w.strip_height))
            .hide_scrollbar(!w.show_scrollbar);
        if let Some(id) = w.id {
            sv = sv.id(id);
        }
        if let Some(cb) = w.on_scroll {
            sv = sv.on_scroll(cb);
        }
        if let Some(cell) = w.publish_translation {
            sv = sv.publish_translation(cell);
        }
        sv.into()
    }
}

/// Query viewport and content geometry for the strip with this ID.
/// Run the operation against a laid-out tree before reading its result.
/// The result remains absent when no matching strip is visited;
/// [`crate::scroll_view::ScrollGeometry::is_overflowing_x`] compares their widths.
pub fn strip_overflow(id: Id) -> QueryScrollGeometry {
    QueryScrollGeometry::new(id)
}
