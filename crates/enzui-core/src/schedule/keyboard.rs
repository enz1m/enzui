//! Keyboard focus navigation and edit proposals for schedules.

use super::*;

/// A keyboard navigation or edit command for the focused schedule event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKey {
    /// Move focus to the next event in time order (Down / Tab).
    FocusNext,
    /// Move focus to the previous event in time order (Up / Shift+Tab).
    FocusPrev,
    /// Move focus to the event in the next column at a similar time (Right).
    FocusColumnNext,
    /// Move focus to the event in the previous column (Left).
    FocusColumnPrev,
    /// Nudge the focused event's whole interval later by one granularity step.
    MoveLater,
    /// Nudge the focused event's whole interval earlier by one step.
    MoveEarlier,
    /// Grow the focused event's end by one granularity step.
    GrowEnd,
    /// Shrink the focused event's end by one step (never below `min_duration`).
    ShrinkEnd,
}

/// The result of applying a [`ScheduleKey`] to a [`ScheduleFocus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKeyOutcome {
    /// Focus moved to `id` (a pure navigation, no model change).
    Focused(EventId),
    /// A model change is proposed for the focused event.
    Propose(ScheduleProposal),
    /// The key had no effect (no focus, no next event, at a bound).
    Ignored,
}

/// A keyboard focus cursor for a schedule grid.
///
/// The grid exposes one tab stop, while arrow navigation moves an internal cursor. Routed IDs derive from [`EventId`], so they remain stable across reorder and filtering.
#[derive(Debug, Clone)]
pub struct ScheduleFocus {
    order: Vec<(EventId, usize, i32)>,
    focused: Option<EventId>,
}

impl ScheduleFocus {
    /// Builds a cursor ordered by `(start_minute, column, id)`, excluding all-day events.
    #[must_use]
    pub fn new<P>(events: &[ScheduleEvent<P>]) -> Self {
        let mut order: Vec<(EventId, usize, i32)> = events
            .iter()
            .filter(|e| !e.all_day)
            .map(|e| (e.id, e.column, e.start_minute))
            .collect();
        order.sort_by_key(|a| (a.2, a.1, a.0));
        Self {
            order,
            focused: None,
        }
    }

    /// The number of navigable events (all-day excluded).
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether there are no navigable events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Returns the grid’s single tab stop, independent of event count.
    #[must_use]
    pub fn tab_stops(&self) -> usize {
        1
    }

    /// Convenience predicate for [`Self::tab_stops`] `== 1`.
    #[must_use]
    pub fn is_single_tab_stop(&self) -> bool {
        self.tab_stops() == 1
    }

    /// The currently focused event, if any.
    #[must_use]
    pub fn focused(&self) -> Option<EventId> {
        self.focused
    }

    /// Focuses `id` when it is navigable and returns whether focus changed.
    pub fn focus(&mut self, id: EventId) -> bool {
        if self.order.iter().any(|(e, _, _)| *e == id) {
            let changed = self.focused != Some(id);
            self.focused = Some(id);
            changed
        } else {
            false
        }
    }

    /// Returns the focused event’s routed assistive-technology ID under `prefix`.
    ///
    /// The ID is derived from [`EventId`], not navigation position, and is `None` without focus.
    #[must_use]
    pub fn routed_id(&self, prefix: &str) -> Option<iced_core::widget::Id> {
        self.focused
            .map(|id| crate::identity::derive_id(prefix, &format!("event-{}", id.get())))
    }

    fn focused_index(&self) -> Option<usize> {
        let f = self.focused?;
        self.order.iter().position(|(e, _, _)| *e == f)
    }

    /// Applies a keyboard command using caller-owned interval data for edit proposals.
    ///
    /// Navigation commands ignore the interval arguments.
    pub fn apply(
        &mut self,
        key: ScheduleKey,
        axis: &ScheduleAxis,
        focused_start: i32,
        focused_end: i32,
        min_duration_minutes: i32,
    ) -> ScheduleKeyOutcome {
        let step = axis.granularity_minutes().max(1);
        let min_dur = min_duration_minutes.max(0);
        match key {
            ScheduleKey::FocusNext => self.step_focus(1),
            ScheduleKey::FocusPrev => self.step_focus(-1),
            ScheduleKey::FocusColumnNext => self.step_column(1),
            ScheduleKey::FocusColumnPrev => self.step_column(-1),
            ScheduleKey::MoveLater | ScheduleKey::MoveEarlier => {
                let Some(id) = self.focused else {
                    return ScheduleKeyOutcome::Ignored;
                };
                let lo = axis.visible_start();
                let hi = axis.visible_end();
                let duration = focused_end.saturating_sub(focused_start).max(0);
                let delta = if key == ScheduleKey::MoveLater {
                    step
                } else {
                    -step
                };
                // Events may outlast the visible interval, so bounds are applied in order.
                let mut new_start = focused_start.saturating_add(delta);
                new_start = new_start.clamp(lo, hi);
                if new_start.saturating_add(duration) > hi {
                    new_start = hi.saturating_sub(duration);
                }
                new_start = new_start.max(lo);
                if new_start == focused_start {
                    return ScheduleKeyOutcome::Ignored;
                }
                ScheduleKeyOutcome::Propose(ScheduleProposal::Move {
                    id,
                    new_start_minute: new_start,
                    new_end_minute: new_start.saturating_add(duration),
                    column_offset: 0,
                })
            }
            ScheduleKey::GrowEnd | ScheduleKey::ShrinkEnd => {
                let Some(id) = self.focused else {
                    return ScheduleKeyOutcome::Ignored;
                };
                let delta = if key == ScheduleKey::GrowEnd {
                    step
                } else {
                    -step
                };
                // The minimum duration may extend past the visible interval.
                let floor = focused_start.saturating_add(min_dur);
                let mut new_end = focused_end.saturating_add(delta);
                new_end = new_end
                    .clamp(axis.visible_start(), axis.visible_end())
                    .max(floor);
                if new_end == focused_end {
                    return ScheduleKeyOutcome::Ignored;
                }
                ScheduleKeyOutcome::Propose(ScheduleProposal::Resize {
                    id,
                    start_minute: focused_start,
                    new_end_minute: new_end,
                })
            }
        }
    }

    fn step_focus(&mut self, dir: i32) -> ScheduleKeyOutcome {
        if self.order.is_empty() {
            return ScheduleKeyOutcome::Ignored;
        }
        let next_idx = match self.focused_index() {
            None => {
                if dir > 0 {
                    0
                } else {
                    self.order.len() - 1
                }
            }
            Some(i) => {
                let ni = i as i32 + dir;
                if ni < 0 || ni as usize >= self.order.len() {
                    return ScheduleKeyOutcome::Ignored;
                }
                ni as usize
            }
        };
        let id = self.order[next_idx].0;
        self.focused = Some(id);
        ScheduleKeyOutcome::Focused(id)
    }

    fn step_column(&mut self, dir: i32) -> ScheduleKeyOutcome {
        let Some(i) = self.focused_index() else {
            return self.step_focus(dir);
        };
        let (_, cur_col, cur_start) = self.order[i];
        let target_col = cur_col as i32 + dir;
        if target_col < 0 {
            return ScheduleKeyOutcome::Ignored;
        }
        let target_col = target_col as usize;
        let best = self
            .order
            .iter()
            .filter(|(_, c, _)| *c == target_col)
            .min_by_key(|(_, _, s)| (s - cur_start).abs());
        match best {
            Some((id, _, _)) => {
                self.focused = Some(*id);
                ScheduleKeyOutcome::Focused(*id)
            }
            None => ScheduleKeyOutcome::Ignored,
        }
    }
}
