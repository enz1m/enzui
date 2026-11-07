//! Completed-click dismissal and optional scroll blocking for a scrim.
//!
//! Left-button presses and touch presses inside the bounds arm dismissal;
//! a subsequent primary release inside publishes the callback. Mouse movement
//! outside cancels the armed gesture until a fresh press. Touch uses event
//! positions but does not pair finger identities. Cancelling tree interactions
//! clears the pending click.
//!
//! The child receives events first. Scroll passes through by default;
//! [`ScrimClick::block_scroll`] captures unhandled wheel and touch movement
//! inside the bounds, plus unmodified paging keys regardless of cursor position.
//! Captured touch movement cancels a pending click. Hosts control layering and
//! dismissal after the callback, for example with [`crate::modal_host::EnzModalHost`].

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector, touch};

#[derive(Debug, Default, Clone, Copy)]
struct State {
    armed: bool,
}

/// A scrim wrapper that publishes a callback for an armed release inside its bounds.
pub struct ScrimClick<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    on_click: Option<Box<dyn Fn() -> Message + 'a>>,
    block_scroll: bool,
}

impl<'a, Message, Theme, Renderer> ScrimClick<'a, Message, Theme, Renderer> {
    /// Wrap the child with completed-click dismissal and scroll pass-through.
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        on_click: impl Fn() -> Message + 'a,
    ) -> Self {
        Self {
            child: child.into(),
            on_click: Some(Box::new(on_click)),
            block_scroll: false,
        }
    }

    /// Capture unhandled wheel and touch movement inside the bounds, and paging keys.
    /// The child retains first refusal; paging does not require a cursor hit.
    #[must_use]
    pub const fn block_scroll(mut self) -> Self {
        self.block_scroll = true;
        self
    }

    /// Build a scroll-blocking scrim without a dismissal callback.
    /// Unhandled left-button and touch presses inside the bounds are captured;
    /// other mouse buttons retain the child's behavior.
    pub fn shield(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
            on_click: None,
            block_scroll: true,
        }
    }
}

fn point_of(event: &Event, cursor: mouse::Cursor) -> Option<iced::Point> {
    match event {
        Event::Touch(
            touch::Event::FingerPressed { position, .. }
            | touch::Event::FingerLifted { position, .. },
        ) => Some(*position),
        _ => cursor.position(),
    }
}

fn is_primary_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. })
    )
}

fn is_primary_release(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerLifted { .. })
    )
}
#[must_use]
pub(crate) fn is_owned_scroll_gesture(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse::Event::WheelScrolled { .. }) => true,
        Event::Touch(touch::Event::FingerMoved { .. }) => true,
        Event::Keyboard(_) => crate::scroll_view::ScrollKey::from_event(event).is_some(),
        _ => false,
    }
}

fn scroll_gesture_targets(event: &Event, cursor: mouse::Cursor, bounds: Rectangle) -> bool {
    match event {
        Event::Keyboard(_) => true,
        Event::Touch(touch::Event::FingerMoved { position, .. }) => bounds.contains(*position),
        _ => cursor.position().is_some_and(|p| bounds.contains(p)),
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ScrimClick<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);

        if shell.is_event_captured() {
            return;
        }

        let bounds = layout.bounds();
        // Capture before routing can reach live siblings outside the host's inert subtree.
        if self.block_scroll
            && is_owned_scroll_gesture(event)
            && scroll_gesture_targets(event, cursor, bounds)
        {
            if matches!(event, Event::Touch(touch::Event::FingerMoved { .. })) {
                tree.state.downcast_mut::<State>().armed = false;
            }
            shell.capture_event();
            return;
        }

        let state = tree.state.downcast_mut::<State>();
        if state.armed
            && matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. }))
            && !cursor.is_over(bounds)
        {
            state.armed = false;
        }

        if is_primary_press(event) {
            if point_of(event, cursor).is_some_and(|p| bounds.contains(p)) {
                state.armed = true;
                shell.capture_event();
            }
            return;
        }

        if is_primary_release(event) {
            let was_armed = state.armed;
            state.armed = false;
            let over = point_of(event, cursor).is_some_and(|p| bounds.contains(p));
            if was_armed && over {
                if let Some(on_click) = self.on_click.as_ref() {
                    shell.publish(on_click());
                }
                shell.capture_event();
            }
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
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
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
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
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
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
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
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<ScrimClick<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(widget: ScrimClick<'a, Message, Theme, Renderer>) -> Self {
        Self::new(widget)
    }
}

impl tree::Interactive for State {
    fn cancel_interaction(&mut self) {
        self.armed = false;
    }
}
