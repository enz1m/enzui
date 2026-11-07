//! Constrain child height while allowing content shorter than the cap to shrink.
//!
//! Values use logical pixels. Zero, negative and non-finite caps are uncapped.
//! Capped layout enables height compression, so ordinary fill children can hug
//! content up to the cap; fixed-height children retain their explicit sizing.
//! Width is preserved and children must respect their offered limits. Paint
//! and overlays are forwarded without installing a clip.
//!
//! [`MaxHeight::clip_interaction`] defaults to false. When enabled on a capped
//! region, operation callbacks require positive-area intersection and layout
//! releases focus from excluded nodes. Partial intersections stay interactive.
//! Events remain forwarded; a cursor outside the region is levitated, which
//! hides `position()` but permits a child to recover it with `land()` or
//! `observe()`. Overlays retain their own interaction policy.
//!
//! # Panics
//!
//! In debug builds, layout can assert when a child measures at the cap and
//! returns a non-finite height under an unbounded diagnostic probe. The probe
//! also performs extra child layouts before restoring the capped layout.

use iced::advanced::layout::Limits;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::operation::{Focusable, Scrollable, TextInput};
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Single-child wrapper that caps child height at `cap` while allowing it to
/// shrink below the cap. See the module docs.
pub struct MaxHeight<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    cap: Option<f32>,
    clip_interaction: bool,
}

impl<'a, Message, Theme, Renderer> MaxHeight<'a, Message, Theme, Renderer> {
    /// Wrap `child`, capping its height at `cap` px. A non-finite or
    /// `<= 0` cap means uncapped.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>, cap: f32) -> Self {
        Self {
            child: child.into(),
            cap: sanitize_cap(Some(cap)),
            clip_interaction: false,
        }
    }

    /// `Option` variant: `None` = uncapped, `Some(cap)` = cap at `cap` px
    /// (again, non-finite/`<= 0` caps degrade to uncapped).
    pub fn new_opt(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        cap: Option<f32>,
    ) -> Self {
        Self {
            child: child.into(),
            cap: sanitize_cap(cap),
            clip_interaction: false,
        }
    }

    /// Filter operation callbacks to positive-area intersections with capped bounds.
    /// Layout also releases excluded focus. Events continue to flow, with outside
    /// cursors levitated; children can recover their positions with `land()` or
    /// `observe()`. Paint and overlays remain unclipped. Defaults to false and has
    /// no effect when uncapped; leave off for content reachable by scrolling.
    #[must_use]
    pub fn clip_interaction(mut self, clip: bool) -> Self {
        self.clip_interaction = clip;
        self
    }

    fn interaction_clip(&self, layout: Layout) -> Option<Rectangle> {
        let cap = self.cap.filter(|_| self.clip_interaction)?;
        let bounds = layout.bounds();
        // A child can exceed its layout limits, so interaction needs an independent cap.
        Some(Rectangle {
            height: bounds.height.min(cap),
            ..bounds
        })
    }

    #[cfg(debug_assertions)]
    fn debug_check_not_pinned_to_cap(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &Limits,
        cap: f32,
        node: &layout::Node,
    ) where
        Renderer: iced::advanced::Renderer,
    {
        if limits.max().height != cap || (node.size().height - cap).abs() >= 0.01 {
            return;
        }
        // An unbounded probe distinguishes fluid sizing from arbitrarily tall intrinsic content.
        let probe_max = f32::INFINITY;
        let probe_limits = Limits::with_compression(
            Size::new(limits.min().width, limits.min().height.min(probe_max)),
            Size::new(limits.max().width, probe_max),
            limits.compression(),
        );
        let probe_h = self
            .child
            .as_widget_mut()
            .measure(tree, renderer, &probe_limits)
            .size()
            .height;
        // Restore caches modified by diagnostic measurement.
        self.child.as_widget_mut().layout(tree, renderer, limits);

        debug_assert!(
            (probe_h - probe_max).abs() >= 0.01,
            "MaxHeight: the capped subtree resolved to exactly the {cap}px cap, \
             and re-measuring it under an unbounded maximum produced \
             {probe_h}px — it grows to fill whatever it is offered and has no \
             intrinsic height of its own. A fluid height inside the cap is \
             re-arming ICED's fill-to-max path, so the maximum is behaving as a \
             fixed height. Give the child `Length::Shrink`, or \
             `Length::Fixed({cap})` if pinning really is the intent."
        );
    }
}

fn sanitize_cap(cap: Option<f32>) -> Option<f32> {
    match cap {
        Some(c) if c.is_finite() && c > 0.0 => Some(c),
        _ => None,
    }
}

fn is_visible_within(clip: Rectangle, bounds: Rectangle) -> bool {
    clip.intersection(&bounds).is_some()
}

struct ReleaseClippedFocus {
    clip: Rectangle,
}

impl Operation<()> for ReleaseClippedFocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
        // Inspect each descendant; a parent intersection does not prove child intersection.
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        let _ =
            enzui_core::focus::release_focus_if_inert(state, is_visible_within(self.clip, bounds));
    }
}

struct ClipInteraction<'a, T> {
    inner: &'a mut dyn Operation<T>,
    clip: Rectangle,
}

impl<T> Operation<T> for ClipInteraction<'_, T> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        let clip = self.clip;
        // Preserve semantics depth while filtering individual callbacks.
        self.inner.traverse(&mut |inner| {
            operate(&mut ClipInteraction { inner, clip });
        });
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        if is_visible_within(self.clip, bounds) {
            self.inner.container(id, bounds, viewport);
        }
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        if is_visible_within(self.clip, bounds) {
            self.inner.focusable(id, bounds, state);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Size,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if is_visible_within(self.clip, bounds) {
            self.inner
                .scrollable(id, bounds, content_bounds, translation, state);
        }
    }

    fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
        if is_visible_within(self.clip, bounds) {
            self.inner.text_input(id, bounds, state);
        }
    }

    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, text: &str) {
        if is_visible_within(self.clip, bounds) {
            self.inner.text(id, bounds, text);
        }
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        if is_visible_within(self.clip, bounds) {
            self.inner.custom(id, bounds, state);
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<T> {
        // Keep chained operations, including focus counting followed by selection.
        self.inner.finish()
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for MaxHeight<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        let child = self.child.as_widget().size();
        match self.cap {
            Some(_) => Size::new(child.width, Length::Shrink),
            None => child,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = match self.cap {
            Some(cap) => {
                let min = limits.min();
                let max = limits.max();
                let capped_h = max.height.min(cap);
                let min_h = min.height.min(capped_h);
                Limits::with_compression(
                    Size::new(min.width, min_h),
                    Size::new(max.width, capped_h),
                    Size::new(limits.compression().width, true),
                )
            }
            None => *limits,
        };
        let node = self.child.as_widget_mut().measure(tree, renderer, &limits);

        #[cfg(debug_assertions)]
        if let Some(cap) = self.cap {
            self.debug_check_not_pinned_to_cap(tree, renderer, &limits, cap, &node);
        }
        // Traversal exclusion alone leaves retained focus active during input dispatch.
        if let Some(clip) = self.interaction_clip(Layout::of(&node)) {
            let mut release = ReleaseClippedFocus { clip };
            self.child.as_widget_mut().operate(
                tree,
                Layout::of(&node),
                &Layout::of(&node).bounds(),
                renderer,
                &mut release,
            );
        }

        node.apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        match self.interaction_clip(layout) {
            Some(clip) => {
                let mut gated = ClipInteraction {
                    inner: operation,
                    clip,
                };
                self.child
                    .as_widget_mut()
                    .operate(tree, layout, viewport, renderer, &mut gated);
            }
            None => {
                self.child
                    .as_widget_mut()
                    .operate(tree, layout, viewport, renderer, operation);
            }
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
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
        let cursor = match self.interaction_clip(layout) {
            Some(clip) => match cursor.position() {
                Some(p) if !clip.contains(p) => cursor.levitate(),
                _ => cursor,
            },
            None => cursor,
        };

        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        let cursor = match self.interaction_clip(layout) {
            Some(clip) => match cursor.position() {
                Some(position) if !clip.contains(position) => cursor.levitate(),
                _ => cursor,
            },
            None => cursor,
        };
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let cursor = match self.interaction_clip(layout) {
            Some(clip) => match cursor.position() {
                Some(p) if !clip.contains(p) => cursor.levitate(),
                _ => cursor,
            },
            None => cursor,
        };

        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<MaxHeight<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: MaxHeight<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
