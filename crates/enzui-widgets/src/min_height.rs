//! Applies a logical-pixel minimum height without capping taller content.
//!
//! Start alignment raises the child's minimum height. Center and end alignment
//! place its natural height within the floor. Input and semantics follow those
//! resulting child bounds.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Alignment, Element, Event, Length, Rectangle, Size, Vector};

/// A height-floor wrapper with alignment-dependent child constraints.
#[must_use]
pub struct MinHeight<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    floor: f32,
    align_y: Alignment,
}

/// Applies a logical-pixel height floor within the parent limits, without capping taller content.
///
/// Non-finite or non-positive floors disable the floor. The default start alignment
/// raises the child's minimum height; other alignments preserve its natural height.
pub fn min_height<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    floor: f32,
) -> MinHeight<'a, Message, Theme, Renderer> {
    let floor = if floor.is_finite() && floor > 0.0 {
        floor
    } else {
        0.0
    };
    MinHeight {
        child: child.into(),
        floor,
        align_y: Alignment::Start,
    }
}

impl<'a, Message, Theme, Renderer> MinHeight<'a, Message, Theme, Renderer> {
    /// Controls how the child occupies the floored region.
    ///
    /// Start alignment (the default) raises its minimum height. Center and end
    /// align its natural height within the floor; taller content retains its height.
    pub fn align_y(mut self, align: Alignment) -> Self {
        self.align_y = align;
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for MinHeight<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        let child_size = self.child.as_widget().size();
        Size {
            width: child_size.width,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        layout::minimum_height(
            limits,
            self.size().width,
            self.floor,
            self.align_y,
            |limits| {
                self.child
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, limits)
            },
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
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
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
        let child_layout = layout.child(&tree.children, 0);
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            child_layout,
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
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child_layout,
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
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
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
        let child_layout = layout.child(&tree.children, 0);
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

impl<'a, Message, Theme, Renderer> From<MinHeight<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: MinHeight<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
