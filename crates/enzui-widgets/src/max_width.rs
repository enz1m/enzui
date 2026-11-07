//! Limit the maximum width offered to a child without adding a paint clip.
//!
//! A finite, nonnegative cap is stored in logical pixels; zero is a real zero
//! width. Negative or non-finite caps disable the limit. The maximum is the
//! smaller of the cap and the parent's maximum, and the incoming minimum is
//! lowered when necessary. Height and compression flags are preserved.
//!
//! The wrapper reports `Shrink` width, but the child keeps its own sizing policy:
//! a fill child can occupy the whole capped maximum. Children must respect their
//! limits; drawing, events, operations and overlays are forwarded unchanged.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A transparent child wrapper that lowers the offered maximum width.
pub struct MaxWidth<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    cap: f32,
}

/// Offer at most `cap` logical pixels of width to a child.
/// Zero is retained; negative and non-finite caps become an unbounded cap.
/// This changes layout limits without clipping child paint or interaction.
pub fn max_width<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    cap: f32,
) -> MaxWidth<'a, Message, Theme, Renderer> {
    let cap = if cap.is_finite() && cap >= 0.0 {
        cap
    } else {
        f32::INFINITY
    };
    MaxWidth {
        child: child.into(),
        cap,
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for MaxWidth<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
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
        let child_size = self.child.as_widget().size();
        Size {
            width: Length::Shrink,
            height: child_size.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let max = limits.max();
        let new_max_w = self.cap.min(max.width);
        let min = limits.min();
        let capped = layout::Limits::with_compression(
            Size::new(min.width.min(new_max_w), min.height),
            Size::new(new_max_w, max.height),
            limits.compression(),
        );
        self.child.as_widget_mut().layout(tree, renderer, &capped);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<MaxWidth<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: MaxWidth<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
