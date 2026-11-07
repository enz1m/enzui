//! Shared swipe-gesture policy for the carousel, sheet drag-to-dismiss and
//! the navigation outlet's edge back-swipe.
//!
//! [`SwipeGesture`] is a pure state machine: given positions, an optional
//! finger id and an optional timestamp, it answers declined, began, moved,
//! committed or cancelled. It owns no element and touches no shell; each
//! widget keeps its own event adapter. One finger owns a gesture, so a second
//! touch cannot take it over and a foreign lift cannot end it. Thresholds
//! belong to the calling widget and are passed in through [`SwipePolicy`].
//!
//! [`SwipePolicy`]s.
//! # Example
//!
//! ```
//! use enzui_core::metrics::FinitePositive;
//! use enzui_core::placement::Direction;
//! use enzui_widgets::swipe::{SwipeAxis, SwipeGesture, SwipePhase, SwipePolicy};
//! use iced::{Point, Rectangle};
//!
//! let bounds = Rectangle::new(Point::new(0.0, 0.0), iced::Size::new(400.0, 300.0));
//! let policy = SwipePolicy::new(
//!     SwipeAxis::Horizontal,
//!     FinitePositive::new(48.0).expect("a positive threshold"),
//! );
//!
//! let mut gesture = SwipeGesture::idle();
//! assert_eq!(
//!     // No edge strip here, so the direction only matters for a logical edge.
//!     gesture.press(&policy, Point::new(200.0, 150.0), None, bounds, Direction::LeftToRight),
//!     SwipePhase::Began,
//! );
//! assert_eq!(
//!     gesture.release(&policy, Point::new(100.0, 150.0), None, None),
//!     SwipePhase::Committed { distance: -100.0, velocity: 0.0 },
//! );
//! ```

use enzui_core::metrics::FinitePositive;
use enzui_core::placement::Direction;
use iced::time::Instant;
use iced::{Point, Rectangle, touch};

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Which axis a swipe travels along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeAxis {
    /// Left/right travel. Distance is `x - origin.x`, so leftward is negative.
    Horizontal,
    /// Up/down travel. Distance is `y - origin.y`, so upward is negative.
    Vertical,
}

impl SwipeAxis {
    /// Signed travel from `origin` to `at` along this axis.
    #[must_use]
    pub fn travel(self, origin: Point, at: Point) -> f32 {
        match self {
            SwipeAxis::Horizontal => at.x - origin.x,
            SwipeAxis::Vertical => at.y - origin.y,
        }
    }

    /// Signed travel from `origin` to `at` along the cross axis — the one
    /// this policy does not measure.
    ///
    /// Used by axis arbitration: a horizontal swipe competes
    /// with a vertical scroll, so deciding which gesture a drag is means
    /// comparing travel on both axes.
    #[must_use]
    pub fn cross_travel(self, origin: Point, at: Point) -> f32 {
        match self {
            SwipeAxis::Horizontal => at.y - origin.y,
            SwipeAxis::Vertical => at.x - origin.x,
        }
    }
}

/// Which edge an [edge strip](SwipePolicy::edge_strip) is anchored to. A
/// leading edge follows the writing direction, so a back-swipe strip sits on
/// the right in right-to-left layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeSide {
    /// The physical low edge — left (horizontal) or top (vertical). Direction
    /// independent. This is a vertical pull-down, or a horizontal strip that
    /// deliberately ignores writing direction.
    Physical,
    /// The logical leading edge: left under LTR, right under RTL. Only
    /// meaningful on a [`SwipeAxis::Horizontal`] policy (a vertical axis has no
    /// writing-direction leading edge, so it falls back to physical top).
    Leading,
}

/// A leading-edge strip: how wide, and which edge it is anchored to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EdgeStrip {
    /// Width (horizontal) or height (vertical) of the strip, in logical px.
    width: f32,
    side: EdgeSide,
}

/// Which way along the axis a swipe is allowed to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwipeDirection {
    /// Either sign commits. The carousel is bidirectional: leftward pages
    /// forward, rightward pages back.
    #[default]
    Either,
    /// Only increasing coordinates commit (rightward / downward). This is the
    /// navigation outlet's back swipe and the sheet's drag-to-dismiss.
    Positive,
    /// Only decreasing coordinates commit (leftward / upward).
    Negative,
}

impl SwipeDirection {
    /// Whether travel of this sign is a gesture in the permitted direction.
    ///
    /// Zero is never a swipe in any direction — a release at the press point
    /// expresses no intent.
    #[must_use]
    pub fn admits(self, distance: f32) -> bool {
        match self {
            SwipeDirection::Either => distance != 0.0,
            SwipeDirection::Positive => distance > 0.0,
            SwipeDirection::Negative => distance < 0.0,
        }
    }
}

/// Where a swipe may begin, and what commits it.
///
/// Construct with [`SwipePolicy::new`] and narrow with the builder methods. The
/// defaults are the least surprising ones: both directions, the whole region
/// (no edge strip), and distance-only commit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwipePolicy {
    axis: SwipeAxis,
    direction: SwipeDirection,
    /// The strip a press must land in to begin the gesture; `None` accepts the
    /// whole region. A zero-width strip accepts no press.
    edge: Option<EdgeStrip>,
    threshold: FinitePositive,
    /// Speed (logical px/s) past which a release commits regardless of
    /// distance. `None` is distance-only, which is what the rules are.
    velocity: Option<FinitePositive>,
}

impl SwipePolicy {
    /// A policy over the whole region, either direction, distance-only.
    #[must_use]
    pub fn new(axis: SwipeAxis, threshold: FinitePositive) -> Self {
        Self {
            axis,
            direction: SwipeDirection::Either,
            edge: None,
            threshold,
            velocity: None,
        }
    }

    /// Restrict which sign of travel commits.
    #[must_use]
    pub fn direction(mut self, direction: SwipeDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Requires presses to land within `width` px of the physical low edge
    /// (left for horizontal, top for vertical), whatever the writing
    /// direction. Use [`edge_strip_leading`](Self::edge_strip_leading) for a
    /// back-swipe. Non-finite or negative widths leave the policy
    /// unconstrained; zero is kept.
    #[must_use]
    pub fn edge_strip(mut self, width: f32) -> Self {
        self.edge = Self::sane_strip(width, EdgeSide::Physical);
        self
    }

    /// Requires presses to land within `width` px of the logical leading edge:
    /// left in left-to-right layouts, right in right-to-left ones, top for a
    /// vertical policy. The side is resolved at press time from the
    /// [`Direction`] passed in. Width sanitising matches
    /// [`edge_strip`](Self::edge_strip).
    #[must_use]
    pub fn edge_strip_leading(mut self, width: f32) -> Self {
        self.edge = Self::sane_strip(width, EdgeSide::Leading);
        self
    }

    /// Build a validated strip, or `None` for a width that cannot describe one.
    fn sane_strip(width: f32, side: EdgeSide) -> Option<EdgeStrip> {
        if width.is_finite() && width >= 0.0 {
            Some(EdgeStrip { width, side })
        } else {
            None
        }
    }

    /// Also commit on speed, at or above `px_per_second`.
    ///
    /// Distance and velocity are or, not and: a slow deliberate drag
    /// past the distance still commits, and a short fast flick expresses the
    /// same intent as a long slow one. Same rule as
    /// [`enzui_core::sheet::DragPolicy`].
    #[must_use]
    pub fn velocity(mut self, px_per_second: FinitePositive) -> Self {
        self.velocity = Some(px_per_second);
        self
    }

    /// The axis this policy measures along.
    #[must_use]
    pub fn axis(&self) -> SwipeAxis {
        self.axis
    }

    /// The edge strip width, if this policy has one.
    #[must_use]
    pub fn edge(&self) -> Option<f32> {
        self.edge.map(|e| e.width)
    }

    /// Which edge this policy's strip is anchored to, if it has one.
    #[must_use]
    pub fn edge_side(&self) -> Option<EdgeSide> {
        self.edge.map(|e| e.side)
    }

    /// The distance past which a release commits.
    #[must_use]
    pub fn threshold(&self) -> FinitePositive {
        self.threshold
    }

    /// Whether a press at `at` inside `bounds` may begin a gesture, resolving a
    /// leading edge strip against `direction`.
    #[must_use]
    pub fn accepts_press(&self, at: Point, bounds: Rectangle, direction: Direction) -> bool {
        if !at.x.is_finite() || !at.y.is_finite() {
            return false;
        }
        if !bounds.contains(at) {
            return false;
        }
        let Some(strip) = self.edge else {
            return true;
        };
        // Distance from the strip's anchored edge, inward.
        let into = match (self.axis, strip.side, direction) {
            // Vertical: always the top, whatever the writing direction.
            (SwipeAxis::Vertical, _, _) => at.y - bounds.y,
            // Horizontal physical, or leading under LTR: the left edge.
            (SwipeAxis::Horizontal, EdgeSide::Physical, _)
            | (SwipeAxis::Horizontal, EdgeSide::Leading, Direction::LeftToRight) => at.x - bounds.x,
            // Horizontal leading under RTL: the right edge. The logical leading
            // edge is at *larger* x, so measure inward from bounds.x + width.
            (SwipeAxis::Horizontal, EdgeSide::Leading, Direction::RightToLeft) => {
                (bounds.x + bounds.width) - at.x
            }
        };
        into <= strip.width
    }

    /// Whether a release with this travel and speed commits.
    ///
    /// Non-finite input never commits: an unmeasurable gesture is not evidence
    /// of intent (the same rule `DragPolicy::commits` and `commit_swipe` both
    /// apply).
    #[must_use]
    pub fn commits(&self, distance: f32, velocity: f32) -> bool {
        if !distance.is_finite() {
            return false;
        }
        if !self.direction.admits(distance) {
            return false;
        }
        if distance.abs() >= self.threshold.get() {
            return true;
        }
        match self.velocity {
            None => false,
            // A velocity commits only when it points in an allowed
            // direction, so a short drag ending in a reversing flick does not
            // commit.
            Some(v) => {
                velocity.is_finite() && velocity.abs() >= v.get() && self.direction.admits(velocity)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phases
// ---------------------------------------------------------------------------

/// What a fed input did to the gesture.
///
/// [`SwipePhase::Declined`] is the answer that makes finger identity a property
/// of the type rather than of each caller: an input from a finger that does not
/// own the gesture is *declined*, which is neither a new gesture nor a
/// cancellation of the running one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwipePhase {
    /// The input belongs to someone else and changed nothing.
    Declined,
    /// A gesture just started.
    Began,
    /// The owning pointer moved; `distance` is signed travel from the origin.
    Moved {
        /// Signed travel along the policy's axis, from the press point.
        distance: f32,
    },
    /// The owning pointer released past the commit rule.
    Committed {
        /// Signed travel along the policy's axis, from the press point.
        distance: f32,
        /// Speed at release, in logical px/s. `0.0` when no timestamps were
        /// supplied.
        velocity: f32,
    },
    /// The gesture ended without committing — released short, or lost.
    Cancelled,
}

impl SwipePhase {
    /// Whether this phase is a commit.
    #[must_use]
    pub fn is_committed(self) -> bool {
        matches!(self, SwipePhase::Committed { .. })
    }

    /// The signed travel this phase carries, if any.
    #[must_use]
    pub fn distance(self) -> Option<f32> {
        match self {
            SwipePhase::Moved { distance } | SwipePhase::Committed { distance, .. } => {
                Some(distance)
            }
            SwipePhase::Declined | SwipePhase::Began | SwipePhase::Cancelled => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The gesture in flight
// ---------------------------------------------------------------------------

/// The result of arbitrating a provisional gesture against a competing scroll.
///
/// See [`SwipeGesture::arbitrate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arbitration {
    /// Not enough travel yet to tell the axes apart. Neither party should
    /// claim; wait for more movement.
    Undecided,
    /// The dominant axis is the policy's, so this is the sheet's swipe. The gesture is
    /// now claimed and the caller should capture the event stream.
    Claimed,
    /// The dominant axis is the cross axis (a scroll), so the gesture is
    /// abandoned and the caller must not capture — it belongs to the scrollable.
    Yielded,
}

/// One swipe's live state, kept in a widget's tree state. The owner is the
/// finger id for touch and `None` for the mouse; input from any other source
/// is declined, so a second finger cannot steal the gesture and a foreign
/// lift cannot resolve it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SwipeGesture {
    origin: Option<Point>,
    owner: Option<touch::Finger>,
    /// Last sampled position and the instant it was sampled, for velocity.
    last: Option<(Point, Option<Instant>)>,
    /// Speed at the most recent sample, px/s along the policy's axis.
    velocity: f32,
    /// Whether the gesture is ours. [`press`](Self::press) claims at once;
    /// [`press_provisional`](Self::press_provisional) stays unclaimed until
    /// [`arbitrate`](Self::arbitrate) decides between swipe and scroll.
    claimed: bool,
}

impl SwipeGesture {
    /// A gesture that is not in flight.
    #[must_use]
    pub fn idle() -> Self {
        Self::default()
    }

    /// Whether a gesture is currently in flight.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.origin.is_some()
    }

    /// The finger that owns the gesture in flight, if it is a touch gesture.
    #[must_use]
    pub fn owner(&self) -> Option<touch::Finger> {
        self.owner
    }

    /// Where the gesture in flight began.
    #[must_use]
    pub fn origin(&self) -> Option<Point> {
        self.origin
    }

    /// Speed of the most recent sample, px/s along the axis.
    #[must_use]
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    /// Forget whatever is in flight, publishing nothing.
    pub fn clear(&mut self) {
        *self = Self::idle();
    }

    /// A pointer went down at `at`. `finger` is `Some` for touch and `None`
    /// for the mouse. `direction` is the ambient writing direction, used only
    /// to resolve a [`Leading`](EdgeSide::Leading) edge strip; policies
    /// without a logical edge can pass [`LeftToRight`](Direction::LeftToRight).
    ///
    /// Returns [`Declined`](SwipePhase::Declined) when a gesture is already in
    /// flight (a second finger must not steal it), or the press is outside the
    /// region or the policy's edge strip.
    pub fn press(
        &mut self,
        policy: &SwipePolicy,
        at: Point,
        finger: Option<touch::Finger>,
        bounds: Rectangle,
        direction: Direction,
    ) -> SwipePhase {
        if self.origin.is_some() {
            // A second press while a gesture is in flight is declined.
            return SwipePhase::Declined;
        }
        if !policy.accepts_press(at, bounds, direction) {
            return SwipePhase::Declined;
        }
        self.origin = Some(at);
        self.owner = finger;
        self.last = None;
        self.velocity = 0.0;
        // A plain press has no competitor: claim it now.
        self.claimed = true;
        SwipePhase::Began
    }

    /// A press that a descendant scrollable may also want. Admission matches
    /// [`press`](Self::press), but the gesture begins unclaimed: it does not
    /// commit on [`release`](Self::release) until
    /// [`arbitrate`](Self::arbitrate) resolves it as a swipe.
    pub fn press_provisional(
        &mut self,
        policy: &SwipePolicy,
        at: Point,
        finger: Option<touch::Finger>,
        bounds: Rectangle,
        direction: Direction,
    ) -> SwipePhase {
        let phase = self.press(policy, at, finger, bounds, direction);
        if phase == SwipePhase::Began {
            self.claimed = false;
        }
        phase
    }

    /// Whether the gesture in flight has been claimed as ours.
    ///
    /// `false` for a provisional gesture still being arbitrated, and for no
    /// gesture at all.
    #[must_use]
    pub fn is_claimed(&self) -> bool {
        self.origin.is_some() && self.claimed
    }

    /// Resolves a provisional gesture by its dominant axis once the pointer
    /// has moved more than `deadband` px: travel along the policy axis gives
    /// [`Claimed`](Arbitration::Claimed) (the caller should capture events),
    /// travel across it gives [`Yielded`](Arbitration::Yielded) (leave the
    /// events to the scroll), less gives [`Undecided`](Arbitration::Undecided).
    /// A claimed gesture stays claimed.
    pub fn arbitrate(
        &mut self,
        policy: &SwipePolicy,
        at: Point,
        finger: Option<touch::Finger>,
        deadband: f32,
    ) -> Arbitration {
        let Some(origin) = self.origin else {
            return Arbitration::Undecided;
        };
        if finger != self.owner {
            return Arbitration::Undecided;
        }
        if self.claimed {
            return Arbitration::Claimed;
        }
        if !at.x.is_finite() || !at.y.is_finite() {
            return Arbitration::Undecided;
        }

        let along = policy.axis.travel(origin, at).abs();
        let cross = policy.axis.cross_travel(origin, at).abs();
        let deadband = if deadband.is_finite() && deadband >= 0.0 {
            deadband
        } else {
            0.0
        };

        // Wait until *something* has moved decisively. Comparing the max travel
        // against the deadband means a diagonal wiggle inside the deadband is
        // still undecided.
        if along.max(cross) < deadband {
            return Arbitration::Undecided;
        }

        if along > cross {
            self.claimed = true;
            Arbitration::Claimed
        } else {
            // The cross axis wins (ties go to the scroll: a purely vertical or
            // ambiguous drag is a scroll, not a page swipe). Abandon ours.
            self.clear();
            Arbitration::Yielded
        }
    }

    /// The pointer moved. `now` is the frame instant from `RedrawRequested`;
    /// pass `None` for a distance-only policy.
    pub fn moved(
        &mut self,
        policy: &SwipePolicy,
        at: Point,
        finger: Option<touch::Finger>,
        now: Option<Instant>,
    ) -> SwipePhase {
        let Some(origin) = self.origin else {
            return SwipePhase::Declined;
        };
        if finger != self.owner {
            return SwipePhase::Declined;
        }
        if !at.x.is_finite() || !at.y.is_finite() {
            // A non-finite sample would poison distance and velocity; keep
            // the last good one.
            return SwipePhase::Declined;
        }

        if let (Some((previous, Some(then))), Some(now)) = (self.last, now) {
            let dt = now.saturating_duration_since(then).as_secs_f32();
            if dt > 0.0 {
                self.velocity = policy.axis.travel(previous, at) / dt;
            }
        }
        self.last = Some((at, now));

        SwipePhase::Moved {
            distance: policy.axis.travel(origin, at),
        }
    }

    /// A pointer was released at `at`. Resolves and clears the gesture:
    /// [`Committed`](SwipePhase::Committed) when the policy's rule is met,
    /// otherwise [`Cancelled`](SwipePhase::Cancelled). A release from a source
    /// that does not own the gesture returns [`Declined`](SwipePhase::Declined)
    /// and leaves it untouched.
    ///
    /// `at` is the raw release position, not clamped to the region: a swipe
    /// that ends outside the widget is still a swipe, and clamping would
    /// shorten it into a cancel.
    pub fn release(
        &mut self,
        policy: &SwipePolicy,
        at: Point,
        finger: Option<touch::Finger>,
        now: Option<Instant>,
    ) -> SwipePhase {
        let Some(origin) = self.origin else {
            return SwipePhase::Declined;
        };
        if finger != self.owner {
            return SwipePhase::Declined;
        }

        // Release-sample-folded velocity estimates: release-*is*-final-movement
        // flicks still-carrying speed.
        if let (Some((previous, Some(then))), Some(now)) = (self.last, now) {
            let dt = now.saturating_duration_since(then).as_secs_f32();
            if dt > 0.0 && at.x.is_finite() && at.y.is_finite() {
                self.velocity = policy.axis.travel(previous, at) / dt;
            }
        }

        let distance = policy.axis.travel(origin, at);
        let velocity = self.velocity;
        let claimed = self.claimed;
        self.clear();

        // An unclaimed gesture never commits, so a quick vertical flick
        // that drifted sideways does not page.
        if claimed && policy.commits(distance, velocity) {
            SwipePhase::Committed { distance, velocity }
        } else {
            SwipePhase::Cancelled
        }
    }

    /// The gesture was aborted: the cursor left, the compositor took the
    /// touch sequence, or the window lost focus. An abort cancels however far
    /// the pointer travelled; it never commits.
    ///
    /// A `finger` that does not own the gesture returns
    /// [`Declined`](SwipePhase::Declined), so one finger going away does not
    /// abort another finger's swipe.
    pub fn cancel(&mut self, finger: Option<touch::Finger>) -> SwipePhase {
        if self.origin.is_none() {
            return SwipePhase::Declined;
        }
        if finger != self.owner {
            return SwipePhase::Declined;
        }
        self.clear();
        SwipePhase::Cancelled
    }

    /// Abandon whatever is in flight regardless of who owns it.
    ///
    /// For events that name no source and end everything — window unfocus,
    /// a surface leaving the screen. Returns [`SwipePhase::Declined`] when
    /// nothing was in flight.
    pub fn cancel_any(&mut self) -> SwipePhase {
        if self.origin.is_none() {
            return SwipePhase::Declined;
        }
        self.clear();
        SwipePhase::Cancelled
    }
}
