use crate::scroll_view::{Axis, enz_scroll_view};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Layout, Widget, layout, overlay, renderer};
use iced::widget::Space;
use iced::{Element, Event, Length, Rectangle, Size, Vector};

use super::EnzOverflowRow;

pub(super) struct OverflowRowWidget<'a, Message> {
    children: Vec<Element<'a, Message>>,
    spacing: f32,
    height: Length,
    has_leading: bool,
    has_trailing: bool,
    leading_priority: i32,
    trailing_priority: i32,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for OverflowRowWidget<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::flex::resolve_bookends(
            renderer,
            limits,
            self.height,
            self.spacing,
            layout::flex::Bookends {
                has_leading: self.has_leading,
                has_trailing: self.has_trailing,
                leading_first: self.leading_priority > self.trailing_priority,
            },
            &mut self.children,
            &mut tree.children,
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (lay, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, lay, viewport, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, lay, cursor, renderer, shell, viewport);
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, lay, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (lay, state))| {
                child
                    .as_widget()
                    .mouse_interaction(state, lay, cursor, viewport, renderer)
            })
            .max()
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
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> From<EnzOverflowRow<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzOverflowRow<'a, Message>) -> Self {
        let mut band = enz_scroll_view(w.content)
            .axis(Axis::Horizontal)
            .width(Length::Fill)
            .height(Length::Shrink)
            .hide_scrollbar(true);
        if let Some(id) = w.content_id {
            band = band.id(id);
        }
        if let Some(cb) = w.on_scroll {
            band = band.on_scroll(cb);
        }
        let band: Element<'a, Message> = band.into();

        let has_leading = w.leading.is_some();
        let has_trailing = w.trailing.is_some();

        let leading: Element<'a, Message> = w.leading.unwrap_or_else(|| Space::new().into());
        let trailing: Element<'a, Message> = w.trailing.unwrap_or_else(|| Space::new().into());

        Element::new(OverflowRowWidget {
            children: vec![leading, band, trailing],
            spacing: w.spacing,
            height: w.height,
            has_leading,
            has_trailing,
            leading_priority: w.leading_priority,
            trailing_priority: w.trailing_priority,
        })
    }
}
