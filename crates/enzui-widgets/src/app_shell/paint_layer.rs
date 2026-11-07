//! Decorative shell paint with no input, focus, semantics or overlay ownership.

use super::*;

pub(super) struct PaintLayer<'a, Message>(pub(super) Element<'a, Message>);

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for PaintLayer<'_, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.0.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.0.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.0.as_widget_mut().diff(tree);
    }
    fn frame_interest(&self, _tree: &Tree) -> bool {
        false
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.0.as_widget_mut().layout(tree, renderer, limits);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.0.as_widget().draw(
            tree,
            renderer,
            theme,
            style,
            layout,
            mouse::Cursor::Unavailable,
            viewport,
        );
    }
}
