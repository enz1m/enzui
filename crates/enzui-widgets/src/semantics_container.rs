//! Add a semantic group or leaf without changing child layout or paint.
//!
//! [`SemanticsContainer::new`] groups the child's published nodes beneath its
//! own node. [`SemanticsContainer::leaf`] suppresses child semantics within
//! operation traversal. Both forward focus, actions and shaped-text collection.
//! Child overlays are forwarded unchanged, outside the leaf semantic boundary.
//! Use [`crate::decorative::decorative`] to suppress child semantics without
//! publishing a wrapper node. These wrappers do not disable interactive content.

use enzui_core::collect::{SemanticsCollectionBoundary, SemanticsNode};
use enzui_core::semantics::Semantics;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A transparent child wrapper that publishes a semantic group or leaf node.
#[must_use]
pub struct SemanticsContainer<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    semantics: Semantics,
    is_group: bool,
    id: Option<iced::advanced::widget::Id>,
}

impl<'a, Message, Theme, Renderer> SemanticsContainer<'a, Message, Theme, Renderer> {
    /// Publish a group node with the child's deeper semantic nodes as descendants.
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        semantics: Semantics,
    ) -> Self {
        Self {
            child: child.into(),
            semantics,
            is_group: true,
            id: None,
        }
    }

    /// Publish a leaf node and suppress semantics during child operation traversal.
    /// Focus, actions and shaped-text collection still reach the child.
    /// Child overlays are forwarded unchanged and retain their own semantics.
    pub fn leaf(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        semantics: Semantics,
    ) -> Self {
        Self {
            child: child.into(),
            semantics,
            is_group: false,
            id: None,
        }
    }

    /// Attach a stable widget id (correlates AT action requests back here).
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for SemanticsContainer<'_, Message, Theme, Renderer>
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut node = if self.is_group {
            SemanticsNode::group(self.semantics.clone())
        } else {
            SemanticsNode::new(self.semantics.clone())
        };
        operation.custom(self.id.as_ref(), layout.bounds(), &mut node);

        if self.is_group {
            operation.container(self.id.as_ref(), layout.bounds(), viewport);
            operation.traverse(&mut |operation| {
                self.child
                    .as_widget_mut()
                    .operate(tree, layout, viewport, renderer, operation);
            });
        } else {
            operation.traverse(&mut |operation| {
                let mut hide = SemanticsCollectionBoundary::Hide;
                operation.custom(None, layout.bounds(), &mut hide);
                self.child
                    .as_widget_mut()
                    .operate(tree, layout, viewport, renderer, operation);
                let mut restore = SemanticsCollectionBoundary::Restore;
                operation.custom(None, layout.bounds(), &mut restore);
            });
        }
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
        renderer: &Renderer,
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

impl<'a, Message, Theme, Renderer> From<SemanticsContainer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: SemanticsContainer<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
