//! App-side bookkeeping for in-flight invocations.
//!
//! One [`InvocationTable`] tracks every invocation the daemon has dispatched
//! but not yet seen complete. It enforces the two rules the daemon relies on
//! from a well-behaved app: an invocation is accepted exactly once, and no event
//! is emitted after a terminal one. A cancel from the daemon fires the
//! invocation's [`CancelToken`]; a cancel for an already-terminal or unknown
//! invocation is a harmless no-op. This is pure state: it performs no I/O and
//! holds no socket.

use std::collections::HashMap;

use crate::cancellation::{CancelCause, CancelToken};
use crate::center::protocol::{InvocationEvent, Uuid};

/// What the connector should do with an event the app produced.
#[derive(Debug, PartialEq, Eq)]
pub enum EmitDecision {
    /// Forward the event to the daemon.
    Send,
    /// Drop the event: the invocation already reached a terminal state, so a
    /// late event must not reach the daemon.
    Suppress,
}

/// Why an invocation could not be opened or advanced.
#[derive(Debug, PartialEq, Eq)]
pub enum LifecycleError {
    /// The daemon dispatched an invocation id that is already live. Duplicate
    /// live ids are rejected rather than overwriting the first.
    DuplicateInvocation {
        /// The offending id.
        id: String,
    },
    /// An event referenced an invocation this app never opened.
    UnknownInvocation {
        /// The unknown id.
        id: String,
    },
}

impl core::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateInvocation { id } => write!(f, "duplicate live invocation {id:?}"),
            Self::UnknownInvocation { id } => write!(f, "unknown invocation {id:?}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

enum State {
    Live { token: CancelToken },
    Terminal,
}

/// Tracks live invocations and their cancellation tokens.
#[derive(Default)]
pub struct InvocationTable {
    entries: HashMap<String, State>,
}

impl InvocationTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// The number of invocations not yet terminal (including terminal entries
    /// still retained for late-event suppression).
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.entries.len()
    }

    /// Opens an invocation and returns a fresh cancellation token to hand to the
    /// app's work. A deadline may be attached by the caller.
    ///
    /// # Errors
    /// Returns [`LifecycleError::DuplicateInvocation`] when the id is already
    /// live, so a replayed dispatch cannot displace the first.
    pub fn open(&mut self, id: &Uuid) -> Result<CancelToken, LifecycleError> {
        self.open_with(id, CancelToken::new())
    }

    /// Opens an invocation with a caller-provided token (for a deadline-bearing
    /// token).
    ///
    /// # Errors
    /// Returns [`LifecycleError::DuplicateInvocation`].
    pub fn open_with(
        &mut self,
        id: &Uuid,
        token: CancelToken,
    ) -> Result<CancelToken, LifecycleError> {
        if self.entries.contains_key(id.as_str()) {
            return Err(LifecycleError::DuplicateInvocation { id: id.to_string() });
        }
        let shared = token.share();
        self.entries
            .insert(id.as_str().to_owned(), State::Live { token });
        Ok(shared)
    }

    /// Records an event the app wants to emit, deciding whether it may be sent.
    ///
    /// A terminal event transitions the invocation to terminal and is sent
    /// once; any further event for that invocation is suppressed. A non-terminal
    /// event on a still-live invocation is sent.
    ///
    /// # Errors
    /// Returns [`LifecycleError::UnknownInvocation`] when the invocation was
    /// never opened.
    pub fn record(&mut self, event: &InvocationEvent) -> Result<EmitDecision, LifecycleError> {
        let id = event.invocation_id();
        match self.entries.get_mut(id.as_str()) {
            None => Err(LifecycleError::UnknownInvocation { id: id.to_string() }),
            Some(State::Terminal) => Ok(EmitDecision::Suppress),
            Some(state @ State::Live { .. }) => {
                if event.is_terminal() {
                    *state = State::Terminal;
                }
                Ok(EmitDecision::Send)
            }
        }
    }

    /// Fires the cancellation token for `id` with [`CancelCause::Cancelled`].
    ///
    /// Returns `true` when a live invocation was cancelled by this call, and
    /// `false` when the invocation is unknown or already terminal. The daemon's
    /// cancel is idempotent, so a redundant cancel is not an error.
    pub fn cancel(&mut self, id: &Uuid) -> bool {
        match self.entries.get(id.as_str()) {
            Some(State::Live { token }) => token.cancel(CancelCause::Cancelled),
            _ => false,
        }
    }

    /// Fires every live invocation with [`CancelCause::Disconnected`], for use
    /// when the connection drops. Returns the number of invocations cancelled.
    pub fn cancel_all_disconnected(&mut self) -> usize {
        let mut cancelled = 0;
        #[allow(
            clippy::iter_over_hash_type,
            reason = "every entry gets the same treatment, so the order does not matter"
        )]
        for state in self.entries.values() {
            if let State::Live { token } = state
                && token.cancel(CancelCause::Disconnected)
            {
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Removes a terminal invocation from the table, freeing its slot.
    ///
    /// Call after the terminal event has been sent to the daemon. Removing a
    /// still-live or unknown invocation does nothing.
    pub fn forget(&mut self, id: &Uuid) {
        if matches!(self.entries.get(id.as_str()), Some(State::Terminal)) {
            self.entries.remove(id.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Uuid {
        Uuid::new_v4().expect("entropy")
    }

    #[test]
    fn duplicate_open_is_rejected() {
        let mut table = InvocationTable::new();
        let invocation = id();
        table.open(&invocation).expect("first open");
        // Compare the error alone: the `Ok` half is a `CancelToken` from the
        // security crate, which has no `PartialEq`, so the whole `Result`
        // cannot be compared.
        let Err(error) = table.open(&invocation) else {
            panic!("a duplicate open must be refused");
        };
        assert_eq!(
            error,
            LifecycleError::DuplicateInvocation {
                id: invocation.to_string()
            }
        );
    }
}
