//! All-day event placement for schedules.

use super::*;

/// Placement of an all-day event in a column-local stacking slot.
///
/// All-day events are outside the timed axis, so placement has no `y` coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllDayPlacement<P> {
    /// The event's stable identity.
    pub id: EventId,
    /// The column the event occupies.
    pub column: usize,
    /// 0-based stacking slot within this column's all-day lane.
    pub slot: usize,
    /// How many slots this column's all-day lane needs (the tallest column's
    /// value is what the shell sizes the band from).
    pub slot_count: usize,
    /// Opaque caller payload, carried through untouched.
    pub payload: P,
}

/// Assigns all-day events deterministic per-column slots and reports each column’s slot count.
///
/// Timed events are excluded and all all-day events in one column coexist.
#[must_use]
pub fn layout_all_day<P: Clone>(events: &[ScheduleEvent<P>]) -> Vec<AllDayPlacement<P>> {
    let mut all_day: Vec<&ScheduleEvent<P>> = events.iter().filter(|e| e.all_day).collect();
    all_day.sort_by(|a, b| {
        (a.start_minute, a.end_minute, a.id).cmp(&(b.start_minute, b.end_minute, b.id))
    });

    let mut by_column: std::collections::BTreeMap<usize, Vec<&ScheduleEvent<P>>> =
        std::collections::BTreeMap::new();
    for e in all_day {
        by_column.entry(e.column).or_default().push(e);
    }

    let mut out = Vec::new();
    for (column, column_events) in by_column {
        let slot_count = column_events.len();
        for (slot, e) in column_events.into_iter().enumerate() {
            out.push(AllDayPlacement {
                id: e.id,
                column,
                slot,
                slot_count,
                payload: e.payload.clone(),
            });
        }
    }
    out
}
