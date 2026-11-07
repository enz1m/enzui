//! A single-child frame whose height follows its resolved width divided by a ratio.
//!
//! Width defaults to [`Length::Fill`]; intrinsic width policies measure the
//! child first. Every policy measures the child twice, with the final pass
//! bounded by the derived frame. A child may remain smaller than that frame.
//!
//! Frame height preserves the ratio even when it exceeds the parent's height
//! limit. Invalid ratios and overflowing quotients collapse height to zero;
//! non-finite resolved widths collapse both frame dimensions to zero.
//!
//! Paint is clipped to the frame's intersection with the viewport. Cursor
//! interaction is reported only inside that intersection, but events,
//! operations and overlays are forwarded with the original viewport.
//! [`crate::video`] uses this frame for media.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Responsive width-driven frame with clipped child paint.
/// Its derived height is not clamped to the parent's height limit.
#[must_use]
pub struct EnzAspectRatio<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    ratio: f32,
    width: Length,
}

/// Wrap a child at a width-over-height ratio, defaulting to filled width.
/// Invalid ratios collapse height to zero; see the module for clipping and
/// parent-height behavior.
pub fn enz_aspect_ratio<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    ratio: f32,
) -> EnzAspectRatio<'a, Message, Theme, Renderer> {
    EnzAspectRatio {
        child: child.into(),
        ratio,
        width: Length::Fill,
    }
}

impl<'a, Message, Theme, Renderer> EnzAspectRatio<'a, Message, Theme, Renderer> {
    /// Set the outer width policy (default [`Length::Fill`]). `Fixed(w)` gives
    /// the fixed-width thumbnail case; `Fill` / `FillPortion` stay responsive.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    // Reject an overflowing quotient as well as invalid operands.
    fn derived_height(&self, resolved_width: f32) -> f32 {
        if self.ratio.is_finite()
            && self.ratio > 0.0
            && resolved_width.is_finite()
            && resolved_width >= 0.0
        {
            let h = resolved_width / self.ratio;
            if h.is_finite() { h.max(0.0) } else { 0.0 }
        } else {
            0.0
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EnzAspectRatio<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width);
        // Intrinsic policies need child width before the final ratio-constrained pass.
        let intrinsic_node =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits);
        let intrinsic_w = intrinsic_node.size().width;
        let resolved_w = limits.resolve_width(self.width, intrinsic_w);
        let frame_w = if resolved_w.is_finite() {
            resolved_w
        } else {
            0.0
        };
        let frame_h = self.derived_height(frame_w);
        let child_limits = layout::Limits::new(Size::ZERO, Size::new(frame_w, frame_h));
        let child_node =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &child_limits);

        layout::Node::with_children(Size::new(frame_w, frame_h), vec![child_node]).apply(tree);
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
        let bounds = layout.bounds();
        let child_layout = layout.child(&tree.children, 0);
        if let Some(clipped) = bounds.intersection(viewport) {
            renderer.with_layer(clipped, |renderer| {
                self.child.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    child_layout,
                    cursor,
                    &clipped,
                );
            });
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let child_layout = layout.child(&tree.children, 0);
        if bounds
            .intersection(viewport)
            .is_some_and(|c| cursor.is_over(c))
        {
            self.child.as_widget().mouse_interaction(
                &tree.children[0],
                child_layout,
                cursor,
                viewport,
                renderer,
            )
        } else {
            mouse::Interaction::default()
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

impl<'a, Message, Theme, Renderer> EnzAspectRatio<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    /// Finish the builder as an [`Element`].
    /// Useful where `.into()` has no target type to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer> {
        Element::from(self)
    }
}

impl<'a, Message, Theme, Renderer> From<EnzAspectRatio<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: EnzAspectRatio<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
