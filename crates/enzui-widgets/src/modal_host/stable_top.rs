//! A centred dialog whose top edge stays put while its content changes size.
//!
//! The surface opens centred. When its height then changes (another settings
//! page, a validation message) its top edge stays where it was and the surface
//! grows or shrinks downward, so headers and tabs stay under the pointer. It
//! moves up only as far as needed to keep its bottom inside the margin, never
//! above the top margin. A new window size centres it again.

use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// The top edge kept from the previous layout, and the space it was in.
#[derive(Debug, Default, Clone, Copy)]
struct Anchor {
    top: Option<f32>,
    space: Size,
}

/// The vertical offset of a `height`-tall surface in `space` with `margin`
/// on each edge: centred when nothing is kept, otherwise the kept `top`,
/// raised only as far as the bottom margin demands and never above the top.
pub(super) fn stable_top(previous: Option<f32>, height: f32, space: f32, margin: f32) -> f32 {
    let centred = ((space - height) / 2.0).max(margin);
    let Some(top) = previous else {
        return centred;
    };
    let lowest = (space - margin - height).max(margin);
    top.min(lowest).max(margin)
}

pub(super) struct StableTop<'a, Message> {
    child: Element<'a, Message>,
    margin: f32,
}

impl<'a, Message> StableTop<'a, Message> {
    pub(super) fn new(child: impl Into<Element<'a, Message>>, margin: f32) -> Self {
        Self {
            child: child.into(),
            margin,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for StableTop<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Anchor>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Anchor::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let space = limits.max();
        let margin = self.margin;
        let inner = Size::new(
            (space.width - 2.0 * margin).max(0.0),
            (space.height - 2.0 * margin).max(0.0),
        );
        let node = self.child.as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(Size::ZERO, inner),
        );
        let size = node.size();
        let anchor = tree.state.downcast_mut::<Anchor>();
        // A resized window centres the surface again.
        let previous = (anchor.space == space).then_some(anchor.top).flatten();
        let top = stable_top(previous, size.height, space.height, margin);
        *anchor = Anchor {
            top: Some(top),
            space,
        };
        let left = ((space.width - size.width) / 2.0).max(margin);
        layout::Node::with_children(space, vec![node.move_to((left, top))]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if let Some((layout, state)) = layout.iter_mut(&mut tree.children).next() {
            self.child
                .as_widget_mut()
                .operate(state, layout, viewport, renderer, operation);
        }
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
        if let Some((layout, state)) = layout.iter_mut(&mut tree.children).next() {
            self.child
                .as_widget_mut()
                .update(state, event, layout, cursor, renderer, shell, viewport);
        }
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
        if let Some((layout, state)) = layout.iter(&tree.children).next() {
            self.child
                .as_widget()
                .draw(state, renderer, theme, style, layout, cursor, viewport);
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
        layout
            .iter(&tree.children)
            .next()
            .map(|(layout, state)| {
                self.child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .unwrap_or_default()
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
        overlay::from_children(
            std::slice::from_mut(&mut self.child),
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<StableTop<'a, Message>> for Element<'a, Message> {
    fn from(value: StableTop<'a, Message>) -> Self {
        Element::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::stable_top;

    const SPACE: f32 = 800.0;
    const MARGIN: f32 = 16.0;

    #[test]
    fn a_new_surface_opens_centred() {
        assert_eq!(stable_top(None, 400.0, SPACE, MARGIN), 200.0);
    }

    #[test]
    fn a_shorter_page_keeps_the_top_edge() {
        assert_eq!(stable_top(Some(200.0), 250.0, SPACE, MARGIN), 200.0);
    }

    #[test]
    fn a_taller_page_grows_down_until_the_bottom_margin() {
        assert_eq!(stable_top(Some(200.0), 500.0, SPACE, MARGIN), 200.0);
        // 700 tall from 200 would end at 900: it rises to end at 784.
        assert_eq!(stable_top(Some(200.0), 700.0, SPACE, MARGIN), 84.0);
    }

    #[test]
    fn a_surface_taller_than_the_space_stays_at_the_top_margin() {
        assert_eq!(stable_top(Some(200.0), 900.0, SPACE, MARGIN), MARGIN);
        assert_eq!(stable_top(None, 900.0, SPACE, MARGIN), MARGIN);
    }
}
