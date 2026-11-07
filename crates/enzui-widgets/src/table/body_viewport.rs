//! Fixed-row viewport used by [`EnzTable`](super::EnzTable).
//!
//! The table keeps the full allocation owned by its parent, but this child
//! clips row paint to the largest whole-row band that fits inside that
//! allocation. It also positions the realised (including overscan) column at
//! its model-space offset, instead of allowing leading overscan rows to occupy
//! visible slots.

use crate::inert_clip::cursor_within;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Clip and position one realised table-row column.
pub(super) struct TableBodyViewport<'a, Message> {
    child: Element<'a, Message>,
    height: f32,
    content_y: f32,
}

impl<'a, Message> TableBodyViewport<'a, Message> {
    pub(super) fn new(child: Element<'a, Message>, height: f32, content_y: f32) -> Self {
        Self {
            child,
            height,
            content_y,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for TableBodyViewport<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let size = limits.resolve(Length::Fill, Length::Fixed(self.height), Size::ZERO);
        // Rows are already viewport-bounded by realization. Let the realised
        // overscan column retain its natural height so it can be translated
        // above/below the clip instead of being compressed into the viewport.
        let child_limits = layout::Limits::new(
            Size::new(size.width, 0.0),
            Size::new(size.width, f32::INFINITY),
        );
        let child = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, &child_limits)
            .move_to(Point::new(0.0, self.content_y));
        layout::Node::with_children(size, vec![child]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return;
        };
        // Overscan rows deliberately remain in the operated tree: retained
        // semantics/state are part of overscan's existing contract. Paint and
        // pointer interaction are clipped independently below.
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
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
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return;
        };
        let bounds = layout.bounds();
        let clip = bounds
            .intersection(viewport)
            .unwrap_or_else(|| Rectangle::new(bounds.position(), Size::ZERO));
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor_within(cursor, clip),
            renderer,
            shell,
            &clip,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return;
        };
        let Some(clip) = layout.bounds().intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                cursor_within(cursor, clip),
                &clip,
            );
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return mouse::Interaction::default();
        };
        let bounds = layout.bounds();
        let clip = bounds
            .intersection(viewport)
            .unwrap_or_else(|| Rectangle::new(bounds.position(), Size::ZERO));
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
            cursor_within(cursor, clip),
            &clip,
            renderer,
        )
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
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return Vec::new();
        };
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<TableBodyViewport<'a, Message>> for Element<'a, Message> {
    fn from(value: TableBodyViewport<'a, Message>) -> Self {
        Element::new(value)
    }
}
