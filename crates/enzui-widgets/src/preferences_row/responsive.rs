//! Settings-row label and accessory layout over one retained child tree.

use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(super) struct PreferencesResponsive<'a, Message> {
    children: Vec<Element<'a, Message>>,
    gap: f32,
    trailing_min_width: Option<f32>,
    label_min_width: Option<f32>,
    expand_when_stacked: bool,
    accessory_value_min: Option<f32>,
    /// Lay the children out from the right edge.
    rtl: bool,
}

impl<'a, Message> PreferencesResponsive<'a, Message> {
    pub(super) fn new(
        label: Element<'a, Message>,
        trailing: Element<'a, Message>,
        gap: f32,
        trailing_min_width: Option<f32>,
        expand_when_stacked: bool,
    ) -> Self {
        Self {
            children: vec![label, trailing],
            gap,
            trailing_min_width,
            label_min_width: None,
            expand_when_stacked,
            accessory_value_min: None,
            rtl: false,
        }
    }

    /// Mirror the layout for right-to-left text: the label from the right
    /// edge, the accessory from the left.
    pub(super) fn rtl(mut self, rtl: bool) -> Self {
        self.rtl = rtl;
        self
    }

    /// `node` mirrored across `width` when laying out right to left.
    fn place(&self, node: layout::Node, x: f32, y: f32, width: f32) -> layout::Node {
        let x = if self.rtl {
            width - x - node.size().width
        } else {
            x
        };
        node.move_to((x, y))
    }

    pub(super) fn label_min_width(mut self, width: Option<f32>) -> Self {
        self.label_min_width = width;
        self
    }

    pub(super) fn action_value(
        value: Element<'a, Message>,
        chevron: Element<'a, Message>,
        gap: f32,
        value_min: f32,
    ) -> Self {
        let mut layout = Self::new(value, chevron, gap, None, false);
        layout.accessory_value_min = Some(value_min);
        layout
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for PreferencesResponsive<'_, Message> {
    // Retain child trees across stacking transitions so focus and field state survive resizing.
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(Length::Fill).height(Length::Shrink);
        let offered_width = limits.max().width;
        let max_height = limits.max().height;

        if let Some(value_min) = self.accessory_value_min {
            let natural = layout::Limits::with_flags(
                Size::ZERO,
                Size::new(0.0, max_height),
                Size::new(false, false),
                Size::new(true, false),
            );
            let chevron_width = self.children[1]
                .as_widget_mut()
                .measure(&mut tree.children[1], renderer, &natural)
                .size()
                .width;
            // Unbounded probes report a finite healthy width rather than a stacked viewport.
            let available = if offered_width.is_finite() {
                offered_width.max(0.0)
            } else {
                self.children[0]
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, &natural)
                    .size()
                    .width
                    .max(value_min)
                    + self.gap
                    + chevron_width
            };
            let show_chevron = available >= value_min + self.gap + chevron_width;
            let reserved = if show_chevron {
                self.gap + chevron_width
            } else {
                0.0
            };
            let exact_width =
                |width| layout::Limits::new(Size::new(width, 0.0), Size::new(width, max_height));
            let value = self.children[0].as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &exact_width(available - reserved),
            );
            let chevron = self.children[1].as_widget_mut().measure(
                &mut tree.children[1],
                renderer,
                &exact_width(if show_chevron { chevron_width } else { 0.0 }),
            );
            let height = value.size().height.max(chevron.size().height);
            let value_y = (height - value.size().height) / 2.0;
            let chevron_y = (height - chevron.size().height) / 2.0;
            let chevron_x = available - chevron.size().width;
            layout::Node::with_children(
                Size::new(available, height),
                vec![
                    self.place(value, 0.0, value_y, available),
                    self.place(chevron, chevron_x, chevron_y, available),
                ],
            )
            .apply(tree);
            return;
        }

        let trailing_min = self.trailing_min_width.unwrap_or_else(|| {
            let probe_limits = layout::Limits::with_flags(
                Size::ZERO,
                Size::new(0.0, max_height),
                Size::new(false, false),
                Size::new(true, false),
            );
            self.children[1]
                .as_widget_mut()
                .measure(&mut tree.children[1], renderer, &probe_limits)
                .size()
                .width
        });
        let threshold = self.label_min_width.unwrap_or(trailing_min) + trailing_min + self.gap;
        // Unbounded probes report a finite healthy width rather than a stacked viewport.
        let available = if offered_width.is_finite() {
            offered_width.max(0.0)
        } else {
            threshold
        };
        let stacked =
            offered_width.is_finite() && offered_width > 0.0 && offered_width + 0.01 < threshold;

        let node = if stacked {
            let exact_width =
                |width| layout::Limits::new(Size::new(width, 0.0), Size::new(width, max_height));
            let label = self.children[0].as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &exact_width(available),
            );
            let trailing_width = if self.expand_when_stacked {
                available
            } else {
                trailing_min.min(available)
            };
            let trailing = self.children[1].as_widget_mut().measure(
                &mut tree.children[1],
                renderer,
                &exact_width(trailing_width),
            );
            let label_height = label.size().height;
            let trailing_height = trailing.size().height;
            let trailing_x = if self.expand_when_stacked {
                0.0
            } else {
                available - trailing_width
            };
            layout::Node::with_children(
                Size::new(available, label_height + self.gap + trailing_height),
                vec![
                    self.place(label, 0.0, 0.0, available),
                    self.place(trailing, trailing_x, label_height + self.gap, available),
                ],
            )
        } else {
            let trailing_width = trailing_min.min(available.max(0.0));
            let label_width = (available - self.gap - trailing_width).max(0.0);
            let exact_width =
                |width| layout::Limits::new(Size::new(width, 0.0), Size::new(width, max_height));
            let mut label = self.children[0].as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &exact_width(label_width),
            );
            let mut trailing = self.children[1].as_widget_mut().measure(
                &mut tree.children[1],
                renderer,
                &exact_width(trailing_width),
            );
            let label_height = label.size().height;
            let trailing_height = trailing.size().height;
            let height = label_height.max(trailing_height);
            label = self.place(label, 0.0, (height - label_height) / 2.0, available);
            trailing = self.place(
                trailing,
                label_width + self.gap,
                (height - trailing_height) / 2.0,
                available,
            );
            layout::Node::with_children(Size::new(available, height), vec![label, trailing])
        };
        node.apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .for_each(|(child, (layout, state))| {
                    child
                        .as_widget_mut()
                        .operate(state, layout, viewport, renderer, operation);
                });
        });
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
        for (child, (layout, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
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
        for (child, (layout, state)) in self
            .children
            .iter()
            .zip(layout.iter(&tree.children))
            .filter(|(_, (layout, _))| layout.bounds().intersects(viewport))
        {
            child
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
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (layout, state))| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
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

impl<'a, Message: 'a> From<PreferencesResponsive<'a, Message>> for Element<'a, Message> {
    fn from(value: PreferencesResponsive<'a, Message>) -> Self {
        Element::new(value)
    }
}
