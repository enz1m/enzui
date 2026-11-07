//! Private rectangular clipping for composed widget content.
//!
//! Paint clipping uses a renderer layer at the allocation intersected with the
//! viewport. Interaction clipping masks cursor hit-testing outside the allocation
//! and intersects operation bounds, including focus and accessibility carriers.
//! Events still reach the child: children must honour the masked cursor, and
//! events carrying their own coordinates are not filtered here.
//!
//! Paint-only and interaction-only variants separate these policies. Overlays
//! are forwarded without this clip because they are independent surfaces.

use iced::advanced::widget::operation::{Focusable, Scrollable, TextInput};
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Rectangle, Size, Vector};

pub(crate) struct InertClip<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    enabled: bool,
    interaction: bool,
    paint: bool,
}

/// Build the private wrapper for frame-routing tests.
/// `interaction` selects paint-and-interaction clipping controlled by `enabled`;
/// false selects always-enabled paint-only clipping and ignores `enabled`.
#[doc(hidden)]
pub fn __frame_routing_fixture<'a, Message, Theme, Renderer>(
    child: Element<'a, Message, Theme, Renderer>,
    enabled: bool,
    interaction: bool,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    let wrapper = if interaction {
        InertClip::new(child, enabled)
    } else {
        InertClip::paint_only(child)
    };
    Element::new(wrapper)
}

impl<'a, Message, Theme, Renderer> InertClip<'a, Message, Theme, Renderer> {
    pub(crate) fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        enabled: bool,
    ) -> Self {
        Self {
            child: child.into(),
            enabled,
            interaction: true,
            paint: true,
        }
    }

    pub(crate) fn paint_only(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
            enabled: true,
            interaction: false,
            paint: true,
        }
    }

    pub(crate) fn interaction_only(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Self {
            child: child.into(),
            enabled: true,
            interaction: true,
            paint: false,
        }
    }
}

pub(crate) fn cursor_within(cursor: mouse::Cursor, clip: Rectangle) -> mouse::Cursor {
    match cursor.position() {
        Some(position) if !clip.contains(position) => cursor.levitate(),
        _ => cursor,
    }
}

pub(crate) struct ClipOperation<'a, T> {
    inner: &'a mut dyn Operation<T>,
    clip: Rectangle,
    whole_targets: bool,
    /// The translation of a scrollable just reported: its content, traversed
    /// next, reports bounds in content coordinates, that far from the screen.
    scrolled: Vector,
}

// Use the spill detector's tolerance so subpixel rounding cannot silently hide a control.
const CONTAINMENT_EPSILON: f32 = crate::bounds_probe::SPILL_EPSILON;

impl<'a, T> ClipOperation<'a, T> {
    pub(crate) fn new(inner: &'a mut dyn Operation<T>, clip: Rectangle) -> Self {
        Self {
            inner,
            clip,
            whole_targets: false,
            scrolled: Vector::ZERO,
        }
    }

    pub(crate) fn whole_targets(mut self, whole: bool) -> Self {
        self.whole_targets = whole;
        self
    }

    fn visible_bounds(&self, bounds: Rectangle) -> Option<Rectangle> {
        self.clip.intersection(&bounds)
    }

    fn target_bounds(&self, bounds: Rectangle) -> Option<Rectangle> {
        let visible = self.visible_bounds(bounds);
        let visible = visible?;
        if !self.whole_targets {
            return Some(visible);
        }
        let whole = visible.width + CONTAINMENT_EPSILON >= bounds.width
            && visible.height + CONTAINMENT_EPSILON >= bounds.height;
        whole.then_some(bounds)
    }
}

impl<T> Operation<T> for ClipOperation<'_, T> {
    // Native geometry reconciliation must pass through clips to reach hidden allocations.
    fn is_native_reconciliation(&self) -> bool {
        self.inner.is_native_reconciliation()
    }
    fn projection_applied(&mut self, root: iced::advanced::renderer::NodeId) {
        self.inner.projection_applied(root);
    }
    fn needs_editor_reconciliation(&self, node: iced::advanced::renderer::NodeId) -> bool {
        self.inner.needs_editor_reconciliation(node)
    }
    fn editor_reconciled(&mut self, node: iced::advanced::renderer::NodeId, success: bool) {
        self.inner.editor_reconciled(node, success);
    }
    fn needs_scroll_reconciliation(&self, node: iced::advanced::renderer::NodeId) -> bool {
        self.inner.needs_scroll_reconciliation(node)
    }
    fn scroll_reconciled(&mut self, node: iced::advanced::renderer::NodeId, success: bool) {
        self.inner.scroll_reconciled(node, success);
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        // Inside a scrollable the clip moves with the content, so a target
        // scrolled into view is kept and one scrolled out of it is not.
        let clip = self.clip + std::mem::take(&mut self.scrolled);
        let whole_targets = self.whole_targets;
        self.inner.traverse(&mut |inner| {
            operate(&mut ClipOperation {
                inner,
                clip,
                whole_targets,
                scrolled: Vector::ZERO,
            });
        });
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        if let Some(visible) = self.visible_bounds(bounds) {
            self.inner.container(id, visible, viewport);
        }
    }

    // Synthetic collapsed-search focus routes need intersections even under whole-target policy.
    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        if let Some(visible) = self.visible_bounds(bounds) {
            self.inner.focusable(id, visible, state);
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
        if let Some(visible) = self.visible_bounds(bounds) {
            self.inner
                .scrollable(id, visible, content_bounds, translation, state);
        }
        self.scrolled = translation;
    }

    fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
        if let Some(visible) = self.visible_bounds(bounds) {
            self.inner.text_input(id, visible, state);
        }
    }

    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, text: &str) {
        if let Some(visible) = self.visible_bounds(bounds) {
            self.inner.text(id, visible, text);
        }
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        // Whole-target policy applies only to interactive semantics, not arbitrary carriers.
        let interactive = state
            .downcast_ref::<enzui_core::collect::SemanticsNode>()
            .is_some_and(|node| node.semantics.is_interactive());
        let visible = if interactive {
            self.target_bounds(bounds)
        } else {
            self.visible_bounds(bounds)
        };
        if let Some(visible) = visible {
            self.inner.custom(id, visible, state);
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<T> {
        self.inner.finish()
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for InertClip<'_, Message, Theme, Renderer>
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

    fn size(&self) -> Size<iced::Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.enabled && self.interaction {
            let mut clipped = ClipOperation::new(operation, layout.bounds());
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, &mut clipped);
        } else {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
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
        let cursor = if self.enabled && self.interaction {
            cursor_within(cursor, layout.bounds())
        } else {
            cursor
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
        let cursor = if self.enabled && self.interaction {
            cursor_within(cursor, layout.bounds())
        } else {
            cursor
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
        if !self.enabled || !self.paint {
            self.child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
            return;
        }

        let Some(clip) = layout.bounds().intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            self.child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, &clip);
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let cursor = if self.enabled && self.interaction {
            cursor_within(cursor, layout.bounds())
        } else {
            cursor
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
        // Popovers and tooltips may extend beyond their ancestor's content allocation.
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<InertClip<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(value: InertClip<'a, Message, Theme, Renderer>) -> Self {
        Element::new(value)
    }
}
