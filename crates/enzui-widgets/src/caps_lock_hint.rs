//! The Caps Lock warning inside a password field.
//!
//! [`caps_lock_hint`] wraps a secure field's input. While Caps Lock is on it
//! narrows the input and shows the Caps Lock symbol at its end, in the
//! warning tone, so a password typed in capitals is noticed before it is
//! submitted. The state comes from `enzui_core::keyboard_locks`, which every
//! keyboard event updates; the warning appears and disappears without the
//! application doing anything.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

use crate::icon::{IconTone, enz_icon};

/// The symbol's size and the gap before it, in logical pixels.
const ICON: f32 = 16.0;
const GAP: f32 = 6.0;

/// Wraps a secure field's `input` with the Caps Lock warning.
pub(crate) fn caps_lock_hint<'a, Message: 'a>(input: Element<'a, Message>) -> Element<'a, Message> {
    Element::new(CapsLockHint {
        input,
        icon: enz_icon("arrow-big-up-dash")
            .size(ICON)
            .tint_theme_tone(IconTone::Warning)
            .into_element(),
    })
}

struct CapsLockHint<'a, Message> {
    input: Element<'a, Message>,
    icon: Element<'a, Message>,
}

#[derive(Debug)]
struct State {
    on: bool,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CapsLockHint<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            on: enzui_core::keyboard_locks::caps_lock(),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        if tree.children.len() != 2 {
            tree.children = vec![
                Tree::new(self.input.as_widget()),
                Tree::new(self.icon.as_widget()),
            ];
        }
        tree.children[0].diff(self.input.as_widget_mut());
        tree.children[1].diff(self.icon.as_widget_mut());
        tree.refresh_frame_children();
    }

    fn size(&self) -> Size<Length> {
        self.input.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let on = tree.state.downcast_ref::<State>().on;
        let reserved = if on { ICON + GAP } else { 0.0 };
        let input = self.input.as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &limits.shrink(Size::new(reserved, 0.0)),
        );
        let size = Size::new(input.size().width + reserved, input.size().height);
        if !on {
            // The symbol stays unplaced, outside layout and traversal.
            layout::Node::with_children(size, vec![input])
                .with_placed_prefix()
                .apply(tree);
            return;
        }
        let icon = self.icon.as_widget_mut().measure(
            &mut tree.children[1],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(ICON, ICON)),
        );
        let icon_at = iced::Point::new(
            size.width - icon.size().width,
            (size.height - icon.size().height) / 2.0,
        );
        layout::Node::with_children(size, vec![input, icon.move_to(icon_at)]).apply(tree);
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
        if let Event::Keyboard(keyboard) = event
            && let Some(on) = enzui_core::keyboard_locks::observe(keyboard)
        {
            let state = tree.state.downcast_mut::<State>();
            if state.on != on {
                state.on = on;
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
        let input_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.input.as_widget_mut().update(
            &mut tree.children[0],
            event,
            input_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.input, tree, 0);
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
        let input_layout = layout.children(&tree.children).next().unwrap_or(layout);
        crate::frame_routing::forward_frame_opaque(
            &mut self.input,
            tree,
            0,
            event,
            input_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        let mut children = layout.children(&tree.children);
        let input_layout = children.next().unwrap_or(layout);
        self.input.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            input_layout,
            cursor,
            viewport,
        );
        if tree.state.downcast_ref::<State>().on
            && let Some(icon_layout) = children.next()
        {
            self.icon.as_widget().draw(
                &tree.children[1],
                renderer,
                theme,
                style,
                icon_layout,
                cursor,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.input.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children(&tree.children).next().unwrap_or(layout),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let input_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.input.as_widget_mut().operate(
            &mut tree.children[0],
            input_layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
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
        let input_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.input.as_widget_mut().overlay(
            &mut tree.children[0],
            input_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
