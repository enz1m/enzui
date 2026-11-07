//! Download lifecycle, attempt gating, bounded queueing, transport events, and persistence.
//!
//! The reducer performs no I/O. Persisted records use [`crate::atomic_state`], while this module defines download-specific reconciliation through [`reconcile`].

use std::collections::{HashMap, HashSet};

/// Application-owned download identity that remains stable across retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(pub u64);

/// Identifies one attempt so replies from superseded attempts can be discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Attempt(pub u32);

impl Attempt {
    /// The first attempt.
    pub const FIRST: Attempt = Attempt(0);

    /// The next attempt, used when retrying.
    #[must_use]
    pub fn next(self) -> Attempt {
        Attempt(self.0 + 1)
    }
}

/// Lifecycle of one download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waits to start or resume without holding a concurrency slot.
    Queued,
    /// Holds a concurrency slot for transfer.
    Active,
    /// Suspended by the owner without a concurrency slot.
    ///
    /// Paused downloads reject progress and resume through [`Download::resume`] under the same attempt.
    Paused,
    /// Finished successfully.
    Complete,
    /// Failed; ordinary failures may be retried, while policy-blocked failures may not.
    Failed,
    /// Cancelled by the owner.
    Cancelled,
}

impl Phase {
    /// Returns whether no transition is allowed without retry.
    ///
    /// [`Phase::Paused`] is not terminal and can still be failed or resumed.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Complete | Phase::Failed | Phase::Cancelled)
    }

    /// Returns the stable phase word used for visible and assistive status text.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            Phase::Queued => crate::localization::text("enzui-download-queued"),
            Phase::Active => crate::localization::text("enzui-download-downloading"),
            Phase::Paused => crate::localization::text("enzui-download-paused"),
            Phase::Complete => crate::localization::text("enzui-download-complete"),
            Phase::Failed => crate::localization::text("enzui-download-failed"),
            Phase::Cancelled => crate::localization::text("enzui-download-cancelled"),
        }
    }
}

/// A canonical action whose availability is determined by [`Download::actions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DownloadAction {
    /// Suspend a queued or active transfer, keeping the attempt.
    Pause,
    /// Resume a paused transfer under the same attempt.
    Resume,
    /// Abandon the download. Terminal.
    Cancel,
    /// Start over, advancing the attempt.
    Retry,
}

impl DownloadAction {
    /// Every action, in the canonical order a row presents them.
    pub const ALL: [DownloadAction; 4] = [
        DownloadAction::Pause,
        DownloadAction::Resume,
        DownloadAction::Cancel,
        DownloadAction::Retry,
    ];

    /// Returns the action’s stable user-facing verb.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            DownloadAction::Pause => crate::localization::text("enzui-download-pause"),
            DownloadAction::Resume => crate::localization::text("enzui-download-resume"),
            DownloadAction::Cancel => crate::localization::text("enzui-download-cancel"),
            DownloadAction::Retry => crate::localization::text("enzui-download-retry"),
        }
    }
}

/// A coalesced owner notification; each download retains at most one pending value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notification {
    /// Cumulative bytes moved, with only the latest progress value retained.
    Progress {
        /// The attempt the tick belongs to.
        attempt: Attempt,
        /// Total bytes received by that attempt so far.
        received: u64,
    },
    /// A terminal transition retained until [`Download::acknowledge`], even after [`Download::take_pending`].
    Terminal {
        /// The attempt that terminated.
        attempt: Attempt,
        /// Which terminal phase was reached.
        phase: Phase,
    },
}

impl Notification {
    /// The attempt this notification belongs to.
    #[must_use]
    pub fn attempt(self) -> Attempt {
        match self {
            Notification::Progress { attempt, .. } | Notification::Terminal { attempt, .. } => {
                attempt
            }
        }
    }

    /// Whether this is a terminal transition awaiting acknowledgement.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Notification::Terminal { .. })
    }
}

/// One download's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    /// Stable, application-owned identity. Never changes, including on retry.
    pub id: DownloadId,
    /// Current attempt generation, advanced by [`Download::retry`].
    pub attempt: Attempt,
    /// Where this download is in its lifecycle.
    pub phase: Phase,
    /// Bytes transferred so far in the current attempt.
    pub received: u64,
    // Expected bytes for the current attempt, or `None` when indeterminate.
    // Updates pass through the attempt gate in [`Download::set_total`].
    total: Option<u64>,
    // The single coalescing slot is mutated only through gated methods so pending terminal events cannot be displaced.
    pending: Option<Notification>,
    // Policy-refused failures are nonretryable and can be set only by `finish_blocked`.
    blocked: bool,
}

impl Download {
    /// A newly queued download at its first attempt.
    #[must_use]
    pub fn queued(id: DownloadId) -> Self {
        Self {
            id,
            attempt: Attempt::FIRST,
            phase: Phase::Queued,
            received: 0,
            total: None,
            pending: None,
            blocked: false,
        }
    }

    /// Returns whether `attempt` is current.
    ///
    /// This gate intentionally ignores lifecycle so harmless late progress and totals for the current attempt may still be recorded.
    #[must_use]
    pub fn accepts(&self, attempt: Attempt) -> bool {
        self.attempt == attempt
    }

    /// Returns whether a range response may mutate this download.
    ///
    /// The attempt must be current and the phase non-terminal, preventing late responses from restarting completed, failed, or cancelled work. Paused downloads remain eligible.
    #[must_use]
    pub fn accepts_range_response(&self, attempt: Attempt) -> bool {
        self.accepts(attempt) && !self.phase.is_terminal()
    }

    /// Advances to a new queued attempt, clearing progress, total, and pending notification.
    ///
    /// Policy-refused failures are not retried.
    pub fn retry(&mut self) {
        if self.blocked {
            return;
        }
        self.attempt = self.attempt.next();
        self.phase = Phase::Queued;
        self.received = 0;
        // A new attempt re-negotiates its own length; carrying the old
        // attempt's total forward would render a fraction against a number
        // nothing has confirmed.
        self.total = None;
        self.pending = None;
    }

    /// Returns the announced total for the current attempt, or `None` when indeterminate.
    #[must_use]
    pub fn total(&self) -> Option<u64> {
        self.total
    }

    /// Records the announced total for `attempt`.
    ///
    /// Stale attempts return `false` without mutation, and `Some(0)` is retained as a known zero-byte total.
    pub fn set_total(&mut self, attempt: Attempt, total: Option<u64>) -> bool {
        if !self.accepts(attempt) {
            return false;
        }
        self.total = total;
        true
    }

    /// Returns progress in `[0, 1]`, or `None` when total size is unknown.
    ///
    /// A known zero total is complete, over-delivery clamps to `1`, and division uses `f64` before the final `f32` conversion to avoid premature completion.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            None => None,
            Some(0) => Some(1.0),
            Some(total) => Some(((self.received as f64) / (total as f64)).clamp(0.0, 1.0) as f32),
        }
    }

    /// Returns whole-percent progress in `0..=100`, or `None` when indeterminate.
    ///
    /// Percentages are truncated and computed with integer arithmetic; `100` is returned only when `received >= total`.
    #[must_use]
    pub fn percent(&self) -> Option<u32> {
        match self.total {
            None => None,
            Some(0) => Some(100),
            Some(total) => {
                if self.received >= total {
                    Some(100)
                } else {
                    // `received < total` → the quotient is in `0..=99`.
                    // `u128::from(u64) * 100` cannot overflow (u64::MAX * 100
                    // is 58 bits short of u128::MAX).
                    let pct = (u128::from(self.received) * 100 / u128::from(total)) as u32;
                    Some(pct.min(99))
                }
            }
        }
    }

    /// Returns whether an action is currently available.
    ///
    /// Queued or active downloads allow pause and cancel; paused downloads allow resume and cancel; failed or cancelled downloads allow retry; complete and policy-blocked downloads allow none.
    #[must_use]
    pub fn can(&self, action: DownloadAction) -> bool {
        match action {
            DownloadAction::Pause => matches!(self.phase, Phase::Queued | Phase::Active),
            DownloadAction::Resume => self.phase == Phase::Paused,
            DownloadAction::Cancel => {
                matches!(self.phase, Phase::Queued | Phase::Active | Phase::Paused)
            }
            DownloadAction::Retry => {
                matches!(self.phase, Phase::Failed | Phase::Cancelled) && !self.blocked
            }
        }
    }

    /// Whether this download failed because its destination was refused by
    /// the URL policy, which makes the failure nonretryable.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    /// The available actions, in [`DownloadAction::ALL`] order.
    #[must_use]
    pub fn actions(&self) -> Vec<DownloadAction> {
        DownloadAction::ALL
            .into_iter()
            .filter(|a| self.can(*a))
            .collect()
    }

    /// Pauses queued or active work while preserving attempt and received bytes.
    ///
    /// Returns `false` when unavailable; pausing releases any concurrency slot.
    pub fn pause(&mut self) -> bool {
        if !self.can(DownloadAction::Pause) {
            return false;
        }
        self.phase = Phase::Paused;
        true
    }

    /// Returns a paused download to the queue under the same attempt, preserving bytes.
    ///
    /// Returns `false` unless paused; promotion remains responsible for acquiring a slot.
    pub fn resume(&mut self) -> bool {
        if !self.can(DownloadAction::Resume) {
            return false;
        }
        self.phase = Phase::Queued;
        true
    }

    /// Cancel the download, recorded as a terminal transition pending
    /// acknowledgement. Returns `false` when cancelling is unavailable.
    pub fn cancel(&mut self) -> bool {
        if !self.can(DownloadAction::Cancel) {
            return false;
        }
        // Route through `finish` so cancellation obeys the same
        // "exactly one durable terminal transition" rule as every other
        // terminal phase, instead of a second path that writes `phase`.
        self.finish(self.attempt, Phase::Cancelled)
    }

    /// Applies a canonical action and returns whether it was available and applied.
    ///
    /// Unavailable actions are no-ops, allowing stale UI events to be ignored.
    pub fn apply(&mut self, action: DownloadAction) -> bool {
        if !self.can(action) {
            return false;
        }
        match action {
            DownloadAction::Pause => self.pause(),
            DownloadAction::Resume => self.resume(),
            DownloadAction::Cancel => self.cancel(),
            DownloadAction::Retry => {
                self.retry();
                true
            }
        }
    }

    /// The pending notification without consuming it.
    #[must_use]
    pub fn pending(&self) -> Option<Notification> {
        self.pending
    }

    /// Records cumulative progress for `attempt`.
    ///
    /// Stale attempts return `false` without mutation. Progress coalesces to the latest value and never displaces an unacknowledged terminal notification.
    pub fn progress(&mut self, attempt: Attempt, received: u64) -> bool {
        if !self.accepts(attempt) {
            return false;
        }
        // Reject progress while paused so continued transport output cannot advance suspended state.
        if self.phase == Phase::Paused {
            return false;
        }
        self.received = received;
        if !self.pending.is_some_and(Notification::is_terminal) {
            self.pending = Some(Notification::Progress { attempt, received });
        }
        true
    }

    /// Applies terminal `phase` for `attempt` and records it pending acknowledgement.
    ///
    /// Returns `false` without mutation for stale attempts, non-terminal phases, or downloads already terminal.
    pub fn finish(&mut self, attempt: Attempt, phase: Phase) -> bool {
        if !self.accepts(attempt) || !phase.is_terminal() || self.phase.is_terminal() {
            return false;
        }
        self.phase = phase;
        self.pending = Some(Notification::Terminal { attempt, phase });
        true
    }

    /// Fails `attempt` as a nonretryable policy refusal.
    ///
    /// Uses the same stale and terminal gates as [`Download::finish`]; refusal records nothing unless that transition succeeds.
    pub fn finish_blocked(&mut self, attempt: Attempt) -> bool {
        if !self.finish(attempt, Phase::Failed) {
            return false;
        }
        self.blocked = true;
        true
    }

    /// Takes the pending notification.
    ///
    /// Progress is consumed, while terminal notification remains pending until [`Download::acknowledge`].
    pub fn take_pending(&mut self) -> Option<Notification> {
        let pending = self.pending;
        if !pending.is_some_and(Notification::is_terminal) {
            self.pending = None;
        }
        pending
    }

    /// Acknowledges the pending notification only when it belongs to `attempt`.
    pub fn acknowledge(&mut self, attempt: Attempt) -> bool {
        if self.pending.is_some_and(|n| n.attempt() == attempt) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// Applies a range decision to a current, non-terminal attempt.
    ///
    /// `Resume` keeps partial bytes; `RestartWithBody` starts a new attempt and records any announced complete-body length; `DiscardAndRetry` starts a new attempt with unknown total; `Refuse` fails the download. Returns `false` without mutation when the gate or transition rejects the decision.
    pub fn apply_range_decision(&mut self, attempt: Attempt, decision: RangeDecision) -> bool {
        if !self.accepts_range_response(attempt) {
            return false;
        }
        match decision {
            RangeDecision::Resume { total, .. } => {
                self.set_total(attempt, total);
                true
            }
            RangeDecision::RestartWithBody { expected, .. } => {
                // Advance the attempt before recording the complete-body length on the new generation.
                self.retry();
                self.set_total(self.attempt, expected.len);
                true
            }
            RangeDecision::DiscardAndRetry { .. } => {
                // Advance the attempt to reject late discarded chunks; this response confirms no total for the retry.
                self.retry();
                true
            }
            RangeDecision::Refuse { .. } => self.finish(attempt, Phase::Failed),
        }
    }

    /// Decides and applies a range response using this download’s stored total.
    ///
    /// Returns `Some` only for a decision that mutated state; stale, terminal, or otherwise refused responses return `None`. `plan` and `partial_coding` describe transport-owned request state.
    pub fn handle_range_response(
        &mut self,
        attempt: Attempt,
        plan: &RangeRequestPlan,
        partial_coding: &ContentCoding,
        response: &RangeResponse<'_>,
    ) -> Option<RangeDecision> {
        if !self.accepts_range_response(attempt) {
            return None;
        }
        let decision = decide_range_response(plan, self.total, partial_coding, response);
        // Report `Some` only after the decision has actually been applied.
        if !self.apply_range_decision(attempt, decision.clone()) {
            return None;
        }
        Some(decision)
    }
}

/// A bounded download queue that promotes oldest-queued work first.
///
/// At most `max_active` entries hold slots, terminal history is bounded, and lifecycle mutations pass through queue methods so completion order and pruning remain consistent.
///
/// ```compile_fail
/// use enzui_core::download::{DownloadId, DownloadQueue};
/// let mut queue = DownloadQueue::new(1);
/// queue.admit(DownloadId(1));
/// let _ = queue.get_mut(DownloadId(1));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadQueue {
    downloads: Vec<Download>,
    // Each id maps to its current `downloads` index; rebuild after removals shift later slots.
    indices: HashMap<DownloadId, usize>,
    // Terminal ids are ordered by completion time so history eviction does not disturb admission-order fairness.
    completion_order: Vec<DownloadId>,
    max_active: usize,
    history_limit: usize,
}

/// Default maximum number of retained terminal downloads.
///
/// Use [`DownloadQueue::with_history_limit`] for another bound; zero retains no terminal history.
pub const DEFAULT_HISTORY_LIMIT: usize = 1_000;

#[cfg(test)]
pub(crate) mod prune_probe {
    use std::cell::Cell;
    thread_local! {
        static TOUCHES: Cell<u64> = const { Cell::new(0) };
    }
    pub(crate) fn touch() {
        TOUCHES.with(|c| c.set(c.get() + 1));
    }
}

#[cfg(not(test))]
mod prune_probe {
    #[inline(always)]
    pub(crate) fn touch() {}
}

#[cfg(any(test, feature = "test-frame"))]
#[doc(hidden)]
pub mod download_queue_lookup_probe {
    use std::cell::Cell;

    thread_local! {
        static LOOKUPS: Cell<u64> = const { Cell::new(0) };
    }

    pub fn reset() {
        LOOKUPS.with(|count| count.set(0));
    }

    #[must_use]
    pub fn count() -> u64 {
        LOOKUPS.with(Cell::get)
    }

    pub(super) fn touch() {
        LOOKUPS.with(|count| count.set(count.get() + 1));
    }
}

#[cfg(not(any(test, feature = "test-frame")))]
mod download_queue_lookup_probe {
    #[inline(always)]
    pub(super) fn touch() {}
}

impl DownloadQueue {
    /// Creates a queue retaining [`DEFAULT_HISTORY_LIMIT`] terminal entries.
    ///
    /// `max_active` is clamped to at least one.
    #[must_use]
    pub fn new(max_active: usize) -> Self {
        Self::with_history_limit(max_active, DEFAULT_HISTORY_LIMIT)
    }

    /// Creates a queue with explicit concurrency and terminal-history limits.
    ///
    /// `max_active` is clamped to at least one. `limit` is literal, including zero, and never evicts non-terminal downloads.
    #[must_use]
    pub fn with_history_limit(max_active: usize, limit: usize) -> Self {
        Self {
            downloads: Vec::new(),
            indices: HashMap::new(),
            completion_order: Vec::new(),
            max_active: max_active.max(1),
            history_limit: limit,
        }
    }

    /// The concurrency cap.
    #[must_use]
    pub fn max_active(&self) -> usize {
        self.max_active
    }

    /// Changes the concurrency cap, clamping values below one to one.
    ///
    /// Existing active transfers are never cancelled; a lower cap takes effect as they finish, while a higher cap is consumed by the next [`Self::promote`] call.
    pub fn set_max_active(&mut self, max_active: usize) {
        self.max_active = max_active.max(1);
    }

    /// The retained-terminal-history cap. See
    /// [`DownloadQueue::with_history_limit`].
    #[must_use]
    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    /// Number of downloads currently holding a slot ([`Phase::Active`]).
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.downloads
            .iter()
            .filter(|d| d.phase == Phase::Active)
            .count()
    }

    /// Total number of tracked downloads, in any phase.
    #[must_use]
    pub fn len(&self) -> usize {
        self.downloads.len()
    }

    /// Whether no downloads are tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.downloads.is_empty()
    }

    /// The tracked downloads, in admission order.
    pub fn iter(&self) -> impl Iterator<Item = &Download> {
        self.downloads.iter()
    }

    // Refuse stale index entries rather than aliasing another download or hiding corruption with a linear scan.
    fn index_of(&self, id: DownloadId) -> Option<usize> {
        download_queue_lookup_probe::touch();
        let index = self.indices.get(&id).copied()?;
        self.downloads
            .get(index)
            .is_some_and(|download| download.id == id)
            .then_some(index)
    }

    fn rebuild_indices(&mut self) {
        self.indices.clear();
        self.indices.reserve(self.downloads.len());
        for (index, download) in self.downloads.iter().enumerate() {
            let previous = self.indices.insert(download.id, index);
            debug_assert!(previous.is_none(), "duplicate download id in queue");
        }
    }

    /// The state of one download, if tracked.
    #[must_use]
    pub fn get(&self, id: DownloadId) -> Option<&Download> {
        let index = self.index_of(id)?;
        self.downloads.get(index)
    }

    // Mutable access stays private so terminal transitions cannot bypass completion ordering and pruning.
    fn get_mut(&mut self, id: DownloadId) -> Option<&mut Download> {
        let index = self.index_of(id)?;
        self.downloads.get_mut(index)
    }

    /// Announce the expected total for a tracked attempt.
    pub fn set_total(&mut self, id: DownloadId, attempt: Attempt, total: Option<u64>) -> bool {
        self.get_mut(id)
            .is_some_and(|download| download.set_total(attempt, total))
    }

    /// Deliver cumulative progress for a tracked attempt.
    pub fn progress(&mut self, id: DownloadId, attempt: Attempt, received: u64) -> bool {
        self.get_mut(id)
            .is_some_and(|download| download.progress(attempt, received))
    }

    /// Take the tracked download's pending notification, if any.
    pub fn take_pending(&mut self, id: DownloadId) -> Option<Notification> {
        self.get_mut(id).and_then(Download::take_pending)
    }

    /// Acknowledge a terminal notification for a tracked attempt.
    pub fn acknowledge(&mut self, id: DownloadId, attempt: Attempt) -> bool {
        self.get_mut(id)
            .is_some_and(|download| download.acknowledge(attempt))
    }

    /// Admits `id` at the back in [`Phase::Queued`].
    ///
    /// Duplicate ids return `false` without mutation; admission never promotes work.
    pub fn admit(&mut self, id: DownloadId) -> bool {
        if self.indices.contains_key(&id) {
            return false;
        }
        let index = self.downloads.len();
        self.downloads.push(Download::queued(id));
        let previous = self.indices.insert(id, index);
        debug_assert!(previous.is_none(), "duplicate download id admitted");
        true
    }

    /// Restores a reconciled download at the back without promotion.
    ///
    /// Duplicate ids and [`Phase::Active`] records are refused without mutation; active restore would bypass slot accounting. Terminal records immediately participate in bounded completion history.
    pub fn restore(&mut self, download: Download) -> bool {
        if self.indices.contains_key(&download.id) {
            return false;
        }
        if download.phase == Phase::Active {
            return false;
        }
        let id = download.id;
        let terminal = download.phase.is_terminal();
        let index = self.downloads.len();
        self.downloads.push(download);
        let previous = self.indices.insert(id, index);
        debug_assert!(previous.is_none(), "duplicate download id restored");
        if terminal {
            // Record restored terminal order and enforce the history bound immediately.
            self.completion_order.push(id);
            self.prune_history();
        }
        true
    }

    fn record_phase_change(&mut self, id: DownloadId, was_terminal: bool) {
        let is_terminal = self.get(id).is_some_and(|d| d.phase.is_terminal());
        match (was_terminal, is_terminal) {
            (false, true) => {
                self.completion_order.retain(|candidate| *candidate != id);
                self.completion_order.push(id);
            }
            (true, false) => {
                self.completion_order.retain(|candidate| *candidate != id);
            }
            _ => {}
        }
    }

    /// Applies an action to a tracked download and promotes queued work if a slot was released.
    ///
    /// Returns `false` for unknown ids or unavailable actions.
    pub fn apply(&mut self, id: DownloadId, action: DownloadAction) -> bool {
        let Some(was_terminal) = self.get(id).map(|d| d.phase.is_terminal()) else {
            return false;
        };
        if !self.get_mut(id).is_some_and(|d| d.apply(action)) {
            return false;
        }
        self.record_phase_change(id, was_terminal);
        self.prune_history();
        self.promote();
        true
    }

    /// Applies terminal `phase` to a tracked attempt, then prunes history and fills any freed slot.
    ///
    /// Unknown ids and refused download transitions return `false` without mutation.
    pub fn finish(&mut self, id: DownloadId, attempt: Attempt, phase: Phase) -> bool {
        let Some(was_terminal) = self.get(id).map(|d| d.phase.is_terminal()) else {
            return false;
        };
        if !self.get_mut(id).is_some_and(|d| d.finish(attempt, phase)) {
            return false;
        }
        self.record_phase_change(id, was_terminal);
        self.prune_history();
        self.promote();
        true
    }

    /// Fails a tracked attempt as nonretryable, recording completion, pruning history, and filling any freed slot.
    pub fn finish_blocked(&mut self, id: DownloadId, attempt: Attempt) -> bool {
        let Some(was_terminal) = self.get(id).map(|d| d.phase.is_terminal()) else {
            return false;
        };
        if !self.get_mut(id).is_some_and(|d| d.finish_blocked(attempt)) {
            return false;
        }
        self.record_phase_change(id, was_terminal);
        self.prune_history();
        self.promote();
        true
    }

    /// Applies one transfer-engine event to the queue.
    ///
    /// Returns whether any part of the event was accepted; unknown ids and stale attempts are ignored. A `Paused` event offers its final byte count through the normal progress gate before pausing queued or active work, and blocked failures become nonretryable.
    pub fn apply_event(&mut self, event: &DownloadEvent) -> bool {
        let id = event.id();
        let attempt = event.attempt();
        match event {
            DownloadEvent::ResponseMeta { meta, .. } => {
                // For 206 responses, complete `Content-Range` length is the resource total; `Content-Length` covers only this response body.
                let total = match meta.content_range.as_deref().map(parse_content_range) {
                    Some(Some(range)) => range.total,
                    Some(None) => None,
                    None => meta.content_length,
                };
                if total.is_none() {
                    return false;
                }
                self.set_total(id, attempt, total)
            }
            DownloadEvent::Progress { received, .. } => self.progress(id, attempt, *received),
            DownloadEvent::Finished { .. } => self.finish(id, attempt, Phase::Complete),
            // Preserve typed policy refusal so the model does not offer retry for a blocked destination.
            DownloadEvent::Failed {
                kind: FailureKind::Blocked,
                ..
            } => self.finish_blocked(id, attempt),
            DownloadEvent::Failed { .. } => self.finish(id, attempt, Phase::Failed),
            DownloadEvent::Paused { received, .. } => {
                // Gate the whole paused event first so a stale worker cannot update bytes or pause the current retry.
                if !self.get(id).is_some_and(|d| d.accepts(attempt)) {
                    return false;
                }
                let counted = self.progress(id, attempt, *received);
                let paused = self
                    .get(id)
                    .is_some_and(|d| d.phase == Phase::Paused && d.accepts(attempt))
                    || self.apply(id, DownloadAction::Pause);
                counted || paused
            }
            DownloadEvent::Cancelled { .. } => self.finish(id, attempt, Phase::Cancelled),
        }
    }

    /// Decides and applies a range response for a tracked download.
    ///
    /// Returns `None` without mutation for unknown, stale, terminal, or unapplied responses. Applied decisions trigger history pruning and promotion; restart decisions leave the download queued under a new attempt.
    pub fn handle_range_response(
        &mut self,
        id: DownloadId,
        attempt: Attempt,
        plan: &RangeRequestPlan,
        partial_coding: &ContentCoding,
        response: &RangeResponse<'_>,
    ) -> Option<RangeDecision> {
        let was_terminal = self.get(id)?.phase.is_terminal();
        let decision =
            self.get_mut(id)?
                .handle_range_response(attempt, plan, partial_coding, response)?;
        self.record_phase_change(id, was_terminal);
        self.prune_history();
        self.promote();
        Some(decision)
    }

    /// Evicts oldest-completed terminal downloads until the configured history limit is met.
    ///
    /// Non-terminal downloads are never evicted, zero keeps no terminal history, and reconciliation runs in linear time. Returns evicted ids oldest first.
    pub fn prune_history(&mut self) -> Vec<DownloadId> {
        // Runtime transitions normally keep completion order aligned; repair only when an O(n) consistency check finds divergence.
        self.reconcile_completion_order();

        let excess = self
            .completion_order
            .len()
            .saturating_sub(self.history_limit);
        if excess == 0 {
            return Vec::new();
        }

        let evicted: Vec<DownloadId> = self.completion_order.drain(..excess).collect();
        // Use set membership so retaining survivors remains O(downloads + evicted).
        let evicted_set: std::collections::HashSet<DownloadId> = evicted.iter().copied().collect();
        self.downloads
            .retain(|download| !evicted_set.contains(&download.id));
        self.rebuild_indices();
        evicted
    }

    // Reconcile completion order to exactly the terminal ids in O(downloads + history), preserving surviving order and appending missing ids.
    fn reconcile_completion_order(&mut self) {
        use std::collections::HashSet;

        let terminal: HashSet<DownloadId> = self
            .downloads
            .iter()
            .inspect(|_| prune_probe::touch())
            .filter(|d| d.phase.is_terminal())
            .map(|d| d.id)
            .collect();

        let mut aligned = self.completion_order.len() == terminal.len();
        if aligned {
            let mut seen: HashSet<DownloadId> = HashSet::with_capacity(self.completion_order.len());
            for id in &self.completion_order {
                prune_probe::touch();
                if !terminal.contains(id) || !seen.insert(*id) {
                    aligned = false;
                    break;
                }
            }
        }
        if aligned {
            return;
        }

        let mut kept: HashSet<DownloadId> = HashSet::with_capacity(self.completion_order.len());
        self.completion_order
            .retain(|id| terminal.contains(id) && kept.insert(*id));
        for download in self.downloads.iter().filter(|d| d.phase.is_terminal()) {
            if kept.insert(download.id) {
                self.completion_order.push(download.id);
            }
        }
    }

    /// Promotes queued downloads into free slots in admission order and returns their ids in promotion order.
    pub fn promote(&mut self) -> Vec<DownloadId> {
        let mut promoted = Vec::new();
        let mut active = self.active_count();
        // Scan front to back so the oldest queued download receives the next slot.
        for d in &mut self.downloads {
            if active >= self.max_active {
                break;
            }
            if d.phase == Phase::Queued {
                d.phase = Phase::Active;
                promoted.push(d.id);
                active += 1;
            }
        }
        promoted
    }
}

mod persistence;
pub use persistence::*;

mod resume;
pub use resume::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cloneable cooperative cancellation signal polled by blocking download workers between chunks.
#[derive(Debug, Clone)]
pub struct CancelSignal(Arc<AtomicBool>);

impl CancelSignal {
    /// A fresh, un-cancelled signal.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Request cancellation. Idempotent — calling twice is harmless.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    // Rearm only between completed transport runs; un-cancelling a worker still unwinding would violate cancellation.
    pub(crate) fn rearm(&self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Default for CancelSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Response metadata returned before a download body.
#[derive(Debug, Clone)]
pub struct TransportResponseMeta {
    /// HTTP status code (e.g. 200, 206, 416).
    pub status: u16,
    /// Raw `Content-Range` header value, if present.
    pub content_range: Option<String>,
    /// Parsed response validator (`ETag` or `Last-Modified`), if present.
    pub response_validator: Option<Validator>,
    /// `Content-Length` of the body (for a 206, this is the part length).
    pub content_length: Option<u64>,
    /// `Content-Encoding`, if present. `None` means identity.
    pub content_encoding: Option<String>,
}

/// The next body-stream result from a [`DownloadTransport`].
#[derive(Debug)]
pub enum BodyChunk {
    /// A chunk of bytes. May be empty (a zero-length chunk is not EOF).
    Bytes(Vec<u8>),
    /// End of the body stream.
    Done,
}

/// An injectable blocking transport for streamed, resumable downloads.
///
/// Implementations return metadata before body chunks and must honor cooperative cancellation.
pub trait DownloadTransport: std::fmt::Debug + Send + Sync + 'static {
    /// Starts a fresh or ranged request and returns metadata before the body.
    ///
    /// Network implementations must apply a safe-by-default [`UrlPolicy`](crate::url_policy::UrlPolicy) to the initial URL, resolved addresses, connected peer, and redirects. Policy refusal returns [`TransportError::Blocked`], and cancellation must be checked promptly.
    ///
    /// # Errors
    ///
    /// A [`TransportError`]: `Blocked` on a policy refusal, and others for network failure and
    /// cancellation.
    fn start(
        &self,
        url: &str,
        plan: Option<&RangeRequestPlan>,
        cancel: &CancelSignal,
    ) -> Result<TransportResponseMeta, TransportError>;

    /// Reads the next body chunk, returning [`BodyChunk::Done`] at end of stream.
    ///
    /// Implementations must check cancellation promptly.
    ///
    /// # Errors
    ///
    /// A [`TransportError`] for network failure and cancellation.
    fn read_chunk(&self, cancel: &CancelSignal) -> Result<BodyChunk, TransportError>;
}

/// Identifies which bounded transport phase timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportTimeout {
    /// Establishing the connection did not complete in time.
    Connect,
    /// The peer connected but did not complete its response headers in time.
    ResponseHeaders,
    /// The response body stopped making progress.
    BodyProgress,
    /// The request-wide deadline expired, even though individual phases may
    /// have continued to make progress.
    Total,
}

impl std::fmt::Display for TransportTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let phase = match self {
            Self::Connect => "connection",
            Self::ResponseHeaders => "response headers",
            Self::BodyProgress => "response body progress",
            Self::Total => "total transfer",
        };
        write!(f, "{phase} timed out")
    }
}

/// An error from a [`DownloadTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The transfer was cooperatively cancelled.
    Cancelled,
    /// A bounded transport deadline expired.
    Timeout(TransportTimeout),
    /// A network or I/O error.
    Io(String),
    /// Response headers exceeded the shared accumulation limit.
    HeaderLimit(String),
    /// No byte-moving backend is available.
    ///
    /// This typed error lets callers render the missing component and installation guidance separately from network failures.
    Unavailable {
        /// What is missing, named as the user would install it (`aria2c`).
        backend: String,
        /// What the user can do about it, rendered verbatim.
        hint: String,
    },
    /// The transport instance is already driving another transfer.
    ///
    /// Each instance owns one receiver, worker, and abort path; construct a fresh instance through [`DownloadTransportFactory`] for another transfer.
    AlreadyActive,
    /// The transport’s URL policy refused the destination.
    ///
    /// This remains distinct from I/O failure so security decisions are not treated as transient retries.
    Blocked {
        /// A short, payload-free reason tag (for example `cleartext-refused`
        /// or `address-blocked: Loopback`).
        reason: String,
    },
}

/// Typed retry classification for a failed download event.
///
/// Classification is produced while the transport error type exists and never inferred from localized or remote-influenced display text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// A potentially transient failure for which retry remains available.
    Transient,
    /// A destination refusal made by [`UrlPolicy`](crate::url_policy::UrlPolicy); retry is withdrawn and the classification is persisted.
    Blocked,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Cancelled => write!(f, "cancelled"),
            TransportError::Timeout(phase) => write!(f, "{phase}"),
            TransportError::Io(msg) => write!(f, "{msg}"),
            TransportError::HeaderLimit(msg) => {
                write!(f, "response header limit exceeded: {msg}")
            }
            TransportError::Unavailable { backend, hint } => {
                write!(f, "no download backend: {backend} is not available. {hint}")
            }
            TransportError::AlreadyActive => {
                write!(f, "download transport is already driving a transfer")
            }
            TransportError::Blocked { reason } => {
                write!(f, "Blocked destination: refused by URL policy ({reason})")
            }
        }
    }
}

/// Creates a fresh [`DownloadTransport`] for each spawned transfer.
pub trait DownloadTransportFactory: std::fmt::Debug + Send + Sync + 'static {
    /// Build a fresh transport for one spawned transfer.
    fn create(&self) -> Box<dyn DownloadTransport>;
}

/// An event emitted by [`run_download`] for the owner’s update loop.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// Response metadata arrived before the body.
    ResponseMeta {
        /// Which download.
        id: DownloadId,
        /// The attempt this response belongs to.
        attempt: Attempt,
        /// Response metadata supplied by the transport.
        meta: TransportResponseMeta,
    },
    /// Cumulative body progress arrived.
    Progress {
        /// Which download.
        id: DownloadId,
        /// The attempt this progress belongs to.
        attempt: Attempt,
        /// Cumulative bytes received so far.
        received: u64,
    },
    /// The download completed successfully.
    Finished {
        /// Which download.
        id: DownloadId,
        /// The attempt that completed.
        attempt: Attempt,
    },
    /// The download failed.
    Failed {
        /// Which download.
        id: DownloadId,
        /// The attempt that failed.
        attempt: Attempt,
        /// Human-readable error description. Display material only: the
        /// lifecycle classifies on `kind`, never on this text.
        error: String,
        /// Whether this failure is retryable or a policy refusal.
        kind: FailureKind,
    },
    /// The owner paused the transfer and retained its partial file for resume.
    Paused {
        /// Which download.
        id: DownloadId,
        /// The attempt that was suspended.
        attempt: Attempt,
        /// Bytes on disk at the moment the transfer stopped — the offset the
        /// resumed attempt will request.
        received: u64,
    },
    /// The transfer was abandoned and its partial file removed. Terminal.
    Cancelled {
        /// Which download.
        id: DownloadId,
        /// The attempt that was abandoned.
        attempt: Attempt,
    },
}

impl DownloadEvent {
    /// An ordinary, retryable failure event.
    #[must_use]
    pub fn failed(id: DownloadId, attempt: Attempt, error: impl Into<String>) -> Self {
        DownloadEvent::Failed {
            id,
            attempt,
            error: error.into(),
            kind: FailureKind::Transient,
        }
    }

    /// Creates a terminal policy-refusal event for which retry is unavailable.
    #[must_use]
    pub fn blocked(id: DownloadId, attempt: Attempt, error: impl Into<String>) -> Self {
        DownloadEvent::Failed {
            id,
            attempt,
            error: error.into(),
            kind: FailureKind::Blocked,
        }
    }

    /// Returns whether this event is a nonretryable URL-policy refusal.
    #[must_use]
    pub fn is_blocked_failure(&self) -> bool {
        matches!(
            self,
            DownloadEvent::Failed {
                kind: FailureKind::Blocked,
                ..
            }
        )
    }

    /// The attempt every event carries, so a caller can gate on staleness
    /// without matching each variant.
    #[must_use]
    pub fn attempt(&self) -> Attempt {
        match self {
            DownloadEvent::ResponseMeta { attempt, .. }
            | DownloadEvent::Progress { attempt, .. }
            | DownloadEvent::Finished { attempt, .. }
            | DownloadEvent::Failed { attempt, .. }
            | DownloadEvent::Paused { attempt, .. }
            | DownloadEvent::Cancelled { attempt, .. } => *attempt,
        }
    }

    /// Which download the event belongs to.
    #[must_use]
    pub fn id(&self) -> DownloadId {
        match self {
            DownloadEvent::ResponseMeta { id, .. }
            | DownloadEvent::Progress { id, .. }
            | DownloadEvent::Finished { id, .. }
            | DownloadEvent::Failed { id, .. }
            | DownloadEvent::Paused { id, .. }
            | DownloadEvent::Cancelled { id, .. } => *id,
        }
    }
}

/// Runs one download on the current thread and emits typed lifecycle events.
///
/// Intended for a background thread, it checks cancellation between transport steps and stamps every event with `attempt` so reducers can reject stale work.
pub fn run_download(
    transport: &dyn DownloadTransport,
    url: &str,
    id: DownloadId,
    attempt: Attempt,
    plan: Option<&RangeRequestPlan>,
    cancel: &CancelSignal,
    mut emit: impl FnMut(DownloadEvent),
) {
    let meta = match transport.start(url, plan, cancel) {
        Ok(m) => m,
        Err(TransportError::Cancelled) => {
            emit(DownloadEvent::failed(id, attempt, "cancelled"));
            return;
        }
        // Preserve blocked classification when converting transport errors into events.
        Err(other) => {
            emit(failure_event(id, attempt, &other));
            return;
        }
    };

    emit(DownloadEvent::ResponseMeta { id, attempt, meta });

    if cancel.is_cancelled() {
        emit(DownloadEvent::failed(id, attempt, "cancelled"));
        return;
    }

    let mut received: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            emit(DownloadEvent::failed(id, attempt, "cancelled"));
            return;
        }

        match transport.read_chunk(cancel) {
            Ok(BodyChunk::Bytes(bytes)) => {
                received = received.saturating_add(bytes.len() as u64);
                emit(DownloadEvent::Progress {
                    id,
                    attempt,
                    received,
                });
            }
            Ok(BodyChunk::Done) => {
                emit(DownloadEvent::Finished { id, attempt });
                return;
            }
            Err(TransportError::Cancelled) => {
                emit(DownloadEvent::failed(id, attempt, "cancelled"));
                return;
            }
            Err(other) => {
                emit(failure_event(id, attempt, &other));
                return;
            }
        }
    }
}

fn failure_event(id: DownloadId, attempt: Attempt, error: &TransportError) -> DownloadEvent {
    let text = error.to_string();
    if matches!(error, TransportError::Blocked { .. }) {
        DownloadEvent::blocked(id, attempt, text)
    } else {
        DownloadEvent::failed(id, attempt, text)
    }
}

pub mod transfer;

#[cfg(test)]
mod tests;
