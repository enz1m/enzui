// Shared lifecycle for pending portal operations.
// Terminal state is first-writer-wins, wire Close is claimed once, and cancellation is rechecked under the transition lock.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::portal::CancelReason;
use crate::portal::cancel::PortalCancel;
use crate::portal::completion::CompletionSignal;

// State for one exact backend request identity.
#[derive(Debug)]
enum State<Id> {
    // No wire request exists; a deferred start may still be claimed.
    NoWire,
    // A start call exclusively owns the transition.
    Claiming,
    Live(Id),
    // A backend interaction owns the request; settlement must back off.
    Busy(Id),
    // Terminal, retaining `Some(id)` only when a wire request existed.
    Finished(Option<Id>),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CloseClaim<Id> {
    // Caller must close this exact wire id with the recorded reason.
    Wire(Id, CancelReason),
    // Seal the lifecycle without a backend call because no wire request existed.
    NoWire(CancelReason),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClaimStart {
    // Caller must publish exactly one started, deferred, or terminal outcome.
    Claimed,
    AlreadyLive,
    Locked,
    Finished,
    // Cancellation won under the lifecycle lock before any wire request was published.
    CancelledBeforeStart,
}

// Shares terminal state, lifecycle transitions, request identity, and the close obligation across clones.
#[derive(Debug, Clone)]
pub(super) struct PendingLifecycle<Id> {
    state: Arc<Mutex<State<Id>>>,
    cancel: PortalCancel,
}

impl<Id: Copy + PartialEq + std::fmt::Debug> PendingLifecycle<Id> {
    pub(super) fn live(id: Id) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Live(id))),
            cancel: PortalCancel::new(),
        }
    }

    pub(super) fn no_wire() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::NoWire)),
            cancel: PortalCancel::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<Id>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn cancel(&self) -> &PortalCancel {
        &self.cancel
    }

    pub(super) fn cancel_handle(&self) -> PortalCancel {
        self.cancel.clone()
    }

    pub(super) fn is_pending(&self) -> bool {
        !self.cancel.is_terminal()
    }

    pub(super) fn outcome_reason(&self) -> Option<CancelReason> {
        self.cancel.outcome_reason()
    }

    pub(super) fn wire_id(&self) -> Option<Id> {
        match *self.lock() {
            State::Live(id) | State::Busy(id) | State::Finished(Some(id)) => Some(id),
            State::NoWire | State::Claiming | State::Finished(None) => None,
        }
    }

    // Finished means the lifecycle and any Close obligation are sealed, not merely that a terminal reason was recorded.
    pub(super) fn is_finished(&self) -> bool {
        matches!(*self.lock(), State::Finished(_))
    }

    pub(super) fn record_cancel(&self, reason: CancelReason) -> bool {
        self.cancel.cancel(reason)
    }

    // Shared across clones, so only one of them delivers the terminal outcome.
    pub(super) fn claim_report(&self) -> bool {
        self.cancel.claim_report()
    }

    pub(super) fn attach_wake(&self, signal: CompletionSignal) {
        self.cancel.attach_wake(signal);
    }

    // Recheck terminal cancellation under the state lock before changing `NoWire` to `Claiming`.
    pub(super) fn try_claim_start(&self) -> ClaimStart {
        let mut guard = self.lock();
        match *guard {
            State::NoWire => {
                if self.cancel.is_terminal() {
                    ClaimStart::CancelledBeforeStart
                } else {
                    *guard = State::Claiming;
                    ClaimStart::Claimed
                }
            }
            State::Live(_) => ClaimStart::AlreadyLive,
            State::Claiming | State::Busy(_) => ClaimStart::Locked,
            State::Finished(_) => ClaimStart::Finished,
        }
    }

    pub(super) fn publish_started(&self, id: Id) {
        let mut guard = self.lock();
        debug_assert!(
            matches!(*guard, State::Claiming),
            "publish_started expects Claiming, saw {:?}",
            *guard
        );
        *guard = State::Live(id);
    }

    pub(super) fn publish_deferred(&self) {
        let mut guard = self.lock();
        debug_assert!(
            matches!(*guard, State::Claiming),
            "publish_deferred expects Claiming, saw {:?}",
            *guard
        );
        *guard = State::NoWire;
    }

    pub(super) fn publish_terminal_no_wire(&self) {
        let mut guard = self.lock();
        debug_assert!(
            matches!(*guard, State::Claiming),
            "publish_terminal_no_wire expects Claiming, saw {:?}",
            *guard
        );
        *guard = State::Finished(None);
    }

    // Claim `Live` as `Busy` only while holding the lock and after rechecking cancellation.
    pub(super) fn try_claim_poll(&self) -> Option<Id> {
        let mut guard = self.lock();
        if self.cancel.is_terminal() {
            return None;
        }
        match *guard {
            State::Live(id) => {
                *guard = State::Busy(id);
                Some(id)
            }
            State::NoWire | State::Claiming | State::Busy(_) | State::Finished(_) => None,
        }
    }

    // Return `Busy` to `Live` after an empty or retryable backend result.
    pub(super) fn release_poll(&self) {
        let mut guard = self.lock();
        match &*guard {
            State::Busy(id) => *guard = State::Live(*id),
            other => debug_assert!(false, "release_poll out of order: {other:?}"),
        }
    }

    // Seal the lifecycle before selecting the terminal writer so settlement cannot follow a consumed reply.
    pub(super) fn commit_terminal_reply(&self) -> Option<Id> {
        let mut guard = self.lock();
        match &*guard {
            State::Busy(id) => {
                let id = *id;
                *guard = State::Finished(Some(id));
                Some(id)
            }
            other => {
                debug_assert!(false, "commit_terminal_reply out of order: {other:?}");
                None
            }
        }
    }

    // Atomically consume the one-shot Close obligation and seal the lifecycle; refuse while another transition owns it or after finish.
    pub(super) fn claim_close(&self) -> Option<CloseClaim<Id>> {
        let mut guard = self.lock();
        match *guard {
            State::Claiming | State::Busy(_) | State::Finished(_) => None,
            State::Live(id) => {
                let reason = self.cancel.take_close_request()?;
                *guard = State::Finished(Some(id));
                Some(CloseClaim::Wire(id, reason))
            }
            State::NoWire => {
                let reason = self.cancel.take_close_request()?;
                *guard = State::Finished(None);
                Some(CloseClaim::NoWire(reason))
            }
        }
    }

    #[cfg(test)]
    pub(super) fn close_taken(&self) -> bool {
        self.cancel.close_taken()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::CancelReason;

    const _SEND_SYNC: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PendingLifecycle<u32>>();
        assert_send_sync::<CloseClaim<u32>>();
    };

    #[test]
    fn exactly_once_close_seals_atomically() {
        let l: PendingLifecycle<u32> = PendingLifecycle::live(3);
        assert!(l.record_cancel(CancelReason::User));

        let first = l.claim_close();
        assert_eq!(first, Some(CloseClaim::Wire(3, CancelReason::User)));
        assert!(l.is_finished(), "seal must be atomic with the close claim");
        assert!(l.close_taken());

        assert_eq!(l.claim_close(), None);
        assert_eq!(l.try_claim_poll(), None);
    }
}
