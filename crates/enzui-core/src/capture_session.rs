//! Host-owned keyboard-shortcut capture sessions.
//!
//! At most one recorder is armed per window, while different windows remain independent. Accepting a candidate ends capture; rejecting one keeps the recorder armed with feedback.

use std::collections::BTreeMap;

use iced_core::window;

use crate::shortcut::Shortcut;

/// Identifies a capture session by window and stable action id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionKey {
    window: window::Id,
    action: String,
}

impl SessionKey {
    /// Names a session by its window and stable action id.
    #[must_use]
    pub fn new(window: window::Id, action: impl Into<String>) -> Self {
        Self {
            window,
            action: action.into(),
        }
    }

    /// The window this session belongs to.
    #[must_use]
    pub fn window(&self) -> window::Id {
        self.window
    }

    /// The stable action id being rebound.
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArmedRecorder {
    prior: Option<Shortcut>,
    rejected: bool,
}

/// Controls capture sessions with at most one armed recorder per window.
#[derive(Debug, Clone, Default)]
pub struct CaptureSession {
    armed: BTreeMap<SessionKey, ArmedRecorder>,
}

/// Outcome of attempting to arm a capture session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmOutcome {
    /// Nothing was armed in this window; `key` is now the sole armed recorder.
    Armed {
        /// The recorder now armed.
        key: SessionKey,
    },
    /// Replaces another recorder in the same window and reports the displaced session.
    Displaced {
        /// The recorder now armed.
        key: SessionKey,
        /// The recorder that was disarmed by this arm.
        displaced: SessionKey,
    },
    /// The same recorder was already armed, so no state changed.
    ///
    /// Treat this outcome as a re-click by calling [`CaptureSession::cancel_reclick`].
    AlreadyArmed {
        /// The recorder that is (still) armed.
        key: SessionKey,
    },
}

impl CaptureSession {
    /// A fresh controller with nothing armed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the sole armed recorder in `window`, if any.
    #[must_use]
    pub fn armed_in(&self, window: window::Id) -> Option<&SessionKey> {
        self.armed.keys().find(|k| k.window == window)
    }

    /// The total number of armed recorders across all windows.
    #[must_use]
    pub fn armed_count(&self) -> usize {
        self.armed.len()
    }

    /// Whether `key` is the armed recorder right now.
    #[must_use]
    pub fn is_armed(&self, key: &SessionKey) -> bool {
        self.armed.contains_key(key)
    }

    /// Arms `key`, retaining `prior` for cancellation.
    ///
    /// Another recorder in the same window is atomically displaced; other windows are untouched. Re-arming the same key returns [`ArmOutcome::AlreadyArmed`] without changing state.
    pub fn arm(&mut self, key: SessionKey, prior: Option<Shortcut>) -> ArmOutcome {
        if self.armed.contains_key(&key) {
            return ArmOutcome::AlreadyArmed { key };
        }

        let displaced = self.armed.keys().find(|k| k.window == key.window).cloned();
        if let Some(old) = &displaced {
            self.armed.remove(old);
        }

        self.armed.insert(
            key.clone(),
            ArmedRecorder {
                prior,
                rejected: false,
            },
        );

        match displaced {
            Some(displaced) => ArmOutcome::Displaced { key, displaced },
            None => ArmOutcome::Armed { key },
        }
    }

    /// Returns `true`; re-arming the active recorder is treated as cancellation.
    #[must_use]
    pub const fn rearm_cancels() -> bool {
        true
    }

    /// Cancels `key` and returns its prior binding unchanged.
    ///
    /// Returns [`CancelOutcome::NotArmed`] when no matching session exists.
    pub fn cancel(&mut self, key: &SessionKey) -> CancelOutcome {
        self.cancel_via(key, CancelRoute::Explicit)
    }

    /// Cancel because the armed chip was re-clicked.
    pub fn cancel_reclick(&mut self, key: &SessionKey) -> CancelOutcome {
        self.cancel_via(key, CancelRoute::Reclick)
    }

    /// Cancel because the armed chip lost keyboard focus.
    pub fn cancel_focus_lost(&mut self, key: &SessionKey) -> CancelOutcome {
        self.cancel_via(key, CancelRoute::FocusLost)
    }

    /// Cancels the recorder in `window`, leaving other windows untouched.
    pub fn cancel_window_deactivated(&mut self, window: window::Id) -> CancelOutcome {
        match self.armed_in(window).cloned() {
            Some(key) => self.cancel_via(&key, CancelRoute::WindowDeactivated),
            None => CancelOutcome::NotArmed,
        }
    }

    /// Cancels `key` through the disabled route and returns its prior binding.
    pub fn cancel_disabled(&mut self, key: &SessionKey) -> CancelOutcome {
        self.cancel_via(key, CancelRoute::Disabled)
    }

    /// Cancel because the chip is being replaced (a new chip armed over it, or
    /// the binding reassigned out from under it).
    pub fn cancel_replaced(&mut self, key: &SessionKey) -> CancelOutcome {
        self.cancel_via(key, CancelRoute::Replaced)
    }

    fn cancel_via(&mut self, key: &SessionKey, route: CancelRoute) -> CancelOutcome {
        match self.armed.remove(key) {
            Some(recorder) => CancelOutcome::Restored {
                route,
                prior: recorder.prior,
            },
            None => CancelOutcome::NotArmed,
        }
    }

    /// Removes the session for a deleted action without affecting any other session.
    ///
    /// Returns the prior binding only as information because the action no longer exists.
    pub fn remove_action(&mut self, key: &SessionKey) -> RemoveOutcome {
        match self.armed.remove(key) {
            Some(recorder) => RemoveOutcome::Cleared {
                prior: recorder.prior,
            },
            None => RemoveOutcome::NotPresent,
        }
    }

    /// Resolves a candidate shortcut using host ownership policy.
    ///
    /// A binding owned by this action is accepted; a reserved binding or one owned by another action conflicts even if that owner is disabled. Acceptance ends capture, while rejection leaves it armed with feedback.
    pub fn resolve_candidate(
        &mut self,
        key: &SessionKey,
        candidate: Shortcut,
        policy: impl Fn(&Shortcut) -> Reservation,
    ) -> CandidateOutcome {
        if !self.armed.contains_key(key) {
            return CandidateOutcome::NotArmed;
        }

        let verdict = match policy(&candidate) {
            Reservation::Free => Verdict::Accept,
            Reservation::Reserved => Verdict::Reserved,
            Reservation::AssignedTo { action, .. } => {
                if action == key.action {
                    Verdict::Accept
                } else {
                    Verdict::Conflict
                }
            }
        };

        match verdict {
            Verdict::Accept => {
                self.armed.remove(key);
                CandidateOutcome::Accepted { binding: candidate }
            }
            reject => {
                if let Some(recorder) = self.armed.get_mut(key) {
                    recorder.rejected = true;
                }
                let reason = match reject {
                    Verdict::Reserved => RejectReason::Reserved,
                    Verdict::Conflict => RejectReason::Conflict,
                    Verdict::Accept => unreachable!(),
                };
                CandidateOutcome::Rejected { reason }
            }
        }
    }

    /// Returns the prior binding retained for an armed session.
    #[must_use]
    pub fn prior_binding(&self, key: &SessionKey) -> Option<&Shortcut> {
        self.armed.get(key).and_then(|r| r.prior.as_ref())
    }

    /// Returns whether the armed session last rejected a candidate, or `false` when it is not armed.
    #[must_use]
    pub fn was_rejected(&self, key: &SessionKey) -> bool {
        self.armed.get(key).is_some_and(|r| r.rejected)
    }
}

/// Identifies why cancellation removed a session and returned its prior binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelRoute {
    /// A generic explicit cancel (Escape, a Cancel button).
    Explicit,
    /// The armed chip was re-clicked.
    Reclick,
    /// The armed chip lost keyboard focus.
    FocusLost,
    /// The chip's window was deactivated.
    WindowDeactivated,
    /// The action / chip was disabled while armed.
    Disabled,
    /// The chip is being replaced or its binding reassigned.
    Replaced,
}

/// What cancelling produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The session was removed and its prior binding returned to the caller.
    Restored {
        /// Which route triggered the cancel.
        route: CancelRoute,
        /// The prior binding, returned unchanged for the caller to restore.
        prior: Option<Shortcut>,
    },
    /// `key` was not armed; nothing to cancel.
    NotArmed,
}

/// What removing an action produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    /// The deleted action’s session was cleared, carrying its former binding for information.
    Cleared {
        /// The prior binding of the cleared session.
        prior: Option<Shortcut>,
    },
    /// No session was armed for this action; nothing to clear.
    NotPresent,
}

/// Describes the host’s current ownership policy for a candidate binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reservation {
    /// The binding is unassigned and not reserved — free to take.
    Free,
    /// The binding is a system / reserved chord the host refuses to assign.
    Reserved,
    /// The binding is currently assigned to an action.
    AssignedTo {
        /// The stable id of the owning action.
        action: String,
        /// Whether the owning action is enabled; ownership still conflicts when this is `false`.
        enabled: bool,
    },
}

/// Outcome of resolving a candidate shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateOutcome {
    /// The candidate was accepted, capture ended, and `binding` is ready to persist.
    Accepted {
        /// The accepted binding.
        binding: Shortcut,
    },
    /// The candidate was rejected while capture remained armed and the prior binding stayed unchanged.
    Rejected {
        /// Why it was rejected.
        reason: RejectReason,
    },
    /// `key` was not armed; there was nothing to resolve.
    NotArmed,
}

/// Why a candidate was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The binding is reserved by the system / host.
    Reserved,
    /// The binding is already assigned to another action (enabled or not).
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Accept,
    Reserved,
    Conflict,
}

/// Returns whether the chip is both armed and enabled.
///
/// Disabled state takes precedence; use [`CaptureSession::cancel_disabled`] to apply the corresponding session transition.
#[must_use]
pub const fn resolve_capture(armed: bool, enabled: bool) -> bool {
    armed && enabled
}
