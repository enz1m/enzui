//! Commit reports: an app telling the broker that a call's side effect
//! committed, on the version-3 connection that dispatched the call.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use crate::center::protocol::{CommitReportDisposition, Invocation, Uuid};

// The broker's own commit-report bounds: it tracks at most 1024 side-effect
// invocations, and accepts 32 report frames a second with a burst of 64.
pub(super) const MAX_REPORTABLE: usize = 1024;
const REPORTS_PER_SECOND: f64 = 32.0;
const REPORT_BURST: f64 = 64.0;

/// What became of a queued commit report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitReportOutcome {
    /// The broker's acknowledgement. Only a durable disposition means the
    /// report was stored; none means the original caller received anything.
    Broker(CommitReportDisposition),
    /// The connection that dispatched the call closed before the broker
    /// answered. The report cannot move to another connection.
    ConnectionClosed,
}

/// Why [`super::CentralConnector::report_commit`] queued nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitReportError {
    /// The connected broker did not negotiate commit reports (a version-2
    /// broker), or no connection is registered.
    Unavailable,
    /// No side-effect call with this id was dispatched to this connector:
    /// a read-only call, an unknown id, or one pruned from the bounded record.
    NotEligible,
    /// The connection that dispatched the call has closed.
    ConnectionClosed,
    /// A report for this call is still waiting for the broker's answer.
    AwaitingAnswer,
    /// Reports are arriving faster than the broker accepts them.
    RateLimited,
    /// The outbound queue is full.
    QueueFull,
    /// The connector has stopped.
    Stopped,
}

impl core::fmt::Display for CommitReportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "the broker does not accept commit reports",
            Self::NotEligible => "no side-effect call with this id was dispatched here",
            Self::ConnectionClosed => "the call's connection has closed",
            Self::AwaitingAnswer => "a report for this call awaits the broker's answer",
            Self::RateLimited => "commit reports are arriving too fast",
            Self::QueueFull => "outbound event queue is full",
            Self::Stopped => "connector has stopped",
        })
    }
}

impl std::error::Error for CommitReportError {}

// Which calls may be reported as committed, and which reports await an answer.
//
// A report binds to the connection that dispatched its call: the broker refuses
// it on any other, so the ledger keeps each call's generation and only the
// current version-3 session may send.
#[derive(Default)]
pub(super) struct CommitLedger {
    // Functions the registration declares with a side effect.
    pub(super) side_effects: HashSet<String>,
    // The generation of the session that negotiated commit reports, if any.
    pub(super) active: Option<u64>,
    // Side-effect invocations by id, with the generation that dispatched them,
    // oldest first in `order`.
    pub(super) dispatched: HashMap<String, u64>,
    pub(super) order: VecDeque<String>,
    // Reports queued and not yet answered, with their generation.
    pub(super) pending: HashMap<String, (u64, Uuid)>,
    pub(super) tokens: f64,
    pub(super) refilled: Option<Instant>,
}

impl CommitLedger {
    pub(super) fn record(&mut self, invocation: &Invocation, generation: u64) {
        if !self.side_effects.contains(&invocation.function_id) {
            return;
        }
        let id = invocation.invocation_id.as_str().to_owned();
        if self.dispatched.insert(id.clone(), generation).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > MAX_REPORTABLE {
            if let Some(oldest) = self.order.pop_front() {
                self.dispatched.remove(&oldest);
            }
        }
    }

    pub(super) fn begin(&mut self, generation: u64, reports: bool) {
        self.active = reports.then_some(generation);
    }

    // Closes `generation` and returns the reports it can no longer answer.
    pub(super) fn end(&mut self, generation: u64) -> Vec<Uuid> {
        if self.active == Some(generation) {
            self.active = None;
        }
        let mut closed = Vec::new();
        self.pending.retain(|_, (owner, id)| {
            if *owner == generation {
                closed.push(id.clone());
                false
            } else {
                true
            }
        });
        closed
    }

    pub(super) fn queue(
        &mut self,
        invocation_id: &Uuid,
        now: Instant,
    ) -> Result<u64, CommitReportError> {
        let active = self.active.ok_or(CommitReportError::Unavailable)?;
        let generation = *self
            .dispatched
            .get(invocation_id.as_str())
            .ok_or(CommitReportError::NotEligible)?;
        if generation != active {
            return Err(CommitReportError::ConnectionClosed);
        }
        if self.pending.contains_key(invocation_id.as_str()) {
            return Err(CommitReportError::AwaitingAnswer);
        }
        let elapsed = self.refilled.map_or(REPORT_BURST, |at| {
            now.saturating_duration_since(at).as_secs_f64() * REPORTS_PER_SECOND
        });
        self.tokens = (self.tokens + elapsed).min(REPORT_BURST);
        self.refilled = Some(now);
        if self.tokens < 1.0 {
            return Err(CommitReportError::RateLimited);
        }
        self.tokens -= 1.0;
        self.pending.insert(
            invocation_id.as_str().to_owned(),
            (generation, invocation_id.clone()),
        );
        Ok(generation)
    }

    pub(super) fn withdraw(&mut self, invocation_id: &str) {
        self.pending.remove(invocation_id);
    }

    // `true` when the answer belongs to a report this generation queued.
    pub(super) fn answer(&mut self, invocation_id: &str, generation: u64) -> bool {
        if self
            .pending
            .get(invocation_id)
            .is_some_and(|(owner, _)| *owner == generation)
        {
            self.pending.remove(invocation_id);
            true
        } else {
            false
        }
    }
}
