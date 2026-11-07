//! Backend contracts and an in-process mock for multi-step portal sessions.
//!
//! Screencast steps share one session identity, preserve handshake ordering, and report out-of-band revocation separately from cancellation and failure.

use std::collections::VecDeque;

use crate::portal::CancelReason;
use crate::portal::parent::ParentHandle;

/// An opaque backend session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u64);

impl SessionId {
    /// Builds an id. Backends only.
    #[must_use]
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// The underlying value, for a backend keying a map on it.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// An ordered step in the screencast handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HandshakeStep {
    /// Creates the session handle shared by later steps.
    CreateSession,
    /// Selects capture sources after session creation.
    SelectSources,
    /// Starts capture after source selection and returns streams.
    Start,
}

impl HandshakeStep {
    /// Returns the required predecessor, or `None` for session creation.
    #[must_use]
    pub fn predecessor(self) -> Option<Self> {
        match self {
            Self::CreateSession => None,
            Self::SelectSources => Some(Self::CreateSession),
            Self::Start => Some(Self::SelectSources),
        }
    }

    /// The next step after this one, or `None` if this is terminal (`Start`).
    #[must_use]
    pub fn next(self) -> Option<Self> {
        match self {
            Self::CreateSession => Some(Self::SelectSources),
            Self::SelectSources => Some(Self::Start),
            Self::Start => None,
        }
    }

    /// The portal interface member this step invokes.
    #[must_use]
    pub fn member(self) -> &'static str {
        match self {
            Self::CreateSession => "CreateSession",
            Self::SelectSources => "SelectSources",
            Self::Start => "Start",
        }
    }
}

/// A PipeWire stream descriptor returned by a successful start.
///
/// It contains the node ID and optional compositor geometry, but not a PipeWire descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeWireStream {
    /// The PipeWire node id the portal assigned to this stream.
    pub node_id: u32,
    /// The stream's position `(x, y)` in the compositor's layout, if reported.
    pub position: Option<(i32, i32)>,
    /// The stream's size `(w, h)`, if reported.
    pub size: Option<(u32, u32)>,
}

/// A backend reply for one driven session step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepReply {
    /// The step succeeded and the handshake may proceed to the next step.
    Ongoing,
    /// The final step (`Start`) succeeded, yielding the PipeWire streams.
    Streams(Vec<PipeWireStream>),
    /// A portal `Response` during the handshake reported user cancellation
    /// (code 1).
    UserCancelled,
    /// A portal `Response` reported "other"/unknown (code 2+): an operational
    /// failure during the handshake.
    Failed,
}

/// A preflight, protocol, capacity, or transport failure while driving a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionError {
    /// The portal frontend is not reachable on this host.
    Unavailable,
    /// Reachable, but does not implement ScreenCast at a usable version.
    Unsupported,
    /// Not declared in the application's manifest, so the layer refuses to try.
    NotDeclared,
    /// The requested step is not legal in the session’s current state.
    OutOfOrder,
    /// A transport error, a malformed reply, a broken connection.
    Failed,
    /// The bounded backend command queue has no capacity; no step was admitted.
    ResourceExhausted,
    /// A bounded wire call exceeded its deadline without detaching work; the session remains closable.
    TimedOut,
    /// A backend cancellation interrupted the current bounded wire call.
    Cancelled,
}

/// An out-of-band compositor revocation delivered independently of step replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Revocation {
    /// The compositor closed the session out of band.
    Revoked,
}

/// Backend interface for an ordered, revocation-aware portal session.
///
/// Polling is nonblocking; implementations reject out-of-order steps and make closure idempotent.
pub trait SessionBackend {
    /// Opens a session (the `CreateSession` step), returning its id.
    ///
    /// # Errors
    ///
    /// A [`SessionError`] when the session cannot be created.
    fn create_session(&mut self) -> Result<SessionId, SessionError>;

    /// Issues the legal next step, returning `OutOfOrder` without issuing another step.
    ///
    /// # Errors
    ///
    /// `SessionError::OutOfOrder` for a step out of sequence, and others when the step cannot be
    /// issued.
    fn drive_step(&mut self, session: SessionId, step: HandshakeStep) -> Result<(), SessionError>;

    /// Polls nonblockingly for the last driven step’s reply.
    fn poll_step(&mut self, session: SessionId) -> Option<StepReply>;

    /// Polls nonblockingly for an out-of-band compositor revocation.
    fn poll_revocation(&mut self, session: SessionId) -> Option<Revocation>;

    /// Names the window a session's user-facing step should be parented to.
    ///
    /// A ScreenCast handshake shows its picker at the `Start` step, which takes
    /// a `parent_window` argument; supplying an exported handle here is what
    /// lets that picker be modal to the window the user acted in. The handle
    /// must stay exported until the session's `Start` has been answered, so the
    /// caller holds the lease for at least that long.
    ///
    /// Defaulted to ignoring the parent, so a backend with no window-parenting
    /// transport keeps an explicitly unparented handshake rather than reporting
    /// a parent it will not send.
    fn set_parent(&mut self, session: SessionId, parent: ParentHandle) {
        let _ = (session, parent);
    }

    /// Closes a session idempotently; unknown and repeated closes must not panic.
    fn close_session(&mut self, session: SessionId, reason: CancelReason);

    /// A one-line description of what this backend is, for diagnostics.
    fn describe(&self) -> String;
}

/// A deterministic backend that scripts replies, enforces ordering, and records calls.
#[derive(Debug, Default)]
pub struct MockSessionBackend {
    next_id: u64,
    create_error: Option<SessionError>,
    exhausted_drives: usize,
    scripted: std::collections::HashMap<SessionId, VecDeque<StepReply>>,
    revocations: std::collections::HashMap<SessionId, VecDeque<Revocation>>,
    last_step: std::collections::HashMap<SessionId, HandshakeStep>,
    in_flight: std::collections::HashMap<SessionId, HandshakeStep>,
    /// Every session created, in order.
    pub created: Vec<SessionId>,
    /// Every step driven, in order, with the session it belonged to.
    pub driven: Vec<(SessionId, HandshakeStep)>,
    /// Sessions closed with their reasons; repeated closes are recorded separately.
    pub closed: Vec<(SessionId, CancelReason)>,
    /// Every repeat close attempt, recorded but not acted on
    pub repeat_close_attempts: Vec<(SessionId, CancelReason)>,
    /// Close attempts for unknown sessions, recorded without taking action.
    pub close_for_unknown_id_attempts: Vec<(SessionId, CancelReason)>,
    /// Every `poll_step` for an unknown session. Returns `None`.
    pub poll_step_for_unknown_id_attempts: Vec<SessionId>,
    /// Every `poll_revocation` for an unknown session. Returns `None`.
    pub poll_revocation_for_unknown_id_attempts: Vec<SessionId>,
    /// Drives attempted for unknown sessions; each is refused as `OutOfOrder`.
    pub drive_step_for_unknown_id_attempts: Vec<(SessionId, HandshakeStep)>,
    /// The parent handle named for each session, last write winning.
    pub parents: std::collections::HashMap<SessionId, ParentHandle>,
}

impl MockSessionBackend {
    /// A backend that creates every session and replies only when scripted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend whose `create_session` fails with `e`.
    #[must_use]
    pub fn failing(e: SessionError) -> Self {
        Self {
            create_error: Some(e),
            ..Self::default()
        }
    }

    /// Makes the next `n` drives fail with queue pressure without advancing the session.
    pub fn exhaust_next_drives(&mut self, n: usize) {
        self.exhausted_drives = n;
    }

    /// Queues a step reply for `session`, delivered by the next matching
    /// `poll_step`.
    pub fn script_step(&mut self, session: SessionId, reply: StepReply) {
        self.scripted.entry(session).or_default().push_back(reply);
    }

    /// Queues an out-of-band revocation for `session`.
    pub fn script_revocation(&mut self, session: SessionId, revocation: Revocation) {
        self.revocations
            .entry(session)
            .or_default()
            .push_back(revocation);
    }

    /// Returns the number of effective closes recorded for `session` (zero or one).
    #[must_use]
    pub fn close_count(&self, session: SessionId) -> usize {
        self.closed.iter().filter(|(s, _)| *s == session).count()
    }

    fn next_step_for(&self, session: SessionId) -> Option<HandshakeStep> {
        match self.last_step.get(&session) {
            None => Some(HandshakeStep::SelectSources),
            Some(HandshakeStep::SelectSources) => Some(HandshakeStep::Start),
            Some(_) => None,
        }
    }
}

impl SessionBackend for MockSessionBackend {
    fn create_session(&mut self) -> Result<SessionId, SessionError> {
        if let Some(e) = self.create_error {
            return Err(e);
        }
        self.next_id += 1;
        let id = SessionId::new(self.next_id);
        self.created.push(id);
        Ok(id)
    }

    fn drive_step(&mut self, session: SessionId, step: HandshakeStep) -> Result<(), SessionError> {
        if !self.created.contains(&session) {
            self.drive_step_for_unknown_id_attempts
                .push((session, step));
            return Err(SessionError::OutOfOrder);
        }
        if self.next_step_for(session) != Some(step) {
            return Err(SessionError::OutOfOrder);
        }
        if self.exhausted_drives > 0 {
            self.exhausted_drives -= 1;
            return Err(SessionError::ResourceExhausted);
        }
        self.driven.push((session, step));
        self.in_flight.insert(session, step);
        Ok(())
    }

    fn poll_step(&mut self, session: SessionId) -> Option<StepReply> {
        if !self.created.contains(&session) {
            self.poll_step_for_unknown_id_attempts.push(session);
            return None;
        }
        let reply = self.scripted.get_mut(&session)?.pop_front()?;
        if matches!(reply, StepReply::Ongoing | StepReply::Streams(_))
            && let Some(step) = self.in_flight.remove(&session)
        {
            self.last_step.insert(session, step);
        }
        Some(reply)
    }

    fn poll_revocation(&mut self, session: SessionId) -> Option<Revocation> {
        if !self.created.contains(&session) {
            self.poll_revocation_for_unknown_id_attempts.push(session);
            return None;
        }
        self.revocations.get_mut(&session)?.pop_front()
    }

    fn set_parent(&mut self, session: SessionId, parent: ParentHandle) {
        let _ = self.parents.insert(session, parent);
    }

    fn close_session(&mut self, session: SessionId, reason: CancelReason) {
        if !self.created.contains(&session) {
            self.close_for_unknown_id_attempts.push((session, reason));
            return;
        }
        if self.closed.iter().any(|(s, _)| *s == session) {
            self.repeat_close_attempts.push((session, reason));
            return;
        }
        self.closed.push((session, reason));
        self.last_step.insert(session, HandshakeStep::Start);
    }

    fn describe(&self) -> String {
        format!(
            "mock session (in-process, no D-Bus) — {} created, {} closed",
            self.created.len(),
            self.closed.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_session_rejects_any_further_step() {
        let mut b = MockSessionBackend::new();
        let id = b.create_session().unwrap();
        b.close_session(id, CancelReason::User);
        assert_eq!(
            b.drive_step(id, HandshakeStep::SelectSources),
            Err(SessionError::OutOfOrder),
            "a closed session must not accept a new step"
        );
    }
}
