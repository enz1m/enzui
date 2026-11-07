//! State and outcomes for two-click date-range gestures.
//!
//! The second click completes a sorted range, including a one-day range when both dates match. A third click starts a replacement range, and a pending anchor is displayed but not committed.

use crate::civil_date::{Date, DateRange};

/// State of a two-click range-selection gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RangeSelection {
    /// Nothing chosen yet.
    #[default]
    Empty,
    /// One endpoint chosen; the next click completes the range.
    Pending(Date),
    /// A complete range. The next click starts a new one.
    Complete(DateRange),
}

/// Describes how a click advanced the gesture so a host can react without diffing states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeStep {
    /// The first endpoint was set. The gesture is now half-done.
    Anchored(Date),
    /// The range was completed by the second click.
    Completed(DateRange),
    /// A completed range was discarded and a new gesture begun.
    Restarted(Date),
}

impl RangeStep {
    /// Returns whether this step completed a range.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.completed().is_some()
    }

    /// Returns the range carried by a completed step, or `None` for anchoring and restarting.
    #[must_use]
    pub const fn completed(self) -> Option<DateRange> {
        match self {
            Self::Completed(range) => Some(range),
            Self::Anchored(_) | Self::Restarted(_) => None,
        }
    }
}

impl RangeSelection {
    /// Start from an existing range, e.g. a value loaded from storage.
    #[must_use]
    pub const fn complete(range: DateRange) -> Self {
        Self::Complete(range)
    }

    /// Returns the completed range, or `None` while empty or awaiting a second click.
    #[must_use]
    pub const fn committed(self) -> Option<DateRange> {
        match self {
            Self::Complete(range) => Some(range),
            Self::Empty | Self::Pending(_) => None,
        }
    }

    /// Returns the range to display, treating a pending anchor as a one-day range.
    #[must_use]
    pub const fn displayed(self) -> Option<DateRange> {
        match self {
            Self::Empty => None,
            Self::Pending(anchor) => Some(DateRange::single(anchor)),
            Self::Complete(range) => Some(range),
        }
    }

    /// The anchor awaiting a second click, if any.
    #[must_use]
    pub const fn pending(self) -> Option<Date> {
        match self {
            Self::Pending(anchor) => Some(anchor),
            Self::Empty | Self::Complete(_) => None,
        }
    }

    /// Applies a click, advances this state, and returns the click’s semantic step.
    pub fn click(&mut self, date: Date) -> RangeStep {
        let (next, step) = match *self {
            Self::Empty => (Self::Pending(date), RangeStep::Anchored(date)),
            Self::Pending(anchor) => {
                let range = DateRange::sorted(anchor, date);
                (Self::Complete(range), RangeStep::Completed(range))
            }
            Self::Complete(_) => (Self::Pending(date), RangeStep::Restarted(date)),
        };
        *self = next;
        step
    }

    /// Previews the sorted range for `date` when awaiting a second endpoint; otherwise returns `None`.
    #[must_use]
    pub fn preview(self, date: Date) -> Option<DateRange> {
        self.pending().map(|anchor| DateRange::sorted(anchor, date))
    }

    /// Abandons a pending gesture while preserving any completed range.
    pub const fn cancel(&mut self) {
        if let Self::Pending(_) = *self {
            *self = Self::Empty;
        }
    }

    /// Clear everything, committed range included.
    pub const fn clear(&mut self) {
        *self = Self::Empty;
    }
}
