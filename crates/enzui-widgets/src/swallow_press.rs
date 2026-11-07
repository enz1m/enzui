//! Child-first shielding of unhandled pointer presses and releases inside a surface.
//!
//! Each event is hit-tested independently against the child's layout bounds.
//! Touch uses the event's finger position; mouse uses the available cursor.
//! This wrapper does not track gesture ownership or swallow releases outside
//! its bounds. Lower layers must track their own completed-click admission.
//! Keyboard, movement and wheel events remain available to lower layers.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Whether the event presses any mouse button or a touch finger.
#[must_use]
pub fn is_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonPressed(_))
            | Event::Touch(iced::touch::Event::FingerPressed { .. })
    )
}

/// Whether the event releases any mouse button or lifts a touch finger.
/// Finger loss is a cancellation, not a release.
#[must_use]
pub fn is_release(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonReleased(_))
            | Event::Touch(iced::touch::Event::FingerLifted { .. })
    )
}

/// Return a touch press's finger position, otherwise the available mouse cursor.
/// Use with [`is_press`]; other touch events also fall back to the cursor.
#[must_use]
pub fn press_position(event: &Event, cursor: mouse::Cursor) -> Option<iced::Point> {
    match event {
        Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => Some(*position),
        _ => cursor.position(),
    }
}

#[must_use]
fn is_over_for_pointer(event: &Event, cursor: mouse::Cursor, bounds: Rectangle) -> bool {
    let point = match event {
        Event::Touch(iced::touch::Event::FingerLifted { position, .. }) => Some(*position),
        _ => press_position(event, cursor),
    };
    point.is_some_and(|point| bounds.contains(point))
}

/// Forward to the child, then capture unhandled pointer presses and releases
/// whose current position lies inside the child's layout bounds.
pub struct SwallowPress<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    /// The wrapped child.
    pub child: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> SwallowPress<'a, Message, Theme, Renderer> {
    /// Wrap `child`.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for SwallowPress<'_, Message, Theme, Renderer>
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
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
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

        if shell.is_event_captured() {
            return;
        }
        if (is_press(event) || is_release(event))
            && is_over_for_pointer(event, cursor, layout.bounds())
        {
            shell.capture_event();
        }
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

impl<'a, Message, Theme, Renderer> From<SwallowPress<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(widget: SwallowPress<'a, Message, Theme, Renderer>) -> Self {
        Self::new(widget)
    }
}
