//! Claim Ctrl+wheel input for zooming before the wrapped content can scroll.
//!
//! Modifier-change events update retained Control state. A wheel event over
//! the wrapper bounds while Control is held is captured before child dispatch,
//! even when it produces no zoom step. Other events reach the content unchanged.
//!
//! Vertical lines accumulate to one notch; vertical pixels use 50 pixels per
//! notch. Each event emits at most one [`ZoomStep`] and discards its remainder
//! after emitting. Horizontal-only, zero and non-finite deltas emit no step.
//! Reversing direction or releasing Control clears partial input. Leaving the
//! bounds alone does not clear it. A touchpad pinch over the bounds emits one
//! step per [`PINCH_PER_STEP`](enzui_core::zoom_levels::PINCH_PER_STEP) of
//! growth, without Control. The application applies the step to its own
//! [`enzui_core::zoom_levels::ZoomLadder`]; keyboard shortcuts are separate.

use enzui_core::zoom_levels::{PinchZoom, WheelZoom, ZoomStep};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Widget, layout, mouse, overlay, renderer, widget as widget_advanced};
use iced::{Element, Event, Length, Rectangle, Size, Vector, keyboard};

/// Wrap `content` so Ctrl+wheel over it publishes `on_zoom(step)` instead of
/// scrolling it.
pub fn enz_zoom_wheel<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    on_zoom: impl Fn(ZoomStep) -> Message + 'a,
) -> EnzZoomWheel<'a, Message> {
    EnzZoomWheel {
        content: content.into(),
        on_zoom: Box::new(on_zoom),
    }
}

/// A retained wheel accumulator that claims Control-modified input before its child.
pub struct EnzZoomWheel<'a, Message> {
    content: Element<'a, Message>,
    on_zoom: Box<dyn Fn(ZoomStep) -> Message + 'a>,
}

#[derive(Default)]
struct State {
    modifiers: keyboard::Modifiers,
    wheel: WheelZoom,
    pinch: PinchZoom,
}

impl<'a, Message: 'a> EnzZoomWheel<'a, Message> {
    /// This wrapper as an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.into()
    }
}

impl<'a, Message: 'a> From<EnzZoomWheel<'a, Message>> for Element<'a, Message> {
    fn from(widget: EnzZoomWheel<'a, Message>) -> Self {
        Element::new(widget)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzZoomWheel<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: layout::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget_advanced::Operation,
    ) {
        self.content.as_widget_mut().operate(
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
        layout: layout::Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        // Frame interest is queried before diff may have built the child tree.
        tree.children
            .first()
            .is_none_or(|child| self.content.as_widget().frame_interest(child))
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update_frame(
            &mut tree.children[0],
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
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                if !modifiers.control() {
                    state.wheel.reset();
                }
                state.modifiers = *modifiers;
            }
            // A touchpad pinch over the content zooms too, a step per quarter
            // of growth.
            Event::Gesture(iced::gesture::Event::Pinch { scale, phase })
                if cursor.is_over(layout.bounds()) =>
            {
                use iced::gesture::Phase;
                if matches!(phase, Phase::Started | Phase::Ended | Phase::Cancelled) {
                    state.pinch.reset();
                } else if let Some(step) = state.pinch.feed(*scale) {
                    shell.publish((self.on_zoom)(step));
                }
                shell.capture_event();
                return;
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta })
                if state.modifiers.control() && cursor.is_over(layout.bounds()) =>
            {
                // Claim before child dispatch so a zoom gesture cannot also scroll the content.
                if let Some(step) = state.wheel.feed(*delta) {
                    shell.publish((self.on_zoom)(step));
                }
                shell.capture_event();
                return;
            }
            _ => {}
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
}
