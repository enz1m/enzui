//! Selects a wrapped text input's whole value when it gains focus.
//!
//! Only the focus rising edge selects, so a focused input keeps its caret
//! while the user types.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Id, Operation, Tree, operation, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size};

#[derive(Default)]
pub(super) struct State {
    // Whether the child was focused at the end of the previous `update`.
    last_focused: bool,
}

pub(super) struct SelectAllOnFocus<'a, Message> {
    child: Element<'a, Message>,
    target: Id,
}

impl<'a, Message> SelectAllOnFocus<'a, Message> {
    pub(super) fn new(child: Element<'a, Message>, target: Id) -> Self {
        Self { child, target }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SelectAllOnFocus<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
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
        // Deliver the event first — that is what turns a click or Tab
        // into a focus-state change we can then observe.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        // Now inspect the child's focus. `find_focused` walks any
        // `Focusable` and reports the currently focused id, if any.
        let mut find = operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut find));
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut *boxed,
        );
        drop(boxed);
        let focused_now = matches!(find.finish(), operation::Outcome::Some(_));

        let state = tree.state.downcast_mut::<State>();
        // Rising edge: unfocused last frame, focused now → select.
        if focused_now && !state.last_focused {
            let mut select = operation::text_input::select_all::<()>(self.target.clone());
            let mut op: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut select));
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                &layout.bounds(),
                renderer,
                &mut *op,
            );
            // `select_all` returns `Outcome::None`; no chaining.
            shell.request_redraw();
        }
        state.last_focused = focused_now;
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message: 'a> From<SelectAllOnFocus<'a, Message>> for Element<'a, Message> {
    fn from(w: SelectAllOnFocus<'a, Message>) -> Self {
        Element::new(w)
    }
}
