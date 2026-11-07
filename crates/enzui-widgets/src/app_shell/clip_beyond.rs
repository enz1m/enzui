//! A child drawn only on one side of a fixed window column.
//!
//! A sidebar header slides beneath the shell's riding toggle while the
//! sidebar folds; clipped at the toggle's trailing edge, its title passes
//! out of sight behind the toggle's place instead of under its icon. At rest
//! the title already starts past that edge, so nothing is cut.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

pub(crate) struct ClipBeyond<'a, Message> {
    child: Element<'a, Message>,
    /// The window column, in window coordinates.
    edge: f32,
    /// Keep what lies after the edge (left to right) rather than before it.
    after: bool,
}

impl<'a, Message> ClipBeyond<'a, Message> {
    /// Draw `child` only past `edge` when `after`, only before it otherwise.
    pub(crate) fn new(child: impl Into<Element<'a, Message>>, edge: f32, after: bool) -> Self {
        Self {
            child: child.into(),
            edge,
            after,
        }
    }

    fn kept(&self, bounds: Rectangle) -> Rectangle {
        if self.after {
            let x = bounds.x.max(self.edge);
            Rectangle {
                x,
                width: (bounds.x + bounds.width - x).max(0.0),
                ..bounds
            }
        } else {
            Rectangle {
                width: (self.edge - bounds.x).clamp(0.0, bounds.width),
                ..bounds
            }
        }
    }
}

impl<Message> Widget<Message, Theme, Renderer> for ClipBeyond<'_, Message> {
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
        layout::pass_through(tree, 0);
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
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let kept = self.kept(bounds);
        if kept.width <= 0.0 {
            return;
        }
        let draw = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        };
        if kept == bounds {
            draw(renderer);
        } else {
            renderer.with_layer(kept, draw);
        }
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

impl<'a, Message: 'a> From<ClipBeyond<'a, Message>> for Element<'a, Message> {
    fn from(clip: ClipBeyond<'a, Message>) -> Self {
        Element::new(clip)
    }
}
