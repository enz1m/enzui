//! The drag handle of a composite reorder-list row.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Marks the region of a row that starts a drag in a list with
/// [`drag_handles`](super::EnzReorderList::drag_handles). Built by
/// [`reorder_handle`].
pub struct ReorderHandle<'a, Message> {
    child: Element<'a, Message, iced::Theme, iced::Renderer>,
}

/// Wraps the part of a row that drags it: a group header, a grip icon. The
/// content keeps its own click; travel past the list's deadband turns the
/// press into a drag of the whole row.
pub fn reorder_handle<'a, Message>(
    child: impl Into<Element<'a, Message, iced::Theme, iced::Renderer>>,
) -> ReorderHandle<'a, Message> {
    ReorderHandle {
        child: child.into(),
    }
}

/// What a handle reports through [`Operation::custom`].
pub(super) struct HandleMarker;

/// Collects the bounds of the handles in a row.
#[derive(Default)]
pub(super) struct HandleProbe {
    pub(super) rects: Vec<Rectangle>,
}

impl Operation for HandleProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn custom(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        if state.is::<HandleMarker>() {
            self.rects.push(bounds);
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ReorderHandle<'_, Message> {
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
        operation.traverse(&mut |operation| {
            operation.custom(None, layout.bounds(), &mut HandleMarker);
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
        });
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
        renderer: &iced::Renderer,
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
        renderer: &iced::Renderer,
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
        match self
            .child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
        {
            mouse::Interaction::None if cursor.is_over(layout.bounds()) => mouse::Interaction::Grab,
            interaction => interaction,
        }
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

impl<'a, Message: 'a> From<ReorderHandle<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
{
    fn from(handle: ReorderHandle<'a, Message>) -> Self {
        Element::new(handle)
    }
}
