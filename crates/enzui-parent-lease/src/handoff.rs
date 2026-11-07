//! Moving a lease from the task that created it into the state that owns it.
//!
//! A parent export is produced asynchronously but must end up in application
//! state, and an application message is normally required to be `Clone`, which
//! an owned protocol object is not. [`Handoff`] carries it across that boundary:
//! the value can be taken exactly once, and every clone of the carrier refers to
//! the same slot, so duplicating the message cannot duplicate the lease.

use std::sync::{Arc, Mutex};

/// A `Clone` carrier that delivers a non-`Clone` value exactly once.
///
/// Cloning shares the slot rather than the value: the first
/// [`take`](Self::take) yields the value and every later one yields `None`.
/// A message that is delivered twice therefore installs one lease, and a
/// message that is dropped destroys the value with the last carrier.
#[derive(Debug)]
pub struct Handoff<T> {
    slot: Arc<Mutex<Option<T>>>,
}

impl<T> Handoff<T> {
    /// A carrier holding `value`.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(value))),
        }
    }

    /// An already-empty carrier, for a path that produced nothing to hand over.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Takes the value, leaving the carrier empty for every other holder.
    pub fn take(&self) -> Option<T> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Whether the value is still available.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self {
            slot: Arc::clone(&self.slot),
        }
    }
}
