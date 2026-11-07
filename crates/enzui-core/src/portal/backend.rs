//! Portal request backend abstraction and deterministic in-process mock.
//!
//! Backends start and close requests without blocking and expose replies through
//! polling plus an optional completion signal.

use std::collections::VecDeque;

use crate::portal::CancelReason;
use crate::portal::chooser::ChooserOptions;
use crate::portal::parent::ParentHandle;

/// File-chooser operation requested from a portal backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortalOperation {
    /// `org.freedesktop.portal.FileChooser.OpenFile`, single selection.
    OpenFile,
    /// `OpenFile` with multiple selection.
    OpenMultiple,
    /// `OpenFile` restricted to directories.
    OpenDirectory,
    /// `org.freedesktop.portal.FileChooser.SaveFile`.
    SaveFile,
}

impl PortalOperation {
    /// Whether a successful reply may contain multiple URIs.
    ///
    /// A single-selection reply with more than one URI is rejected as malformed.
    #[must_use]
    pub fn allows_multiple(self) -> bool {
        matches!(self, Self::OpenMultiple)
    }

    /// Returns the portal interface member invoked for this operation.
    #[must_use]
    pub fn member(self) -> &'static str {
        match self {
            Self::OpenFile | Self::OpenMultiple | Self::OpenDirectory => "OpenFile",
            Self::SaveFile => "SaveFile",
        }
    }
}

/// Maximum encoded body a chooser response may carry.
///
/// A transport refuses a larger response before decoding it, so an oversized
/// reply never allocates a typed URI list.
pub const MAX_REPLY_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Maximum number of URIs one successful chooser reply may carry.
pub const MAX_REPLY_URIS: usize = 1024;

/// Maximum byte length of one returned URI.
pub const MAX_REPLY_URI_BYTES: usize = 32 * 1024;

/// Maximum total byte length of the URIs one successful reply may retain.
pub const MAX_REPLY_URIS_TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// Identifier assigned to an in-flight backend request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates an identifier from a backend-assigned numeric value.
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

/// Failure to start a portal request.
///
/// `Initialising` is deferred automatically and `ResourceExhausted` may be
/// retried after progress; other variants are terminal pre-flight failures.
/// This enum is non-exhaustive.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendError {
    /// The portal frontend is not reachable on this host.
    Unavailable,
    /// Reachable, but does not implement this operation at a usable version.
    Unsupported,
    /// Not declared in the application's manifest, so the layer refuses to try.
    NotDeclared,
    /// The portal or bus explicitly refused permission.
    Denied,
    /// A transport error, a malformed reply, a broken connection.
    Failed,
    /// The backend's bounded command queue has no unreserved capacity.
    ///
    /// This is a local, non-blocking resource refusal: no portal request was
    /// started and no in-flight id was recorded. A caller may retry after
    /// earlier requests make progress.
    ResourceExhausted,
    /// The availability probe has not settled.
    ///
    /// This is transient: callers defer the request and retry on a later portal event.
    Initialising,
}

/// Raw portal response and result URIs for a started request.
///
/// Keeping the response code separate ensures `Other` maps to failure rather than denial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendReply {
    /// The raw portal response code.
    pub response: crate::portal::PortalResponse,
    /// URIs returned with a successful reply, verbatim.
    pub uris: Vec<String>,
}

impl BackendReply {
    /// Whether the returned URIs are within [`MAX_REPLY_URIS`],
    /// [`MAX_REPLY_URI_BYTES`], and [`MAX_REPLY_URIS_TOTAL_BYTES`].
    ///
    /// The running total saturates, so an aggregate that would overflow is
    /// refused rather than wrapped. Non-success replies carry no URIs and are
    /// always within limits.
    #[must_use]
    pub fn within_reply_limits(&self) -> bool {
        if self.uris.len() > MAX_REPLY_URIS {
            return false;
        }
        let mut total = 0usize;
        for uri in &self.uris {
            if uri.len() > MAX_REPLY_URI_BYTES {
                return false;
            }
            total = total.saturating_add(uri.len());
            if total > MAX_REPLY_URIS_TOTAL_BYTES {
                return false;
            }
        }
        true
    }
}

/// Backend interface used by typed portal operations.
pub trait PortalBackend {
    /// Starts `operation`, returning an id for the in-flight request.
    ///
    /// Called only after the parent has been resolved, so a backend never has to
    /// decide what to do about a dead window.
    ///
    /// # Errors
    ///
    /// A [`BackendError`] when the backend is initialising, out of resources, or cannot start the
    /// request.
    fn start(&mut self, operation: PortalOperation) -> Result<RequestId, BackendError>;

    /// Starts an operation with chooser options and an encoded parent handle.
    ///
    /// The default implementation ignores both additions and delegates to [`Self::start`].
    /// An empty parent handle is valid and requests an unparented chooser.
    ///
    /// # Errors
    ///
    /// As [`Self::start`].
    fn start_with_options(
        &mut self,
        operation: PortalOperation,
        options: &ChooserOptions,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        let _ = (options, parent);
        self.start(operation)
    }

    /// Polls without blocking, returning `None` until a reply is available.
    fn poll_reply(&mut self, id: RequestId) -> Option<BackendReply>;

    /// Returns an asynchronous completion edge when the backend provides one.
    ///
    /// `None` means the caller must drive progress by pumping the backend.
    fn completion_signal(&self) -> Option<crate::portal::completion::CompletionSignal> {
        None
    }

    /// Closes a request at most once per id on this backend.
    ///
    /// Repeated closes and ids unknown to this backend are harmless no-ops.
    fn close(&mut self, id: RequestId, reason: CancelReason);

    /// Returns a one-line, side-effect-free backend description for diagnostics.
    fn describe(&self) -> String;
}

/// Deterministic in-process portal backend with scripted replies and call records.
///
/// Repeated or unknown closes are recorded but never sent as additional closes.
#[derive(Debug, Default)]
pub struct MockBackend {
    next_id: u64,
    scripted: VecDeque<BackendReply>,
    start_error: Option<BackendError>,
    initialising_starts: usize,
    transient_start_error: Option<BackendError>,
    /// Every request started, in order.
    pub started: Vec<(RequestId, PortalOperation)>,
    /// Requests closed in order with their cancellation routes.
    ///
    /// Repeated closes are omitted here and recorded in [`Self::repeat_close_attempts`].
    pub closed: Vec<(RequestId, CancelReason)>,
    /// Repeated close attempts for ids already in [`Self::closed`]; no additional close is performed.
    pub repeat_close_attempts: Vec<(RequestId, CancelReason)>,
    /// Close attempts for ids this backend did not start; no close is performed.
    pub close_for_unknown_id_attempts: Vec<(RequestId, CancelReason)>,
    /// Poll attempts for ids this backend did not start; each poll returned `None`.
    pub poll_for_unknown_id_attempts: Vec<RequestId>,
    /// Options and parent handles recorded for each started request.
    pub started_options: Vec<(RequestId, ChooserOptions, ParentHandle)>,
}

impl MockBackend {
    /// A backend that starts every request and never replies until scripted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend whose `start` fails with `e`.
    #[must_use]
    pub fn failing(e: BackendError) -> Self {
        Self {
            start_error: Some(e),
            ..Self::default()
        }
    }

    /// Reports [`BackendError::Initialising`] for the first `n` starts, then starts normally.
    #[must_use]
    pub fn initialising_then_ready(n: usize) -> Self {
        Self {
            initialising_starts: n,
            ..Self::default()
        }
    }

    /// Reports [`BackendError::Initialising`] for `n` starts, then returns `e` on every start.
    #[must_use]
    pub fn initialising_then_error(n: usize, e: BackendError) -> Self {
        Self {
            initialising_starts: n,
            start_error: Some(e),
            ..Self::default()
        }
    }

    /// Reports [`BackendError::ResourceExhausted`] for `n` starts, then starts normally.
    #[must_use]
    pub fn exhausted_then_ready(n: usize) -> Self {
        Self {
            initialising_starts: n,
            transient_start_error: Some(BackendError::ResourceExhausted),
            ..Self::default()
        }
    }

    /// Queues a reply to be delivered by the next `poll_reply`.
    pub fn script(&mut self, reply: BackendReply) {
        self.scripted.push_back(reply);
    }

    /// Returns the number of primary close records for `id`, excluding repeated and unknown attempts.
    #[must_use]
    pub fn close_count(&self, id: RequestId) -> usize {
        self.closed.iter().filter(|(i, _)| *i == id).count()
    }

    /// The [`ChooserOptions`] request `id` was started with, if it carried any.
    #[must_use]
    pub fn options_for(&self, id: RequestId) -> Option<&ChooserOptions> {
        self.started_options
            .iter()
            .find(|(i, _, _)| *i == id)
            .map(|(_, o, _)| o)
    }

    /// The [`ParentHandle`] request `id` was started with, if any.
    #[must_use]
    pub fn parent_for(&self, id: RequestId) -> Option<&ParentHandle> {
        self.started_options
            .iter()
            .find(|(i, _, _)| *i == id)
            .map(|(_, _, p)| p)
    }
}

impl PortalBackend for MockBackend {
    fn start(&mut self, operation: PortalOperation) -> Result<RequestId, BackendError> {
        if self.initialising_starts > 0 {
            self.initialising_starts -= 1;
            return Err(self
                .transient_start_error
                .unwrap_or(BackendError::Initialising));
        }
        if let Some(e) = self.start_error {
            return Err(e);
        }
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        self.started.push((id, operation));
        Ok(id)
    }

    fn start_with_options(
        &mut self,
        operation: PortalOperation,
        options: &ChooserOptions,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        let id = self.start(operation)?;
        self.started_options
            .push((id, options.clone(), parent.clone()));
        Ok(id)
    }

    fn poll_reply(&mut self, id: RequestId) -> Option<BackendReply> {
        // Requests from an earlier backend are harmless here: record the unknown id
        // and return `None` rather than panicking.
        if !self.started.iter().any(|(i, _)| *i == id) {
            self.poll_for_unknown_id_attempts.push(id);
            return None;
        }
        self.scripted.pop_front()
    }

    fn close(&mut self, id: RequestId, reason: CancelReason) {
        // Record unknown and repeated closes, but never send or append a second close.
        if !self.started.iter().any(|(i, _)| *i == id) {
            self.close_for_unknown_id_attempts.push((id, reason));
            return;
        }
        if self.closed.iter().any(|(i, _)| *i == id) {
            self.repeat_close_attempts.push((id, reason));
            return;
        }
        self.closed.push((id, reason));
    }

    fn describe(&self) -> String {
        format!(
            "mock (in-process, no D-Bus) — {} started, {} closed",
            self.started.len(),
            self.closed.len()
        )
    }
}
