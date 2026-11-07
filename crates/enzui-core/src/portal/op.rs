//! Cancellable file-portal operations with deferred starts and exactly-once settlement.
//!
//! Parent liveness is checked before backend access. Cancellation records intent, while [`PendingRequest::settle`] performs any owed wire close.

use std::sync::{Arc, Mutex};

use crate::portal::backend::{BackendError, PortalBackend, PortalOperation, RequestId};
use crate::portal::cancel::PortalCancel;
use crate::portal::chooser::ChooserOptions;
use crate::portal::parent::{ParentHandle, ParentResolution, PortalParent};
use crate::portal::pending::{ClaimStart, CloseClaim, PendingLifecycle};
use crate::portal::{CancelReason, PortalOutcome, PortalResponse};

/// A portal-returned URI preserved verbatim, including non-file schemes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileUri(String);

impl FileUri {
    /// Wraps a URI verbatim.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self(uri.into())
    }

    /// The URI, unmodified.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Decodes this URI as a local filesystem path without modifying the stored URI.
    ///
    /// # Errors
    ///
    /// Returns [`UriDecodeError`](crate::portal::uri::UriDecodeError) for non-file, remote-host, or malformed local-file URIs.
    pub fn to_path(&self) -> Result<std::path::PathBuf, crate::portal::uri::UriDecodeError> {
        crate::portal::uri::file_uri_to_path(&self.0)
    }
}

/// What a completed file operation produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileResult {
    /// The chosen URIs, in the order the portal returned them.
    pub uris: Vec<FileUri>,
}

impl FileResult {
    /// Returns the first chosen URI, or `None` when the result is empty.
    #[must_use]
    pub fn first(&self) -> Option<&FileUri> {
        self.uris.first()
    }

    /// Decodes local-file URIs and skips every non-local entry.
    ///
    /// Use [`Self::uris`] with [`FileUri::to_path`] to observe per-entry decoding failures.
    #[must_use]
    pub fn local_paths(&self) -> Vec<std::path::PathBuf> {
        self.uris.iter().filter_map(|u| u.to_path().ok()).collect()
    }

    /// The first chosen local file or folder: what a single-choice request
    /// returns.
    #[must_use]
    pub fn first_local_path(&self) -> Option<std::path::PathBuf> {
        self.uris.iter().find_map(|u| u.to_path().ok())
    }
}

/// A file operation configured with an explicit window or unparented policy.
#[derive(Debug, Clone)]
pub struct FileRequest {
    operation: PortalOperation,
    parent: PortalParent,
    options: ChooserOptions,
    // Encoded transport handle, separate from the parent-liveness policy.
    parent_handle: ParentHandle,
}

impl FileRequest {
    /// Builds an operation with no chooser options and an empty parent handle.
    #[must_use]
    pub fn new(operation: PortalOperation, parent: PortalParent) -> Self {
        Self {
            operation,
            parent,
            options: ChooserOptions::new(),
            parent_handle: ParentHandle::none(),
        }
    }

    /// Attaches chooser options — filters, starting directory, suggested name.
    #[must_use]
    pub fn with_options(mut self, options: ChooserOptions) -> Self {
        self.options = options;
        self
    }

    /// Sets the encoded portal parent handle.
    ///
    /// The default empty handle opens an unparented dialog when platform export is unavailable.
    #[must_use]
    pub fn with_parent_handle(mut self, handle: ParentHandle) -> Self {
        self.parent_handle = handle;
        self
    }

    /// Returns the chooser options sent with this request.
    #[must_use]
    pub fn options(&self) -> &ChooserOptions {
        &self.options
    }

    /// Returns the encoded parent handle sent with this request.
    #[must_use]
    pub fn parent_handle(&self) -> &ParentHandle {
        &self.parent_handle
    }

    /// Returns the requested portal operation.
    #[must_use]
    pub fn operation(&self) -> PortalOperation {
        self.operation
    }

    /// Returns the requested parent policy.
    #[must_use]
    pub fn parent(&self) -> PortalParent {
        self.parent
    }

    /// Resolves parent liveness and starts or defers this request.
    ///
    /// A stale requested parent returns [`PortalOutcome::ParentUnavailable`] before backend access. Options over any bound return [`PortalOutcome::Failed`], also before backend access; [`ChooserOptions::refusal`] names the exact bound without repeating the refused value. An initializing or capacity-limited backend returns a deferred [`PendingRequest`] whose [`PendingRequest::poll`] retries the same payload.
    ///
    /// # Errors
    ///
    /// [`PortalOutcome::ParentUnavailable`] for a dead parent window, `Failed` for options over a
    /// bound, and the backend's terminal refusal.
    pub fn start(
        self,
        backend: &mut dyn PortalBackend,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Result<PendingRequest, PortalOutcome<FileResult>> {
        if let ParentResolution::Unavailable(_) = self.parent.resolve(is_live) {
            return Err(PortalOutcome::ParentUnavailable);
        }
        if self.options.refusal().is_some() {
            return Err(PortalOutcome::Failed);
        }
        match backend.start_with_options(self.operation, &self.options, &self.parent_handle) {
            Ok(id) => {
                let pending = PendingRequest::new(self.operation, id);
                attach_backend_wake(&pending, backend);
                Ok(pending)
            }
            // Defer initialization and queue pressure; neither is a terminal portal outcome.
            Err(BackendError::Initialising | BackendError::ResourceExhausted) => {
                let pending =
                    PendingRequest::deferred(self.operation, self.options, self.parent_handle);
                attach_backend_wake(&pending, backend);
                Ok(pending)
            }
            Err(e) => Err(outcome_for(e)),
        }
    }
}

// Attach the backend completion signal so cross-thread cancellation wakes the owner.
fn attach_backend_wake(pending: &PendingRequest, backend: &dyn PortalBackend) {
    if let Some(signal) = backend.completion_signal() {
        pending.lifecycle.attach_wake(signal);
    }
}

fn outcome_for<T>(e: BackendError) -> PortalOutcome<T> {
    match e {
        BackendError::Unavailable => PortalOutcome::Unavailable,
        BackendError::Unsupported => PortalOutcome::Unsupported,
        BackendError::NotDeclared => PortalOutcome::NotDeclared,
        BackendError::Denied => PortalOutcome::Denied,
        BackendError::Failed => PortalOutcome::Failed,
        // Normally deferred; defensively map leaked queue pressure to operational failure.
        BackendError::ResourceExhausted => PortalOutcome::Failed,
        // Normally deferred; defensively map leaked initialization to unavailable.
        BackendError::Initialising => PortalOutcome::Unavailable,
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct PollClaimGate {
    entered: std::sync::mpsc::SyncSender<()>,
    resume: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

#[cfg(test)]
impl PollClaimGate {
    fn pause(&self) {
        self.entered
            .send(())
            .expect("poll-claim test observer must remain alive");
        self.resume
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("poll-claim test must release its deterministic barrier");
    }
}

/// A live or deferred file-portal request.
///
/// Clones share one logical request, cancellation state, eventual wire id, and exactly-once close obligation.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    operation: PortalOperation,
    lifecycle: PendingLifecycle<RequestId>,
    retry: Arc<Mutex<Option<(ChooserOptions, ParentHandle)>>>,
    #[cfg(test)]
    poll_claim_gate: Option<PollClaimGate>,
}

impl PendingRequest {
    fn new(operation: PortalOperation, id: RequestId) -> Self {
        Self {
            operation,
            lifecycle: PendingLifecycle::live(id),
            retry: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            poll_claim_gate: None,
        }
    }

    fn deferred(
        operation: PortalOperation,
        options: ChooserOptions,
        parent_handle: ParentHandle,
    ) -> Self {
        Self {
            operation,
            lifecycle: PendingLifecycle::no_wire(),
            retry: Arc::new(Mutex::new(Some((options, parent_handle)))),
            #[cfg(test)]
            poll_claim_gate: None,
        }
    }

    fn retry_take(&self) -> Option<(ChooserOptions, ParentHandle)> {
        self.retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn retry_restore(&self, payload: (ChooserOptions, ParentHandle)) {
        *self
            .retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(payload);
    }

    // Clones share one terminal outcome, so only the clone that takes the
    // one-shot report delivers it.
    fn report(&self, outcome: PortalOutcome<FileResult>) -> Option<PortalOutcome<FileResult>> {
        self.lifecycle.claim_report().then_some(outcome)
    }

    fn report_cancelled(&self) -> Option<PortalOutcome<FileResult>> {
        let reason = self.lifecycle.outcome_reason()?;
        self.report(PortalOutcome::Cancelled(reason))
    }

    /// Takes the one-shot right to report a terminal state recorded elsewhere.
    ///
    /// Returns the cancellation route for the caller that wins, and `None` while
    /// the request is live or once any clone has reported. Use this rather than
    /// [`Self::cancel_reason`] when the value is about to be delivered as an
    /// outcome; the close obligation is claimed separately by [`Self::settle`].
    pub fn claim_cancel_report(&self) -> Option<CancelReason> {
        let reason = self.lifecycle.outcome_reason()?;
        self.lifecycle.claim_report().then_some(reason)
    }

    /// Returns the wire request id once started.
    ///
    /// Returns `None` while start is deferred; a published id remains available after terminal completion.
    #[must_use]
    pub fn id(&self) -> Option<RequestId> {
        self.started_id()
    }

    /// Returns the wire request id, if one was published.
    ///
    /// Presence does not imply the remote request is still live.
    #[must_use]
    pub fn started_id(&self) -> Option<RequestId> {
        self.lifecycle.wire_id()
    }

    /// Returns whether a wire request has started, including a dialog awaiting its reply.
    #[must_use]
    pub fn has_started(&self) -> bool {
        self.started_id().is_some()
    }

    /// Returns the portal operation for this request.
    #[must_use]
    pub fn operation(&self) -> PortalOperation {
        self.operation
    }

    /// Returns a `Send` cancellation handle sharing this request’s terminal state and close obligation.
    #[must_use]
    pub fn cancel_handle(&self) -> PortalCancel {
        self.lifecycle.cancel_handle()
    }

    /// Returns whether no terminal event has been recorded.
    ///
    /// A cancelled request may return `false` while still requiring [`Self::settle`].
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.lifecycle.is_pending()
    }

    /// Returns whether this request is pending or still owes settlement.
    ///
    /// It becomes false after reply consumption or after [`Self::settle`] claims the close obligation.
    #[must_use]
    pub fn is_unsettled(&self) -> bool {
        // Treat a still-pending terminal writer as unsettled after lifecycle sealing.
        self.is_pending() || !self.lifecycle.is_finished()
    }

    /// The cancellation route, or `None` if live or completed.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.lifecycle.outcome_reason()
    }

    /// Records cancellation and returns its outcome only if this call wins.
    ///
    /// This is the canceller's own answer and does not consume the one-shot
    /// delivery claim, so a reducer driven by [`Self::poll`] still receives the
    /// terminal outcome exactly once. Call [`Self::settle`] separately to
    /// perform any owed wire close.
    pub fn cancel(&mut self, reason: CancelReason) -> Option<PortalOutcome<FileResult>> {
        self.lifecycle
            .record_cancel(reason)
            .then_some(PortalOutcome::Cancelled(reason))
    }

    /// Retries a deferred start or polls the current wire request.
    ///
    /// Returns one terminal outcome when available, to the first clone that asks; every later clone receives `None`. A reply arriving after cancellation is discarded, as is a reply over any [`BackendReply`](crate::portal::backend::BackendReply) URI bound.
    #[allow(
        clippy::unwrap_in_result,
        reason = "the deferred state always carries its retry payload; losing it is a bug, not a \
                  pending request"
    )]
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: the deferred state always carries its retry \
                  payload"
    )]
    pub fn poll(&mut self, backend: &mut dyn PortalBackend) -> Option<PortalOutcome<FileResult>> {
        if !self.lifecycle.is_pending() {
            return None;
        }

        #[cfg(test)]
        if let Some(gate) = &self.poll_claim_gate {
            gate.pause();
        }

        // Claim deferred start under the lifecycle lock so clones cannot start twice or race no-wire settlement.
        match self.lifecycle.try_claim_start() {
            ClaimStart::Claimed => {
                let (options, parent) = self
                    .retry_take()
                    .expect("NoWire deferred request always carries a retry payload");
                match backend.start_with_options(self.operation, &options, &parent) {
                    Ok(id) => {
                        self.lifecycle.publish_started(id);
                        // If cancellation wins during start, publish the new id so settlement can close it.
                        return self.report_cancelled();
                    }
                    Err(BackendError::Initialising | BackendError::ResourceExhausted) => {
                        // Restore the payload before publishing `NoWire` to racing clones.
                        self.retry_restore((options, parent));
                        self.lifecycle.publish_deferred();
                        return self.report_cancelled();
                    }
                    Err(e) => {
                        self.lifecycle.publish_terminal_no_wire();
                        // Preserve overlapping cancellation when start fails without creating a wire request.
                        if self.lifecycle.cancel().complete() {
                            return self.report(outcome_for(e));
                        }
                        return self.report_cancelled();
                    }
                }
            }
            ClaimStart::CancelledBeforeStart | ClaimStart::Locked | ClaimStart::Finished => {
                return None;
            }
            ClaimStart::AlreadyLive => {}
        }

        let id = self.lifecycle.try_claim_poll()?;
        let Some(reply) = backend.poll_reply(id) else {
            self.lifecycle.release_poll();
            // Recheck cancellation after an empty backend poll.
            return self.report_cancelled();
        };

        // Seal the wire lifecycle after consuming a terminal reply; no Close remains valid.
        self.lifecycle.commit_terminal_reply();
        if !self.lifecycle.cancel().complete() {
            return self.report_cancelled();
        }

        // Admit the whole reply before any URI is retained, so an over-bound
        // answer yields no partial grant list.
        if reply.response == PortalResponse::Success && !reply.within_reply_limits() {
            return self.report(PortalOutcome::Failed);
        }

        // Single-selection success requires exactly one URI.
        if reply.response == PortalResponse::Success
            && reply.uris.len() != 1
            && !self.operation.allows_multiple()
        {
            return self.report(PortalOutcome::Failed);
        }

        self.report(PortalOutcome::from_response(
            reply.response,
            FileResult {
                uris: reply.uris.into_iter().map(FileUri::new).collect(),
            },
        ))
    }

    /// Claims and performs an owed wire close at most once.
    ///
    /// Returns the cancellation reason that caused the close, or `None` while live, after normal completion, or after an earlier settlement.
    pub fn settle(&mut self, backend: &mut dyn PortalBackend) -> Option<CancelReason> {
        match self.lifecycle.claim_close()? {
            CloseClaim::Wire(id, reason) => {
                backend.close(id, reason);
                Some(reason)
            }
            // Seal a cancelled deferred request without issuing a wire Close.
            CloseClaim::NoWire(_) => None,
        }
    }
}
