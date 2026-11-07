//! Portal-mediated download destinations.
//!
//! A destination can be created only from a completed `SaveFile` portal result, so the user-selected URI bounds write authority. Cancellation, denial, capability refusal, and operational failure remain distinct outcomes.
//!
//! On a host without a file-chooser portal, request through
//! `enzui_widgets::file_chooser::FallbackBackend`: the in-app chooser asks the
//! user, and its answer completes the request as a portal's would.

use crate::download::DownloadId;
use crate::portal::backend::{PortalBackend, PortalOperation};
use crate::portal::chooser::ChooserOptions;
use crate::portal::completion::{CompletionSignal, PortalWake};
use crate::portal::op::{FileRequest, FileResult, PendingRequest};
use crate::portal::{CancelReason, PortalOutcome, PortalParent};

use iced_futures::Subscription;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
fn remove_reserved_file(parent: &File, path: &Path, file: &File) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "download destination has no file name",
        )
    })?;
    enzui_security::tempstage::remove_if_parent_entry_names_file(parent, name, file)
}

#[cfg(not(unix))]
fn remove_reserved_file(_parent: &File, path: &Path, file: &File) -> io::Result<()> {
    enzui_security::tempstage::remove_if_path_names_file(path, file)
}

/// A destination chosen by the user through the portal.
///
/// The URI is preserved verbatim, and no public path-based constructor is provided.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadDestination(String);

impl DownloadDestination {
    /// The destination URI, unmodified.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.0
    }

    /// Decodes a local `file://` grant into its writable path without creating a path-to-grant conversion.
    ///
    /// # Errors
    ///
    /// Returns [`UriDecodeError`](crate::portal::uri::UriDecodeError) for malformed or non-local file URIs.
    pub fn to_path(&self) -> Result<PathBuf, crate::portal::uri::UriDecodeError> {
        crate::portal::uri::file_uri_to_path(&self.0)
    }

    /// Reserves a new local file while retaining the inode and initial
    /// metadata authorized by this grant.
    ///
    /// On Unix the empty reservation is normalized to mode `0644` through its
    /// open descriptor before publication can write user data.
    ///
    /// # Errors
    ///
    /// Returns an error for non-local, relative, occupied, or uncreatable destinations.
    pub fn reserve_file(&self) -> io::Result<ReservedDownloadDestination> {
        let path = self
            .to_path()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "download destination is not an absolute local path",
            ));
        }
        let parent_path = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "download destination has no parent",
            )
        })?;
        let parent = File::open(parent_path)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        let metadata = match crate::user_file::normalize_and_capture_document_metadata(&file) {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = remove_reserved_file(&parent, &path, &file);
                return Err(error);
            }
        };
        if let Err(error) = enzui_security::tempstage::verify_path_names_file(&parent, parent_path)
            .and_then(|()| enzui_security::tempstage::verify_path_names_file(&file, &path))
        {
            let _ = remove_reserved_file(&parent, &path, &file);
            return Err(error);
        }
        Ok(ReservedDownloadDestination {
            path,
            file: Arc::new(file),
            metadata,
            parent: Arc::new(parent),
        })
    }
}

/// A portal-granted file reservation whose open inode authorizes later publication.
#[derive(Debug, Clone)]
pub struct ReservedDownloadDestination {
    path: PathBuf,
    file: Arc<File>,
    metadata: crate::user_file::RetainedMetadata,
    parent: Arc<File>,
}

impl ReservedDownloadDestination {
    /// The local path represented by the retained reservation.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomically replaces the reservation only if its pathname still names the retained inode.
    ///
    /// # Errors
    ///
    /// Refuses identity, mode, or ownership changes made after reservation.
    pub fn publish_with(
        &self,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<(), crate::user_file::SaveError> {
        crate::user_file::save_with_expected_destination(
            &self.path,
            &self.file,
            self.metadata,
            &self.parent,
            write,
        )
    }

    /// Removes the reservation through its retained parent directory.
    ///
    /// Replacing the parent pathname cannot redirect cleanup to another tree.
    /// The entry identity is checked immediately before the descriptor-relative
    /// unlink, with the residual POSIX check-to-unlink window.
    ///
    /// # Errors
    ///
    /// Refuses to remove a substituted entry and propagates filesystem failures.
    pub fn remove_if_reserved(&self) -> io::Result<()> {
        remove_reserved_file(&self.parent, &self.path, &self.file)
    }
}

/// The typed outcome of requesting a user-selected download destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationOutcome {
    /// The user chose a destination.
    Completed(DownloadDestination),
    /// The request was cancelled by the user, application, or closing parent; cancellation is not an error.
    Cancelled(CancelReason),
    /// The user was asked and explicitly refused. A decision, not a fault.
    Denied,
    /// The capability was never declared in the manifest, so the request was
    /// refused before any dialog opened.
    NotDeclared,
    /// The portal is reachable but does not implement `SaveFile`.
    Unsupported,
    /// No portal frontend is reachable on this host.
    Unavailable,
    /// The requested parent could not be resolved, and the request was not downgraded to unparented.
    ParentUnavailable,
    /// An operational failure: transport error, malformed response, or a
    /// response code that is neither success nor user-cancellation.
    Failed,
}

impl DestinationOutcome {
    /// Returns whether the outcome is an operational error.
    ///
    /// Completion, every cancellation route, and explicit denial return `false`.
    #[must_use]
    pub fn is_error(&self) -> bool {
        match self {
            Self::Completed(_) | Self::Cancelled(_) | Self::Denied => false,
            Self::NotDeclared
            | Self::Unsupported
            | Self::Unavailable
            | Self::ParentUnavailable
            | Self::Failed => true,
        }
    }

    /// The chosen destination, if the user chose one.
    #[must_use]
    pub fn destination(&self) -> Option<&DownloadDestination> {
        match self {
            Self::Completed(d) => Some(d),
            _ => None,
        }
    }

    // Accept a successful save result only when it contains exactly one URI.
    fn from_portal(outcome: PortalOutcome<FileResult>) -> Self {
        match outcome {
            PortalOutcome::Completed(result) => match result.uris.as_slice() {
                [one] => Self::Completed(DownloadDestination(one.as_str().to_owned())),
                _ => Self::Failed,
            },
            PortalOutcome::Cancelled(reason) => Self::Cancelled(reason),
            PortalOutcome::Denied => Self::Denied,
            PortalOutcome::NotDeclared => Self::NotDeclared,
            PortalOutcome::Unsupported => Self::Unsupported,
            PortalOutcome::Unavailable => Self::Unavailable,
            PortalOutcome::ParentUnavailable => Self::ParentUnavailable,
            PortalOutcome::Failed => Self::Failed,
        }
    }
}

/// An in-flight destination request backed by a portal [`PendingRequest`].
#[derive(Debug, Clone)]
pub struct DestinationRequest {
    pending: PendingRequest,
}

impl DestinationRequest {
    /// Starts a `SaveFile` request for a user-selected destination.
    ///
    /// Requests that fail preflight return a terminal outcome without opening a dialog.
    ///
    /// # Errors
    ///
    /// Returns the terminal [`DestinationOutcome`] when the request cannot start.
    pub fn start(
        backend: &mut dyn PortalBackend,
        parent: PortalParent,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Result<Self, DestinationOutcome> {
        Self::start_with_options(backend, parent, ChooserOptions::new(), is_live)
    }

    /// Starts a `SaveFile` request with native chooser presentation hints.
    ///
    /// A suggested name does not authorize the returned location.
    ///
    /// # Errors
    ///
    /// Returns the terminal [`DestinationOutcome`] when the request cannot start.
    pub fn start_with_options(
        backend: &mut dyn PortalBackend,
        parent: PortalParent,
        options: ChooserOptions,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Result<Self, DestinationOutcome> {
        match FileRequest::new(PortalOperation::SaveFile, parent)
            .with_options(options)
            .start(backend, is_live)
        {
            Ok(pending) => Ok(Self { pending }),
            Err(outcome) => Err(DestinationOutcome::from_portal(outcome)),
        }
    }

    /// Returns whether no terminal outcome has been recorded.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending.is_pending()
    }

    /// Returns whether the portal request has started rather than remaining deferred during availability probing.
    #[must_use]
    pub fn has_started(&self) -> bool {
        self.pending.has_started()
    }

    /// Returns the recorded cancellation route, including cancellation before a dialog started.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.pending.cancel_reason()
    }

    /// Takes the one-shot right to report a cancellation recorded elsewhere.
    ///
    /// Returns the route for the caller that wins and `None` once any holder of
    /// this request has reported it.
    #[must_use]
    pub fn claim_cancel_report(&self) -> Option<CancelReason> {
        self.pending.claim_cancel_report()
    }

    /// A `Send` clone of the cancellation handle, for a task or another thread.
    #[must_use]
    pub fn cancel_handle(&self) -> crate::portal::cancel::PortalCancel {
        self.pending.cancel_handle()
    }

    /// Cancels the request and records `reason`.
    ///
    /// Returns the terminal outcome only if cancellation won the reply race. Any owed close is performed later by [`settle`](Self::settle).
    pub fn cancel(&mut self, reason: CancelReason) -> Option<DestinationOutcome> {
        self.pending
            .cancel(reason)
            .map(DestinationOutcome::from_portal)
    }

    /// Polls for a portal outcome, returning `None` while pending and discarding replies received after cancellation.
    pub fn poll(&mut self, backend: &mut dyn PortalBackend) -> Option<DestinationOutcome> {
        self.pending
            .poll(backend)
            .map(DestinationOutcome::from_portal)
    }

    /// Performs an owed portal close at most once.
    ///
    /// `None` means no close is owed and does not indicate whether the request is pending; use [`is_pending`](Self::is_pending) for lifecycle state.
    pub fn settle(&mut self, backend: &mut dyn PortalBackend) -> Option<CancelReason> {
        self.pending.settle(backend)
    }
}

/// Tracks portal destination requests and grants by download id.
///
/// [`destination`](Self::destination) remains `None` until the user grants a URI, and terminal requests are settled by [`pump`](Self::pump).
#[derive(Debug, Default)]
pub struct DestinationBroker {
    pending: Vec<(DownloadId, DestinationRequest)>,
    granted: Vec<(DownloadId, DownloadDestination)>,
    // Retain the last terminal outcome independently of granted destinations.
    outcomes: Vec<(DownloadId, DestinationOutcome)>,
    closes: usize,
    // The allocation keeps subscription identity stable across broker moves and unique while old streams live.
    subscription_source: std::sync::Arc<()>,
    // Incrementing for each request forces iced to replace the prior completion recipe.
    generation: u64,
}

impl DestinationBroker {
    /// An empty broker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests a save destination for `id`.
    ///
    /// Existing pending requests or grants make this a no-op. Preflight refusal returns and records a terminal outcome without opening a dialog.
    pub fn request(
        &mut self,
        id: DownloadId,
        backend: &mut dyn PortalBackend,
        parent: PortalParent,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Option<DestinationOutcome> {
        self.request_with_options(id, backend, parent, ChooserOptions::new(), is_live)
    }

    /// Requests a save destination with chooser hints.
    ///
    /// Suggested names do not grant a location, and duplicate requests remain no-ops.
    pub fn request_with_options(
        &mut self,
        id: DownloadId,
        backend: &mut dyn PortalBackend,
        parent: PortalParent,
        options: ChooserOptions,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Option<DestinationOutcome> {
        if self.is_pending(id) || self.destination(id).is_some() {
            return None;
        }
        match DestinationRequest::start_with_options(backend, parent, options, is_live) {
            Ok(req) => {
                self.pending.push((id, req));
                self.generation = self.generation.wrapping_add(1);
                None
            }
            Err(outcome) => {
                self.record_outcome(id, outcome.clone());
                Some(outcome)
            }
        }
    }

    /// Polls all live requests and settles terminal ones.
    ///
    /// Newly terminal outcomes are returned in request order; only completed outcomes store a destination.
    pub fn pump(
        &mut self,
        backend: &mut dyn PortalBackend,
    ) -> Vec<(DownloadId, DestinationOutcome)> {
        let mut settled = Vec::new();
        let mut still_pending = Vec::new();
        for (id, mut req) in std::mem::take(&mut self.pending) {
            match req.poll(backend) {
                Some(outcome) => {
                    if req.settle(backend).is_some() {
                        self.closes += 1;
                    }
                    settled.push((id, outcome));
                }
                None if req.is_pending() => still_pending.push((id, req)),
                None => {
                    if req.settle(backend).is_some() {
                        self.closes += 1;
                    }
                    // Claiming rather than reading the reason keeps one report per
                    // request: a deferred cancel owes no close, so settlement
                    // cannot decide who reports it.
                    if let Some(reason) = req.claim_cancel_report() {
                        settled.push((id, DestinationOutcome::Cancelled(reason)));
                    }
                }
            }
        }
        self.pending = still_pending;
        for (id, outcome) in &settled {
            self.record_outcome(*id, outcome.clone());
        }
        settled
    }

    /// Cancels the live request for `id`; the next [`pump`](Self::pump) performs any owed close.
    ///
    /// Returns the terminal outcome only when cancellation wins the reply race.
    pub fn cancel(&mut self, id: DownloadId, reason: CancelReason) -> Option<DestinationOutcome> {
        let req = self
            .pending
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, r)| r)?;
        req.cancel(reason)
    }

    /// Returns the user-granted destination for `id`, or `None` until a grant exists.
    #[must_use]
    pub fn destination(&self, id: DownloadId) -> Option<&DownloadDestination> {
        self.granted.iter().find(|(i, _)| *i == id).map(|(_, d)| d)
    }

    /// Returns whether `id` has a request with no recorded terminal outcome.
    #[must_use]
    pub fn is_pending(&self, id: DownloadId) -> bool {
        self.pending.iter().any(|(i, r)| *i == id && r.is_pending())
    }

    /// Returns whether `id` has started its portal request rather than remaining deferred.
    #[must_use]
    pub fn is_started(&self, id: DownloadId) -> bool {
        self.pending
            .iter()
            .any(|(i, r)| *i == id && r.has_started())
    }

    /// Returns the last terminal outcome for `id`, allowing cancellation and operational failure to be distinguished.
    #[must_use]
    pub fn outcome(&self, id: DownloadId) -> Option<&DestinationOutcome> {
        self.outcomes.iter().find(|(i, _)| *i == id).map(|(_, o)| o)
    }

    /// Returns the number of portal close calls performed by this broker.
    #[must_use]
    pub fn closes(&self) -> usize {
        self.closes
    }

    /// Returns whether the broker owns a request that has not yet been polled, settled, and removed.
    ///
    /// This can remain true after cancellation while a portal close or terminal report is still owed.
    #[must_use]
    pub fn has_unsettled_request(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Subscribes to portal events while this broker owns an unsettled request.
    ///
    /// Returns no subscription without unsettled work or an asynchronous backend. The broker identity and request generation provide a stable iced recipe key.
    pub fn completion_subscription(
        &self,
        backend: Option<&dyn PortalBackend>,
    ) -> Subscription<PortalWake> {
        if !self.has_unsettled_request() {
            return Subscription::none();
        }
        let Some(signal) = backend.and_then(PortalBackend::completion_signal) else {
            return Subscription::none();
        };
        Subscription::run_with(
            DestinationCompletionKey {
                source: Arc::clone(&self.subscription_source),
                generation: self.generation,
                signal,
            },
            |data| {
                let signal = data.signal.clone();
                iced_futures::futures::stream::unfold(
                    (signal, None::<u64>),
                    async |(signal, seen): (CompletionSignal, Option<u64>)| match seen {
                        None => {
                            let generation = signal.generation();
                            Some((PortalWake { generation }, (signal, Some(generation))))
                        }
                        Some(seen) => {
                            let generation = signal.wait_after(seen).await;
                            Some((PortalWake { generation }, (signal, Some(generation))))
                        }
                    },
                )
            },
        )
    }

    fn record_outcome(&mut self, id: DownloadId, outcome: DestinationOutcome) {
        if let DestinationOutcome::Completed(dest) = &outcome {
            match self.granted.iter_mut().find(|(i, _)| *i == id) {
                Some((_, slot)) => *slot = dest.clone(),
                None => self.granted.push((id, dest.clone())),
            }
        }
        match self.outcomes.iter_mut().find(|(i, _)| *i == id) {
            Some((_, slot)) => *slot = outcome,
            None => self.outcomes.push((id, outcome)),
        }
    }
}

// Key one broker completion source and request generation for iced subscription deduplication.
#[derive(Clone)]
struct DestinationCompletionKey {
    source: std::sync::Arc<()>,
    generation: u64,
    signal: CompletionSignal,
}

impl std::hash::Hash for DestinationCompletionKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::sync::Arc::as_ptr(&self.source).hash(state);
        // The signal's changing generation is data, not identity.
        self.generation.hash(state);
    }
}

#[cfg(test)]
mod tests;
