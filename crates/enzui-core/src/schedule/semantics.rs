//! Accessible event descriptions and visible-window culling.

use super::*;

/// Structured accessibility data for one schedule event, including its date, time, duration, all-day state, and focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSemantics {
    /// The event id, for routing.
    pub id: EventId,
    /// A caller-supplied date label, because the engine does not assign calendar dates to minute zero.
    pub date_label: String,
    /// The event's title / payload text.
    pub title: String,
    /// Whether the event is all-day.
    pub all_day: bool,
    /// Start minute (ignored for all-day).
    pub start_minute: i32,
    /// End minute (ignored for all-day).
    pub end_minute: i32,
    /// Whether this event currently holds the internal focus.
    pub focused: bool,
}

impl EventSemantics {
    /// Returns an announcement ordered as title, date, time range and duration, then selection state.
    ///
    /// All-day events use `"all day"` instead of a time range.
    #[must_use]
    pub fn announcement(&self) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(5);
        if !self.title.is_empty() {
            parts.push(self.title.clone());
        }
        if !self.date_label.is_empty() {
            parts.push(self.date_label.clone());
        }
        if self.all_day {
            parts.push(crate::localization::text("enzui-schedule-all-day"));
        } else {
            parts.push(crate::localized!("enzui-time-range", "start" => format_localized_clock(self.start_minute), "end" => format_localized_clock(self.end_minute)));
            parts.push(format_duration(
                self.end_minute.max(self.start_minute) - self.start_minute,
            ));
        }
        if self.focused {
            parts.push(crate::localization::text("enzui-date-selected"));
        }
        parts.join(&crate::localization::text("enzui-list-separator"))
    }
}

// Counts examined events per thread so concurrent culls cannot affect exact test measurements.
#[cfg(test)]
thread_local! {
    static VISIBLE_EXAMINATIONS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
#[inline]
fn count_examination() {
    VISIBLE_EXAMINATIONS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[cfg(not(test))]
#[inline]
fn count_examination() {}

/// Returns events intersecting the axis’s half-open visible interval.
///
/// Edge-touching timed events are excluded, all-day events are retained, and the scan is O(n) in the input size.
#[must_use]
pub fn visible_events<P: Clone>(
    events: &[ScheduleEvent<P>],
    axis: &ScheduleAxis,
) -> Vec<ScheduleEvent<P>> {
    let vs = axis.visible_start();
    let ve = axis.visible_end();
    let mut out = Vec::new();
    for e in events {
        count_examination();
        if e.all_day {
            out.push(e.clone());
            continue;
        }
        let (s, en) = e.interval();
        if s < ve && en > vs {
            out.push(e.clone());
        }
    }
    out
}
