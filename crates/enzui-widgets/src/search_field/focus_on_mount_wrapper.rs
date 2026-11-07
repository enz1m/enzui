//! Focuses a wrapped text input once, when it mounts.
//!
//! The flag lives in tree state, so re-renders and typing never refocus; a
//! surface that closes drops the state, and its next open focuses again.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Id, Operation, Tree, operation, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size};

#[derive(Default)]
pub(super) struct State {
    focused_once: bool,
}

pub(super) struct FocusOnMount<'a, Message> {
    child: Element<'a, Message>,
    target: Id,
}

impl<'a, Message> FocusOnMount<'a, Message> {
    pub(super) fn new(child: Element<'a, Message>, target: Id) -> Self {
        Self { child, target }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FocusOnMount<'_, Message> {
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
        let already = tree.state.downcast_ref::<State>().focused_once;

        // First frame after mount: claim focus for the
        // inner input, once. Do this before delivering the
        // event so the very first keystroke that reaches an
        // already-open field still lands in it.
        if !already {
            // A shell may have just restored this same identified
            // field after an ancestor shape flip. `focus(target)` would
            // focus it again and iced's text input would move the caret to
            // the end, undoing the shell's exact caret restoration. Treat
            // an already-focused target as this mount's one shot having
            // succeeded; only issue focus when the target is not focused.
            let mut find = operation::focusable::find_focused();
            let mut probe: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut find));
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                &layout.bounds(),
                renderer,
                &mut *probe,
            );
            drop(probe);
            let target_already_focused =
                matches!(find.finish(), operation::Outcome::Some(id) if id == self.target);

            if !target_already_focused {
                // Focusing here reaches only this subtree; the window root
                // makes the field the only focus owner.
                crate::focus_claim::claim(self.target.clone());
                let mut focus = operation::focusable::focus::<()>(self.target.clone());
                let mut op: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut focus));
                self.child.as_widget_mut().operate(
                    &mut tree.children[0],
                    layout,
                    &layout.bounds(),
                    renderer,
                    &mut *op,
                );
            }
            tree.state.downcast_mut::<State>().focused_once = true;
            shell.request_redraw();
        }

        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        tree.children[0].refresh_frame_interest(self.child.as_widget());
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        // A default `true` would keep an open Find field on every frame
        // route forever, even after this wrapper's one-shot work settled.
        !tree.state.downcast_ref::<State>().focused_once
            || crate::frame_routing::interest_opaque(tree)
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
        if !tree.state.downcast_ref::<State>().focused_once {
            // The first frame still owns the mount-focus side effect.
            self.update(tree, event, layout, cursor, renderer, shell, viewport);
            return;
        }
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

impl<'a, Message: 'a> From<FocusOnMount<'a, Message>> for Element<'a, Message> {
    fn from(w: FocusOnMount<'a, Message>) -> Self {
        Element::new(w)
    }
}
