//! Typed lifecycle, cancellation, parenting, notifications, URI, print, session, and policy primitives for XDG desktop portals.
//!
//! Transport-independent operations expose explicit terminal outcomes and
//! cancellation state with at-most-once close handling.

pub mod backend;
#[cfg(feature = "portal-dbus")]
pub mod background;
pub mod cancel;
pub mod chooser;
pub mod completion;
#[cfg(feature = "portal-dbus")]
pub mod dbus_backend;
#[cfg(feature = "portal-dbus")]
pub mod file_transfer;
#[cfg(feature = "portal-dbus")]
pub mod global_shortcuts;
#[cfg(feature = "portal-dbus")]
pub mod inhibit;
pub mod notification;
pub mod op;
pub mod open_uri;
pub mod parent;
mod pending;
pub mod pipewire;
pub mod policy;
pub mod print;
#[cfg(feature = "portal-dbus")]
pub mod runtime;
pub mod sandbox;
#[cfg(feature = "portal-dbus")]
pub mod screenshot;
pub mod session;
#[cfg(feature = "portal-dbus")]
pub mod session_monitor;
pub mod session_op;
pub mod uri;

use crate::cancellation::{CancelCause, CancelToken};

/// Reason an interactive portal operation was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancelReason {
    /// The user dismissed the portal dialog themselves.
    User,
    /// The application requested cancellation through its handle.
    Application,
    /// The parent window closed while the request was active.
    ParentClosed,
}

/// Terminal outcome of a portal operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalOutcome<T> {
    /// The operation ran to completion and produced a value.
    Completed(T),
    /// Cancellation by the user, application, or parent-window closure.
    Cancelled(CancelReason),
    /// The user explicitly denied consent.
    Denied,
    /// The application did not declare the operation.
    NotDeclared,
    /// The portal frontend is reachable but does not implement this operation
    /// (or this version of it).
    Unsupported,
    /// The portal frontend is not reachable at all on this host.
    Unavailable,
    /// A requested parent could not be resolved; the operation is not made unparented.
    ParentUnavailable,
    /// An operational or protocol failure, including response code `2`.
    Failed,
}

/// Raw response code emitted by an XDG portal request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortalResponse {
    /// Response code `0`: the request completed successfully.
    Success,
    /// Response code `1`: the user cancelled the interaction.
    Cancelled,
    /// Response code `2`, treated as failure rather than denial.
    Other,
    /// An undefined response code, treated as failure.
    Unknown(u32),
}

impl PortalResponse {
    /// Maps `0`, `1`, and `2` to named variants and preserves unknown codes.
    #[must_use]
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Success,
            1 => Self::Cancelled,
            2 => Self::Other,
            n => Self::Unknown(n),
        }
    }
}

impl<T> PortalOutcome<T> {
    /// Maps success to the supplied value, cancellation to `User`, and other codes to failure.
    #[must_use]
    pub fn from_response(response: PortalResponse, value: T) -> Self {
        match response {
            PortalResponse::Success => Self::Completed(value),
            PortalResponse::Cancelled => Self::Cancelled(CancelReason::User),
            PortalResponse::Other | PortalResponse::Unknown(_) => Self::Failed,
        }
    }

    /// Whether this outcome is [`Completed`](Self::Completed).
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }

    /// Whether this outcome is any flavour of [`Cancelled`](Self::Cancelled).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled(_))
    }

    /// The value a [`Completed`](Self::Completed) outcome carries, or `None`.
    #[must_use]
    pub fn completed(&self) -> Option<&T> {
        match self {
            Self::Completed(v) => Some(v),
            _ => None,
        }
    }

    /// The cancellation reason, if this outcome is
    /// [`Cancelled`](Self::Cancelled).
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        match self {
            Self::Cancelled(r) => Some(*r),
            _ => None,
        }
    }
}

/// Cancellation authority for one live portal request.
///
/// The first cancellation invokes close exactly once; task abortion alone does not cancel.
pub struct CancelHandle {
    token: CancelToken,
    close: Option<Box<dyn FnMut()>>,
    reason: Option<CancelReason>,
}

impl CancelHandle {
    /// Creates a cancellation handle whose close action can run at most once.
    #[must_use]
    pub fn new(close: impl FnMut() + 'static) -> Self {
        Self {
            token: CancelToken::new(),
            close: Some(Box::new(close)),
            reason: None,
        }
    }

    /// Returns the shared token used by polling to observe termination.
    #[must_use]
    pub fn token(&self) -> CancelToken {
        self.token.share()
    }

    /// Cancels on behalf of the application and reports whether this call won.
    pub fn cancel(&mut self) -> bool {
        self.cancel_for(CancelReason::Application)
    }

    /// Cancels because the parent closed and reports whether this call won.
    pub fn parent_closed(&mut self) -> bool {
        self.cancel_for(CancelReason::ParentClosed)
    }

    /// Cancels for a specified reason, preserving the first winning reason.
    pub fn cancel_for(&mut self, reason: CancelReason) -> bool {
        let cause = match reason {
            CancelReason::User | CancelReason::Application | CancelReason::ParentClosed => {
                CancelCause::Cancelled
            }
        };
        // Gate the close action on the token’s single terminal transition.
        if self.token.cancel(cause) {
            // Record the winning reason before invoking the close callback.
            self.reason = Some(reason);
            if let Some(mut close) = self.close.take() {
                close();
            }
            true
        } else {
            false
        }
    }

    /// Returns the winning cancellation reason, or `None` for live or completed requests.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.reason
    }

    /// Whether this handle's request has reached a terminal state (completed or
    /// cancelled by any route).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.token.is_terminal()
    }

    /// Marks completion without closing the request and reports whether this call won.
    pub fn complete(&mut self) -> bool {
        if self.token.cancel(CancelCause::Shutdown) {
            self.close = None;
            true
        } else {
            false
        }
    }
}

impl std::fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelHandle")
            .field("terminal", &self.token.is_terminal())
            .field("close_pending", &self.close.is_some())
            .field("reason", &self.reason)
            .finish()
    }
}

pub use completion::{CompletionSignal, CompletionWait, PortalWake};
pub use parent::PortalParent;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn cancel_runs_close_exactly_once() {
        let calls = Rc::new(Cell::new(0u32));
        let c = Rc::clone(&calls);
        let mut h = CancelHandle::new(move || c.set(c.get() + 1));

        assert!(!h.is_terminal(), "fresh handle must be live");
        assert_eq!(calls.get(), 0, "close must not run before cancel");

        assert!(h.cancel(), "first cancel must win");
        assert!(h.is_terminal(), "must be terminal after cancel");
        assert_eq!(calls.get(), 1, "close must run exactly once");
    }
}
