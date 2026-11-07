//! Drives one screencast session through source selection and start.
//!
//! The driver preserves step order, observes out-of-band revocation, and uses the shared pending lifecycle for cancellation and exactly-once closure.

use crate::portal::CancelReason;
use crate::portal::cancel::PortalCancel;
use crate::portal::pending::{CloseClaim, PendingLifecycle};
use crate::portal::session::{
    HandshakeStep, PipeWireStream, Revocation, SessionBackend, SessionError, SessionId, StepReply,
};

/// Terminal outcome of a screencast session, keeping cancellation, revocation, and failure distinct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The handshake completed and the session is streaming. Carries the
    /// PipeWire streams `Start` negotiated.
    Completed(Vec<PipeWireStream>),
    /// The session was cancelled, carrying the winning cancellation reason.
    Cancelled(CancelReason),
    /// The compositor revoked the session out of band.
    Revoked,
    /// ScreenCast was never declared in the application's manifest.
    NotDeclared,
    /// The portal is reachable but does not implement ScreenCast usably.
    Unsupported,
    /// The portal frontend is not reachable on this host.
    Unavailable,
    /// An operational, protocol, or ordering failure.
    Failed,
}

impl SessionOutcome {
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

    /// Whether this outcome is [`Revoked`](Self::Revoked).
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        matches!(self, Self::Revoked)
    }

    /// The streams a [`Completed`](Self::Completed) outcome carries, or `None`.
    #[must_use]
    pub fn streams(&self) -> Option<&[PipeWireStream]> {
        match self {
            Self::Completed(s) => Some(s),
            _ => None,
        }
    }

    /// The cancellation route, or `None` for any non-cancel outcome.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        match self {
            Self::Cancelled(r) => Some(*r),
            _ => None,
        }
    }
}

fn outcome_for(e: SessionError) -> SessionOutcome {
    match e {
        SessionError::Unavailable => SessionOutcome::Unavailable,
        SessionError::Unsupported => SessionOutcome::Unsupported,
        SessionError::NotDeclared => SessionOutcome::NotDeclared,
        SessionError::OutOfOrder
        | SessionError::Failed
        | SessionError::ResourceExhausted
        | SessionError::TimedOut
        | SessionError::Cancelled => SessionOutcome::Failed,
    }
}

/// A screencast session request, built and then started.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScreencastRequest {
    _private: (),
}

impl ScreencastRequest {
    /// Builds a screencast session request.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Creates a session positioned for source selection, or returns a creation failure outcome.
    ///
    /// # Errors
    ///
    /// The terminal [`SessionOutcome`] when the session cannot be created.
    pub fn start(
        self,
        backend: &mut dyn SessionBackend,
    ) -> Result<ScreencastSession, SessionOutcome> {
        match backend.create_session() {
            Ok(id) => Ok(ScreencastSession::new(id)),
            Err(e) => Err(outcome_for(e)),
        }
    }
}

/// A move-only driver for one live screencast session.
///
/// A cancellation handle may cross threads, but only this value advances the handshake.
#[derive(Debug)]
pub struct ScreencastSession {
    lifecycle: PendingLifecycle<SessionId>,
    step: HandshakeStep,
    awaiting_reply: bool,
}

impl ScreencastSession {
    fn new(id: SessionId) -> Self {
        Self {
            lifecycle: PendingLifecycle::live(id),
            step: HandshakeStep::SelectSources,
            awaiting_reply: false,
        }
    }

    /// The backend session id.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a live session always holds its wire id"
    )]
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.lifecycle
            .wire_id()
            .expect("a live session always holds its wire id")
    }

    /// A `Send` clone of the cancellation handle.
    #[must_use]
    pub fn cancel_handle(&self) -> PortalCancel {
        self.lifecycle.cancel_handle()
    }

    /// Whether the session is still awaiting a terminal event.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.lifecycle.is_pending()
    }

    /// The step currently being driven or awaited, or `None` if terminal.
    #[must_use]
    pub fn current_step(&self) -> Option<HandshakeStep> {
        if !self.lifecycle.is_pending() || self.lifecycle.is_finished() {
            return None;
        }
        Some(self.step)
    }

    /// The cancellation route, or `None` if live or completed.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.lifecycle.outcome_reason()
    }

    /// Records cancellation and an owed close if this call wins the terminal race.
    pub fn cancel(&mut self, reason: CancelReason) -> Option<SessionOutcome> {
        self.lifecycle
            .record_cancel(reason)
            .then_some(SessionOutcome::Cancelled(reason))
    }

    /// Polls for revocation or a step reply and advances only to a successful step’s successor.
    ///
    /// Revocation is checked first and wins over a simultaneously available reply.
    pub fn poll(&mut self, backend: &mut dyn SessionBackend) -> Option<SessionOutcome> {
        // Claim polling under the shared lock so cancellation can seal the session before a reply is consumed.
        let id = self.lifecycle.try_claim_poll()?;

        // Revocation takes precedence because a reply is unusable after the capability disappears.

        if let Some(Revocation::Revoked) = backend.poll_revocation(id) {
            self.lifecycle.commit_terminal_reply();
            return Some(self.first_terminal(SessionOutcome::Revoked));
        }

        if !self.awaiting_reply {
            match backend.drive_step(id, self.step) {
                Ok(()) => {
                    self.awaiting_reply = true;
                    self.lifecycle.release_poll();
                    self.concurrent_cancel_outcome()
                }
                // Queue pressure admits no step, so release the claim and retry the same step.
                Err(SessionError::ResourceExhausted) => {
                    self.lifecycle.release_poll();
                    self.concurrent_cancel_outcome()
                }
                Err(_) => {
                    self.lifecycle.commit_terminal_reply();
                    Some(self.first_terminal(SessionOutcome::Failed))
                }
            }
        } else {
            let step = self.step;
            let Some(reply) = backend.poll_step(id) else {
                self.lifecycle.release_poll();
                return self.concurrent_cancel_outcome();
            };
            self.handle_step_reply(step, reply)
        }
    }

    fn concurrent_cancel_outcome(&self) -> Option<SessionOutcome> {
        self.lifecycle
            .outcome_reason()
            .map(SessionOutcome::Cancelled)
    }

    fn first_terminal(&self, backend_outcome: SessionOutcome) -> SessionOutcome {
        if self.lifecycle.cancel().complete() {
            return backend_outcome;
        }
        match self.lifecycle.outcome_reason() {
            Some(reason) => SessionOutcome::Cancelled(reason),
            // A single driver should win completion; retain the backend outcome rather than panic if that invariant changes.
            None => backend_outcome,
        }
    }

    fn handle_step_reply(
        &mut self,
        step: HandshakeStep,
        reply: StepReply,
    ) -> Option<SessionOutcome> {
        match reply {
            StepReply::Ongoing => match step.next() {
                Some(next) => {
                    self.step = next;
                    self.awaiting_reply = false;
                    self.lifecycle.release_poll();
                    self.concurrent_cancel_outcome()
                }
                None => {
                    // `Start` must return streams; an ongoing reply at the terminal step is a protocol failure.
                    self.lifecycle.commit_terminal_reply();
                    Some(self.first_terminal(SessionOutcome::Failed))
                }
            },
            StepReply::Streams(streams) => {
                self.lifecycle.commit_terminal_reply();
                let outcome = if step == HandshakeStep::Start {
                    SessionOutcome::Completed(streams)
                } else {
                    SessionOutcome::Failed
                };
                Some(self.first_terminal(outcome))
            }
            StepReply::UserCancelled => {
                self.lifecycle.commit_terminal_reply();
                if self.lifecycle.record_cancel(CancelReason::User) {
                    Some(SessionOutcome::Cancelled(CancelReason::User))
                } else {
                    self.concurrent_cancel_outcome()
                }
            }
            StepReply::Failed => {
                self.lifecycle.commit_terminal_reply();
                Some(self.first_terminal(SessionOutcome::Failed))
            }
        }
    }

    /// Performs an owed session close at most once.
    ///
    /// Completed and revoked sessions owe no close because their remote object is gone.
    pub fn settle(&mut self, backend: &mut dyn SessionBackend) -> Option<CancelReason> {
        match self.lifecycle.claim_close()? {
            CloseClaim::Wire(id, reason) => {
                backend.close_session(id, reason);
                Some(reason)
            }
            CloseClaim::NoWire(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::session::MockSessionBackend;

    fn stream(node_id: u32) -> PipeWireStream {
        PipeWireStream {
            node_id,
            position: Some((0, 0)),
            size: Some((1920, 1080)),
        }
    }

    fn drive_to_end(
        session: &mut ScreencastSession,
        backend: &mut MockSessionBackend,
    ) -> SessionOutcome {
        for _ in 0..20 {
            if let Some(out) = session.poll(backend) {
                return out;
            }
        }
        panic!("session did not terminate within 20 polls");
    }

    #[test]
    fn a_completed_session_is_never_closed() {
        let mut b = MockSessionBackend::new();
        let mut s = ScreencastRequest::new().start(&mut b).unwrap();
        let id = s.id();
        b.script_step(id, StepReply::Ongoing);
        b.script_step(id, StepReply::Streams(vec![stream(7)]));
        assert!(drive_to_end(&mut s, &mut b).is_completed());

        assert_eq!(
            s.cancel(CancelReason::Application),
            None,
            "cancelling a completed session must be absorbed"
        );
        assert_eq!(s.settle(&mut b), None, "a completed session owes no close");
        assert_eq!(
            b.close_count(id),
            0,
            "an answered session must NEVER be closed"
        );
    }
}
