//! Shared row-background motion: an `Animation<bool>` that eases a row's
//! hover / selection background over the theme's `motion-fast` duration.
//!
//! # Why this exists
//!
//! `menu_row`, `list_row_surface` and `tree_view` all deferred the same thing —
//! the `motion-fast` transition on a row background — for the same (false)
//! reason: "iced has no animation primitive". It does
//! (`iced/core/src/animation.rs`), and `switch.rs` / `code_block.rs` /
//! `expander.rs` already drive one.
//!
//! All three of those widgets are composed builders: each ends in
//! `focus_button(..).style_focus(closure)`, and the closure receives only
//! `(&Theme, FocusStatus)`. There is no seam through which a per-frame
//! animation progress could reach it, and none of the three owns a `Widget`
//! impl of its own where an `Animation` could live. This module supplies both
//! halves, once:
//!
//! * [`RowMotionState`] — the `Animation<bool>` plus the duration/reduced-motion
//!   latches, following `switch.rs::WidgetState` exactly (raw `fast_ms` and
//!   `theme_reduced_motion` latched *separately*, so a per-instance
//!   `reduced_motion(false)` can recover a duration a reduced-motion theme
//!   collapsed).
//! * [`RowMotion`] — a thin wrapper `Widget` that owns the composed row, drives
//!   the animation, and publishes the interpolated progress through a shared
//!   [`RowMotionProgress`] the style closure reads at draw time.
//!
//! Three call sites is not yet an abstraction, but three *copies* of a
//! redraw-gating state machine is how the compositor spin gets
//! reintroduced in two of three files. The shared piece is deliberately the
//! mechanism only: each widget decides for itself *what* the progress means
//! (which colours it blends between).
//!
//! # The redraw contract
//!
//! `update` requests the next frame only while `is_animating(now)` is true.
//! An unconditional request spins the compositor at 100% forever — a worse
//! defect than the missing animation. [`row_motion_settles`] pins the negative.
//!
//! # Reduced motion
//!
//! Two independent mechanisms are required:
//!
//! 1. `MotionScale::ms()` returns 0 under a reduced-motion theme, and
//! 2. [`RowMotionState::make_anim`] gates on the resolved flag directly.
//!
//! The gate in (2) is what a per-instance `reduced_motion(true)` override acts
//! through when the *theme* is animating, so it is not redundant with (1).

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Shell, overlay, renderer};
use iced::animation::Animation;
use iced::mouse;
use iced::time::Instant;
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector, window};
use std::cell::Cell;
use std::rc::Rc;

/// `MotionScale::default().fast_ms` — the duration used before any theme has
/// been observed (widget state is constructed before the first `draw`).
pub(crate) const DEFAULT_FAST_MS: f32 = 120.0;

/// The interpolated row-motion progress, shared between the [`RowMotion`]
/// wrapper (which writes it each frame) and the row's `style_focus` closure
/// (which reads it at draw time).
///
/// `Rc<Cell<f32>>` rather than a channel or a lock because iced's view/draw
/// cycle is single-threaded and the write always happens in the same frame,
/// strictly before the read: `RowMotion::draw` sets it and *then* calls the
/// inner element's `draw`, which is what invokes the style closure.
pub(crate) type RowMotionProgress = Rc<Cell<f32>>;

/// The row's active endpoint appearance, latched across frames so a
/// fade-*out* knows which colour it is easing away from.
///
/// The style closure cannot remember this itself: the view — and every `Rc`
/// created in it, including [`RowMotionProgress`] — is rebuilt from scratch on
/// every frame, so a view-local latch is reset before it can carry anything.
/// The *persistent* copy therefore lives in [`RowMotionState`]; [`RowMotion`]
/// copies it into this shared handle before the inner element draws (so the
/// closure reads last frame's endpoint) and copies the closure's write back
/// out afterwards (so the next frame inherits it). Two channels — background
/// and border — because the row eases both.
pub(crate) type RowActiveColor = Rc<Cell<Color>>;

/// shared group-corner progress for one direction (above or below).
///
/// `0.0` = standalone (fully rounded), `1.0` = grouped (corners squared).
/// [`RowMotion`] publishes the frame-coherent value before the inner element
/// draws, and the row's style/clip closures read it at draw time.
pub(crate) type RowGroupHandle = Rc<Cell<f32>>;

// Premultiplied channels keep transparent endpoints from tinting a fade black.
pub(crate) fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let alpha = a.a + (b.a - a.a) * t;
    if alpha <= f32::EPSILON {
        return Color::TRANSPARENT;
    }
    Color {
        r: (a.r * a.a * (1.0 - t) + b.r * b.a * t) / alpha,
        g: (a.g * a.a * (1.0 - t) + b.g * b.a * t) / alpha,
        b: (a.b * a.a * (1.0 - t) + b.b * b.a * t) / alpha,
        a: alpha,
    }
}

/// The animation state for one animated row background.
///
/// Follows `switch.rs::WidgetState`: the raw theme duration and the theme's
/// reduced-motion flag are latched separately in `draw`, and combined in
/// [`Self::effective_fast_ms`] with the per-instance override. Latching
/// `MotionScale::ms(Fast)` instead would already be 0 under a reduced-motion
/// theme, and a `reduced_motion(false)` override could never un-collapse it.
#[derive(Debug)]
pub(crate) struct RowMotionState {
    /// `false` = resting background, `true` = active (hovered / selected).
    anim: Animation<bool>,
    /// The active flag as of the previous `diff` — the edge detector.
    prev_active: bool,
    /// Set by `diff` when the flag changed; consumed by the next
    /// `RedrawRequested`, which starts the transition at *that frame's*
    /// instant.
    ///
    /// Not started in `diff` for the reason `switch.rs` documents at length:
    /// `diff` runs during `UserInterface::build`, outside the frame loop, where
    /// the only available instants are the wall clock and a possibly-stale
    /// latched frame. A transition stamped with a seconds-old instant is
    /// already finished before it is first drawn — the row teleports, which is
    /// the bug this module exists to remove.
    pending: bool,
    /// Set by `diff` when the row becomes selected beside an already selected
    /// neighbour: it joins that run at full strength on the next frame
    /// instead of fading in, or the run shows a dark band through its middle
    /// for the length of the fade.
    snap: bool,
    /// The instant of the most recent `RedrawRequested`, so the phase is a
    /// function of the frame timestamp iced supplies rather than
    /// `Instant::now()` (deterministic under test).
    frame_at: Cell<Option<Instant>>,
    /// Shared raw-duration, unknown-policy, easing, and applied-duration latch.
    motion: crate::motion::MotionLatch,
    /// A per-instance raw duration override in ms, from a widget token that
    /// names its own fade (`CarouselTokens::indicator_fade_ms`). `None` follows
    /// the theme's `motion-fast`.
    ///
    /// Raw, like [`Self::fast_ms`]: reduced motion is applied downstream in
    /// [`Self::effective_fast_ms`], so a `0` here means the caller asked for an
    /// instant change and a reduced-motion theme can still collapse a non-zero
    /// one.
    raw_override_ms: Cell<Option<f32>>,
    /// The last active-endpoint background colour the style closure
    /// painted, latched across frames so a fade-out eases away from it rather
    /// than snapping to resting. `None` until a closure has written one;
    /// see [`RowActiveColor`].
    active_bg: Cell<Option<Color>>,
    /// The last active-endpoint border colour, latched the same way.
    active_border: Cell<Option<Color>>,

    // --- group-corner transition state ---
    /// Group flags from the last frame the row was actively selected. Latched
    /// in `diff` each frame `active` is true, so a deselection fade-out keeps
    /// the corners the row had while it was selected, and cleared in
    /// `update` as soon as the pointer takes the fill over.
    ///
    /// Both halves are load-bearing. Held too briefly, the fade breaks its own
    /// group edge. Held too long, the row incorrectly carries squared group
    /// geometry into a hover or pressed pill instead of keeping the full
    /// `row_radius`.
    latched_group_above: bool,
    latched_group_below: bool,
    /// Edge detectors for the builder-supplied group flags.
    prev_group_above: bool,
    prev_group_below: bool,
    /// Independent corner transitions for when a *still-selected* row loses
    /// a neighbor. `true` = neighbor present (squared), `false` = standalone
    /// (rounded). Uses the same `motion-fast` duration as the main ease.
    group_above_anim: Animation<bool>,
    group_below_anim: Animation<bool>,
    /// Pending edge for the group animations — same deferred-start contract
    /// as [`Self::pending`].
    pending_group_above: bool,
    pending_group_below: bool,
    /// Applied-ms tracker for the group animations, separate from the main
    /// animation's `motion.applied_ms` so `sync_animation` does not short-
    /// circuit when called for both.
    group_applied_ms: f32,
}

impl RowMotionState {
    /// A state resting at `active`, before any theme has been observed.
    pub(crate) fn new(active: bool) -> Self {
        let default_easing = enzui_theme::MotionEasing::default();
        Self {
            anim: Self::make_anim(active, DEFAULT_FAST_MS, default_easing),
            prev_active: active,
            pending: false,
            snap: false,
            frame_at: Cell::new(None),
            motion: crate::motion::MotionLatch::new(
                DEFAULT_FAST_MS,
                DEFAULT_FAST_MS,
                default_easing,
            ),
            raw_override_ms: Cell::new(None),
            active_bg: Cell::new(None),
            active_border: Cell::new(None),
            // group corners start standalone
            latched_group_above: false,
            latched_group_below: false,
            prev_group_above: false,
            prev_group_below: false,
            group_above_anim: Self::make_anim(false, DEFAULT_FAST_MS, default_easing),
            group_below_anim: Self::make_anim(false, DEFAULT_FAST_MS, default_easing),
            pending_group_above: false,
            pending_group_below: false,
            group_applied_ms: DEFAULT_FAST_MS,
        }
    }

    /// Row backgrounds animate over `motion-fast` with `ease`; `EaseOut`
    /// is the `lilt` equivalent `switch.rs` and `code_block.rs` both picked.
    ///
    /// A `fast_ms` of 0 (reduced motion) yields `Duration::ZERO`, i.e. the row
    /// lands on its target colour on the very frame the flag changes. That is
    /// correct for a *transition*; it would be wrong for a dwell (see the
    /// `toast_host::sync_motion` note).
    fn make_anim(
        initial: bool,
        fast_ms: f32,
        easing: enzui_theme::MotionEasing,
    ) -> Animation<bool> {
        crate::motion::build_anim(initial, fast_ms, easing)
    }

    /// Rebuild `anim` when the effective duration changes — including
    /// collapsing to 0 under reduced motion and expanding back when the policy
    /// lifts. Preserves the current target.
    pub(crate) fn sync_duration(&mut self, effective_ms: f32) {
        self.motion.sync_animation(&mut self.anim, effective_ms);
    }

    /// Rebuild the group-corner animations when the effective duration
    /// changes.
    ///
    /// They carry their own applied-ms tracker so this does not short-circuit
    /// against the main animation's [`crate::motion::MotionLatch::applied_ms`]
    /// (or the other way round) when both are synced on one frame.
    fn sync_group_durations(&mut self, effective_ms: f32) {
        if (effective_ms - self.group_applied_ms).abs() < f32::EPSILON {
            return;
        }
        self.group_applied_ms = effective_ms;
        self.group_above_anim = self
            .motion
            .build_animation(self.group_above_anim.value(), effective_ms);
        self.group_below_anim = self
            .motion
            .build_animation(self.group_below_anim.value(), effective_ms);
    }

    /// The effective `motion-fast` in ms: 0 when reduced motion is in force
    /// (per-instance override beating the theme), else the latched raw value.
    ///
    /// This is reduced-motion mechanism (2). It is independent of
    /// `MotionScale::ms()` returning 0 (mechanism (1)) because the override can
    /// force reduction while the theme animates.
    /// When the override is unset and the theme policy is still *unknown*
    /// (`None`, no `draw` yet), motion is suppressed (0 ms) — the
    /// conservative default, so a transition mounted before the first draw does
    /// not animate against an unknown preference.
    pub(crate) fn effective_fast_ms(&self, override_reduced: Option<bool>) -> f32 {
        self.motion
            .effective_ms_with_raw(override_reduced, self.raw_override_ms.get())
    }

    /// The instant to evaluate the animation at: the latest frame timestamp,
    /// falling back to the wall clock before the first frame.
    fn now(&self) -> Instant {
        self.frame_at.get().unwrap_or_else(Instant::now)
    }

    /// The current interpolated progress in `0.0..=1.0`.
    pub(crate) fn progress(&self) -> f32 {
        self.anim.interpolate(0.0, 1.0, self.now()).clamp(0.0, 1.0)
    }

    /// compute the frame-coherent group-corner progress for one
    /// direction, accounting for deselection fade-out and independent
    /// neighbor-loss transitions.
    ///
    /// Returns `0.0` (standalone/rounded) to `1.0` (grouped/squared).
    ///
    /// `hovered` is what keeps hover and press out of the geometry.
    /// `row_style_at` paints `lerp(resting, endpoint, progress)`, and for an
    /// unselected *hovered* row the endpoint is the hover/pressed pill, so no
    /// selection ink is left on the row at all; holding group geometry would
    /// fuse a bare hover pill to its neighbour. Deselecting with the pointer
    /// parked on the row therefore rounds immediately, which is exactly what
    /// that row paints. `update` clears the latch on the same condition, so
    /// the corner cannot spring back to squared when the pointer leaves
    /// mid-fade.
    ///
    /// `instant` is reduced motion / a zero effective duration. It makes the
    /// active branch land on the model's flag on the very next painted frame
    /// even when `update` has not yet consumed the `pending_group_*` edge —
    /// the same contract `draw` applies to the main progress, without which a
    /// reduced-motion first draw after a neighbor loss still paints squared.
    #[expect(
        clippy::too_many_arguments,
        reason = "group geometry keeps each independently varying transition input explicit"
    )]
    fn effective_group_t(
        &self,
        active: bool,
        fade_progress: f32,
        group_flag: bool,
        latched_flag: bool,
        group_anim: &Animation<bool>,
        instant: bool,
        hovered: bool,
    ) -> f32 {
        if active {
            // Row is currently selected.
            if group_flag {
                // Neighbor present (or just gained): snap to grouped, so the
                // shared edge never shows a seam while the new member's fill
                // fades in.
                1.0
            } else if instant {
                // Reduced motion: standalone now, not one frame later.
                0.0
            } else {
                // Just lost a neighbor: independent corner ease.
                group_anim.interpolate(0.0, 1.0, self.now()).clamp(0.0, 1.0)
            }
        } else if latched_flag && fade_progress > 0.0 && !hovered {
            // Deselecting. HOLD the group geometry for the whole fade instead
            // of interpolating it: while the row still paints any part of the
            // selection fill, an interior radius is exactly the reported
            // artifact (two rounded stubs with a background notch between
            // them). The drop to 0 lands on the frame the fill reaches
            // resting, where the row is already indistinguishable from an
            // unselected one.
            1.0
        } else {
            0.0
        }
    }
}

/// A wrapper `Widget` that owns a composed row element and animates its
/// background between a resting and an active appearance.
///
/// The wrapper is deliberately transparent to layout, semantics, focus and
/// events: every trait method delegates to the inner element. Its only job is
/// to own the [`RowMotionState`], advance it on frames, and publish the
/// progress to the shared [`RowMotionProgress`] the row's style closure reads.
pub(crate) struct RowMotion<'a, Message> {
    inner: Element<'a, Message>,
    /// Whether the row is in its active (hovered / selected) appearance.
    ///
    /// For selection this is a builder flag. For hover it is recomputed each
    /// frame from the cursor in [`Self::update`], because hover is not a
    /// builder input — `focus_button` derives it from the live cursor.
    active: bool,
    /// When true, `active` is recomputed from the cursor each frame (hover
    /// rows) rather than only edge-detected from the builder flag.
    track_hover: bool,
    /// Per-instance reduced-motion override; `None` follows the theme.
    reduced_motion: Option<bool>,
    /// A per-instance raw duration in ms from a widget's own fade token
    /// (`CarouselTokens::indicator_fade_ms`); `None` uses the theme's
    /// `motion-fast`. See [`Self::duration_ms`].
    raw_duration_ms: Option<f32>,
    progress: RowMotionProgress,
    /// The shared active-endpoint latch handles the style closure reads/writes
    /// `None` for callers that do not ease a colour endpoint (they get
    /// the progress scalar only). When present, `draw` publishes the persistent
    /// latch from [`RowMotionState`] into these before the inner element draws
    /// and reads the closure's write back afterwards. See [`RowActiveColor`].
    active_colors: Option<(RowActiveColor, RowActiveColor)>,
    /// current model group flags and shared handles for the
    /// frame-coherent effective group-corner progress.
    group_state: Option<GroupStateHandles>,
    /// paint-only visibility signal for a permanently mounted trailing
    /// action, plus the owning composite's model focus (embedded rows).
    trailing_reveal: Option<(crate::list_row_surface::TrailingReveal, bool, bool)>,
}

/// the group-flag inputs and shared output handles.
pub(crate) struct GroupStateHandles {
    pub above: bool,
    pub below: bool,
    pub above_handle: RowGroupHandle,
    pub below_handle: RowGroupHandle,
}

impl<'a, Message> RowMotion<'a, Message> {
    /// Wrap `inner`, animating between resting and active as `active` changes.
    ///
    /// `progress` must be the same handle the row's style closure captured.
    pub(crate) fn new(
        inner: Element<'a, Message>,
        active: bool,
        reduced_motion: Option<bool>,
        progress: RowMotionProgress,
    ) -> Self {
        Self {
            inner,
            active,
            track_hover: false,
            reduced_motion,
            raw_duration_ms: None,
            progress,
            active_colors: None,
            group_state: None,
            trailing_reveal: None,
        }
    }

    /// Latch the row's active-endpoint background and border colours across
    /// frames, so a fade-*out* eases away from the last active colour instead
    /// of snapping to resting.
    ///
    /// `bg` and `border` must be the same handles the row's style closure
    /// captured. Only rows that ease a colour endpoint (list rows, tree rows)
    /// call this; the carousel dot, which eases a single scalar, does not.
    pub(crate) fn active_colors(mut self, bg: RowActiveColor, border: RowActiveColor) -> Self {
        self.active_colors = Some((bg, border));
        self
    }

    /// supply the model's group flags and the shared handles the
    /// style/clip closures will read for frame-coherent corner geometry.
    pub(crate) fn group(
        mut self,
        above: bool,
        below: bool,
        above_handle: RowGroupHandle,
        below_handle: RowGroupHandle,
    ) -> Self {
        self.group_state = Some(GroupStateHandles {
            above,
            below,
            above_handle,
            below_handle,
        });
        self
    }

    /// Publish the row's live hover/focus state to its trailing paint wrapper.
    pub(crate) fn trailing_reveal(
        mut self,
        reveal: Option<crate::list_row_surface::TrailingReveal>,
        model_focus: bool,
        parented: bool,
    ) -> Self {
        self.trailing_reveal = reveal.map(|reveal| (reveal, model_focus, parented));
        self
    }

    /// Drive this transition at `ms` raw milliseconds instead of the
    /// theme's `motion-fast`, for a widget that owns a named fade token.
    ///
    /// The value is raw on purpose: reduced motion is still applied on top at
    /// the point of use, so a reduced-motion theme (or a per-instance
    /// `reduced_motion(true)`) collapses it, and `.reduced_motion(false)`
    /// recovers exactly this number. A `0` is honoured literally as
    /// "instant"; non-finite and negative inputs are ignored (the theme's
    /// duration is used instead) rather than poisoning `Duration`.
    pub(crate) fn duration_ms(mut self, ms: f32) -> Self {
        if ms.is_finite() && ms >= 0.0 {
            self.raw_duration_ms = Some(ms);
        }
        self
    }

    /// Also drive the animation from pointer hover, recomputed each frame from
    /// the cursor position over the row's bounds.
    pub(crate) fn track_hover(mut self, track: bool) -> Self {
        self.track_hover = track;
        self
    }

    /// The target the animation should be heading toward this frame.
    fn target(&self, hovered: bool) -> bool {
        self.active || (self.track_hover && hovered)
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for RowMotion<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<RowMotionState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(RowMotionState::new(self.active))
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        let state = tree.state.downcast_mut::<RowMotionState>();
        // Resync first from what `draw` latched, so a reduced-motion theme
        // collapses *this* transition rather than the one after it — the same
        // ordering `switch.rs::diff` documents.
        let effective_ms = state.effective_fast_ms(self.reduced_motion);
        state.sync_duration(effective_ms);
        state.sync_group_durations(effective_ms);

        // Edge-detect the builder flag. Hover is handled in `update`, where a
        // cursor is available.
        if self.active != state.prev_active {
            state.prev_active = self.active;
            state.pending = true;
            state.snap = self.active
                && self
                    .group_state
                    .as_ref()
                    .is_some_and(|group| group.above || group.below);
        }

        // latch group flags while active; edge-detect group changes.
        if let Some(gs) = &self.group_state {
            // Latch while actively selected so a deselect fade-out inherits
            // the corners the row had when it was last fully selected.
            if self.active {
                state.latched_group_above = gs.above;
                state.latched_group_below = gs.below;
            }

            // Edge-detect group flag changes on a still-selected row.
            if gs.above != state.prev_group_above {
                state.prev_group_above = gs.above;
                if self.active {
                    state.pending_group_above = true;
                }
            }
            if gs.below != state.prev_group_below {
                state.prev_group_below = gs.below;
                if self.active {
                    state.pending_group_below = true;
                }
            }
        }

        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.inner.as_widget().is_void()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
            let hovered = cursor.is_over(layout.bounds());
            let target = self.target(hovered);
            let state = tree.state.downcast_mut::<RowMotionState>();
            state.frame_at.set(Some(*now));

            let effective_ms = state.effective_fast_ms(self.reduced_motion);
            state.sync_duration(effective_ms);
            state.sync_group_durations(effective_ms);

            // A hover change is its own edge, detected here because `diff` has
            // no cursor. `anim.value()` is the current target, so this fires
            // once per transition rather than every frame.
            if state.snap {
                // Joining a selected run: no fade, so the run stays whole.
                state.snap = false;
                state.pending = false;
                state.anim = state.motion.build_animation(target, effective_ms);
            } else if state.anim.value() != target || state.pending {
                state.pending = false;
                state.anim.go_mut(target, *now);
            }

            self.progress.set(state.progress());

            // drop the group latch the moment the pointer takes the
            // fill over. An unselected hovered/pressed row paints its own
            // pill from the rewritten endpoint latch, so the selection
            // ink is gone for good; keeping the latch would spring the corner
            // back to squared as soon as the pointer left again, over a fill
            // that holds no selection colour at all.
            //
            // No drain-time reset is needed beside this: the only reader gates
            // on `fade_progress > 0.0`, which is the very number the style
            // closure blends the fill with, so a latch that outlives its fade
            // can only be read on frames where the fill is still not resting.
            let pointer_pill = self.track_hover && hovered;
            if !self.active && pointer_pill {
                state.latched_group_above = false;
                state.latched_group_below = false;
            }

            // start / snap group-corner animations.
            if let Some(gs) = &self.group_state {
                // Above
                if state.pending_group_above {
                    state.pending_group_above = false;
                    if gs.above {
                        // Gaining a neighbor: snap to grouped. Built through
                        // the latch so the curve stays the theme's.
                        state.group_above_anim = state.motion.build_animation(true, effective_ms);
                    } else {
                        // Losing a neighbor: animate from grouped → standalone.
                        state.group_above_anim.go_mut(false, *now);
                    }
                }
                // Below
                if state.pending_group_below {
                    state.pending_group_below = false;
                    if gs.below {
                        state.group_below_anim = state.motion.build_animation(true, effective_ms);
                    } else {
                        state.group_below_anim.go_mut(false, *now);
                    }
                }
            }

            // Ask for the next frame only while still moving. Unconditional
            // requests spin the compositor forever.
            let group_animating = state.group_above_anim.is_animating(*now)
                || state.group_below_anim.is_animating(*now);
            if state.anim.is_animating(*now) || group_animating {
                shell.request_redraw();
            }
        } else if matches!(event, Event::Mouse(_)) && self.track_hover {
            // A bare cursor move does not carry a frame instant, but it is what
            // tells us hover changed. Schedule a frame so the branch above can
            // start the transition at a real timestamp.
            let hovered = cursor.is_over(layout.bounds());
            let target = self.target(hovered);
            let state = tree.state.downcast_mut::<RowMotionState>();
            if state.anim.value() != target {
                state.pending = true;
                shell.request_redraw();
            }
        }

        self.inner.as_widget_mut().update(
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
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(
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
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<RowMotionState>();

        // resolve reveal from the same live row bounds/cursor used by
        // hover motion, plus actual retained FocusButton state anywhere in the
        // row subtree. This runs before the inner draw, so the trailing wrapper
        // sees the current value on this frame. Layout, events and operations
        // never consult the signal.
        if let Some((reveal, model_focus, parented)) = &self.trailing_reveal {
            reveal.set(
                (*parented && reveal.get())
                    || cursor.is_over(layout.bounds())
                    || *model_focus
                    || crate::focus_button::any_focused_in(&tree.children[0]),
            );
        }

        // Latch the theme's motion policy — `draw` is the only lifecycle method
        // with a `&Theme`. The RAW duration, not `motion.ms(Fast)`,
        // which is already 0 under a reduced-motion theme and could never be
        // recovered by a per-instance `reduced_motion(false)`.
        let motion = crate::theme_lookup::motion(theme);
        // `Some(..)` so `update`/`diff` can tell "policy known" from the
        // pre-draw `None`.
        state.motion.observe(
            motion.fast_ms,
            motion.reduced_motion,
            motion.easing_or(None),
        );
        // Also RAW (see `raw_override_ms`): the widget's own fade token, if it
        // has one. Latched here beside the theme so `diff`/`update` resolve
        // both through the single `effective_fast_ms` seam.
        state.raw_override_ms.set(self.raw_duration_ms);

        // Publish this frame's progress before the inner element draws, since
        // drawing is what invokes the style closure that reads it.
        //
        // Before the first `RedrawRequested` there is no frame instant, so a
        // reduced-motion theme has not yet had a chance to collapse the
        // duration through `update`. Resolve the policy here too, so the very
        // first painted frame already honours it.
        let effective_ms = state.effective_fast_ms(self.reduced_motion);

        // The settled truth for this frame, from the live cursor.
        let target = self.target(cursor.is_over(layout.bounds()));

        if effective_ms <= 0.0 {
            // Reduced motion: land on the target now.
            //
            // This deliberately includes a `pending` edge that `diff` recorded
            // and no frame has started yet. "Reduced motion" means the change
            // is visible on the very next painted frame, not one frame later —
            // waiting for `update` would insert exactly the one-frame delay the
            // preference exists to remove. It is also what makes the policy
            // *observable*: `render_transition` draws immediately after the
            // edge, so this branch is the only place a reduced-motion theme can
            // distinguish itself from an animating one.
            self.progress.set(if target { 1.0 } else { 0.0 });
        } else if !state.pending
            && !state.anim.is_animating(state.now())
            && state.anim.value() != target
        {
            // No transition pending, none in flight, yet the animation's value
            // disagrees with what the cursor says. That is the signature of a
            // *static* render: a tree built with the pointer already parked
            // over the row and drawn without ever running a frame loop.
            //
            // `!state.pending` is load-bearing. A pending edge means a real
            // transition is about to start on the next frame, and snapping here
            // would skip it entirely — including under an animating theme,
            // which would make the animation unobservable and every
            // reduced-motion measurement vacuous.
            //
            // Without the branch as a whole, a widget built with the cursor
            // already over it paints un-hovered on its first frame, which
            // `preferences_row_geometry::action_row_radius_follows_the_token`
            // catches.
            self.progress.set(if target { 1.0 } else { 0.0 });
        } else {
            self.progress.set(state.progress());
        }

        // publish the persistent active-endpoint latch into the shared
        // handles the style closure reads. The closure blends
        // `lerp(resting, latch, progress)` and overwrites the latch only when
        // this frame is itself eased-active, so a fade-*out* frame reads the
        // colour the last active frame painted rather than snapping to resting.
        // A `None` persistent latch (never active yet) leaves the view's seed
        // untouched; at that point `progress` is 0 so the endpoint is moot.
        if let Some((bg, border)) = &self.active_colors {
            if let Some(c) = state.active_bg.get() {
                bg.set(c);
            }
            if let Some(c) = state.active_border.get() {
                border.set(c);
            }
        }

        // publish frame-coherent group-corner progress.
        if let Some(gs) = &self.group_state {
            // This frame's painted fade, already reduced-motion-resolved by
            // the branches above -- the same number the style closure blends
            // with, so the corner cannot disagree with the fill.
            let fade_p = self.progress.get();
            let instant = effective_ms <= 0.0;
            // `target` above already folded in `track_hover`; recover the
            // hover component on its own, since only an *unselected* hovered
            // row needs it.
            let hovered = self.track_hover && cursor.is_over(layout.bounds());
            let above_t = state.effective_group_t(
                self.active,
                fade_p,
                gs.above,
                state.latched_group_above,
                &state.group_above_anim,
                instant,
                hovered,
            );
            let below_t = state.effective_group_t(
                self.active,
                fade_p,
                gs.below,
                state.latched_group_below,
                &state.group_below_anim,
                instant,
                hovered,
            );
            gs.above_handle.set(above_t);
            gs.below_handle.set(below_t);
        }

        self.inner.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        // Read the closure's write back into persistent state so the next
        // frame inherits it. The closure overwrites the shared handle only on
        // eased-active frames; on inactive frames it is left holding the value
        // we just published, so this is a no-op that preserves the latch.
        if let Some((bg, border)) = &self.active_colors {
            state.active_bg.set(Some(bg.get()));
            state.active_border.set(Some(border.get()));
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.inner.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<RowMotion<'a, Message>> for Element<'a, Message> {
    fn from(w: RowMotion<'a, Message>) -> Self {
        Element::new(w)
    }
}
