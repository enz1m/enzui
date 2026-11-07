//! The widget-side edge of the theme's motion policy.
//!
//! [`enzui_theme::MotionScale`] is deliberately renderer-neutral: it
//! carries durations as plain `f32` milliseconds and the curve as a semantic
//! [`MotionEasing`] enum, so the theme crate stays free of `iced_core`. This
//! module is the single place those semantic values become the `iced` types an
//! `Animation` actually wants.
//!
//! # The easing half
//!
//! Motion policy is a semantic duration plus an easing policy, both reaching
//! every animated widget, so a theme restyles motion feel as well as speed
//! instead of each widget hard-coding a curve at its `Animation::easing(..)`
//! call site. This module is the easing half.
//!
//! # The deviation seam
//!
//! The tooltip's symmetric fade is a *documented* deviation, not an oversight:
//! both ends of a reveal should be soft. It states that with
//! `MotionScale::easing_or(Some(MotionEasing::EaseInOut))` rather than by
//! hard-coding a curve, so a theme that changes the policy still reaches every
//! *other* widget and the deviation is one greppable call rather than an
//! invisible literal.
//!
//! # Named effects for any element
//!
//! [`motion`] wraps any `Element` with a theme-driven [`MotionEffect`]. Slide
//! supports all four edges, size supports width, height, or both, scale is a
//! paint-only centre transform, and fade composites the complete descendant
//! subtree at one alpha. Nest wrappers to compose effects. Size is the only
//! effect that invalidates layout; slide, scale, and fade reuse the cached node.
//! Every effect stops requesting frames offscreen and at rest, jumps under
//! reduced motion, and removes its settled-hidden child from the widget tree.
//! Slide, size, and scale allocate nothing while interpolating; fade uses the
//! renderer's pooled compositing target only at partial alpha, while both
//! endpoints take the ordinary allocation-free path. Reachability is
//! asymmetric: an
//! *arriving* child is focusable and publishes accessibility from its first
//! mounted frame, while only the pointer waits for usable painted geometry; a
//! *departing* child is inert immediately. [`RetainedPresentation`] keeps a
//! pane's small app model alive while an exit is painted, using the same
//! generation protocol as modal presentation; the outgoing subtree becomes
//! inert and leaves accessibility immediately. Framework [`crate::stack`]
//! containers and [`crate::sidebar_content`] charge spacing only between
//! non-empty laid-out children, so a hidden wrapper also relinquishes its
//! separator. Two adjacent hidden wrappers therefore leave no extra gap — the
//! surviving neighbours retain their one ordinary gap, exactly as if both
//! wrappers were absent.
//!
//! # Fade and cross-fade compositing
//!
//! A generic fade must composite an arbitrary subtree into an offscreen layer
//! and apply one alpha when the completed layer is blended back. Scaling a
//! wrapper's resolved colours is observably partial: it cannot reach text,
//! images, SVGs, canvases, or custom descendants. The fork's
//! `start_layer_alpha(bounds, alpha)` seam now implements real group
//! compositing in both tiny-skia and wgpu, so [`MotionEffect::Fade`] uses that
//! seam directly. Nested fades multiply and overlapping descendants are
//! attenuated once.
//!
//! [`CrossFade`] lays out both subtrees on every frame and reserves the maximum
//! of their widths and heights. This costs two retained trees, two layouts, and
//! two draws while moving, but its reservation cannot jump when differently
//! sized content reaches the endpoint. At an intermediate fraction, the
//! incoming subtree is the backing image and the outgoing subtree is one alpha
//! group at `1 - t`. In an opaque overlap this is exactly
//! `outgoing * (1 - t) + incoming * t`, while using one compositing group avoids
//! the under-opaque overlap produced by two source-over alpha groups. Fully
//! settled endpoints draw only their corresponding subtree.

use std::cell::Cell;

use enzui_theme::{MotionEasing, MotionScale, MotionToken};
use iced::advanced::widget::{self, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, overlay, renderer};
use iced::animation::{Animation, Easing};
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse};

/// A generation identifying one retained presentation.
///
/// Exit-completion messages carry this key so a late completion from a pane
/// that was dismissed and then replaced cannot remove the replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationKey(u64);

impl PresentationKey {
    /// The monotonically increasing generation value.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentationPhase {
    Open,
    Closing,
}

/// App-owned data retained just long enough for a [`Motion`] exit.
///
/// iced elements may borrow application state for one `view()` lifetime, so a
/// widget cannot safely retain yesterday's element. This state machine retains
/// the small presentation model instead. Rebuild the child from [`Self::value`]
/// on each view, pass [`Self::is_open`] to [`motion`], and route
/// [`Motion::on_hidden_with`] back to [`Self::finish_exit`]. A closing value is
/// paint-only: `Motion` removes it from input, focus, overlays, and
/// accessibility as soon as the target becomes hidden.
///
/// This is also the implementation behind
/// [`crate::modal_host::ModalPresentation`], so panes and dialogs share the
/// same generation/race protocol instead of maintaining two vocabularies.
#[derive(Debug, Clone)]
pub struct RetainedPresentation<T> {
    current: Option<(PresentationKey, PresentationPhase, T)>,
    next_generation: u64,
}

impl<T> Default for RetainedPresentation<T> {
    fn default() -> Self {
        Self {
            current: None,
            next_generation: 1,
        }
    }
}

impl<T> RetainedPresentation<T> {
    /// Present `value`, replacing any open or exiting generation.
    pub fn present(&mut self, value: T) -> PresentationKey {
        let key = PresentationKey(self.next_generation);
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.current = Some((key, PresentationPhase::Open, value));
        key
    }

    /// Begin exit and return the generation whose completion is expected.
    /// Calling this again while already closing is idempotent.
    pub fn dismiss(&mut self) -> Option<PresentationKey> {
        let (key, phase, _) = self.current.as_mut()?;
        *phase = PresentationPhase::Closing;
        Some(*key)
    }

    /// Remove an outgoing value after its matching motion reports settlement.
    /// A stale completion after [`Self::present`] is deliberately ignored.
    pub fn finish_exit(&mut self, key: PresentationKey) -> bool {
        let matches = self.current.as_ref().is_some_and(|(current, phase, _)| {
            *current == key && *phase == PresentationPhase::Closing
        });
        if matches {
            self.current = None;
        }
        matches
    }

    /// The retained model to render, including during exit.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        self.current.as_ref().map(|(_, _, value)| value)
    }

    /// Mutable access to the retained model.
    pub fn value_mut(&mut self) -> Option<&mut T> {
        self.current.as_mut().map(|(_, _, value)| value)
    }

    /// The current generation, while either open or closing.
    #[must_use]
    pub fn key(&self) -> Option<PresentationKey> {
        self.current.as_ref().map(|(key, _, _)| *key)
    }

    /// Whether the retained generation is still active rather than exiting.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(_, phase, _)| *phase == PresentationPhase::Open)
    }

    /// Whether a dismissed generation is retained for visual exit only.
    #[must_use]
    pub fn is_closing(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(_, phase, _)| *phase == PresentationPhase::Closing)
    }
}

/// Theme motion observed by a widget whose non-draw lifecycle has no `&Theme`.
///
/// Animated widgets keep this beside their [`Animation<bool>`]. [`Self::observe`]
/// is called from `draw` and deliberately stores the raw duration separately
/// from the reduced-motion policy. [`Self::effective_ms`] combines them only at
/// the point of use, treating an unobserved theme as reduced motion. The plain
/// `applied_ms` field records what is actually baked into the animation, so
/// change detection never compares a freshly written latch against itself.
///
/// The cells are intentional: `Widget::draw` receives shared state, while
/// `diff` and `update` later need the values it observed.
#[derive(Debug, Clone)]
pub(crate) struct MotionLatch {
    raw_ms: Cell<f32>,
    theme_reduced_motion: Cell<Option<bool>>,
    easing: Cell<MotionEasing>,
    applied_ms: f32,
}

impl MotionLatch {
    /// Create a latch before any theme-bearing draw.
    ///
    /// `raw_ms` is the unreduced seed. `applied_ms` must match the duration of
    /// the animation stored beside this latch; keeping it explicit preserves
    /// each widget's existing pre-draw initialization exactly.
    pub(crate) fn new(raw_ms: f32, applied_ms: f32, easing: MotionEasing) -> Self {
        Self {
            raw_ms: Cell::new(raw_ms),
            theme_reduced_motion: Cell::new(None),
            easing: Cell::new(easing),
            applied_ms,
        }
    }

    /// Record one draw-time theme observation.
    ///
    /// `raw_ms` must be `MotionScale::{fast,normal,...}_ms`, never `ms(token)`:
    /// the latter has already reduced to zero and cannot be recovered by an
    /// explicit per-instance `reduced_motion(false)` override.
    pub(crate) fn observe(&self, raw_ms: f32, theme_reduced_motion: bool, easing: MotionEasing) {
        self.raw_ms.set(raw_ms);
        self.theme_reduced_motion.set(Some(theme_reduced_motion));
        self.easing.set(easing);
    }

    /// Resolve the policy, with a per-instance override winning over the theme.
    /// An unobserved theme (`None`) is reduced motion, the conservative a11y
    /// default for a transition that can begin before its first draw.
    pub(crate) fn effective_reduced_motion(&self, override_reduced: Option<bool>) -> bool {
        override_reduced.unwrap_or_else(|| self.theme_reduced_motion.get().unwrap_or(true))
    }

    /// Effective duration after applying reduced motion to the raw latch.
    pub(crate) fn effective_ms(&self, override_reduced: Option<bool>) -> f32 {
        self.effective_ms_with_raw(override_reduced, None)
    }

    /// Effective duration with an optional widget-specific raw duration.
    ///
    /// This is the row-motion case: a token such as an indicator fade may
    /// replace the theme's duration, but reduction still applies afterwards.
    pub(crate) fn effective_ms_with_raw(
        &self,
        override_reduced: Option<bool>,
        raw_override_ms: Option<f32>,
    ) -> f32 {
        if self.effective_reduced_motion(override_reduced) {
            0.0
        } else {
            raw_override_ms.unwrap_or_else(|| self.raw_ms.get())
        }
    }

    /// Rebuild when `effective_ms` differs from what the animation actually
    /// contains, preserving its current target and using the latched easing.
    pub(crate) fn sync_animation(&mut self, anim: &mut Animation<bool>, effective_ms: f32) {
        if (effective_ms - self.applied_ms).abs() < f32::EPSILON {
            return;
        }
        self.applied_ms = effective_ms;
        *anim = build_anim(anim.value(), effective_ms, self.easing.get());
    }

    /// Build another animation with this latch's current easing.
    ///
    /// Used by widgets such as elevation that intentionally restart from a
    /// fresh endpoint on an edge instead of preserving the previous target.
    pub(crate) fn build_animation(&self, initial: bool, effective_ms: f32) -> Animation<bool> {
        build_anim(initial, effective_ms, self.easing.get())
    }

    /// Build a transition held at a previously presented fractional value.
    /// Panel wrappers use this when a target flip is queued until the next
    /// compositor frame supplies its timestamp.
    pub(crate) fn build_holding_animation(
        &self,
        from: f32,
        at: iced::time::Instant,
        effective_ms: f32,
    ) -> Animation<bool> {
        anim_holding(from, at, effective_ms, self.easing.get())
    }
}

/// Map the theme's semantic [`MotionEasing`] to the `iced` curve.
///
/// The mapping is total and 1:1 — no fallback branch, so a new variant added to
/// [`MotionEasing`] is a compile error here rather than silently degrading to
/// linear at runtime.
#[must_use]
pub fn iced_easing(easing: MotionEasing) -> Easing {
    match easing {
        MotionEasing::Linear => Easing::Linear,
        MotionEasing::EaseIn => Easing::EaseIn,
        MotionEasing::EaseOut => Easing::EaseOut,
        MotionEasing::EaseInOut => Easing::EaseInOut,
    }
}

/// The curve a widget should animate with: its own documented `preferred`
/// deviation if it has one, else the runtime theme's policy.
///
/// Every `Animation::easing(..)` site in this crate goes through this call,
/// so no widget names an `Easing::` variant directly.
#[must_use]
pub fn easing_for_theme(theme: &iced::Theme, preferred: Option<MotionEasing>) -> Easing {
    iced_easing(crate::theme_lookup::motion(theme).easing_or(preferred))
}

/// [`easing_for_theme`] without a `&Theme` in scope.
///
/// `state()` and `Widget::diff` receive no theme, so a widget building
/// its initial `Animation` before the first `draw` has nothing to resolve
/// against. It uses the policy default, and the first `draw` latch corrects it
/// — the same arrangement the duration latches use.
#[must_use]
pub fn default_easing(preferred: Option<MotionEasing>) -> Easing {
    iced_easing(enzui_theme::MotionScale::default().easing_or(preferred))
}

/// A nonzero floor for one-shot timer scheduling, not motion.
///
/// The type makes the semantic distinction reviewable at the call site and by
/// the motion-literal guard. Its only operation raises an already-computed
/// remaining delay off zero; it never drives paint or animation progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimerWakeFloor(std::time::Duration);

impl TimerWakeFloor {
    /// Name a nonzero wake floor at the scheduling site.
    pub(crate) const fn of(min_nonzero: std::time::Duration) -> Self {
        assert!(
            !min_nonzero.is_zero(),
            "a timer wake floor exists to keep a one-shot wake off zero; zero is not a floor"
        );
        Self(min_nonzero)
    }

    /// Raise an already-computed remaining delay to the nonzero floor.
    #[must_use]
    pub(crate) fn wake_after(self, remaining: std::time::Duration) -> std::time::Duration {
        remaining.max(self.0)
    }
}

/// Build a transition `Animation<bool>` from a resolved effective duration
/// in ms and a resolved easing curve — the single constructor every
/// animated widget in this crate routes through.
///
/// Centralised for two reasons:
///
/// * it is the one place `f32` milliseconds become an `iced::time::Duration`,
///   so the `ms / 1000.0` conversion and its
///   [`enzui_theme::sane_motion_ms`] guard exist once rather than in six
///   copies that can drift; and
/// * it means no widget names an `Easing::` variant or a `Duration::from_*`
///   literal of its own.
///
/// `effective_ms` is effective, not raw: reduced motion has already been
/// applied by the caller's `effective_*_ms`. A `0` is honoured literally as
/// [`Duration::ZERO`] — the transition lands on its target on the very frame it
/// starts, which is correct for a transition and is also what a theme or caller
/// asking for `0` actually meant (`0` is expressible intent, not
/// "unset"). Non-finite and negative inputs floor to zero, and an oversized
/// finite input saturates to [`enzui_theme::MAX_MOTION_MS`], rather than
/// panicking in `Duration::from_secs_f32`.
#[must_use]
/// A travelling group's offset rounded to whole device
/// pixels, the same rounding the renderer's retained replay applies, so a
/// directly drawn frame and a lane-replayed frame of the same travel agree and
/// a label never wobbles against its own background.
pub(crate) fn snap_to_device(offset: Vector, renderer: &impl iced::advanced::Renderer) -> Vector {
    let factor = renderer.hint_factor().unwrap_or(1.0);
    if !factor.is_finite() || factor <= 0.0 {
        return offset;
    }
    Vector::new(
        (offset.x * factor).round() / factor,
        (offset.y * factor).round() / factor,
    )
}

pub(crate) fn build_anim(
    initial: bool,
    effective_ms: f32,
    easing: MotionEasing,
) -> iced::animation::Animation<bool> {
    let ms = enzui_theme::sane_motion_ms(effective_ms);
    iced::animation::Animation::new(initial)
        .easing(iced_easing(easing))
        .duration(iced::time::Duration::from_secs_f32(ms / 1000.0))
}

/// Build an animation whose value at `at` is exactly `progress`, ready to be
/// retargeted from there.
///
/// `Animation<bool>` can only be *constructed* at an endpoint, so an
/// interrupted travel would have to restart from 0 or 1 -- which is precisely
/// the jump a reversal must not make. The curve is monotone, so the instant at
/// which it passes `progress` can be found by bisection and the animation
/// simply anchored that far in the past: its value at `at` is then the value
/// the user last saw, and `go_mut(target, at)` reverses from there with the
/// remaining travel it deserves.
///
/// Endpoints short-circuit to a plain settled animation, so the common case (a
/// flip triggered while nothing was moving) costs nothing.
pub(crate) fn anim_holding(
    progress: f32,
    at: iced::time::Instant,
    effective_ms: f32,
    easing: MotionEasing,
) -> iced::animation::Animation<bool> {
    if !progress.is_finite() || progress <= f32::EPSILON {
        return build_anim(false, effective_ms, easing);
    }
    if progress >= 1.0 - f32::EPSILON {
        return build_anim(true, effective_ms, easing);
    }

    let mut probe = build_anim(false, effective_ms, easing);
    probe.go_mut(true, at);

    // Bisect the eased curve for the offset at which it reads `progress`.
    // Twenty-four halvings of a 180 ms travel resolve to ~10 ns, far below any
    // frame period, and the loop count is fixed so a pathological easing cannot
    // spin here.
    let mut low = 0.0_f32;
    // Bound the search so midpoint duration conversion cannot overflow.
    let mut high = enzui_theme::sane_motion_ms(effective_ms);

    for _ in 0..24 {
        let mid = 0.5 * (low + high);
        let sampled = probe.interpolate(
            0.0,
            1.0,
            at + iced::time::Duration::from_secs_f32(mid / 1000.0),
        );

        if sampled < progress {
            low = mid;
        } else {
            high = mid;
        }
    }

    // Named rather than inlined: this is the *theme's* duration being
    // converted, not a baked-in timing, and the motion-literal guard reads the shape.
    let held_ms = 0.5 * (low + high);
    let offset = iced::time::Duration::from_secs_f32(held_ms / 1000.0);
    let mut held = build_anim(false, effective_ms, easing);

    // `checked_sub` rather than `-`: an `Instant` close to the process start
    // cannot represent the subtraction, and a panic in a motion primitive is
    // not an acceptable way to discover that. Falling back to `at` holds the
    // origin value, which is the honest degenerate answer.
    held.go_mut(true, at.checked_sub(offset).unwrap_or(at));
    held
}

// Retained panel lifecycle and rendering are isolated from generic effects.
// The modules stay private: these re-exports preserve the established
// `enzui_widgets::motion::*` API without exposing implementation paths.
mod panels;
pub use panels::{
    PanelMotion, PanelReveal, PanelRevealClock, PanelRevealEdge, PanelRevealMode, TrackCurve,
    next_scene_node,
};
pub(crate) use panels::{RevealClock, TRACK_SAMPLES, publish_travel};

/// Asks for the next frame of a motion effect: one its published tracks
/// carry when it published one (`carried`), so the render lane can serve it,
/// otherwise a drawn one, as for a node not recorded yet. See
/// [`Shell::request_track_frame`](iced::advanced::Shell::request_track_frame).
#[track_caller]
pub(crate) fn request_motion_frame<Message>(
    shell: &mut iced::advanced::Shell<'_, Message>,
    carried: bool,
) {
    if carried {
        shell.request_track_frame();
    } else {
        shell.request_redraw();
    }
}

mod animated;
pub use animated::{Animated, animated};
mod drift;
pub(crate) use drift::Drift;
mod moves;
pub(crate) use moves::Moves;
mod rekeyed;
mod transition;
pub use rekeyed::{FadeOnChange, fade_on_change};
pub(crate) use transition::{Lerp, StateFade};
mod effects;
mod expressive;
mod spring;
pub use effects::{
    CrossFade, Motion, MotionAxis, MotionEffect, SlideDirection, cross_fade, motion,
};
pub use expressive::{EXPRESSIVE_TRACK_SAMPLES, ExpressiveSample, ExpressiveTravel};
pub use spring::{SPRING_TRACK_RATE, SpringAnimation, SpringTravel, SpringValue};

/// A 0-to-1 state change eased out over a duration (scaled by the theme's
/// motion pace) from wherever it was: the small motions of controls (a slider knob swelling under the
/// pointer, a tick popping into a checkbox). Read at presented-frame
/// instants; before the first frame a state is taken at once. A change seen
/// between frames (a click) starts from the last presented frame when that
/// frame is recent, so no frame of it is lost, and from the next frame when
/// the window had been idle: the last frame can be seconds past, and a
/// motion timed from it would already be over.
/// How old the last frame can be for a change made since to start from it:
/// a few frames at a low refresh rate.
const RECENT_FRAME: iced::time::Duration = iced::time::Duration::from_millis(64);

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Swell {
    /// The state being moved toward.
    pub(crate) target: bool,
    from: f32,
    started: Option<iced::time::Instant>,
    /// Changed between frames, after the frame at this instant; the next
    /// frame decides when the motion started.
    pending: Option<iced::time::Instant>,
}

impl Swell {
    /// Take `target` at once, with no motion.
    pub(crate) fn snap(target: bool) -> Self {
        Self {
            target,
            from: if target { 1.0 } else { 0.0 },
            started: None,
            pending: None,
        }
    }

    /// The eased value at `now` for a motion lasting `ms` at the stock pace.
    pub(crate) fn value(&self, now: iced::time::Instant, ms: f32) -> f32 {
        let ms = ms * enzui_theme::text_defaults::motion_pace();
        let to = if self.target { 1.0 } else { 0.0 };
        if self.pending.is_some() {
            return self.from;
        }
        let Some(started) = self.started else {
            return to;
        };
        let t = (now.saturating_duration_since(started).as_secs_f32() * 1000.0 / ms.max(1.0))
            .clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (to - self.from) * eased
    }

    /// Move toward `target` from the current value, at `now`.
    pub(crate) fn set(&mut self, target: bool, now: iced::time::Instant, ms: f32) {
        if target != self.target {
            self.from = self.value(now, ms);
            self.target = target;
            self.started = Some(now);
            self.pending = None;
        }
    }

    /// Whether the motion is still running at `now`, or waiting for the
    /// frame that starts it.
    pub(crate) fn animating(&self, now: iced::time::Instant, ms: f32) -> bool {
        let ms = ms * enzui_theme::text_defaults::motion_pace();
        self.pending.is_some()
            || self.started.is_some_and(|started| {
                now.saturating_duration_since(started).as_secs_f32() * 1000.0 < ms
            })
    }

    /// The value to paint: the target under reduced motion or before a frame.
    pub(crate) fn paint(
        &self,
        frame_at: Option<iced::time::Instant>,
        ms: f32,
        theme: &iced::Theme,
    ) -> f32 {
        match frame_at {
            Some(now) if !crate::theme_lookup::motion(theme).reduced_motion => self.value(now, ms),
            _ => {
                if self.target {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    /// Track `target` from an event: latch the frame instant, move, and ask
    /// for frames while moving.
    pub(crate) fn track<Message>(
        &mut self,
        frame_at: &mut Option<iced::time::Instant>,
        event: &iced::Event,
        target: bool,
        ms: f32,
        shell: &mut iced::advanced::Shell<'_, Message>,
    ) {
        let frame = match event {
            iced::Event::Window(iced::window::Event::RedrawRequested(now)) => {
                *frame_at = Some(*now);
                if let Some(last) = self.pending.take() {
                    let idle = now.saturating_duration_since(last) > RECENT_FRAME;
                    self.started = Some(if idle { *now } else { last });
                }
                true
            }
            _ => false,
        };
        match *frame_at {
            Some(now) if !frame && target != self.target => {
                self.from = self.value(now, ms);
                self.target = target;
                self.started = None;
                self.pending = Some(now);
                shell.request_redraw();
            }
            Some(now) => {
                self.set(target, now, ms);
                if self.animating(now, ms) {
                    shell.request_redraw();
                }
            }
            None => *self = Self::snap(target),
        }
    }
}
