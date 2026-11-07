//! Pointer-drag state and proposals for schedules.

use super::*;

/// Identifies what a schedule drag grabbed; the mode remains fixed until the gesture ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleGrab {
    /// Moves an event while preserving its duration and allowing a whole-column offset.
    MoveEvent {
        /// The event being moved.
        id: EventId,
    },
    /// Resizes an event by moving its end while keeping its start fixed.
    ResizeEnd {
        /// The event being resized.
        id: EventId,
    },
    /// Creates a time range from a drag that began on empty grid space.
    CreateRange,
}

/// A drag proposal that the caller may apply to its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleProposal {
    /// Moves an event while preserving its duration; `column_offset` is relative to its original column.
    Move {
        /// The event being moved.
        id: EventId,
        /// The proposed start minute; the end preserves the original event duration.
        new_start_minute: i32,
        /// Proposed new end minute (= `new_start + original_duration`).
        new_end_minute: i32,
        /// Signed whole-column offset from the event's original column.
        column_offset: i32,
    },
    /// Resizes the end while keeping it at least `min_duration_minutes` after the fixed start.
    Resize {
        /// The event being resized.
        id: EventId,
        /// The unchanged start minute, included for direct proposal consumption.
        start_minute: i32,
        /// The proposed end, no earlier than the start plus the minimum duration.
        new_end_minute: i32,
    },
    /// Creates an ordered range, extending it to the minimum duration when the configured bounds permit.
    Create {
        /// Proposed start minute of the new event.
        start_minute: i32,
        /// Proposed end minute of the new event.
        end_minute: i32,
        /// The column the drag began in.
        column: usize,
    },
}

/// The terminal outcome of a [`ScheduleDrag`], produced by
/// [`ScheduleDrag::release`] or [`ScheduleDrag::cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleDragOutcome {
    /// A proposal committed after the gesture crossed the deadband.
    Commit(ScheduleProposal),
    /// A press released inside the deadband, activating an event or empty-grid slot without proposing a model change.
    Activate {
        /// The event clicked, or `None` for an empty-grid slot click.
        event: Option<EventId>,
        /// The minute the press landed on (snapped).
        minute: i32,
        /// The column the press landed in.
        column: usize,
    },
    /// The gesture was cancelled without producing a proposal.
    Cancelled,
}

/// The lifecycle phase of a [`ScheduleDrag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleDragPhase {
    /// Pointer down, deadband not yet exceeded — still potentially a click.
    Pending,
    /// Deadband exceeded; the drag is live and emitting proposals.
    Active,
    /// Released or cancelled; inert.
    Finished,
}

/// A renderer-neutral drag session that computes proposals without mutating the caller model.
///
/// The grab mode is fixed for the session, and pointer positions are snapped and clamped through the supplied [`ScheduleAxis`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduleDrag {
    grab: ScheduleGrab,
    origin_column: usize,
    origin_start: i32,
    origin_end: i32,
    origin_px: (f32, f32),
    column_width: f32,
    granularity: i32,
    min_duration: i32,
    // Inclusive minute bounds supplied by the caller.
    min_minute: i32,
    max_minute: i32,
    deadband: f32,
    phase: ScheduleDragPhase,
    // Fixed snapped anchor used for every pointer delta.
    press_minute: i32,
    current_minute: i32,
    current_column: usize,
}

impl ScheduleDrag {
    /// Begins a schedule drag using axis-local pointer coordinates.
    ///
    /// Pass `(0, 0)` origins for creation and `f32::INFINITY` for `column_width` to lock columns. Invalid geometry is normalized, and `min_duration_minutes` may be zero.
    #[allow(
        clippy::too_many_arguments,
        reason = "a grab and every measure it is checked against"
    )]
    #[must_use]
    pub fn begin(
        grab: ScheduleGrab,
        origin_column: usize,
        origin_start: i32,
        origin_end: i32,
        origin_px: (f32, f32),
        axis: &ScheduleAxis,
        column_width: f32,
        min_duration_minutes: i32,
        deadband_px: f32,
    ) -> Self {
        let ox = if origin_px.0.is_finite() {
            origin_px.0
        } else {
            0.0
        };
        let oy = if origin_px.1.is_finite() {
            origin_px.1
        } else {
            0.0
        };
        // Infinity locks the drag to its origin column; all non-positive or non-finite widths normalize to that sentinel.
        let cw = if column_width.is_finite() && column_width > 0.0 {
            column_width
        } else {
            f32::INFINITY
        };
        let g = axis.granularity_minutes().max(1);
        let min_dur = min_duration_minutes.max(0);
        let press_minute = snap(axis.minute_of(oy), g);
        Self {
            grab,
            origin_column,
            origin_start,
            origin_end: origin_end.max(origin_start),
            origin_px: (ox, oy),
            column_width: cw,
            granularity: g,
            min_duration: min_dur,
            min_minute: axis.visible_start(),
            max_minute: axis.visible_end(),
            deadband: crate::metrics::FiniteNonNeg::new_or(deadband_px, 0.0).get(),
            phase: ScheduleDragPhase::Pending,
            press_minute,
            current_minute: press_minute,
            current_column: origin_column,
        }
    }

    /// Overrides the minute bounds, which default to the inclusive axis interval.
    ///
    /// Reversed bounds are sorted. Creation endpoints and move starts remain within the interval; preserving a move duration or enforcing a resize minimum can place an end above `hi`.
    #[must_use]
    pub fn with_minute_bounds(mut self, lo: i32, hi: i32) -> Self {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        self.min_minute = lo;
        self.max_minute = hi;
        self
    }

    /// The current lifecycle phase.
    #[must_use]
    pub fn phase(&self) -> ScheduleDragPhase {
        self.phase
    }

    /// Whether the drag has passed the deadband and is emitting proposals.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase == ScheduleDragPhase::Active
    }

    /// The grab kind, fixed at [`Self::begin`].
    #[must_use]
    pub fn grab(&self) -> ScheduleGrab {
        self.grab
    }

    /// The proposal this session would commit right now, or `None` while the
    /// gesture is still inside its deadband or already finished.
    ///
    /// The same value [`Self::pointer_moved`] publishes, exposed so a renderer
    /// can paint the in-flight gesture without mirroring it into its own state,
    /// where a stale copy could paint a range the session no longer proposes.
    #[must_use]
    pub fn preview(&self) -> Option<ScheduleProposal> {
        if self.is_active() {
            self.current_proposal()
        } else {
            None
        }
    }

    /// Updates the drag from an axis-local pointer position.
    ///
    /// Finite movement returns `None` until the deadband is crossed, and a finished session always returns `None`. Non-finite coordinates leave the session state unchanged but return the current proposal, even while pending.
    pub fn pointer_moved(
        &mut self,
        axis: &ScheduleAxis,
        position: (f32, f32),
    ) -> Option<ScheduleProposal> {
        if self.phase == ScheduleDragPhase::Finished {
            return None;
        }
        if !position.0.is_finite() || !position.1.is_finite() {
            return self.current_proposal();
        }
        if self.phase == ScheduleDragPhase::Pending {
            let dx = position.0 - self.origin_px.0;
            let dy = position.1 - self.origin_px.1;
            if (dx * dx + dy * dy) > self.deadband * self.deadband {
                self.phase = ScheduleDragPhase::Active;
            }
        }
        self.current_minute = snap(axis.minute_of(position.1), self.granularity);
        self.current_column = self.column_at(position.0);
        if self.is_active() {
            self.current_proposal()
        } else {
            None
        }
    }

    fn column_offset(&self, x: f32) -> i32 {
        if !self.column_width.is_finite() || self.column_width <= 0.0 {
            return 0;
        }
        let dx = x - self.origin_px.0;
        (dx / self.column_width).round() as i32
    }

    fn column_at(&self, x: f32) -> usize {
        let off = self.column_offset(x);
        (self.origin_column as i32 + off).max(0) as usize
    }

    fn clamp_minute(&self, m: i32) -> i32 {
        m.clamp(self.min_minute, self.max_minute)
    }

    fn current_proposal(&self) -> Option<ScheduleProposal> {
        match self.grab {
            ScheduleGrab::MoveEvent { id } => {
                let duration = self.origin_end - self.origin_start;
                let delta = self.current_minute - self.press_minute;
                let mut new_start = self.origin_start + delta;
                new_start = self.clamp_minute(new_start);
                if new_start + duration > self.max_minute {
                    new_start = self.max_minute - duration;
                }
                new_start = new_start.max(self.min_minute);
                let column_offset = self.current_column as i32 - self.origin_column as i32;
                Some(ScheduleProposal::Move {
                    id,
                    new_start_minute: new_start,
                    new_end_minute: new_start + duration,
                    column_offset,
                })
            }
            ScheduleGrab::ResizeEnd { id } => {
                let floor = self.origin_start + self.min_duration;
                let new_end = self.clamp_minute(self.current_minute).max(floor);
                Some(ScheduleProposal::Resize {
                    id,
                    start_minute: self.origin_start,
                    new_end_minute: new_end,
                })
            }
            ScheduleGrab::CreateRange => {
                let a = self.clamp_minute(self.press_minute);
                let b = self.clamp_minute(self.current_minute);
                let (mut lo, mut hi) = if a <= b { (a, b) } else { (b, a) };
                if hi - lo < self.min_duration {
                    hi = (lo + self.min_duration).min(self.max_minute);
                    lo = (hi - self.min_duration).max(self.min_minute);
                }
                Some(ScheduleProposal::Create {
                    start_minute: lo,
                    end_minute: hi,
                    column: self.origin_column,
                })
            }
        }
    }

    /// Finishes the session and returns its terminal outcome once.
    ///
    /// Active drags commit, while pending presses activate their event or slot.
    pub fn release(&mut self) -> Option<ScheduleDragOutcome> {
        if self.phase == ScheduleDragPhase::Finished {
            return None;
        }
        let out = if self.is_active() {
            ScheduleDragOutcome::Commit(self.current_proposal()?)
        } else {
            let event = match self.grab {
                ScheduleGrab::MoveEvent { id } | ScheduleGrab::ResizeEnd { id } => Some(id),
                ScheduleGrab::CreateRange => None,
            };
            ScheduleDragOutcome::Activate {
                event,
                minute: self.press_minute,
                column: self.origin_column,
            }
        };
        self.phase = ScheduleDragPhase::Finished;
        Some(out)
    }

    /// Cancels the session, returning [`ScheduleDragOutcome::Cancelled`] once.
    pub fn cancel(&mut self) -> Option<ScheduleDragOutcome> {
        if self.phase == ScheduleDragPhase::Finished {
            return None;
        }
        self.phase = ScheduleDragPhase::Finished;
        Some(ScheduleDragOutcome::Cancelled)
    }
}

// Rounds to the nearest granularity multiple; non-positive granularity rounds directly to an integer minute.
#[must_use]
pub(crate) fn snap(minute: f32, granularity: i32) -> i32 {
    if granularity <= 0 {
        return minute.round() as i32;
    }
    let g = granularity as f32;
    ((minute / g).round() * g) as i32
}
