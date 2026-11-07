//! [`BodyGate`]: suppress hidden body work when settled collapsed.

use super::*;
use iced::advanced::layout::Measure as _;

// ---------------------------------------------------------------------------
// BodyGate: suppress hidden body work when settled collapsed
// ---------------------------------------------------------------------------

/// Shared handle between [`ExpanderAnim`] and [`BodyGate`].
///
/// Each frame, `ExpanderAnim` computes whether the body is fully hidden and a
/// steady state (settled collapsed) and writes `suppress` before the layout
/// pass recurses into the body. `BodyGate` reads it and, when set, skips the
/// body's expensive work: it neither lays the child out afresh, updates it,
/// draws it, nor lets it open an overlay. Because a settled-collapsed body is
/// clipped to zero visible height by `ExpanderAnim`, none of that work is
/// perceivable — but a self-animating child would otherwise keep running its
/// `update`/`draw` and requesting animation frames while invisible.
///
/// The animated height itself is derived by `ExpanderAnim` from the body's
/// *preferred* height; when suppressed, `BodyGate` reports its last measured
/// height (`cached_h`) instead of re-measuring, so the collapsed height is
/// unchanged and re-expansion (which un-suppresses the moment the target flips
/// to expanded, via `diff`'s `go_mut`) measures the real content again.
#[derive(Clone, Default)]
pub(crate) struct BodyGateShared {
    /// Written by `ExpanderAnim` before recursion; read by `BodyGate`.
    suppress: Rc<Cell<bool>>,
}

impl BodyGateShared {
    pub(crate) fn set_suppress(&self, v: bool) {
        self.suppress.set(v);
    }
    fn suppress(&self) -> bool {
        self.suppress.get()
    }
}

/// Per-tree state for [`BodyGate`]: the child's last measured preferred size,
/// so a suppressed frame can synthesize a same-shaped node without paying for
/// the child's full layout.
#[derive(Default)]
struct BodyGateState {
    /// Last full (unsuppressed) child size. `None` until the first real layout.
    cached: Cell<Option<Size>>,
}

/// A single-child wrapper that, when its shared `suppress` flag is set, stops
/// forwarding layout / update / draw / overlay to the body content.
pub(crate) struct BodyGate<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    shared: BodyGateShared,
}

impl<'a, Message, Theme, Renderer> BodyGate<'a, Message, Theme, Renderer> {
    pub(crate) fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        shared: BodyGateShared,
    ) -> Self {
        Self {
            child: child.into(),
            shared,
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for BodyGate<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<BodyGateState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(BodyGateState::default())
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut widget::Tree, renderer: &Renderer, limits: &layout::Limits) {
        let state = tree.state.downcast_ref::<BodyGateState>();
        // Suppressed and we have a cached preferred size: reuse it and skip the
        // child's full layout entirely. The synthesized node
        // has no children, but the suppressed body is never forwarded an event
        // or a draw, so nothing indexes into it.
        //
        // If suppressed but there is no cached size yet (never expanded), we
        // fall through to a real layout so the very first collapsed frame still
        // measures — this happens at most once, before any expansion.
        if self.shared.suppress()
            && let Some(size) = state.cached.get()
        {
            return layout::Node::new(size).apply(tree);
        }
        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        state.cached.set(Some(node.size()));
        layout::Node::new(node.size()).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if self.shared.suppress() {
            return;
        }
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.shared.suppress() {
            return;
        }
        self.child.as_widget_mut().update(
            &mut tree.children[0],
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
        tree: &widget::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.shared.suppress() {
            return mouse::Interaction::default();
        }
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if self.shared.suppress() {
            return;
        }
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        if self.shared.suppress() {
            return Vec::new();
        }
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<BodyGate<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(value: BodyGate<'a, Message, Theme, Renderer>) -> Self {
        Element::new(value)
    }
}
