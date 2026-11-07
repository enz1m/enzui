//! Size a rendered child using the preferred width of a separate measuring child.
//!
//! The measuring child is laid out under an unbounded width and the parent's
//! maximum height. Supply a content-sized twin with matching content and padding;
//! it is never painted, operated, updated or overlaid. Both children retain
//! separate positional tree slots.
//!
//! The preferred width is floored, capped, then limited by the parent's maximum.
//! A ceiling smaller than the floor wins; zero is a real zero-width ceiling.
//! Invalid or non-positive floors become zero, while negative or non-finite
//! ceilings become unbounded. Width reports `Shrink`; height follows the rendered
//! child. The wrapper does not clip paint or interaction.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A rendered child whose offered width comes from a separate measuring child.
pub struct HugToPreferred<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    children: Vec<Element<'a, Message, Theme, Renderer>>,
    floor: f32,
    ceiling: f32,
}

/// Measure preferred width from `measure`, then lay out `render` at the bounded width.
/// Values are logical pixels. Zero ceiling is retained; invalid floors become
/// zero and invalid ceilings become unbounded. The ceiling and parent maximum
/// can reduce the result below the floor.
pub fn hug_to_preferred<'a, Message, Theme, Renderer>(
    render: impl Into<Element<'a, Message, Theme, Renderer>>,
    measure: impl Into<Element<'a, Message, Theme, Renderer>>,
    floor: f32,
    ceiling: f32,
) -> HugToPreferred<'a, Message, Theme, Renderer> {
    let floor = if floor.is_finite() && floor > 0.0 {
        floor
    } else {
        0.0
    };
    let ceiling = if ceiling.is_finite() && ceiling >= 0.0 {
        ceiling
    } else {
        f32::INFINITY
    };
    HugToPreferred {
        children: vec![render.into(), measure.into()],
        floor,
        ceiling,
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for HugToPreferred<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::stateless()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::None
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: self.children[0].as_widget().size().height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let max = limits.max();
        // Measure independently of the parent width so a fill-shaped render child cannot set the preference.
        let measure_limits = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, max.height));
        let preferred = self.children[1]
            .as_widget_mut()
            .measure(&mut tree.children[1], renderer, &measure_limits)
            .size()
            .width;
        let target = preferred
            .max(self.floor)
            .min(self.ceiling)
            .min(max.width)
            .max(0.0);
        let render_limits = layout::Limits::new(
            Size::new(target, limits.min().height),
            Size::new(target, max.height),
        );
        let render_node = self.children[0].as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &render_limits,
        );
        // Keep the measuring tree slot unplaced and excluded from traversal.
        let size = render_node.size();
        layout::Node::with_children(size, vec![render_node])
            .with_placed_prefix()
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
        let child_layout = layout.children(&tree.children).next().expect("render node");
        self.children[0].as_widget_mut().operate(
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
        let child_layout = layout.children(&tree.children).next().expect("render node");
        self.children[0].as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.children[0], tree, 0);
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
        crate::frame_routing::forward_frame_opaque(
            &mut self.children[0],
            tree,
            0,
            event,
            layout.children(&tree.children).next().expect("render node"),
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
        self.children[0].as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children(&tree.children).next().expect("render node"),
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
        self.children[0].as_widget().mouse_interaction(
            &tree.children[0],
            layout.children(&tree.children).next().expect("render node"),
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
        let child_layout = layout.children(&tree.children).next().expect("render node");
        self.children[0].as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<HugToPreferred<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: HugToPreferred<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
