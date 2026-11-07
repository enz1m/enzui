//! Expander body-height animation: [`ExpanderAnimState`] and the [`ExpanderAnim`] wrapper widget.

use super::*;
use iced::advanced::layout::Measure as _;

// ---------------------------------------------------------------------------
// Animation state
// ---------------------------------------------------------------------------

/// Persistent animation state for the expander body height transition.
///
/// Follows the same `Animation<bool>` pattern as `code_block.rs::PulseState`:
/// the `bool` target represents collapsed (`false`) / expanded (`true`), and
/// the interpolated progress drives the visible body height between 0 and the
/// content’s preferred height.
pub(crate) struct ExpanderAnimState {
    /// The open/close animation. `false` = collapsed, `true` = expanded.
    pub(crate) anim: Animation<bool>,
    prev_expanded: bool,
    /// Shared raw-duration, unknown-policy, easing, and applied-duration latch.
    motion: crate::motion::MotionLatch,
    // Timestamp from the latest `RedrawRequested`. Layout, draw, input,
    // operation and overlay all sample this one frame instant.
    frame_at: std::cell::Cell<Option<Instant>>,
    // Visual endpoint used until the first compositor timestamp arrives.
    pre_frame_endpoint: bool,
    // A queued reversal retains presented progress until the next compositor timestamp.
    pending_from: Option<f32>,
}

/// `MotionScale::default().normal_ms` — the raw duration used before any
/// theme has been observed (the state is constructed before the first `draw`).
const DEFAULT_NORMAL_MS: f32 = 180.0;

impl ExpanderAnimState {
    pub(crate) fn new(expanded: bool, reduced_motion: bool) -> Self {
        let applied_ms = if reduced_motion {
            0.0
        } else {
            DEFAULT_NORMAL_MS
        };
        let easing = enzui_theme::MotionEasing::default();
        let anim = Self::make_anim(expanded, applied_ms, easing);
        Self {
            anim,
            prev_expanded: expanded,
            motion: crate::motion::MotionLatch::new(DEFAULT_NORMAL_MS, applied_ms, easing),
            frame_at: std::cell::Cell::new(None),
            pre_frame_endpoint: expanded,
            pending_from: None,
        }
    }

    /// Build an `Animation<bool>` with the given effective duration and the
    /// shared easing. `0` yields [`Duration::ZERO`] — the body lands on its
    /// target height on the frame the flag changes.
    fn make_anim(
        initial: bool,
        effective_ms: f32,
        easing: enzui_theme::MotionEasing,
    ) -> Animation<bool> {
        crate::motion::build_anim(initial, effective_ms, easing)
    }

    /// Rebuild the animation when the effective duration changes. Preserves the
    /// current target.
    ///
    /// The shared latch compares against its plain applied-duration field —
    /// what is actually baked into `anim` — not against its raw latch cell.
    /// `update` must compare against its retained duration rather than a value
    /// just assigned in `draw`; otherwise it could
    /// never differ, so a runtime theme duration change never rebuilt anything.
    pub(crate) fn sync_duration(&mut self, effective_ms: f32) {
        self.motion.sync_animation(&mut self.anim, effective_ms);
    }

    fn note_frame(&mut self, now: Instant, reduced_motion: Option<bool>) {
        self.frame_at.set(Some(now));
        if let Some(from) = self.pending_from.take() {
            let effective_ms = self.motion.effective_ms(reduced_motion);
            self.anim = self.motion.build_holding_animation(from, now, effective_ms);
            self.anim.go_mut(self.prev_expanded, now);
            self.pre_frame_endpoint = from >= 1.0 - f32::EPSILON;
        }
    }

    // Before any frame arrives the defined phase is the pre-transition
    // endpoint, never wall time.
    fn sampled_progress(&self) -> f32 {
        if let Some(from) = self.pending_from {
            return from;
        }
        self.frame_at.get().map_or_else(
            || if self.pre_frame_endpoint { 1.0 } else { 0.0 },
            |now| self.anim.interpolate(0.0, 1.0, now).clamp(0.0, 1.0),
        )
    }

    fn sampled_animating(&self) -> bool {
        self.pending_from.is_some()
            || self.frame_at.get().map_or_else(
                || self.anim.value() != self.pre_frame_endpoint,
                |now| self.anim.is_animating(now),
            )
    }
}

// ---------------------------------------------------------------------------
// ExpanderAnim wrapper widget
// ---------------------------------------------------------------------------

/// Resolves the card frame's fill and border ink from the live theme.
///
/// See [`ExpanderAnim::frame`] for why the frame is painted by this wrapper
/// rather than by the composed container it wraps.
pub(crate) type FrameResolver = Rc<dyn Fn(&iced::Theme) -> (Color, Color)>;

/// A thin wrapper widget that owns the composed expander tree and drives the
/// body height animation. The inner element is always built in the expanded
/// state (with body content present); the wrapper clips the body to the
/// animated height during the transition.
pub(crate) struct ExpanderAnim<'a, Message: Clone> {
    inner: Element<'a, Message>,
    /// The `effective_expanded` flag from the builder.
    expanded: bool,
    /// Per-instance reduced-motion override (`None` = follow theme).
    reduced_motion_override: Option<bool>,
    /// Corner radius of the card this wrapper's clip layer must follow
    /// (`0.0` for the unframed `Plain` variant). See [`Self::draw`].
    mask_radius: f32,
    /// The `Card` variant's frame (fill + 1 px border), or `None` for `Plain`.
    ///
    /// # Why the frame is painted here
    ///
    /// The framed variant's border and fill are painted by this widget rather
    /// than by a `container(..).style(..)` inside the clip layer. This
    /// widget's own `layout.bounds()` is the animated box, so the quad's four
    /// edges follow the animated height frame by frame — correct while
    /// collapsed, correct mid-transition, correct expanded. A container
    /// nested inside the clip would lay out at the full expanded height
    /// (header + divider + body) while the clip layer's height is
    /// `collapsed_h + body_h * progress`; at `progress == 0` its bottom
    /// border would sit `body_h` px below the clip and be scissored away,
    /// leaving a card with a top, a left and a right border and no bottom
    /// border for every collapsed framed expander. The quad is drawn before the clipped
    /// content so the content still paints over the fill, and it is drawn
    /// outside the clip so no edge can be scissored.
    frame: Option<FrameResolver>,
    /// Shared with the body's [`BodyGate`]. Written each frame before recursion
    /// so a settled-collapsed body suppresses its layout/update/draw/overlay
    /// work.
    body_gate: BodyGateShared,
}

impl<'a, Message: Clone + 'a> ExpanderAnim<'a, Message> {
    pub(crate) fn new(
        inner: Element<'a, Message>,
        expanded: bool,
        reduced_motion_override: Option<bool>,
        mask_radius: f32,
        frame: Option<FrameResolver>,
        body_gate: BodyGateShared,
    ) -> Self {
        Self {
            inner,
            expanded,
            reduced_motion_override,
            frame,
            mask_radius: if mask_radius.is_finite() && mask_radius > 0.0 {
                mask_radius
            } else {
                0.0
            },
            body_gate,
        }
    }

    /// Whether the body is fully hidden and in a steady state — i.e. the
    /// target is collapsed *and* the open/close animation is not in flight.
    ///
    /// This is the settled/transitioning boundary:
    /// - collapsing (`expanded == false` but still animating) is not
    ///   settled — the body is shrinking and every intermediate frame is
    ///   visible, so it must keep updating/drawing;
    /// - settled collapsed (`expanded == false` and not animating) is —
    ///   progress is 0, the body has zero visible height, and no further frame
    ///   will change that until the caller re-expands.
    ///
    /// An expanded or expanding expander is never suppressed.
    fn settled_collapsed(&self, state: &ExpanderAnimState) -> bool {
        !self.expanded && !state.sampled_animating()
    }

    /// Compute the suppression verdict and publish it into the shared gate so
    /// the body's [`BodyGate`] sees it during this same top-down pass. Returns
    /// the verdict for the caller's own use.
    fn publish_gate(&self, state: &ExpanderAnimState) -> bool {
        let suppress = self.settled_collapsed(state);
        self.body_gate.set_suppress(suppress);
        suppress
    }

    /// The cursor the inner widget should see, given the framed card's rounded
    /// mask. The clip layer masks `layout.bounds()` to
    /// a uniform `mask_radius`; a cursor sitting in a clipped corner is
    /// invisible, so it is presented to the inner content as unavailable. An
    /// unframed expander has `mask_radius == 0`, so this is a no-op there
    /// (radius 0 includes every point in the box).
    fn masked_cursor(&self, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Cursor {
        let radius = iced::border::Radius::from(self.mask_radius);
        match cursor.position() {
            Some(p) if !enzui_core::rounded_geometry::point_in_rounded_rect(p, bounds, radius) => {
                mouse::Cursor::Unavailable
            }
            _ => cursor,
        }
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for ExpanderAnim<'a, Message>
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<ExpanderAnimState>()
    }

    fn state(&self) -> widget::tree::State {
        // Policy is unknown here (there is no theme): seed the duration from the
        // explicit override if present, else suppress motion until the first
        // `draw` latches the theme — a transition that begins before
        // that draw must not animate against an unknown preference.
        let initial_rm = self.reduced_motion_override.unwrap_or(true);
        widget::tree::State::new(ExpanderAnimState::new(self.expanded, initial_rm))
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        let state = tree.state.downcast_mut::<ExpanderAnimState>();

        // Resync first from what `draw` latched, so a policy or duration change
        // collapses/rebuilds *this* transition rather than the one after it —
        // the ordering `switch.rs::diff` and `row_motion.rs::diff` document.
        // The shared latch resolves a still-unknown (`None`) theme to reduced
        // motion.
        let effective_ms = state.motion.effective_ms(self.reduced_motion_override);
        state.sync_duration(effective_ms);

        if self.expanded != state.prev_expanded {
            let from = state.sampled_progress();
            if state.frame_at.get().is_none() {
                state.pre_frame_endpoint = state.prev_expanded;
            }
            state.prev_expanded = self.expanded;
            if effective_ms <= f32::EPSILON {
                // A policy of "no motion" must not defer to the next frame.
                state.anim = state.motion.build_animation(self.expanded, 0.0);
                state.pending_from = None;
                state.pre_frame_endpoint = self.expanded;
            } else {
                // `get_or_insert` keeps the earliest queued fraction across
                // rapid rebuilds before the next frame commits it.
                state.pending_from.get_or_insert(from);
            }
        }

        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        let state = tree.state.downcast_ref::<ExpanderAnimState>();
        let progress = state.sampled_progress();

        // Publish the settled-collapsed verdict before recursing so the
        // body's `BodyGate` (reached via `inner.layout`) can skip its own full
        // child layout and reuse its cached preferred size. When re-expanding,
        // `diff` already flipped the anim target to expanded and started
        // animating, so `settled_collapsed` is false here and the real content
        // is re-measured — re-expansion never opens to a stale zero height.
        self.publish_gate(state);

        // Lay out the inner element at full (expanded) size so the content's
        // preferred height is measured. When settled collapsed the `BodyGate`
        // returns its cached size instead of re-laying-out the child, so this
        // pass is cheap; the derived `body_h` is still the real preferred
        // height and `progress == 0` collapses `animated_h` to `collapsed_h`.
        let full_node = self
            .inner
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        let full_h = full_node.size().height;
        let full_w = full_node.size().width;

        // Note: we deliberately wrap in every case, including progress == 1.0.
        // Returning `full_node` bare at the settled state made the node shape
        // ambiguous: callers discriminated on `layout.children().count() > 0`,
        // but a bare `full_node` also has children (it is a container holding a
        // column), so the settled state was misread as the wrapped state and
        // every consumer forwarded a *grandchild* layout to the inner widget.
        // That panicked `container::update` on `layout.children().next().unwrap()`
        // for an expanded expander on click. One shape, always.

        // Derive body_h from the layout tree. The inner is:
        //   container( column[ header_button, body_with_divider ] )
        // So children[0] is the column, its children[1] is the body+divider.
        //   animated_h = collapsed_h + body_h * progress
        // The measured node is shallow; the descendants' geometry is in the
        // inner's tree.
        let body_h = tree.children[0]
            .children
            .first()
            .and_then(|container_child| container_child.children.get(1))
            .map_or(0.0, |body| body.size.height);

        let collapsed_h = full_h - body_h;
        let animated_h = collapsed_h + body_h * progress;

        // Return a node with the animated height, keeping the full inner
        // layout as a child so draw can clip properly.
        layout::Node::with_children(Size::new(full_w, animated_h), vec![full_node]).apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<ExpanderAnimState>();
            let effective_ms = state.motion.effective_ms(self.reduced_motion_override);
            state.sync_duration(effective_ms);
            state.note_frame(*now, self.reduced_motion_override);
        }

        // Keep the open/close transition *driven* for as long as it is
        // running, on every event rather than only on `RedrawRequested`.
        //
        // Two distinct things are needed:
        //
        // 1. `request_redraw` — ask for the next frame. The transition is
        //    started by `diff` (the view rebuild that follows the toggle
        //    message), and `Widget::diff` is handed no `Shell` at all — its
        //    whole signature is `fn diff(&mut self, tree: &mut Tree)`. `update`
        //    is the only `Widget` hook that receives a `Shell`, so the request
        //    physically cannot be issued from the pass that starts the
        //    animation; it has to be issued from the next event to arrive.
        //    Gating it on `RedrawRequested` would make it reachable only from a
        //    frame that already exists, never true when a transition begins.
        //
        // 2. `invalidate_layout` — and this is the half that actually keeps the
        //    body at 0 px. This animation interpolates a height, but
        //    `UserInterface` lays the tree out once in `build` and thereafter
        //    re-runs `layout` only when a widget marks the layout invalid
        //    (`user_interface.rs`, `shell.revalidate_layout`). A plain
        //    `RedrawRequested` re-`draw`s the cached `base` node, so asking for
        //    frames without invalidating animates a height that is never
        //    recomputed.
        //
        // Both are strictly gated on `is_animating`, so a settled expander asks
        // for nothing and invalidates nothing — an unconditional request here
        // is a compositor spin, and an unconditional invalidation would
        // relayout the whole window on every event forever.
        {
            let state = tree.state.downcast_ref::<ExpanderAnimState>();
            if state.sampled_animating() {
                shell.request_redraw();
                // `Diff::Skip`: the widget tree itself is unchanged — only this
                // node's animated height moved — so a relayout is enough and a
                // full `diff` pass would be wasted work.
                shell.invalidate_layout();
            }
        }

        // Publish the settled-collapsed verdict before forwarding, so a
        // hidden body's `BodyGate` drops this event instead of forwarding it to
        // a self-animating child that would `request_redraw()` while invisible.
        // A `collapsing` (still-animating) body is not suppressed — it is still
        // shrinking on screen and must keep updating.
        {
            let state = tree.state.downcast_ref::<ExpanderAnimState>();
            self.publish_gate(state);
        }

        // Rectangular + radius-aware hit exclusion: the fully laid-out body
        // below the animated bounds and the framed corner cutouts are both
        // invisible, so neither may receive a real pointer.
        let cursor = crate::inert_clip::cursor_within(cursor, layout.bounds());
        let cursor = self.masked_cursor(layout.bounds(), cursor);

        // Forward the event to the inner widget. We need to reconstruct the
        // layout that matches what the inner element expects.
        // `layout()` always wraps, so the inner layout is always child 0.
        let inner_layout = layout
            .children(&tree.children)
            .next()
            .expect("expander layout always wraps the inner node");

        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            inner_layout,
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
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // Same rectangular + corner exclusion as `update`: neither collapsed
        // body space nor a framed corner may produce an inner cursor shape.
        let state = tree.state.downcast_ref::<ExpanderAnimState>();
        self.publish_gate(state);
        let cursor = crate::inert_clip::cursor_within(cursor, layout.bounds());
        let cursor = self.masked_cursor(layout.bounds(), cursor);
        let inner_layout = layout
            .children(&tree.children)
            .next()
            .expect("expander layout always wraps the inner node");
        self.inner.as_widget().mouse_interaction(
            &tree.children[0],
            inner_layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Latch the theme's reduced-motion and motion-normal duration so
        // `update` (which has no `&Theme`) can honour them.
        let state = tree.state.downcast_ref::<ExpanderAnimState>();
        let motion = crate::theme_lookup::motion(theme);
        // `Some(..)` so a later `update` can tell "policy known" from the
        // pre-draw `None`.
        state.motion.observe(
            motion.normal_ms,
            motion.reduced_motion,
            motion.easing_or(None),
        );

        // Publish the settled-collapsed verdict before recursing so the
        // hidden body's `BodyGate` skips its child `draw` entirely. The clip
        // below already reduces the body to zero visible height, but a
        // self-animating child still *executes* its `draw` (and can request
        // frames) unless the gate stops the recursion.
        self.publish_gate(state);

        let inner_layout = layout
            .children(&tree.children)
            .next()
            .expect("expander layout always wraps the inner node");

        // Clip to the animated bounds so the body doesn't overpaint.
        //
        // Follow the card's corner ARC, not just its box. This is the
        // widget's only clip layer -- the framed container's `.clip(true)` is
        // iced's rectangular scissor, and a `RoundedClip` placed there would be
        // bounds-identical to this layer and folded into it by
        // `graphics::layer::Stack::merge` (iced_graphics' `layer` compares bounds and
        // ignores `clip_radius`), silently losing the radius on wgpu.
        //
        // Rounding here is also the only version that stays correct *during*
        // the transition: `bounds` is the animated height, so the mask tracks
        // it frame by frame instead of masking to a stale resting size.
        //
        // `with_layer_rounded`'s trait default ignores the radius and forwards
        // to `start_layer`, so a backend without rounded support keeps exactly
        // the previous rectangular behaviour -- never *no* clip.
        let bounds = layout.bounds();
        // Draw-time hover status uses the same visible pointer region as event
        // hit-testing; a cursor over clipped body space cannot paint Hovered at
        // the transition edge.
        let cursor = crate::inert_clip::cursor_within(cursor, bounds);
        let cursor = self.masked_cursor(bounds, cursor);

        // The card frame is painted at the ANIMATED bounds and
        // outside the clip below, so all four edges survive at every progress
        // value. See the `frame` field docs for the defect this replaces.
        if let Some(frame) = &self.frame {
            let (fill, border) = frame(theme);
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: Border {
                        color: border,
                        width: 1.0,
                        radius: self.mask_radius.into(),
                    },
                    ..Default::default()
                },
                fill,
            );
        }

        renderer.with_layer_rounded(bounds, self.mask_radius.into(), |renderer| {
            self.inner.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                inner_layout,
                cursor,
                viewport,
            );
        });
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        // Publish the settled-collapsed verdict before recursing so a
        // hidden body's `BodyGate` drops the operation traversal entirely
        // (the clip already limits *what reaches* the collector; this stops the
        // walk from descending into the hidden body at all).
        let state = tree.state.downcast_ref::<ExpanderAnimState>();
        self.publish_gate(state);

        let inner_layout = layout
            .children(&tree.children)
            .next()
            .expect("expander layout always wraps the inner node");
        // The inner tree is always laid out at expanded height for animation,
        // but only nodes intersecting the animated rectangle are perceivable.
        // Screen focus, routed operations, text, and custom semantics through
        // that same rectangle.
        let mut clipped = crate::inert_clip::ClipOperation::new(operation, layout.bounds());
        self.inner.as_widget_mut().operate(
            &mut tree.children[0],
            inner_layout,
            viewport,
            renderer,
            &mut clipped,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let inner_layout = layout
            .children(&tree.children)
            .next()
            .expect("expander layout always wraps the inner node");

        // A settled collapsed body is not a source for an overlay. During a
        // transition, a still-visible control's menu remains an intentional
        // overlay and may escape the ancestor clip.
        let state = tree.state.downcast_ref::<ExpanderAnimState>();
        let progress = state.sampled_progress();
        if progress <= f32::EPSILON {
            Vec::new()
        } else {
            self.inner.as_widget_mut().overlay(
                &mut tree.children[0],
                inner_layout,
                renderer,
                viewport,
                translation,
                window,
            )
        }
    }
}

impl<'a, Message: Clone + 'a> From<ExpanderAnim<'a, Message>> for Element<'a, Message> {
    fn from(w: ExpanderAnim<'a, Message>) -> Self {
        Element::new(w)
    }
}
