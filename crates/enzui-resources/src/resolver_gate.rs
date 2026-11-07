//! The process-wide ceiling on blocking name-resolver work.
//!
//! `getaddrinfo` cannot be interrupted. Every transport in this crate
//! therefore runs it on a helper thread and waits on a channel, so a stalled
//! or hostile resolver ends the *wait* without ending the *work*. That alone
//! is not bounded: a caller that gives up leaves its helper parked inside
//! libc, and a request/cancel loop then creates one abandoned stack per
//! attempt.
//!
//! This gate is the bound. Two properties make it one:
//!
//! * Process-wide. Each transport instance (and each `ImageService`) owns
//!   its own workers, so a per-instance limit still grows without end across
//!   instances. There is one gate, shared by the image transport and the
//!   streaming download transport, because the resource being rationed —
//!   threads parked in the system resolver — is a property of the process.
//! * The permit belongs to the work, not to the waiter. A caller that
//!   cancels or times out returns immediately and releases *nothing*: the
//!   helper drops the permit only after the real lookup returns. Releasing on
//!   the caller's path would let N cancellations admit N new lookups while the
//!   old N are still inside libc — precisely the unbounded growth the gate
//!   exists to prevent.
//!
//! Callers differ in how they express "stop waiting" (typed cancellation
//! causes, deadlines, a single total budget), so [`ResolverGate::acquire`]
//! takes a poll closure returning either how long it may block or the
//! caller's own error. The gate owns the counting; the caller owns the
//! vocabulary.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Process-wide ceiling for blocking libc resolver calls.
pub(crate) const RESOLVER_THREAD_LIMIT: usize = 4;

#[derive(Debug)]
pub(crate) struct ResolverGateState {
    active: usize,
    waiting: usize,
}

/// A counting gate whose permits outlive the callers that requested them.
#[derive(Clone, Debug)]
pub(crate) struct ResolverGate {
    shared: Arc<(Mutex<ResolverGateState>, Condvar)>,
    limit: usize,
}

impl ResolverGate {
    /// A gate admitting at most `limit` concurrent lookup bodies.
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "resolver concurrency limit must be non-zero");
        Self {
            shared: Arc::new((
                Mutex::new(ResolverGateState {
                    active: 0,
                    waiting: 0,
                }),
                Condvar::new(),
            )),
            limit,
        }
    }

    /// Wait for capacity, re-consulting `poll` between waits.
    ///
    /// `poll` returns the longest this call may block before it must be asked
    /// again, or the caller's error when the caller has stopped waiting
    /// (cancellation, an expired deadline or budget). A refusal here has
    /// released nothing and started nothing, so it cannot disturb the
    /// ceiling.
    pub(crate) fn acquire<E>(
        &self,
        mut poll: impl FnMut() -> Result<Duration, E>,
    ) -> Result<ResolverPermit, E> {
        let (lock, available) = &*self.shared;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut counted_waiter = false;
        loop {
            let wait = match poll() {
                Ok(wait) => wait,
                Err(error) => {
                    if counted_waiter {
                        state.waiting -= 1;
                    }
                    return Err(error);
                }
            };
            if state.active < self.limit {
                if counted_waiter {
                    state.waiting -= 1;
                }
                state.active += 1;
                return Ok(ResolverPermit { gate: self.clone() });
            }
            if !counted_waiter {
                state.waiting += 1;
                counted_waiter = true;
            }
            let (next, _) = available
                .wait_timeout(state, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
    }

    /// `(active, waiting)` — occupied slots and callers queued for one.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let state = self
            .shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.active, state.waiting)
    }
}

/// One occupied slot. Held by the thread running the real lookup; dropping it
/// is what admits the next caller.
#[derive(Debug)]
pub(crate) struct ResolverPermit {
    gate: ResolverGate,
}

impl Drop for ResolverPermit {
    fn drop(&mut self) {
        let (lock, available) = &*self.gate.shared;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        // Exactly one waiter is woken per released slot: the slot admits one
        // lookup, so waking the rest would only make them re-queue.
        available.notify_one();
    }
}

/// The one gate every blocking resolver call in this crate passes through.
pub(crate) fn shared_resolver_gate() -> &'static ResolverGate {
    static GATE: std::sync::OnceLock<ResolverGate> = std::sync::OnceLock::new();
    GATE.get_or_init(|| ResolverGate::new(RESOLVER_THREAD_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poisoning the bookkeeping mutex cannot disable admission or make permit
    /// cleanup panic while another panic is unwinding.
    ///
    /// SABOTAGE: restore either lock `.unwrap()` in `acquire` or `Drop`; the
    /// corresponding acquire/drop below panics on the calibrated poison.
    #[test]
    fn poisoned_gate_still_admits_and_reclaims_its_permit() {
        let gate = ResolverGate::new(1);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = gate.shared.0.lock().expect("fresh gate lock");
            panic!("deliberately poison resolver gate");
        }));
        assert!(poisoned.is_err(), "poison witness must panic");
        assert!(
            gate.shared.0.is_poisoned(),
            "real gate lock was not poisoned"
        );

        let permit = gate
            .acquire(|| Ok::<Duration, ()>(Duration::from_millis(1)))
            .expect("poison recovery must still admit");
        assert_eq!(gate.counts(), (1, 0));
        drop(permit);
        assert_eq!(gate.counts(), (0, 0), "poisoned Drop must reclaim capacity");
    }
}
