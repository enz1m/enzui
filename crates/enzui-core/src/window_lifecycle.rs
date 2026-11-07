//! Multi-window lifecycle decisions for iced daemon applications.
//!
//! Daemons do not exit automatically when the last window closes. [`WindowLedger`] returns an explicit [`LifecycleAction`], with configurable handling for closing the primary window.

use std::collections::BTreeSet;

use iced_core::window;

/// The action a host must perform after a window lifecycle event.
///
/// This enum is intentionally exhaustive so new actions require host match updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleAction {
    /// Continue running because windows remain or the event was untracked.
    Continue,

    /// Exit because no tracked windows remain.
    Exit,

    /// Close the remaining windows before exiting after the primary closes.
    CloseThenExit(Vec<window::Id>),
}

impl LifecycleAction {
    /// Returns whether the action terminates the process.
    #[must_use]
    pub fn exits(&self) -> bool {
        matches!(
            self,
            LifecycleAction::Exit | LifecycleAction::CloseThenExit(_)
        )
    }
}

/// Policy applied when the primary window closes while others remain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrimaryClosePolicy {
    /// Close all remaining dependent windows and exit; this is the default.
    #[default]
    CloseEverything,

    /// Treat the primary as a peer and exit only after the final window closes.
    LastWindowWins,
}

/// Tracks process-local open window ids and derives exit actions; use [`crate::window_placement::PlacementKey`] for persistent identity.
#[derive(Debug, Clone, Default)]
pub struct WindowLedger {
    open: BTreeSet<window::Id>,
    primary: Option<window::Id>,
    policy: PrimaryClosePolicy,
}

impl WindowLedger {
    /// Creates an empty ledger, matching an iced daemon before its first window opens.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A ledger with an explicit [`PrimaryClosePolicy`].
    #[must_use]
    pub fn with_policy(policy: PrimaryClosePolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    /// Record that `id` opened.
    pub fn opened(&mut self, id: window::Id) {
        self.open.insert(id);
    }

    /// Records `id` as open and makes it the current primary, replacing any prior primary id.
    pub fn opened_primary(&mut self, id: window::Id) {
        self.open.insert(id);
        self.primary = Some(id);
    }

    /// Removes `id` and returns the required action; unknown or duplicate ids return [`LifecycleAction::Continue`].
    pub fn closed(&mut self, id: window::Id) -> LifecycleAction {
        if !self.open.remove(&id) {
            return LifecycleAction::Continue;
        }

        let was_primary = self.primary == Some(id);
        if was_primary {
            self.primary = None;
        }

        // A daemon with no tracked windows must exit regardless of primary policy.
        if self.open.is_empty() {
            return LifecycleAction::Exit;
        }

        if was_primary && self.policy == PrimaryClosePolicy::CloseEverything {
            // Close dependent windows before terminating the runtime.
            let remaining: Vec<window::Id> = self.open.iter().copied().collect();
            self.open.clear();
            return LifecycleAction::CloseThenExit(remaining);
        }

        LifecycleAction::Continue
    }

    /// How many windows are currently open.
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Is `id` currently open?
    #[must_use]
    pub fn is_open(&self, id: window::Id) -> bool {
        self.open.contains(&id)
    }

    /// The primary window's id, if one is open.
    #[must_use]
    pub fn primary(&self) -> Option<window::Id> {
        self.primary
    }

    /// Every open window, in id order.
    pub fn open_windows(&self) -> impl Iterator<Item = window::Id> + '_ {
        self.open.iter().copied()
    }

    /// The policy in force.
    #[must_use]
    pub fn policy(&self) -> PrimaryClosePolicy {
        self.policy
    }
}

#[cfg(test)]
#[path = "window_lifecycle/tests.rs"]
mod tests;
