//! Event-driven portal completion notifications.
//!
//! [`CompletionSignal`] is a cloneable cross-thread generation counter. A
//! notification is retained even when no waiter is registered, concurrent
//! waiters have independent slots, and dropping a wait removes its slot.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

/// Marks a portal event and carries the generation observed by the waiter.
///
/// An unchanged generation can be used to distinguish a subscription priming wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortalWake {
    /// The signal generation observed at this wake.
    pub generation: u64,
}

#[derive(Default)]
struct SignalState {
    generation: u64,
    wakers: HashMap<u64, Waker>,
    next_waiter: u64,
}

struct Inner {
    state: Mutex<SignalState>,
}

fn lock(inner: &Inner) -> MutexGuard<'_, SignalState> {
    // Recover a poisoned state lock so later portal notifications and waits remain usable.
    inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A cloneable completion signal shared by portal producers and waiters.
#[derive(Clone)]
pub struct CompletionSignal {
    inner: Arc<Inner>,
}

impl Default for CompletionSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionSignal {
    /// A fresh signal at generation `0` with no waiters.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(SignalState::default()),
            }),
        }
    }

    /// Records one portal-side event and wakes every live waiter.
    ///
    /// Returns the new generation and may be called from any thread.
    pub fn notify(&self) -> u64 {
        let wakers = {
            let mut state = lock(&self.inner);
            state.generation += 1;
            // Drain rather than iterate: a waker is single-use, and the waiter
            // re-registers on its next poll.
            state.wakers.drain().map(|(_, w)| w).collect::<Vec<_>>()
        };
        // Wake outside the lock so a waker that polls re-entrantly cannot
        // deadlock against `notify`.
        for waker in wakers {
            waker.wake();
        }
        self.generation()
    }

    /// The current generation: how many portal-side events this signal has
    /// recorded.
    #[must_use]
    pub fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    /// Returns the number of currently registered waiters, including zero after
    /// their futures are dropped.
    #[must_use]
    pub fn waiter_count(&self) -> usize {
        lock(&self.inner).wakers.len()
    }

    /// Returns a future that is immediately ready if the generation already exceeds `seen`, or waits until it does.
    #[must_use]
    pub fn wait_after(&self, seen: u64) -> CompletionWait {
        CompletionWait {
            signal: self.clone(),
            seen,
            waiter: None,
        }
    }

    /// Returns a future for the first event after the generation snapshot taken by this call.
    #[must_use]
    pub fn wait(&self) -> CompletionWait {
        self.wait_after(self.generation())
    }

    /// Whether two handles name the same underlying signal.
    #[must_use]
    pub fn same_signal(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }
}

impl std::fmt::Debug for CompletionSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.inner);
        f.debug_struct("CompletionSignal")
            .field("generation", &state.generation)
            .field("waiters", &state.wakers.len())
            .finish()
    }
}

/// Future resolving to the first observed generation after its baseline.
///
/// Dropping it deregisters its waker.
pub struct CompletionWait {
    signal: CompletionSignal,
    seen: u64,
    waiter: Option<u64>,
}

impl Future for CompletionWait {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        let this = self.get_mut();
        let mut state = lock(&this.signal.inner);
        if state.generation > this.seen {
            let generation = state.generation;
            if let Some(id) = this.waiter.take() {
                state.wakers.remove(&id);
            }
            return Poll::Ready(generation);
        }
        let id = *this.waiter.get_or_insert_with(|| {
            state.next_waiter += 1;
            state.next_waiter
        });
        state.wakers.insert(id, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for CompletionWait {
    fn drop(&mut self) {
        if let Some(id) = self.waiter.take() {
            lock(&self.signal.inner).wakers.remove(&id);
        }
    }
}

impl std::fmt::Debug for CompletionWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionWait")
            .field("seen", &self.seen)
            .field("registered", &self.waiter.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_waker() -> Waker {
        Waker::noop().clone()
    }

    #[test]
    fn dropping_a_waiter_removes_it_from_the_signal() {
        let s = CompletionSignal::new();
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        {
            let mut fut = Box::pin(s.wait());
            assert!(fut.as_mut().poll(&mut cx).is_pending());
            assert_eq!(s.waiter_count(), 1);
        }
        assert_eq!(
            s.waiter_count(),
            0,
            "a dropped waiter must not stay registered"
        );
        assert_eq!(s.notify(), 1);
        assert_eq!(s.waiter_count(), 0);
    }
}
