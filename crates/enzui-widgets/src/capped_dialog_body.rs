//! Dialog layout with measured header/footer chrome and a capped body.
//!
//! Chrome rows are measured at unbounded height, and their heights plus row
//! spacing determine the body's remaining room. Supply a scrollable body to
//! scroll overflow; this layout does not add scrolling itself. Chrome must have
//! finite intrinsic height. Fill-height chrome can expand under those limits.
//!
//! When chrome exceeds the cap, the body receives zero height and the stack
//! exceeds the requested maximum. [`CappedDialogBody::fill_available`] instead
//! uses the incoming height and fixes the body to the remaining gap, pinning
//! the footer when chrome fits. Finite nonnegative spacing is a caller requirement.

use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Limits};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A dialog stack that caps its whole height, deriving the scrolling
/// body's room from the measured chrome. See the module docs.
///
/// Layout order is every `above` child, then `body`, then every `below` child,
/// separated by `spacing`. Only the body receives a derived height cap; supply
/// a scrollable body when overflow must scroll.
pub struct CappedDialogBody<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    // Tree slots, layout nodes and overlay forwarding share this child order.
    children: Vec<Element<'a, Message, Theme, Renderer>>,
    body_index: usize,
    spacing: f32,
    max_height: Option<f32>,
    fill_available: bool,
}

/// Build a dialog stack with measured chrome above and below the capped body.
///
/// Equivalent to [`CappedDialogBody::new`]. The body must provide its own scrolling.
#[must_use]
pub fn enz_capped_dialog_body<'a, Message, Theme, Renderer>(
    above: Vec<Element<'a, Message, Theme, Renderer>>,
    body: impl Into<Element<'a, Message, Theme, Renderer>>,
    below: Vec<Element<'a, Message, Theme, Renderer>>,
    spacing: f32,
    max_height: f32,
) -> CappedDialogBody<'a, Message, Theme, Renderer> {
    CappedDialogBody::new(above, body, below, spacing, max_height)
}

impl<'a, Message, Theme, Renderer> CappedDialogBody<'a, Message, Theme, Renderer> {
    /// Build a capped dialog stack.
    ///
    /// `above` and `below` are the fixed chrome rows; `body` is the single
    /// body row whose cap is derived from the measured chrome.
    /// `max_height` is the maximum for the *whole* stack; a non-finite or
    /// `<= 0` value removes this cap, although incoming limits still constrain
    /// the body. Chrome is measured at unbounded height. Spacing is used raw.
    pub fn new(
        above: Vec<Element<'a, Message, Theme, Renderer>>,
        body: impl Into<Element<'a, Message, Theme, Renderer>>,
        below: Vec<Element<'a, Message, Theme, Renderer>>,
        spacing: f32,
        max_height: f32,
    ) -> Self {
        let body_index = above.len();
        let mut children = above;
        children.push(body.into());
        children.extend(below);
        Self {
            children,
            body_index,
            spacing,
            max_height: sanitize_cap(max_height),
            fill_available: false,
        }
    }

    /// Fill the offered height and give the body the gap left by measured chrome.
    ///
    /// Defaults to `false`. Ignores the configured cap while enabled. If chrome
    /// exceeds the offered height, the body gets zero and the stack overflows.
    #[must_use]
    pub fn fill_available(mut self, yes: bool) -> Self {
        self.fill_available = yes;
        self
    }

    /// Consume this layout into an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer>
    where
        Message: 'a,
        Theme: 'a,
        Renderer: 'a + iced::advanced::Renderer,
    {
        Element::from(self)
    }
}

fn sanitize_cap(cap: f32) -> Option<f32> {
    if cap.is_finite() && cap > 0.0 {
        Some(cap)
    } else {
        None
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for CappedDialogBody<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        let height = if self.fill_available {
            Length::Fill
        } else {
            Length::Shrink
        };
        Size::new(Length::Shrink, height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let body_index = self.body_index;
        let spacing = self.spacing;
        let fill_available = self.fill_available;

        let available = limits.max();

        let max_height = if fill_available {
            Some(available.height)
        } else {
            self.max_height
        };

        let children = &mut self.children;
        let n = children.len();
        let mut nodes: Vec<layout::Node> = Vec::with_capacity(n);
        nodes.resize_with(n, layout::Node::default);

        // Intrinsic chrome remains intact when it cannot fit the incoming height.
        let mut chrome_height = 0.0f32;
        for i in 0..n {
            if i == body_index {
                continue;
            }
            let child_limits = Limits::new(
                Size::new(0.0, 0.0),
                Size::new(available.width, f32::INFINITY),
            );
            let node =
                children[i]
                    .as_widget_mut()
                    .measure(&mut tree.children[i], renderer, &child_limits);
            chrome_height += node.size().height;
            nodes[i] = node;
        }
        let total_spacing = spacing * (n.saturating_sub(1)) as f32;

        let body_max_h = (available.height - chrome_height - total_spacing).max(0.0);
        let body_cap =
            max_height.map_or(body_max_h, |m| (m - chrome_height - total_spacing).max(0.0));

        let capped_h = body_max_h.min(body_cap);
        let body_limits = if fill_available {
            Limits::new(
                Size::new(0.0, capped_h),
                Size::new(available.width, capped_h),
            )
        } else {
            // Compression keeps Fill-height bodies intrinsic until they reach the cap.
            Limits::with_compression(
                Size::new(0.0, 0.0),
                Size::new(available.width, capped_h),
                Size::new(limits.compression().width, true),
            )
        };
        let body_node = children[body_index].as_widget_mut().measure(
            &mut tree.children[body_index],
            renderer,
            &body_limits,
        );
        nodes[body_index] = body_node;

        let mut y = 0.0f32;
        let mut width = 0.0f32;
        for (i, node) in nodes.iter_mut().enumerate() {
            if i > 0 {
                y += spacing;
            }
            node.move_to_mut(iced::Point::new(0.0, y));
            let sz = node.size();
            y += sz.height;
            width = width.max(sz.width);
        }

        layout::Node::with_children(Size::new(width, y), nodes).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        let children = &mut self.children;
        operation.traverse(&mut |operation| {
            for (child, (lay, state)) in
                children.iter_mut().zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, lay, viewport, renderer, operation);
            }
        });
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
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
        for (child, (lay, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, lay, cursor, renderer, shell, viewport);
            state.refresh_frame_interest(child.as_widget());
        }
        tree.refresh_frame_children();
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
        crate::frame_routing::forward_frame_children(
            &mut self.children,
            tree,
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
        for (child, (lay, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, lay, cursor, viewport);
        }
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
        iced::advanced::overlay::from_children(
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

impl<'a, Message, Theme, Renderer> From<CappedDialogBody<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: CappedDialogBody<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
