//! Cross-thread cancellation for portal requests.
//!
//! Cancellation is first-writer-wins, and exactly one async-side caller may
//! take the resulting `Request::close()` obligation. A second one-shot claim
//! delivers the terminal outcome to exactly one consumer, independently of the
//! close. Completion is terminal but never creates a close request. No timeout
//! is imposed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::portal::CancelReason;

/// First-writer-wins terminal state for a portal operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortalTerminal {
    /// The operation was cancelled, by the route recorded.
    Cancelled(CancelReason),
    /// The operation completed normally and has no cancellation reason.
    Completed,
}

impl PortalTerminal {
    /// Returns the cancellation route, or `None` for normal completion.
    #[must_use]
    pub fn cancel_reason(self) -> Option<CancelReason> {
        match self {
            Self::Cancelled(r) => Some(r),
            Self::Completed => None,
        }
    }

    /// Returns whether this terminal state is a cancellation.
    #[must_use]
    pub fn is_cancelled(self) -> bool {
        matches!(self, Self::Cancelled(_))
    }
}

// The mutex makes the multiword terminal transition atomic so the first writer wins.
#[derive(Debug, Default)]
struct Shared {
    terminal: Mutex<Option<PortalTerminal>>,
    close_taken: AtomicBool,
    // Independent of `close_taken`: a completed request owes no close but still
    // has exactly one terminal outcome to deliver.
    reported: AtomicBool,
    // An attached completion signal wakes the owner when cross-thread cancellation
    // creates a close obligation.
    wake: Mutex<Option<crate::portal::completion::CompletionSignal>>,
}

/// A cloneable, thread-safe portal cancellation handle.
///
/// The first cancellation or completion wins. A cancellation yields at most one
/// close request; normal completion yields none.
#[derive(Debug, Clone)]
pub struct PortalCancel {
    shared: Arc<Shared>,
}

impl PortalCancel {
    /// Creates a live handle.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
        }
    }

    /// Attempts to cancel with `reason`.
    ///
    /// Returns `true` only when this call wins the terminal transition; an earlier cancellation or completion leaves the state unchanged.
    pub fn cancel(&self, reason: CancelReason) -> bool {
        let won = self.set_terminal(PortalTerminal::Cancelled(reason));
        if won {
            // Notify only the cancellation that wins; absorbed cancellations create no pump event.
            self.notify_wake();
        }
        won
    }

    /// Records normal completion.
    ///
    /// Returns `true` when this call wins the terminal transition; completion never creates a close request.
    pub fn complete(&self) -> bool {
        self.set_terminal(PortalTerminal::Completed)
    }

    /// Attaches the completion signal notified by a winning cancellation.
    ///
    /// The latest attachment replaces any previous signal; no attachment leaves
    /// cancellation unwoken for manually pumped backends.
    pub fn attach_wake(&self, signal: crate::portal::completion::CompletionSignal) {
        let mut guard = match self.shared.wake.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = Some(signal);
    }

    /// Returns whether a completion signal is attached.
    #[must_use]
    pub fn has_wake(&self) -> bool {
        match self.shared.wake.lock() {
            Ok(g) => g.is_some(),
            Err(poisoned) => poisoned.into_inner().is_some(),
        }
    }

    // Clone the signal before waking so re-entrant polling cannot deadlock on the terminal lock.
    fn notify_wake(&self) {
        let signal = {
            match self.shared.wake.lock() {
                Ok(g) => g.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        };
        if let Some(signal) = signal {
            signal.notify();
        }
    }

    fn set_terminal(&self, t: PortalTerminal) -> bool {
        // Recover a poisoned transition lock so cancellation can still release the remote request.
        let mut guard = match self.shared.terminal.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            return false;
        }
        *guard = Some(t);
        true
    }

    /// The terminal state, or `None` while the operation is still live.
    #[must_use]
    pub fn terminal(&self) -> Option<PortalTerminal> {
        match self.shared.terminal.lock() {
            Ok(g) => *g,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Whether the operation has reached a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.terminal().is_some()
    }

    /// Returns the cancellation route, or `None` while live or normally completed.
    #[must_use]
    pub fn outcome_reason(&self) -> Option<CancelReason> {
        self.terminal().and_then(PortalTerminal::cancel_reason)
    }

    /// Takes the `Request::close()` obligation at most once.
    ///
    /// Returns the winning cancellation reason once; live and completed operations
    /// never yield a close request.
    pub fn take_close_request(&self) -> Option<CancelReason> {
        let reason = self.outcome_reason()?;
        if self.shared.close_taken.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(reason)
    }

    /// Returns whether the one-shot close obligation has been taken.
    #[must_use]
    pub fn close_taken(&self) -> bool {
        self.shared.close_taken.load(Ordering::SeqCst)
    }

    /// Takes the one-shot right to deliver this operation's terminal outcome.
    ///
    /// Returns `true` for the first caller after a terminal state exists, and
    /// `false` while the operation is live or once another holder has taken it.
    /// This is independent of [`Self::take_close_request`]: a normally completed
    /// operation owes no close but still has one outcome to deliver.
    pub fn claim_report(&self) -> bool {
        if !self.is_terminal() {
            return false;
        }
        !self.shared.reported.swap(true, Ordering::SeqCst)
    }

    /// Returns whether the one-shot terminal report has been claimed.
    #[must_use]
    pub fn report_claimed(&self) -> bool {
        self.shared.reported.load(Ordering::SeqCst)
    }
}

impl Default for PortalCancel {
    fn default() -> Self {
        Self::new()
    }
}
