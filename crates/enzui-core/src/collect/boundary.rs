//! A transparent widget-tree marker for measuring app-owned subtrees independently of framework shell nodes.
//!
//! The marker adds one tree node while delegating layout, paint, events, operations, overlays, and mouse interaction to its child.

use crate::collect::{AppSubtreeBoundary, app_subtree_boundary_tag};
use iced_core::layout::Measure as _;
use iced_core::widget::{Operation, Tree, tree};
use iced_core::{
    Element, Event, Layout, Length, Rectangle, Shell, Size, Vector, layout, mouse, overlay,
    renderer,
};

/// A transparent single-child marker recognized by the app-subtree census.
pub struct AppSubtreeBoundaryElement<'a, Message, Theme, Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

/// Wraps `child` in a transparent marker discoverable by the app-subtree census.
pub fn app_subtree_boundary<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> AppSubtreeBoundaryElement<'a, Message, Theme, Renderer> {
    AppSubtreeBoundaryElement {
        child: child.into(),
    }
}

/// The child covers the boundary exactly, so it shares its layout.
fn child_layout(layout: Layout) -> Layout {
    layout
}

impl<Message, Theme, Renderer> iced_core::Widget<Message, Theme, Renderer>
    for AppSubtreeBoundaryElement<'_, Message, Theme, Renderer>
where
    Renderer: iced_core::Renderer,
{
    // A distinct tagged tree node is required for the census, so the wrapped child occupies `tree.children[0]`.
    fn tag(&self) -> tree::Tag {
        app_subtree_boundary_tag()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AppSubtreeBoundary)
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        // Preserve the child’s void status so parent containers continue filtering it normally.
        self.child.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        // Use unchanged limits and a zero-offset wrapper of the child’s size to preserve absolute bounds.
        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(node.size(), vec![node]).apply(tree);
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
            child_layout(layout),
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout(layout),
            cursor,
            renderer,
            shell,
            viewport,
        );
        // A normal event may start animation, so refresh cached child frame interest before propagating it.
        tree.children[0].refresh_frame_interest(self.child.as_widget());
        tree.refresh_frame_child(0);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.first().is_some_and(Tree::has_frame_interest)
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
        if !tree.children[0].has_frame_interest() {
            return;
        }
        self.child.as_widget_mut().update_frame(
            &mut tree.children[0],
            event,
            child_layout(layout),
            cursor,
            renderer,
            shell,
            viewport,
        );
        tree.children[0].refresh_frame_interest(self.child.as_widget());
        tree.refresh_frame_child(0);
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
            child_layout(layout),
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout(layout),
            cursor,
            viewport,
            renderer,
        )
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
            child_layout(layout),
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<AppSubtreeBoundaryElement<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced_core::Renderer,
{
    fn from(boundary: AppSubtreeBoundaryElement<'a, Message, Theme, Renderer>) -> Self {
        Element::new(boundary)
    }
}
