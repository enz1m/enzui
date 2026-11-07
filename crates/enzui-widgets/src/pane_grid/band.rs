//! Keep a press on a splitter's grab band from reaching the pane under it.
//!
//! The native grid hands every event to its panes before it looks for a
//! splitter under the pointer, and the grab band reaches [`super::HANDLE_SLOP`]
//! into the panes on both sides. A press that starts a resize therefore also
//! reached the pane, which took it as a click: a seam drag focused the pane
//! beside it, or started a text selection there. [`SplitterBand`] wraps each
//! pane body and holds back a primary press inside the band along the
//! pane's interior edges; the grid still sees it and starts the resize.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector, touch};

/// Which edges of a pane border another pane (and so carry a splitter).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Interior {
    pub(super) left: bool,
    pub(super) right: bool,
    pub(super) top: bool,
    pub(super) bottom: bool,
}

impl Interior {
    /// The edges of `region` that lie inside a grid of `size`.
    pub(super) fn of(region: Rectangle, size: Size) -> Self {
        const EPSILON: f32 = 0.5;
        Self {
            left: region.x > EPSILON,
            top: region.y > EPSILON,
            right: region.x + region.width < size.width - EPSILON,
            bottom: region.y + region.height < size.height - EPSILON,
        }
    }

    /// Whether `point` lies within `band` of an interior edge of `bounds`.
    pub(super) fn grabs(self, bounds: Rectangle, band: f32, point: iced::Point) -> bool {
        (self.left && point.x < bounds.x + band)
            || (self.right && point.x > bounds.x + bounds.width - band)
            || (self.top && point.y < bounds.y + band)
            || (self.bottom && point.y > bounds.y + bounds.height - band)
    }
}

/// A pane body that does not receive presses on its splitters' grab bands.
pub(super) struct SplitterBand<'a, Message> {
    child: Element<'a, Message>,
    interior: Interior,
    band: f32,
}

impl<'a, Message> SplitterBand<'a, Message> {
    pub(super) fn new(child: Element<'a, Message>, interior: Interior, band: f32) -> Self {
        Self {
            child,
            interior,
            band: band.max(0.0),
        }
    }

    fn on_band(&self, event: &Event, layout: Layout, cursor: mouse::Cursor) -> bool {
        let point = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => cursor.position(),
            Event::Touch(touch::Event::FingerPressed { position, .. }) => Some(*position),
            _ => None,
        };
        point.is_some_and(|point| {
            let bounds = layout.bounds();
            bounds.contains(point) && self.interior.grabs(bounds, self.band, point)
        })
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SplitterBand<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.on_band(event, layout, cursor) {
            return;
        }
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    // This wrapper shares the child tree node, so frame interest must remain transparent.
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: 'a> From<SplitterBand<'a, Message>> for Element<'a, Message> {
    fn from(band: SplitterBand<'a, Message>) -> Self {
        Element::new(band)
    }
}
