//! Movement-gated wheel routing for caller-owned scroll offsets.
//!
//! Unlike `iced::widget::mouse_area::on_scroll`, this transparent wrapper only
//! captures when its callback returns a message. Callers compute that decision
//! from their current offset and extent, allowing a nested scrollable ancestor
//! to receive wheel input at an endpoint.

use iced::advanced::{Widget, layout, mouse, overlay, renderer, widget as widget_advanced};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(crate) fn offset_can_move(offset: f32, max_offset: f32, wheel_delta: f32) -> bool {
    let max_offset = max_offset.max(0.0);
    let current = offset.clamp(0.0, max_offset);
    let next = (current - wheel_delta).clamp(0.0, max_offset);
    (next - current).abs() > f32::EPSILON
}

pub(crate) fn wheel_capture<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
    on_scroll: impl Fn(mouse::ScrollDelta) -> Option<Message> + 'a,
) -> Element<'a, Message> {
    Element::new(WheelCapture {
        child: child.into(),
        on_scroll: Box::new(on_scroll),
    })
}

struct WheelCapture<'a, Message> {
    child: Element<'a, Message>,
    on_scroll: Box<dyn Fn(mouse::ScrollDelta) -> Option<Message> + 'a>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for WheelCapture<'_, Message> {
    fn tag(&self) -> widget_advanced::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> widget_advanced::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut widget_advanced::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget_advanced::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn draw(
        &self,
        tree: &widget_advanced::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &widget_advanced::Tree,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut widget_advanced::Tree,
        layout: layout::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget_advanced::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut widget_advanced::Tree,
        layout: layout::Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }

    fn frame_interest(&self, tree: &widget_advanced::Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn update(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Children get first refusal so nested editors retain their own wheel routing.
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        if shell.is_event_captured() {
            return;
        }

        let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event else {
            return;
        };
        if !cursor.is_over(layout.bounds()) {
            return;
        }

        if let Some(message) = (self.on_scroll)(*delta) {
            shell.publish(message);
            shell.capture_event();
        }
    }
}
