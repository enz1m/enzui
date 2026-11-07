//! Rounded masking of a single child's paint and cursor-based corner hits.
//!
//! Put the wrapper inside a chrome container to preserve its outer shadow.
//! [`RoundedClip::outset`] can grow the mask through the container's padding;
//! it does not add layout space. Keyboard traversal, semantics and child
//! overlays are forwarded without applying the corner mask.
//!
//! Corner exclusion changes the cursor passed to event handling and cursor
//! queries; touch-event positions are forwarded unchanged. Theme-resolved
//! hit geometry uses the last drawn radius, or the constructor radius before
//! the first draw. Drawing itself receives the original cursor.
//!
//! Rounded coverage depends on the renderer: the trait default clips to a
//! rectangle, and the wgpu text path currently uses rectangular glyph bounds.

use iced::advanced::layout::Measure as _;
use std::cell::Cell;

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::border::Radius;
use iced::{Alignment, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// Wrap one child with a rounded paint mask and cursor-based corner exclusion.
#[must_use]
pub struct RoundedClip<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    radius: Radius,
    #[allow(
        clippy::type_complexity,
        reason = "one boxed theme callback; an alias would only hide it"
    )]
    radius_fn: Option<Box<dyn Fn(&Theme) -> Radius + 'a>>,
    outset: Padding,
    content_layout: Option<ContentLayout>,
    paint_only: bool,
}

#[derive(Debug, Clone, Copy)]
struct ContentLayout {
    sizing: Size<Length>,
    alignment: [Alignment; 2],
    preserve_fill_minimum: bool,
}

struct ClipState {
    // Event handling has no theme; retain the last painted radius across view rebuilds.
    radius: Cell<Radius>,
}

fn sane(r: f32) -> f32 {
    if r.is_finite() && r > 0.0 { r } else { 0.0 }
}

pub(crate) fn sane_radius(r: Radius) -> Radius {
    Radius {
        top_left: sane(r.top_left),
        top_right: sane(r.top_right),
        bottom_right: sane(r.bottom_right),
        bottom_left: sane(r.bottom_left),
    }
}

/// Mask child paint using logical-pixel corner radii; invalid or negative radii become zero.
pub fn rounded_clip<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    radius: impl Into<Radius>,
) -> RoundedClip<'a, Message, Theme, Renderer> {
    RoundedClip::new(child, radius)
}

impl<'a, Message, Theme, Renderer> RoundedClip<'a, Message, Theme, Renderer> {
    /// Create a clip with fixed logical-pixel radii and zero outset.
    /// Negative or non-finite radii become zero; zero keeps square corners.
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        radius: impl Into<Radius>,
    ) -> Self {
        let radius: Radius = radius.into();
        let radius = sane_radius(radius);
        Self {
            child: child.into(),
            radius,
            radius_fn: None,
            outset: Padding::ZERO,
            content_layout: None,
            paint_only: false,
        }
    }

    /// Mask paint only: the cursor reaches the corners unchanged, for a mask
    /// over chrome that answers input there (a window's corner resize grips).
    pub(crate) fn paint_only(mut self) -> Self {
        self.paint_only = true;
        self
    }

    /// Resolve and sanitize corner radii from the theme during draw.
    /// Overrides the constructor radius. Cursor hit handling uses the last drawn
    /// radius, starting with the constructor radius before a draw occurs.
    pub fn radius_resolved(mut self, f: impl Fn(&Theme) -> Radius + 'a) -> Self {
        self.radius_fn = Some(Box::new(move |t| sane_radius(f(t))));
        self
    }

    pub(crate) fn align_content(mut self, alignment: [Alignment; 2]) -> Self {
        self.content_layout = Some(ContentLayout {
            sizing: Size::new(Length::Fill, Length::Fill),
            alignment,
            preserve_fill_minimum: false,
        });
        self
    }

    pub(crate) fn align_control_content(
        mut self,
        sizing: Size<Length>,
        alignment: [Alignment; 2],
    ) -> Self {
        self.content_layout = Some(ContentLayout {
            sizing,
            alignment,
            preserve_fill_minimum: true,
        });
        self
    }

    /// Grow the mask outward by logical-pixel padding without adding layout space.
    /// Defaults to zero; negative or non-finite components become zero.
    pub fn outset(mut self, outset: impl Into<Padding>) -> Self {
        let p: Padding = outset.into();
        self.outset = Padding {
            top: sane(p.top),
            right: sane(p.right),
            bottom: sane(p.bottom),
            left: sane(p.left),
        };
        self
    }

    fn clip_bounds(&self, bounds: Rectangle) -> Rectangle {
        layout::clip::bounds(bounds, self.outset)
    }
}

impl<Message, Theme, Renderer> RoundedClip<'_, Message, Theme, Renderer> {
    fn mask_radius(&self, theme: &Theme) -> Radius {
        match &self.radius_fn {
            Some(f) => f(theme),
            None => self.radius,
        }
    }

    fn masked_cursor(
        &self,
        radius: Radius,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Cursor {
        if self.paint_only {
            return cursor;
        }
        let clip = self.clip_bounds(bounds);
        // Preserve outside-rectangle positions so children can detect outside clicks.
        match cursor.position() {
            Some(p)
                if clip.contains(p)
                    && !enzui_core::rounded_geometry::point_in_rounded_rect(p, clip, radius) =>
            {
                cursor.levitate()
            }
            _ => cursor,
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for RoundedClip<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<ClipState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(ClipState {
            radius: Cell::new(self.radius),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut [self.child.as_widget_mut()]);
    }

    fn size(&self) -> Size<iced::Length> {
        self.content_layout
            .map_or_else(|| self.child.as_widget().size(), |content| content.sizing)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let node = if let Some(content) = self.content_layout {
            let measure = |limits: &layout::Limits| {
                let mut child_limits = *limits;
                if content.preserve_fill_minimum {
                    let sizing = self.child.as_widget().size();
                    // Fluid axes retain allocation minima; intrinsic axes measure loosely.
                    if !sizing.width.is_fill() {
                        child_limits.min.width = 0.0;
                    }
                    if !sizing.height.is_fill() {
                        child_limits.min.height = 0.0;
                    }
                }
                self.child
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, &child_limits)
            };
            if content.preserve_fill_minimum {
                layout::padded_aligned(
                    limits,
                    content.sizing.width,
                    content.sizing.height,
                    Padding::ZERO,
                    content.alignment,
                    measure,
                )
            } else {
                layout::aligned(
                    limits,
                    content.sizing.width,
                    content.sizing.height,
                    Padding::ZERO,
                    content.alignment,
                    measure,
                )
            }
        } else {
            let child = self
                .child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits);
            layout::Node::with_children(child.size(), vec![child])
        };
        node.with_clip_presentation(*limits, self.size(), self.outset)
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
        // Visual masking does not remove keyboard or accessibility targets.
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
        );
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
        let radius = tree.state.downcast_ref::<ClipState>().radius.get();
        let cursor = self.masked_cursor(radius, layout.bounds(), cursor);
        let child_layout = layout.child(&tree.children, 0);
        // Children reaching into a corner (a scrollbar rail) keep clear of
        // its arc: they measure it while they handle events.
        let clip = self.clip_bounds(layout.bounds());
        iced_core::clip_shape::within(clip, radius, || {
            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        });
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.first().is_some_and(Tree::has_frame_interest)
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
        let radius = tree.state.downcast_ref::<ClipState>().radius.get();
        let cursor = self.masked_cursor(radius, layout.bounds(), cursor);
        let child_layout = layout.child(&tree.children, 0);
        let clip = self.clip_bounds(layout.bounds());
        iced_core::clip_shape::within(clip, radius, || {
            self.child.as_widget_mut().update_frame(
                &mut tree.children[0],
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        });
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
        let clip = self.clip_bounds(layout.bounds());
        // Offscreen material must survive capture so owned reflow can expose it.
        let capture = tree
            .clip_presentation_node()
            .and_then(|node| renderer.layout_clip_capture(node));
        let (child_viewport, layer_bounds) = if let Some(capture) = capture {
            (capture.viewport, capture.mask)
        } else if let Some(visible) = clip.intersection(viewport) {
            (visible, clip)
        } else {
            return;
        };
        // Viewport cropping must not relocate the child’s corner arcs.
        let radius = self.mask_radius(theme);
        tree.state.downcast_ref::<ClipState>().radius.set(radius);
        let draw = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout.child(&tree.children, 0),
                cursor,
                &child_viewport,
            );
        };
        let draw_masked = |renderer: &mut Renderer| {
            if let Some(node) = tree.clip_presentation_node() {
                renderer.with_layout_layer(
                    renderer::LayoutLayer::Clip(node),
                    layer_bounds,
                    Some(radius),
                    draw,
                );
            } else {
                renderer.with_layer_rounded(layer_bounds, radius, draw);
            }
        };
        if capture.is_some() {
            draw_masked(renderer);
        } else {
            renderer.with_layer(child_viewport, draw_masked);
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
        let radius = tree.state.downcast_ref::<ClipState>().radius.get();
        let cursor = self.masked_cursor(radius, layout.bounds(), cursor);
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout.child(&tree.children, 0),
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
        // Child popouts may extend beyond the content mask.
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

impl<'a, Message, Theme, Renderer> From<RoundedClip<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: RoundedClip<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
