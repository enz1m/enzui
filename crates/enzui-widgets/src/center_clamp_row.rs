//! Three slots with a middle child centred on the full bar width.
//!
//! Width and height default to `Fill`. Layout uses the constrained maximum on
//! both axes; `Shrink` and `Fit` do not compute an intrinsic row size. Supply
//! finite parent bounds or fixed dimensions.
//!
//! Side children are measured before the middle. If their combined widths
//! exceed the row, a second pass shares the available width, giving a smaller
//! side's unused budget to its peer. The middle receives a symmetric band that
//! clears both measured sides and can collapse to zero. Children must respect
//! their offered limits; the row does not clip paint, events or overlays.
//! Children are vertically centred and reconcile by slot position.
//!
//! [`crate::header_bar`] uses this layout for its title and action zones.

use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// A `[start | centred-middle | end]` bar that clamps the middle against the
/// measured side widths, sharing the side budget when their natural widths exceed it.
#[must_use]
pub struct CenterClampRow<'a, Message> {
    children: Vec<Element<'a, Message>>,
    width: Length,
    height: Length,
}

/// Build a [`CenterClampRow`] from its three zones.
pub fn center_clamp_row<'a, Message>(
    start: Element<'a, Message>,
    middle: Element<'a, Message>,
    end: Element<'a, Message>,
) -> CenterClampRow<'a, Message>
where
    Message: 'a,
{
    CenterClampRow {
        children: vec![start, middle, end],
        width: Length::Fill,
        height: Length::Fill,
    }
}

impl<'a, Message> CenterClampRow<'a, Message>
where
    Message: 'a,
{
    /// Row width. Defaults to [`Length::Fill`].
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Row height. Defaults to [`Length::Fill`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CenterClampRow<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        let max = limits.max();
        let total_w = max.width;
        let total_h = max.height;
        let centre_y = |node: layout::Node| -> layout::Node {
            let h = node.size().height;
            node.translate(Vector::new(0.0, ((total_h - h) / 2.0).max(0.0)))
        };
        let side_limits = layout::Limits::new(Size::ZERO, Size::new(total_w, total_h));

        let mut start_node =
            self.children[0]
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &side_limits);
        let mut end_node =
            self.children[2]
                .as_widget_mut()
                .measure(&mut tree.children[2], renderer, &side_limits);

        let start_width = start_node.size().width;
        let end_width = end_node.size().width;
        if start_width + end_width > total_w {
            let start_budget = start_width.min((total_w / 2.0).max(total_w - end_width));
            let end_budget = (total_w - start_budget).max(0.0);
            start_node = self.children[0].as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::new(start_budget, total_h)),
            );
            end_node = self.children[2].as_widget_mut().measure(
                &mut tree.children[2],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::new(end_budget, total_h)),
            );
        }

        let sw = start_node.size().width;
        let ew = end_node.size().width;
        let half = total_w / 2.0;
        let mid_half = (half - sw).min(half - ew).max(0.0);
        let mid_avail_w = (mid_half * 2.0).max(0.0);
        let mid_limits = layout::Limits::new(Size::ZERO, Size::new(mid_avail_w, total_h));
        let mid_node =
            self.children[1]
                .as_widget_mut()
                .measure(&mut tree.children[1], renderer, &mid_limits);
        let start_node = centre_y(start_node.move_to(Point::new(0.0, 0.0)));
        let end_node = centre_y(end_node.move_to(Point::new((total_w - ew).max(0.0), 0.0)));

        let mid_w = mid_node.size().width;
        let mid_x = ((total_w - mid_w) / 2.0).max(0.0);
        let mid_node = centre_y(mid_node.move_to(Point::new(mid_x, 0.0)));

        layout::Node::with_children(
            Size::new(total_w, total_h),
            vec![start_node, mid_node, end_node],
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

impl<'a, Message> From<CenterClampRow<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(row: CenterClampRow<'a, Message>) -> Self {
        Element::new(row)
    }
}
