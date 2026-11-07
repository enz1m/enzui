//! Sheet detents, viewport resolution, drag outcomes, and settle animation.
//!
//! Fractional detents resolve against usable viewport height but never below content height.

use crate::metrics::{FinitePositive, UnitInterval};

/// Default committed-drag distance threshold in logical pixels.
pub const DEFAULT_DISMISS_THRESHOLD: f32 = 96.0;

/// Default committed-drag velocity threshold in logical pixels per second.
pub const DEFAULT_DISMISS_VELOCITY: f32 = 700.0;

/// A height at which a sheet may rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Detent {
    /// Hug the content's preferred height.
    Content,
    /// A fraction of the available viewport height, clamped up to the content
    /// height so a detent can never hide content.
    Fraction(UnitInterval),
}

impl Detent {
    /// The `"medium"` detent (half the viewport).
    #[must_use]
    pub fn medium() -> Self {
        Self::Fraction(UnitInterval::clamped(0.5))
    }

    /// The `"large"` detent (90% of the viewport).
    #[must_use]
    pub fn large() -> Self {
        Self::Fraction(UnitInterval::clamped(0.9))
    }

    /// The share of the viewport this detent asks for, if it is fractional.
    ///
    /// `Content` has no fraction — it is defined by what is inside the sheet,
    /// not by the viewport — which is why this is an `Option` rather than a
    /// `0.0` that would sort below every real detent.
    #[must_use]
    pub fn fraction(self) -> Option<f32> {
        match self {
            Self::Content => None,
            Self::Fraction(f) => Some(f.get()),
        }
    }
}

/// Why a ladder was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LadderError {
    /// A ladder with no stops. A sheet must be able to rest somewhere.
    Empty,
    /// A duplicate detent at the reported index.
    Duplicate(usize),
    /// A detent at the reported index is lower than its predecessor.
    Unordered(usize),
    /// The content detent is duplicated or is not the smallest stop.
    ContentNotSmallest,
}

impl std::fmt::Display for LadderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "a sheet needs at least one detent to rest at"),
            Self::Duplicate(i) => write!(f, "detent {i} repeats an earlier stop"),
            Self::Unordered(i) => write!(f, "detent {i} is smaller than the stop before it"),
            Self::ContentNotSmallest => {
                write!(
                    f,
                    "the content detent must be the smallest stop, and appear once"
                )
            }
        }
    }
}

impl std::error::Error for LadderError {}

/// A non-empty validated sequence of detents ordered by increasing requested height.
#[derive(Debug, Clone, PartialEq)]
pub struct DetentLadder {
    stops: Vec<Detent>,
}

impl DetentLadder {
    /// Validate `stops` into a ladder.
    ///
    /// # Errors
    ///
    /// See [`LadderError`].
    pub fn new(stops: Vec<Detent>) -> Result<Self, LadderError> {
        if stops.is_empty() {
            return Err(LadderError::Empty);
        }

        // `Content` is the runtime height floor and therefore must be the first stop.
        let content_count = stops.iter().filter(|d| **d == Detent::Content).count();
        if content_count > 1 || (content_count == 1 && stops[0] != Detent::Content) {
            return Err(LadderError::ContentNotSmallest);
        }

        for (i, window) in stops.windows(2).enumerate() {
            let (a, b) = (window[0], window[1]);
            match (a.fraction(), b.fraction()) {
                (None, Some(_)) => {}
                (Some(x), Some(y)) => {
                    if (x - y).abs() < f32::EPSILON {
                        return Err(LadderError::Duplicate(i + 1));
                    }
                    if y < x {
                        return Err(LadderError::Unordered(i + 1));
                    }
                }
                (_, None) => return Err(LadderError::ContentNotSmallest),
            }
        }

        Ok(Self { stops })
    }

    /// The detent ladder: content, medium, large.
    #[must_use]
    pub fn standard() -> Self {
        // Ordered and unique, so it needs no validation.
        Self {
            stops: vec![Detent::Content, Detent::medium(), Detent::large()],
        }
    }

    /// A ladder with a single content-hugging stop.
    #[must_use]
    pub fn content_only() -> Self {
        Self {
            stops: vec![Detent::Content],
        }
    }

    /// The stops, smallest first.
    #[must_use]
    pub fn stops(&self) -> &[Detent] {
        &self.stops
    }

    /// How many stops the ladder has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stops.len()
    }

    /// Returns `false` because validated ladders are never empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stops.is_empty()
    }

    /// The detent at `index`, if it exists.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Detent> {
        self.stops.get(index).copied()
    }

    /// Resolves a stop height against viewport and content heights.
    ///
    /// Fractional stops never fall below content height; invalid indices or non-positive viewport heights resolve to content height.
    #[must_use]
    pub fn resolve(&self, index: usize, available: f32, content: f32) -> f32 {
        let content = if content.is_finite() && content > 0.0 {
            content
        } else {
            0.0
        };
        let Some(detent) = self.get(index) else {
            return content;
        };
        match detent.fraction() {
            None => content,
            Some(fraction) => {
                if !available.is_finite() || available <= 0.0 {
                    content
                } else {
                    (available * fraction).max(content)
                }
            }
        }
    }

    /// Resolves a stop against viewport height after insets, while preserving the content-height floor.
    #[must_use]
    pub fn resolve_within(
        &self,
        index: usize,
        available: f32,
        content: f32,
        insets: SafeInsets,
    ) -> f32 {
        self.resolve(index, insets.usable_height(available), content)
    }

    /// Returns the greatest index below `from` with a different resolved height.
    ///
    /// Returns `None` for an invalid index or when every lower stop resolves exactly equal. Callers should use this only with a measured viewport.
    #[must_use]
    pub fn next_distinct_below(&self, from: usize, viewport: SheetViewport) -> Option<usize> {
        if from >= self.len() {
            return None;
        }
        let height = |index: usize| viewport.height(self, index);
        let current = height(from);
        (0..from).rev().find(|&i| height(i) != current)
    }

    /// Returns the least index above `from` with a different resolved height.
    ///
    /// Returns `None` for an invalid index or when every higher stop resolves
    /// to the same height. Callers should use this only with a measured viewport.
    #[must_use]
    pub fn next_distinct_above(&self, from: usize, viewport: SheetViewport) -> Option<usize> {
        if from >= self.len() {
            return None;
        }
        let current = viewport.height(self, from);
        ((from + 1)..self.len()).find(|&i| viewport.height(self, i) != current)
    }
}

/// A viewport measurement used to resolve and compare sheet detent heights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetViewport {
    available: f32,
    content: f32,
    insets: SafeInsets,
    // Distinguishes a measured zero-size viewport from the unmeasured sentinel.
    measured: bool,
}

impl Default for SheetViewport {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

impl SheetViewport {
    /// An unmeasured viewport sentinel; drag resolution falls back to index stepping.
    pub const UNKNOWN: Self = Self {
        available: 0.0,
        content: 0.0,
        insets: SafeInsets::NONE,
        measured: false,
    };

    /// Creates a measured, uninset viewport without sanitizing its inputs.
    ///
    /// A zero height is still a real measurement; use [`Self::UNKNOWN`] when layout has not run.
    #[must_use]
    pub const fn new(available: f32, content: f32) -> Self {
        Self {
            available,
            content,
            insets: SafeInsets::NONE,
            measured: true,
        }
    }

    /// Creates a measured viewport, replacing each negative or non-finite input independently with zero.
    #[must_use]
    pub fn measured(available: f32, content: f32) -> Self {
        Self {
            available: finite_non_negative(available),
            content: finite_non_negative(content),
            insets: SafeInsets::NONE,
            measured: true,
        }
    }

    /// Applies safe-area or keyboard insets while preserving the viewport’s measured status.
    #[must_use]
    pub const fn with_insets(self, insets: SafeInsets) -> Self {
        Self { insets, ..self }
    }

    /// The raw viewport height, before insets.
    #[must_use]
    pub const fn available(self) -> f32 {
        self.available
    }

    /// The sheet's natural content height.
    #[must_use]
    pub const fn content(self) -> f32 {
        self.content
    }

    /// The insets the sheet must stay clear of.
    #[must_use]
    pub const fn insets(self) -> SafeInsets {
        self.insets
    }

    /// Returns whether layout produced this viewport, including a zero-size measurement.
    #[must_use]
    pub const fn is_measured(self) -> bool {
        self.measured
    }

    /// Returns viewport height remaining after vertical insets.
    #[must_use]
    pub fn usable_height(self) -> f32 {
        self.insets.usable_height(self.available)
    }

    /// Returns whether the viewport is measured and has positive usable height.
    #[must_use]
    pub fn has_usable_height(self) -> bool {
        let usable = self.usable_height();
        self.measured && usable.is_finite() && usable > 0.0
    }

    /// Resolves a ladder stop using this viewport’s height, content, and insets.
    #[must_use]
    pub fn height(self, ladder: &DetentLadder, index: usize) -> f32 {
        ladder.resolve_within(index, self.available, self.content, self.insets)
    }
}

/// Sanitized top and bottom exclusions from a sheet’s usable viewport height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SafeInsets {
    top: f32,
    bottom: f32,
}

impl Default for SafeInsets {
    fn default() -> Self {
        Self::NONE
    }
}

impl SafeInsets {
    /// No insets: the whole viewport is usable.
    pub const NONE: Self = Self {
        top: 0.0,
        bottom: 0.0,
    };

    /// Creates vertical insets, replacing negative or non-finite components with zero.
    #[must_use]
    pub fn new(top: f32, bottom: f32) -> Self {
        Self {
            top: finite_non_negative(top),
            bottom: finite_non_negative(bottom),
        }
    }

    /// Creates a sanitized bottom-only inset.
    #[must_use]
    pub fn bottom(bottom: f32) -> Self {
        Self::new(0.0, bottom)
    }

    /// The top inset, in logical pixels (always finite and ≥ 0).
    #[must_use]
    pub const fn top_inset(self) -> f32 {
        self.top
    }

    /// The bottom inset, in logical pixels (always finite and ≥ 0).
    #[must_use]
    pub const fn bottom_inset(self) -> f32 {
        self.bottom
    }

    /// Subtracts both insets and clamps a finite positive result at zero.
    ///
    /// Non-positive or non-finite `available` values pass through unchanged.
    #[must_use]
    pub fn usable_height(self, available: f32) -> f32 {
        if !available.is_finite() || available <= 0.0 {
            return available;
        }
        (available - self.top - self.bottom).max(0.0)
    }
}

fn finite_non_negative(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// What a completed drag asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragResolution {
    /// Makes no detent change.
    SpringBack,
    /// Step to the stop at this index.
    Settle(usize),
    /// Dismisses the sheet when no lower distinct stop can be selected.
    Dismiss,
}

/// Thresholds and dismissal permission for a released downward drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragPolicy {
    /// Minimum downward travel in logical pixels that commits a release.
    pub distance: FinitePositive,
    /// Minimum downward speed in logical pixels per second that commits after finite positive travel.
    pub velocity: FinitePositive,
    /// Whether a committed drag below the smallest stop dismisses the sheet.
    pub dismissible: bool,
}

impl Default for DragPolicy {
    fn default() -> Self {
        Self {
            distance: FinitePositive::new_or(DEFAULT_DISMISS_THRESHOLD, DEFAULT_DISMISS_THRESHOLD),
            velocity: FinitePositive::new_or(DEFAULT_DISMISS_VELOCITY, DEFAULT_DISMISS_VELOCITY),
            dismissible: true,
        }
    }
}

impl DragPolicy {
    /// A policy that collapses through the ladder but never dismisses.
    #[must_use]
    pub fn non_dismissible() -> Self {
        Self {
            dismissible: false,
            ..Self::default()
        }
    }

    /// Returns whether finite positive downward travel reaches either the distance or velocity threshold.
    #[must_use]
    pub fn commits(self, distance: f32, velocity: f32) -> bool {
        if !distance.is_finite() || distance <= 0.0 {
            return false;
        }
        if distance >= self.distance.get() {
            return true;
        }
        velocity.is_finite() && velocity >= self.velocity.get()
    }

    /// Returns whether signed travel commits in its own direction.
    ///
    /// Positive values mean downward motion and negative values mean upward
    /// motion. Velocity commits only when it points in the same direction as
    /// the travel, preventing a last-moment reversal from selecting the wrong stop.
    #[must_use]
    pub fn commits_signed(self, distance: f32, velocity: f32) -> bool {
        if !distance.is_finite() || distance == 0.0 {
            return false;
        }
        if distance.abs() >= self.distance.get() {
            return true;
        }
        velocity.is_finite()
            && velocity.signum() == distance.signum()
            && velocity.abs() >= self.velocity.get()
    }
}

/// Resolves a released downward drag to spring-back, a lower distinct stop, or dismissal.
///
/// Uncommitted drags spring back. Measured viewports skip equal-height stops; measured viewports with no usable height dismiss or hold immediately. Unmeasured viewports step by index. Invalid current indices dismiss or hold according to policy.
#[must_use]
pub fn resolve_drag(
    ladder: &DetentLadder,
    current: usize,
    distance: f32,
    velocity: f32,
    policy: DragPolicy,
    viewport: SheetViewport,
) -> DragResolution {
    if !policy.commits(distance, velocity) {
        return DragResolution::SpringBack;
    }

    let dismiss_or_hold = || {
        if policy.dismissible {
            DragResolution::Dismiss
        } else {
            DragResolution::SpringBack
        }
    };

    // Treat an invalid current index as having no lower stop.
    if current >= ladder.len() {
        return dismiss_or_hold();
    }

    // With a measured but unusable viewport, no distinct lower height exists.
    if viewport.is_measured() && !viewport.has_usable_height() {
        return dismiss_or_hold();
    }

    let target = if viewport.has_usable_height() {
        // Skip lower indices that resolve to the current height.
        ladder.next_distinct_below(current, viewport)
    } else {
        // Before measurement, preserve ordinary index stepping.
        current.checked_sub(1)
    };

    match target {
        Some(index) => DragResolution::Settle(index),
        None => dismiss_or_hold(),
    }
}

/// Resolves a signed resize drag to the next distinct detent in its direction.
///
/// Positive distance moves toward smaller stops and may dismiss below the
/// smallest stop. Negative distance moves toward larger stops and always holds
/// at the largest stop. Equal-height runtime stops are skipped.
#[must_use]
pub fn resolve_resize(
    ladder: &DetentLadder,
    current: usize,
    distance: f32,
    velocity: f32,
    policy: DragPolicy,
    viewport: SheetViewport,
) -> DragResolution {
    if !policy.commits_signed(distance, velocity) || current >= ladder.len() {
        return DragResolution::SpringBack;
    }

    if viewport.is_measured() && !viewport.has_usable_height() {
        return if distance > 0.0 && policy.dismissible {
            DragResolution::Dismiss
        } else {
            DragResolution::SpringBack
        };
    }

    let target = if distance < 0.0 {
        if viewport.has_usable_height() {
            ladder.next_distinct_above(current, viewport)
        } else {
            current.checked_add(1).filter(|next| *next < ladder.len())
        }
    } else if viewport.has_usable_height() {
        ladder.next_distinct_below(current, viewport)
    } else {
        current.checked_sub(1)
    };

    match target {
        Some(index) => DragResolution::Settle(index),
        None if distance > 0.0 && policy.dismissible => DragResolution::Dismiss,
        None => DragResolution::SpringBack,
    }
}

/// Initial 180 ms settle duration used until theme motion is synchronized.
pub const DEFAULT_SETTLE_DURATION: std::time::Duration = std::time::Duration::from_millis(180);

/// Animates sheet height between detents using a configured duration and
/// easing curve, or on a [`Spring`](enzui_theme::Spring) that carries a drag's
/// release velocity ([`Self::set_spring`]).
///
/// Reduced motion resolves immediately to the target, and callers provide explicit instants for deterministic sampling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetSettle {
    from: f32,
    to: f32,
    started: Option<std::time::Instant>,
    // Stored unreduced so disabling reduced motion restores the configured timing.
    raw_duration: std::time::Duration,
    easing: enzui_theme::MotionEasing,
    reduced_motion: bool,
    spring: Option<enzui_theme::Spring>,
    /// The theme's speed relative to its default, applied to the spring.
    spring_slowdown: f32,
    /// The spring's velocity at `started`, px/s of height.
    velocity: f32,
    /// How long after `started` the spring lands.
    spring_settle: std::time::Duration,
}

impl SheetSettle {
    /// Creates a settled animation at `height`; reduced motion makes subsequent settles immediate.
    #[must_use]
    pub fn new(height: f32, reduced_motion: bool) -> Self {
        Self {
            from: height,
            to: height,
            started: None,
            // Keep the raw seed duration; reduction is applied by `duration`.
            raw_duration: DEFAULT_SETTLE_DURATION,
            easing: enzui_theme::MotionScale::default().easing_or(None),
            reduced_motion,
            spring: None,
            spring_slowdown: 1.0,
            velocity: 0.0,
            spring_settle: std::time::Duration::ZERO,
        }
    }

    /// Settle on `spring` instead of the duration and easing, or `None` for
    /// the curve. A spring keeps the velocity it is given
    /// ([`Self::begin_from_with_velocity`]) and the velocity it has when a
    /// new settle starts mid-flight, so a flick carries on and a re-aimed
    /// settle turns smoothly. The theme's motion speed scales it.
    pub fn set_spring(&mut self, spring: Option<enzui_theme::Spring>) {
        self.spring = spring;
    }

    /// The spring this settle uses, if any.
    #[must_use]
    pub const fn spring(&self) -> Option<enzui_theme::Spring> {
        self.spring
    }

    /// Adopts the theme’s normal duration and easing.
    ///
    /// Zero duration is allowed; negative or non-finite durations leave the current value unchanged. Reduced motion is applied only when the duration is consumed.
    pub fn sync_motion(&mut self, motion: &enzui_theme::MotionScale) {
        if motion.normal_ms.is_finite() && motion.normal_ms >= 0.0 {
            let ms = enzui_theme::sane_motion_ms(motion.normal_ms);
            self.raw_duration = std::time::Duration::from_secs_f32(ms / 1000.0);
        }
        self.easing = motion.easing_or(None);
        if let Some(slowdown) = motion.spring_slowdown() {
            self.spring_slowdown = slowdown;
        }
    }

    /// Returns zero under reduced motion, otherwise the configured raw duration.
    #[must_use]
    pub const fn duration(&self) -> std::time::Duration {
        if self.reduced_motion {
            std::time::Duration::ZERO
        } else {
            self.raw_duration
        }
    }

    /// Returns the configured duration without applying reduced motion.
    #[must_use]
    pub const fn raw_duration(&self) -> std::time::Duration {
        self.raw_duration
    }

    /// Returns the configured easing curve.
    #[must_use]
    pub const fn easing(&self) -> enzui_theme::MotionEasing {
        self.easing
    }

    /// Whether reduced motion is active.
    #[must_use]
    pub const fn reduced_motion(self) -> bool {
        self.reduced_motion
    }

    /// Sets reduced-motion mode, finishing any in-flight settle when enabling it.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        if reduced_motion == self.reduced_motion {
            return;
        }
        self.reduced_motion = reduced_motion;
        // Do not overwrite raw duration when reduced motion changes.
        if reduced_motion {
            // Finish an in-flight settle at its target.
            self.started = None;
        }
    }

    /// Starts settling toward a finite target.
    ///
    /// Non-finite targets are ignored, and an unchanged target leaves the animation idle.
    pub fn begin(&mut self, target: f32, at: std::time::Instant) {
        self.begin_from(self.height(at), target, at);
    }

    /// Starts settling from an explicitly rendered height toward a finite target.
    ///
    /// This is used after direct manipulation, where the visible height came
    /// from a live drag rather than from the previous animation sample.
    pub fn begin_from(&mut self, from: f32, target: f32, at: std::time::Instant) {
        if self.spring.is_some() {
            let velocity = self.velocity(at);
            self.begin_from_with_velocity(from, target, velocity, at);
            return;
        }
        if !from.is_finite() || !target.is_finite() {
            return;
        }
        if (target - from).abs() < f32::EPSILON {
            self.from = target;
            self.to = target;
            self.started = None;
            return;
        }
        self.from = from;
        self.to = target;
        self.started = if self.reduced_motion { None } else { Some(at) };
    }

    /// Starts settling from `from`, moving at `velocity` px/s of height (the
    /// released drag's), toward `target`. With a spring the motion carries
    /// that velocity on; with the curve the velocity is ignored.
    pub fn begin_from_with_velocity(
        &mut self,
        from: f32,
        target: f32,
        velocity: f32,
        at: std::time::Instant,
    ) {
        let Some(spring) = self.spring else {
            self.begin_from(from, target, at);
            return;
        };
        if !from.is_finite() || !target.is_finite() {
            return;
        }
        let velocity = if velocity.is_finite() { velocity } else { 0.0 };
        self.from = from;
        self.to = target;
        self.velocity = velocity;
        let settle =
            spring
                .slowed(self.spring_slowdown)
                .settle_time(from - target, velocity, 0.05, 0.5);
        if self.reduced_motion || settle <= 0.0 {
            self.from = target;
            self.velocity = 0.0;
            self.started = None;
            return;
        }
        self.spring_settle = std::time::Duration::from_secs_f32(settle);
        self.started = Some(at);
    }

    /// The spring's seconds into the running settle at `at`, or `None` when
    /// not on a running spring.
    fn spring_elapsed(&self, at: std::time::Instant) -> Option<(enzui_theme::Spring, f32)> {
        let spring = self.spring?.slowed(self.spring_slowdown);
        let elapsed = at.saturating_duration_since(self.started?);
        (elapsed < self.spring_settle).then_some((spring, elapsed.as_secs_f32()))
    }

    /// The settle's speed at `at`, in px/s of height; zero at rest and on
    /// the curve.
    #[must_use]
    pub fn velocity(&self, at: std::time::Instant) -> f32 {
        self.spring_elapsed(at).map_or(0.0, |(spring, t)| {
            spring.evaluate(self.from - self.to, self.velocity, t).1
        })
    }

    /// Progress `0.0..=1.0` at `at`.
    #[must_use]
    pub fn progress(&self, at: std::time::Instant) -> f32 {
        let Some(started) = self.started else {
            return 1.0;
        };
        if self.spring.is_some() {
            let elapsed = at.saturating_duration_since(started).as_secs_f32();
            let settle = self.spring_settle.as_secs_f32();
            return if settle > 0.0 {
                (elapsed / settle).clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        // Compute progress from the effective duration.
        let duration = self.duration();
        if duration.is_zero() {
            return 1.0;
        }
        let elapsed = at.saturating_duration_since(started).as_secs_f32();
        (elapsed / duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Returns the eased animated height at `at`; reduced motion returns the target.
    #[must_use]
    pub fn height(&self, at: std::time::Instant) -> f32 {
        if self.spring.is_some() {
            return self.spring_elapsed(at).map_or(self.to, |(spring, t)| {
                self.to + spring.evaluate(self.from - self.to, self.velocity, t).0
            });
        }
        let t = self.easing.apply(self.progress(at));
        self.from + (self.to - self.from) * t
    }

    /// The target height this settle is heading toward.
    #[must_use]
    pub const fn target(self) -> f32 {
        self.to
    }

    /// Whether the settle is still in flight at `at` (the caller keeps
    /// requesting redraws until this is `false`).
    #[must_use]
    pub fn is_animating(&self, at: std::time::Instant) -> bool {
        self.started.is_some() && self.progress(at) < 1.0
    }
}
