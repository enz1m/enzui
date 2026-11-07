//! Sheet direct-manipulation state for compatibility dismissal and signed resize.

use super::*;

/// The live state of a drag in progress.
///
/// Not application state: it has no meaning once the finger lifts, which is
/// why it lives here rather than in a host's model. A host stores one of these
/// beside its sheet and calls [`SheetDrag::release`] to find out what the
/// gesture meant.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SheetDrag {
    offset: f32,
    distance: f32,
    /// Downward speed exposed by the compatibility drag path, in px/s.
    velocity: f32,
    signed_velocity: f32,
    active: bool,
    /// The drag's positions on its own frame clock, for the release velocity
    /// a settle spring carries on.
    tracker: enzui_core::velocity::VelocityTracker,
    /// Seconds since the press, summed from the samples' `dt`s.
    clock: f64,
}

impl SheetDrag {
    /// A sheet at rest.
    #[must_use]
    pub const fn resting() -> Self {
        Self {
            offset: 0.0,
            distance: 0.0,
            velocity: 0.0,
            signed_velocity: 0.0,
            active: false,
            tracker: enzui_core::velocity::VelocityTracker::new(),
            clock: 0.0,
        }
    }

    /// Begin a drag.
    pub const fn press(&mut self) {
        self.offset = 0.0;
        self.distance = 0.0;
        self.velocity = 0.0;
        self.signed_velocity = 0.0;
        self.active = true;
        self.tracker.reset();
        self.clock = 0.0;
    }

    /// Whether a finger is currently down.
    #[must_use]
    pub const fn is_active(self) -> bool {
        self.active
    }

    /// How far the surface is currently translated, in logical px.
    ///
    /// Never negative: an upward pull past rest is clamped, so the sheet never
    /// lifts off its edge.
    #[must_use]
    pub const fn offset(self) -> f32 {
        self.offset
    }

    /// Signed displacement from the press point, in logical pixels.
    ///
    /// Positive values are downward and negative values are upward. The
    /// compatibility [`Self::offset`] accessor remains nonnegative.
    #[must_use]
    pub const fn distance(self) -> f32 {
        self.distance
    }

    /// The most recently measured downward speed, in px/s.
    #[must_use]
    pub const fn velocity(self) -> f32 {
        self.velocity
    }

    /// The most recently measured signed speed, in px/s.
    #[must_use]
    pub const fn signed_velocity(self) -> f32 {
        self.signed_velocity
    }

    /// Record movement to `distance` px below the press point, `dt` seconds
    /// after the previous sample.
    ///
    /// A non-finite distance is ignored rather than clamped: it is a
    /// measurement failure, and treating it as `0.0` would silently snap a
    /// dragging sheet back to rest under the finger.
    pub fn moved(&mut self, distance: f32, dt: f32) {
        if !self.active || !distance.is_finite() {
            return;
        }
        // A zero or non-finite dt would divide to infinity and make every
        // sample a committing flick.
        let next_offset = distance.max(0.0);
        if dt.is_finite() && dt > 0.0 {
            self.velocity = (next_offset - self.offset) / dt;
            self.signed_velocity = (distance - self.distance) / dt;
            self.clock += f64::from(dt);
        }
        self.tracker.add_secs(self.clock, [0.0, distance]);
        self.distance = distance;
        self.offset = next_offset;
    }

    /// Release the drag and resolve what it meant.
    ///
    /// Resets to rest either way: whatever the caller does with the
    /// resolution, the transient state is over. Returning it and clearing it
    /// in one step is what stops a released drag from being resolved twice.
    ///
    /// `terminal` is the release edge: `(distance, dt)` measured at the lift,
    /// where `distance` is the finger's final position below the press point
    /// and `dt` the frame-clock gap since the previous sample. `None` means
    /// the caller has no terminal sample (a keyboard-driven step, or a lift
    /// with no fresh frame), and the resolution then uses the last `Moved`.
    ///
    /// # Observing the release edge
    ///
    /// Without it, `release` resolves on the velocity computed at the last
    /// [`moved`](Self::moved), with no elapsed time since. A finger that
    /// flicks fast, then rests, then lifts commits on the stale flick
    /// velocity: the user stopped, but the sheet still flies.
    ///
    /// The rule is not a time-based decay constant. The release sample must be
    /// folded into the velocity estimate exactly as
    /// [`moved`](Self::moved) folds every other sample —
    /// `velocity = (terminal - offset) / dt` over the press→lift interval —
    /// the same rule [`SwipeGesture::release`](crate::swipe::SwipeGesture::release)
    /// applies. A finger at rest across a long release gap has
    /// `terminal ≈ offset` over a large `dt`, so its velocity recomputes to
    /// near zero: the flick physically decays because the physics is applied,
    /// not because a rule assumed it did. A genuine flick whose final movement
    /// *is* the lift keeps its speed, because then `terminal` moved over a
    /// short `dt`.
    ///
    /// Ordering: the terminal sample is recorded (into `offset`/`velocity`)
    /// before [`resolve_drag`] reads them. Swapping the two — resolving on
    /// the stale sample, then recording — reintroduces exactly the bug, and is
    /// caught by
    /// `a_finger_that_rests_before_lifting_does_not_commit_on_a_stale_flick`.
    ///
    /// `viewport` is the measured box the host renders the sheet in — the same
    /// one it passes to
    /// [`DetentLadder::resolve_within`](enzui_core::sheet::DetentLadder::resolve_within)
    /// to get the surface height. It is what lets a step skip stops the content
    /// clamp has collapsed together, so a committed drag can never settle at
    /// the height the sheet is already at. A host that has not measured
    /// yet passes [`SheetViewport::UNKNOWN`], which keeps the plain index step.
    pub fn release(
        &mut self,
        ladder: &DetentLadder,
        current: usize,
        policy: DragPolicy,
        viewport: SheetViewport,
        terminal: Option<(f32, f32)>,
    ) -> DragResolution {
        // Record the release edge before resolving. `moved` already owns the
        // finiteness and `dt > 0` rules; reusing it means the release sample
        // and every mid-drag sample decay velocity by the same physics, and
        // there is no second copy of the rule to drift.
        if let Some((distance, dt)) = terminal {
            self.moved(distance, dt);
        }
        let (distance, velocity) = (self.offset, self.velocity);
        *self = Self::resting();
        if !ladder.is_empty() {
            resolve_drag(ladder, current, distance, velocity, policy, viewport)
        } else {
            DragResolution::SpringBack
        }
    }

    /// Release a signed resize drag toward the next distinct stop.
    ///
    /// This is the high-level resizable-sheet path. [`Self::release`] retains
    /// the original downward-only dismissal contract.
    pub fn release_resize(
        &mut self,
        ladder: &DetentLadder,
        current: usize,
        policy: DragPolicy,
        viewport: SheetViewport,
        terminal: Option<(f32, f32)>,
    ) -> DragResolution {
        if let Some((distance, dt)) = terminal {
            self.moved(distance, dt);
        }
        let (distance, velocity) = (self.distance, self.signed_velocity);
        *self = Self::resting();
        if ladder.is_empty() {
            DragResolution::SpringBack
        } else {
            enzui_core::sheet::resolve_resize(ladder, current, distance, velocity, policy, viewport)
        }
    }

    /// The signed speed the drag is released at, in px/s (positive
    /// downward), with the release edge `terminal` folded in as
    /// [`Self::release`] folds it: a least-squares estimate over the drag's
    /// last 100 ms, zero when the finger rested before lifting. Read it
    /// before releasing, and hand it to the settle so the sheet carries on at
    /// the finger's speed.
    #[must_use]
    pub fn release_velocity(self, terminal: Option<(f32, f32)>) -> f32 {
        let mut released = self;
        if let Some((distance, dt)) = terminal {
            released.moved(distance, dt);
        }
        released.tracker.velocity_secs(released.clock)[1]
    }

    /// Abandon the drag without resolving it.
    ///
    /// A cancelled gesture (pointer left the surface, window lost focus) must
    /// spring back rather than commit — the user did not finish asking.
    pub const fn cancel(&mut self) {
        *self = Self::resting();
    }
}
