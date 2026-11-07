//! One cancellation primitive, one terminal state.
//!
//! # Why this exists, and why it is a single object
//!
//! An agent-native function dispatch can be ended by four independent causes
//! that can race each other:
//!
//! | Cause | Who raises it |
//! |---|---|
//! | [`CancelCause::Deadline`] | a local deadline elapses |
//! | [`CancelCause::Cancelled`] | the caller (or UI) explicitly cancels |
//! | [`CancelCause::Disconnected`] | the broker peer drops the connection |
//! | [`CancelCause::Shutdown`] | the application is shutting down |
//!
//! One token is created before dispatch and passed as that exact object to
//! the function or task; deadline, cancel, disconnect and shutdown converge
//! on one terminal state.
//!
//! The failure this prevents is two tokens that both believe they own one
//! operation. That happens when a token is *reconstructed from an id* on the
//! far side of a dispatch: the dispatcher makes one, the task rebuilds another
//! from the invocation id, and now a `cancel` on one is invisible to the other.
//! So there is no constructor from an id here — the only way to observe a
//! token's terminal state is to hold the token the dispatcher created and
//! [`share`](CancelToken::share)d. Reconstruction is not expressible.
//!
//! # The "exactly one" invariant, precisely
//!
//! [`CancelToken::cancel`] is the single writer. The first call to reach a
//! live token wins: it records the winning [`CancelCause`], moves the token to
//! its terminal state, and returns `true`. Every later call — *of any cause* —
//! is absorbed: it changes nothing and returns `false`. The recorded cause is
//! therefore always the one that *won the race*, never the one that arrived
//! second. "Terminal" is reached exactly once, and the cause that reached it is
//! observable forever after.
//!
//! # No clock or I/O
//!
//! Deadlines here are expressed against an injected tick counter, never
//! `Instant::now`. The host advances a monotone `u64` (from its own frame
//! clock, a subscription tick, whatever) and asks the token whether the
//! deadline has passed via [`CancelToken::poll_deadline`]. Nothing in this
//! module reads real time or touches I/O. The shared terminal cell is atomic,
//! so an owner may move a shared handle to a bounded blocking worker while the
//! UI retains the original cancellation handle.
//!
//! # The seam a real dispatcher uses
//!
//! ```text
//! let token = CancelToken::with_deadline(deadline_tick);   // BEFORE dispatch
//! let handle = token.share();                              // hand THIS to the task
//! spawn(move || run(handle));                              // task holds the same state
//! // meanwhile, on the UI / broker side:
//! token.cancel(CancelCause::Cancelled);   // user pressed cancel
//! token.cancel(CancelCause::Disconnected);// broker dropped — absorbed if lost the race
//! for tick in frame_clock { token.poll_deadline(tick); }   // deadline arm
//! ```
//!
//! The dispatcher creates the token, shares it once, and every cause writes
//! through the *same* `Shared` state. The task learns it is done by observing
//! [`CancelToken::cause`] / [`CancelToken::is_terminal`] on its own handle. No
//! `Task`, socket, thread, or widget lives here — those are other slices.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const LIVE: u8 = 0;

const fn encode(cause: CancelCause) -> u8 {
    match cause {
        CancelCause::Deadline => 1,
        CancelCause::Cancelled => 2,
        CancelCause::Disconnected => 3,
        CancelCause::Shutdown => 4,
    }
}

const fn decode(value: u8) -> Option<CancelCause> {
    match value {
        1 => Some(CancelCause::Deadline),
        2 => Some(CancelCause::Cancelled),
        3 => Some(CancelCause::Disconnected),
        4 => Some(CancelCause::Shutdown),
        _ => None,
    }
}

/// Which cause ended an operation.
///
/// The variants are deliberately ordered `Deadline, Cancelled, Disconnected,
/// Shutdown`, but the terminal state records the cause that *won the race*, not
/// the lowest-numbered variant. Tests must fire causes out of declaration order
/// to keep "first wins" from silently coinciding with "lowest variant wins".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CancelCause {
    /// A local deadline elapsed.
    Deadline,
    /// The caller (or UI) explicitly cancelled.
    Cancelled,
    /// The broker peer dropped the connection.
    Disconnected,
    /// The application is shutting down.
    Shutdown,
}

/// The shared, single-writer terminal state behind every clone of a token.
///
/// One atomic byte is the whole state machine: zero is live and the four
/// non-zero values encode the terminal causes. It lives behind an [`Arc`] so
/// the dispatcher and a bounded worker observe the *same* transition; there is
/// no `Clone` that produces an independent state, which makes "two tokens that
/// both think they own it" unrepresentable.
#[derive(Debug, Default)]
struct Shared {
    cause: AtomicU8,
}

/// A cancellation token: one object, created before dispatch, shared into the
/// task, and written by whichever of the four causes arrives first.
///
/// Clones (via [`share`](CancelToken::share)) all point at the same terminal
/// state. There is intentionally no way to build a token from an id — see
/// the module docs for why reconstruction is the bug this prevents.
#[derive(Debug, Clone)]
pub struct CancelToken {
    shared: Arc<Shared>,
    /// Deadline expressed as a tick on the host's injected clock. `None` means
    /// "no deadline"; [`poll_deadline`](CancelToken::poll_deadline) is then a
    /// no-op that always reports "not expired".
    deadline: Option<u64>,
}

impl CancelToken {
    /// Creates a live token with no deadline.
    ///
    /// The operation can still be ended by [`cancel`](CancelToken::cancel) with
    /// any cause; it simply never expires on its own.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            deadline: None,
        }
    }

    /// Creates a live token that will expire once the host's clock reaches
    /// `deadline_tick` (see [`poll_deadline`](CancelToken::poll_deadline)).
    #[must_use]
    pub fn with_deadline(deadline_tick: u64) -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            deadline: Some(deadline_tick),
        }
    }

    /// Returns a clone that shares this token's terminal state.
    ///
    /// This is what the dispatcher hands to the task. A `cancel` through *any*
    /// clone is visible through *every* clone, because they hold one [`Arc`] to
    /// one `Shared`. This is the *only* sanctioned way to obtain a second
    /// handle — there is no reconstruct-from-id path.
    #[must_use]
    pub fn share(&self) -> Self {
        self.clone()
    }

    /// Attempts to end the operation with `cause`.
    ///
    /// Returns `true` if this call reached the terminal state — i.e. it won
    /// the race and its `cause` is now the recorded winner. Returns `false` if
    /// the token was already terminal, regardless of which cause got there
    /// first or which cause this call carried. Idempotent and single-writer:
    /// exactly one `cancel` across the whole lifetime of the token (and all its
    /// shares) ever returns `true`.
    pub fn cancel(&self, cause: CancelCause) -> bool {
        self.shared
            .cause
            .compare_exchange(LIVE, encode(cause), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Advances the deadline arm to `now_tick` and, if the deadline has been
    /// reached, attempts to cancel with [`CancelCause::Deadline`].
    ///
    /// Returns `true` only when *this* call is the one that drove the token
    /// terminal via the deadline — i.e. the deadline is set, `now_tick` has
    /// reached it, and the token was still live. If some other cause already
    /// won, or there is no deadline, or the deadline has not yet been reached,
    /// this returns `false` and changes nothing. It routes through
    /// [`cancel`](CancelToken::cancel), so it obeys the same exactly-once rule:
    /// polling past the deadline many times still only ever wins once.
    pub fn poll_deadline(&self, now_tick: u64) -> bool {
        match self.deadline {
            Some(deadline) if now_tick >= deadline => self.cancel(CancelCause::Deadline),
            _ => false,
        }
    }

    /// The winning cause, or `None` while the token is still live.
    #[must_use]
    pub fn cause(&self) -> Option<CancelCause> {
        decode(self.shared.cause.load(Ordering::Acquire))
    }

    /// Whether the token has reached its (single, immutable) terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.shared.cause.load(Ordering::Acquire) != LIVE
    }

    /// The deadline tick this token was created with, if any. Purely
    /// informational; the host owns the clock.
    #[must_use]
    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cause, listed so the pair-race matrix can iterate all ordered
    /// pairs without hand-transcribing sixteen combinations. Deliberately *not*
    /// in `CancelCause` declaration order, so a test driven off this list fires
    /// causes out of declaration order (spec requirement 3).
    const CAUSES: [CancelCause; 4] = [
        CancelCause::Shutdown,
        CancelCause::Deadline,
        CancelCause::Disconnected,
        CancelCause::Cancelled,
    ];

    // ---- Requirement 4: calibrate the positive before asserting the negative.

    #[test]
    fn simultaneous_all_cause_race_has_exactly_one_winner() {
        use std::sync::{Arc, Barrier};

        let token = CancelToken::new();
        let barrier = Arc::new(Barrier::new(CAUSES.len() + 1));
        let workers: Vec<_> = CAUSES
            .into_iter()
            .map(|cause| {
                let worker = token.share();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (cause, worker.cancel(cause))
                })
            })
            .collect();
        barrier.wait();

        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("racing canceller joined"))
            .collect();
        let winners: Vec<_> = results
            .iter()
            .filter_map(|(cause, won)| won.then_some(*cause))
            .collect();
        assert_eq!(winners.len(), 1, "one simultaneous writer must win");
        assert_eq!(token.cause(), winners.first().copied());
    }

    #[test]
    fn dropping_a_shared_handle_cannot_erase_a_racing_cancel() {
        use std::sync::{Arc, Barrier};

        let token = CancelToken::new();
        let dropped = token.share();
        let canceller = token.share();
        let barrier = Arc::new(Barrier::new(3));
        let drop_barrier = Arc::clone(&barrier);
        let dropping = std::thread::spawn(move || {
            drop_barrier.wait();
            drop(dropped);
        });
        let cancel_barrier = Arc::clone(&barrier);
        let cancelling = std::thread::spawn(move || {
            cancel_barrier.wait();
            canceller.cancel(CancelCause::Disconnected)
        });
        barrier.wait();

        dropping.join().expect("dropping worker joined");
        assert!(cancelling.join().expect("cancelling worker joined"));
        assert_eq!(token.cause(), Some(CancelCause::Disconnected));
    }

    #[test]
    fn cancel_drives_terminal_and_records_cause() {
        // Prove a transition is observable when one should happen, before any
        // "absorbed / no transition" assertion is trustworthy.
        let t = CancelToken::new();
        assert!(t.cancel(CancelCause::Disconnected), "first cancel must win");
        assert!(t.is_terminal(), "must be terminal after a winning cancel");
        assert_eq!(t.cause(), Some(CancelCause::Disconnected));
    }

    // ---- Requirement 1: exactly-once — count, don't just check is_terminal.

    // ---- Requirement 2 + 3: full ordered pair-race matrix, out of decl order.

    // ---- share(): one object, not two that each think they own it.

    // ---- Deadline via injected clock, no real time.
}
