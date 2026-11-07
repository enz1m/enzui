//! When to save without being asked: a short pause after the last change, and
//! at the latest a while after the first unsaved one.
//!
//! [`AutosaveScheduler`] is the timing policy only. Call
//! [`AutosaveScheduler::changed`] on every edit, wake at
//! [`AutosaveScheduler::next_deadline`] (an `iced::time` subscription or a
//! delayed task), and when [`AutosaveScheduler::due`] says so, take a
//! [`SaveTicket`] with [`AutosaveScheduler::begin`], write the document (to
//! its file, or to a [`recovery`](crate::recovery) copy) off the UI thread,
//! and report it with [`AutosaveScheduler::finished`]. An edit made while the
//! write runs keeps the document dirty, so the next save follows it.

use std::time::{Duration, Instant};

/// The autosave timing of one document.
#[derive(Debug, Clone)]
pub struct AutosaveScheduler {
    idle: Duration,
    max_wait: Duration,
    /// When the oldest unsaved change was made.
    dirty_since: Option<Instant>,
    last_change: Option<Instant>,
    /// Counts changes, so a save can tell whether it caught the latest.
    generation: u64,
    saving: Option<u64>,
}

/// One autosave in progress: what it saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "report the save with `AutosaveScheduler::finished`"]
pub struct SaveTicket {
    generation: u64,
}

impl Default for AutosaveScheduler {
    /// Two seconds after typing stops, and at most thirty seconds after the
    /// first unsaved change.
    fn default() -> Self {
        Self::new(Duration::from_secs(2), Duration::from_secs(30))
    }
}

impl AutosaveScheduler {
    /// Saves `idle` after the last change, and no later than `max_wait` after
    /// the first unsaved one even while changes keep coming.
    #[must_use]
    pub fn new(idle: Duration, max_wait: Duration) -> Self {
        Self {
            idle,
            max_wait: max_wait.max(idle),
            dirty_since: None,
            last_change: None,
            generation: 0,
            saving: None,
        }
    }

    /// Records a change at `now`.
    pub fn changed(&mut self, now: Instant) {
        self.generation += 1;
        self.dirty_since.get_or_insert(now);
        self.last_change = Some(now);
    }

    /// Whether there are unsaved changes.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// When a save becomes due; `None` when clean, while a save is running, or
    /// when both delays reach past what an [`Instant`] can hold (never due).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        use crate::monotonic_ms::{Policy, checked_instant_deadline};
        if self.saving.is_some() {
            return None;
        }
        let first = self.dirty_since?;
        let last = self.last_change?;
        let idle = checked_instant_deadline(last, self.idle, Policy::Clamp);
        let cap = checked_instant_deadline(first, self.max_wait, Policy::Clamp);
        match (idle, cap) {
            (Some(idle), Some(cap)) => Some(idle.min(cap)),
            (deadline, None) | (None, deadline) => deadline,
        }
    }

    /// Whether to save now.
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| now >= deadline)
    }

    /// Starts a save of the current state; `None` while another runs.
    pub fn begin(&mut self) -> Option<SaveTicket> {
        if self.saving.is_some() {
            return None;
        }
        self.saving = Some(self.generation);
        Some(SaveTicket {
            generation: self.generation,
        })
    }

    /// Reports the save `ticket` started, at `now`. A successful save clears
    /// the dirty state unless something changed since it began; then the
    /// unsaved time restarts at `now`. A failed save leaves it dirty.
    pub fn finished(&mut self, ticket: SaveTicket, succeeded: bool, now: Instant) {
        if self.saving != Some(ticket.generation) {
            return;
        }
        self.saving = None;
        if !succeeded {
            return;
        }
        if self.generation == ticket.generation {
            self.dirty_since = None;
            self.last_change = None;
        } else {
            self.dirty_since = Some(now);
        }
    }

    /// Records that the document was saved some other way (the user saved),
    /// so nothing is pending.
    pub fn saved(&mut self) {
        self.dirty_since = None;
        self.last_change = None;
        self.saving = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn autosave_preserves_dirty_work_across_delay_failure_and_inflight_edits() {
        // a_save_follows_a_pause_or_the_longest_wait
        {
            let start = Instant::now();
            let mut autosave = AutosaveScheduler::new(2 * SECOND, 10 * SECOND);
            assert_eq!(autosave.next_deadline(), None, "nothing to save");
            autosave.changed(start);
            assert_eq!(autosave.next_deadline(), Some(start + 2 * SECOND));
            assert!(!autosave.due(start + SECOND));
            // Steady typing keeps pushing the pause back, up to the longest wait.
            for second in 1..=9 {
                autosave.changed(start + second * SECOND);
            }
            assert_eq!(autosave.next_deadline(), Some(start + 10 * SECOND));
            assert!(autosave.due(start + 10 * SECOND));
        }

        // unrepresentable_delays_do_not_panic
        {
            let start = Instant::now();
            let mut pause_only = AutosaveScheduler::new(2 * SECOND, Duration::MAX);
            pause_only.changed(start);
            assert_eq!(pause_only.next_deadline(), Some(start + 2 * SECOND));
            // The longest wait is never shorter than the pause, so this never saves.
            let mut never = AutosaveScheduler::new(Duration::MAX, 10 * SECOND);
            never.changed(start);
            assert_eq!(never.next_deadline(), None);
            assert!(!never.due(start + 1000 * SECOND));
        }

        // an_edit_during_a_save_keeps_the_document_dirty
        {
            let start = Instant::now();
            let mut autosave = AutosaveScheduler::new(2 * SECOND, 10 * SECOND);
            autosave.changed(start);
            let ticket = autosave.begin().unwrap();
            assert!(autosave.begin().is_none(), "one save at a time");
            assert_eq!(autosave.next_deadline(), None, "no deadline while saving");
            autosave.changed(start + SECOND);
            autosave.finished(ticket, true, start + 3 * SECOND);
            assert!(autosave.is_dirty());
            assert_eq!(autosave.next_deadline(), Some(start + 3 * SECOND));

            let ticket = autosave.begin().unwrap();
            autosave.finished(ticket, true, start + 4 * SECOND);
            assert!(!autosave.is_dirty());
        }

        // a_failed_save_stays_due
        {
            let start = Instant::now();
            let mut autosave = AutosaveScheduler::new(2 * SECOND, 10 * SECOND);
            autosave.changed(start);
            let ticket = autosave.begin().unwrap();
            autosave.finished(ticket, false, start + 3 * SECOND);
            assert!(autosave.due(start + 3 * SECOND));
            autosave.saved();
            assert!(!autosave.is_dirty());
        }
    }
}
