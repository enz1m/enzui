//! Blocking, resumable transfer execution.
//!
//! The engine binds resume offsets and writes to one destination descriptor and emits reducer events. It syncs the file before completion and before reporting a pause after writes; a newly created completed destination also syncs its parent directory. Cancellation attempts to remove the destination path, with inode matching enforced on Unix. No network backend is provided by default.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::io_class::IoFailureCategory;

use super::{
    Attempt, BodyCheck, BodyChunk, CancelSignal, ContentCoding, DownloadEvent, DownloadId,
    DownloadTransport, ExpectedBody, FailureKind, Phase, RangeDecision, RangeRequestPlan,
    RangeResponse, TransportError, Validator, decide_range_response,
};

/// What the owner currently wants a running transfer to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferIntent {
    /// Keep moving bytes.
    Run,
    /// Stops at the next chunk boundary and keeps the partial file.
    Pause,
    /// Stops at the next chunk boundary and removes the partial file.
    Cancel,
}

const INTENT_RUN: u8 = 0;
const INTENT_PAUSE: u8 = 1;
const INTENT_CANCEL: u8 = 2;

/// Controls a running transfer.
///
/// A separate cooperative signal lets the transport stop blocking work, while the intent distinguishes pause from cancellation.
#[derive(Debug, Clone)]
pub struct TransferControl {
    intent: Arc<AtomicU8>,
    signal: CancelSignal,
}

impl TransferControl {
    /// A fresh control in [`TransferIntent::Run`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            intent: Arc::new(AtomicU8::new(INTENT_RUN)),
            signal: CancelSignal::new(),
        }
    }

    /// The current intent.
    #[must_use]
    pub fn intent(&self) -> TransferIntent {
        match self.intent.load(Ordering::Acquire) {
            INTENT_PAUSE => TransferIntent::Pause,
            INTENT_CANCEL => TransferIntent::Cancel,
            _ => TransferIntent::Run,
        }
    }

    /// Requests a stop that retains the partial; a prior cancellation remains terminal.
    pub fn pause(&self) {
        let _ = self.intent.compare_exchange(
            INTENT_RUN,
            INTENT_PAUSE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.signal.cancel();
    }

    /// Ask a running transfer to stop and delete its partial file. Terminal
    /// and idempotent; overrides a pending pause.
    pub fn cancel(&self) {
        self.intent.store(INTENT_CANCEL, Ordering::Release);
        self.signal.cancel();
    }

    /// Returns a paused control to running state, or returns `false` without
    /// changing a cancelled control. Call it between runs, once the paused
    /// run has returned, to start the next one. A cancellation racing it stays
    /// terminal: the signal is cancelled again if one lands while re-arming.
    pub fn resume(&self) -> bool {
        if self
            .intent
            .compare_exchange(
                INTENT_PAUSE,
                INTENT_RUN,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return self.intent() == TransferIntent::Run;
        }
        self.signal.rearm();
        if self.intent() == TransferIntent::Cancel {
            self.signal.cancel();
            return false;
        }
        true
    }

    /// The signal handed to the transport, so it can abandon a blocking read.
    #[must_use]
    pub fn signal(&self) -> &CancelSignal {
        &self.signal
    }
}

impl Default for TransferControl {
    fn default() -> Self {
        Self::new()
    }
}

/// A transport that reports [`TransportError::Unavailable`] for every operation.
#[derive(Debug, Clone)]
pub struct UnavailableTransport {
    backend: String,
    hint: String,
}

impl UnavailableTransport {
    /// Name the missing backend and what the user can do about it.
    #[must_use]
    pub fn new(backend: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            hint: hint.into(),
        }
    }

    fn error(&self) -> TransportError {
        TransportError::Unavailable {
            backend: self.backend.clone(),
            hint: self.hint.clone(),
        }
    }
}

impl DownloadTransport for UnavailableTransport {
    fn start(
        &self,
        _url: &str,
        _plan: Option<&RangeRequestPlan>,
        _cancel: &CancelSignal,
    ) -> Result<super::TransportResponseMeta, TransportError> {
        Err(self.error())
    }

    fn read_chunk(&self, _cancel: &CancelSignal) -> Result<BodyChunk, TransportError> {
        Err(self.error())
    }
}

fn io_failure_reason(category: IoFailureCategory) -> &'static str {
    match category {
        IoFailureCategory::Permission => "permission denied",
        IoFailureCategory::NotFound | IoFailureCategory::NotADirectory => {
            "the destination was not found"
        }
        IoFailureCategory::StorageExhausted => "no storage space remaining",
        IoFailureCategory::Unknown => "a write error occurred",
    }
}

/// Why a network transport operation failed, classified at construction so
/// the visible message is bounded and payload-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCategory {
    /// The server returned an unexpected HTTP status.
    UnexpectedStatus,
    /// A connection or network timeout.
    Timeout,
    /// The transport was already occupied.
    AlreadyActive,
    /// A response header exceeded the shared ceiling.
    HeaderLimit,
    /// The destination was refused by the transport's URL policy.
    Blocked,
    /// A network or protocol error not in the above categories.
    Other,
}

impl TransportCategory {
    /// The safe user-facing reason fragment.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::UnexpectedStatus => "the server returned an unexpected response",
            Self::Timeout => "the connection timed out",
            Self::AlreadyActive => "the download transport is already busy",
            Self::HeaderLimit => "a response header limit was exceeded",
            Self::Blocked => "the destination is not allowed",
            Self::Other => "a network error occurred",
        }
    }
}

impl std::fmt::Display for TransportCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// The reason a transfer stopped without completing.
///
/// Use [`message`](Self::message) for visible text; diagnostic details may contain unbounded or unsafe payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// No backend is present to move bytes. See [`UnavailableTransport`].
    BackendUnavailable {
        /// What is missing.
        backend: String,
        /// How to get it.
        hint: String,
    },
    /// The transport failed mid-flight.
    Transport {
        /// Classified failure reason.
        category: TransportCategory,
        /// Full detail for structured diagnostics, not for visible text.
        detail: String,
    },
    /// A local destination or durability operation failed.
    Io {
        /// Shared classification of the local I/O failure.
        category: IoFailureCategory,
        /// Full detail for structured diagnostics, not for visible text.
        detail: String,
    },
    /// The destination path no longer names the descriptor receiving bytes.
    DestinationChanged {
        /// Full detail for structured diagnostics, not for visible text.
        detail: String,
    },
    /// The server's answer to a range request could not be used. The partial
    /// file is untouched.
    Range(String),
    /// The body ended before its declared length; the partial remains available for resumption.
    ShortBody {
        /// Octets the response promised.
        expected: u64,
        /// Octets that arrived.
        got: u64,
    },
    /// The response body contained more octets than declared.
    OverlongBody {
        /// Octets the response promised.
        expected: u64,
        /// Octets that arrived.
        got: u64,
    },
}

impl TransferError {
    /// Construct an [`Io`](TransferError::Io) variant from a
    /// [`std::io::Error`], classifying at construction.
    #[must_use]
    pub fn from_io(err: &std::io::Error) -> Self {
        Self::Io {
            category: IoFailureCategory::classify(err),
            detail: err.to_string(),
        }
    }

    /// Formats the transfer error.
    ///
    /// `Io`, `DestinationChanged`, and `Transport` use bounded category text; `BackendUnavailable` and `Range` preserve caller-provided text.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::BackendUnavailable { backend, hint } => {
                crate::localized!("enzui-transfer-no-backend", "backend" => backend, "hint" => hint)
            }
            Self::Transport { category, .. } => crate::localization::safe_text(match category {
                TransportCategory::UnexpectedStatus => "enzui-transfer-unexpected-response",
                TransportCategory::Timeout => "enzui-transfer-timeout",
                TransportCategory::AlreadyActive => "enzui-transfer-busy",
                TransportCategory::HeaderLimit => "enzui-transfer-header-limit",
                TransportCategory::Blocked => "enzui-transfer-blocked",
                TransportCategory::Other => "enzui-transfer-network-error",
            }),
            Self::Io { category, .. } => crate::localization::safe_text(match category {
                IoFailureCategory::Permission => "enzui-transfer-permission",
                IoFailureCategory::NotFound | IoFailureCategory::NotADirectory => {
                    "enzui-transfer-missing-destination"
                }
                IoFailureCategory::StorageExhausted => "enzui-transfer-storage-full",
                IoFailureCategory::Unknown => "enzui-transfer-write-error",
            }),
            Self::DestinationChanged { .. } => {
                crate::localization::safe_text("enzui-transfer-destination-changed")
            }
            Self::Range(reason) => {
                crate::localized!("enzui-transfer-resume-refused", "reason" => reason)
            }
            Self::ShortBody { expected, got } => {
                crate::localized!("enzui-transfer-short-body", "expected" => expected, "received" => got)
            }
            Self::OverlongBody { expected, got } => {
                crate::localized!("enzui-transfer-overlong-body", "expected" => expected, "received" => got)
            }
        }
    }

    /// Returns retained diagnostic or compatibility detail.
    ///
    /// `Io`, `DestinationChanged`, and `Transport` details may contain paths, errno text, backend text, or control characters and are unsuitable for visible text.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            TransferError::BackendUnavailable { hint, .. } => hint,
            TransferError::Transport { detail, .. } => detail,
            TransferError::Io { detail, .. } | TransferError::DestinationChanged { detail } => {
                detail
            }
            TransferError::Range(detail) => detail,
            TransferError::ShortBody { .. } | TransferError::OverlongBody { .. } => "",
        }
    }

    /// Whether this is the absent-backend case, which apps render differently
    /// (an install hint, not a retry button).
    #[must_use]
    pub fn is_backend_unavailable(&self) -> bool {
        matches!(self, TransferError::BackendUnavailable { .. })
    }

    /// Reports whether retry may succeed; URL-policy refusals are nonretryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        !matches!(
            self,
            TransferError::Transport {
                category: TransportCategory::Blocked,
                ..
            }
        )
    }
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::BackendUnavailable { backend, hint } => {
                write!(f, "No download backend: {backend} is not available. {hint}")
            }
            // Retryability is carried separately in `FailureKind`; display wording must not control the Retry action.
            TransferError::Transport {
                category: TransportCategory::Blocked,
                ..
            } => write!(
                f,
                "Blocked destination: the download was not allowed to reach it"
            ),
            TransferError::Transport { category, .. } => {
                write!(f, "Transfer failed: {}", category.reason())
            }
            TransferError::Io { category, .. } => {
                write!(
                    f,
                    "Could not write the download: {}",
                    io_failure_reason(*category)
                )
            }
            TransferError::DestinationChanged { .. } => {
                f.write_str("Could not write the download: the destination changed during transfer")
            }
            TransferError::Range(msg) => write!(f, "The server refused to resume: {msg}"),
            TransferError::ShortBody { expected, got } => write!(
                f,
                "The download ended early: {got} of {expected} bytes arrived"
            ),
            TransferError::OverlongBody { expected, got } => write!(
                f,
                "The server sent more than it promised: {got} bytes for a {expected}-byte range"
            ),
        }
    }
}

/// How one [`run_transfer`] ended, in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    /// The body ended and the byte contract held (or there was none).
    Completed {
        /// Octets written by this run; resumed transfers report only the remainder.
        bytes_written: u64,
        /// Length of the destination file afterwards.
        bytes_on_disk: u64,
        /// Whether completion was verified against a declared byte count.
        verified: bool,
    },
    /// Stopped at the owner's request, retaining any partial as a resume point.
    Paused {
        /// Octets this run wrote before stopping.
        bytes_written: u64,
        /// Length of the destination file, i.e. the next resume offset.
        bytes_on_disk: u64,
    },
    /// Abandoned at the owner's request.
    Cancelled {
        /// Whether cancellation removed a file at the destination path.
        partial_removed: bool,
    },
    /// Stopped by an error.
    Failed {
        /// What went wrong, renderable.
        error: TransferError,
        /// Length of the destination file, which a retry may resume from.
        bytes_on_disk: u64,
    },
}

impl TransferOutcome {
    /// Returns the implied terminal phase, or `None` for a pause.
    #[must_use]
    pub fn phase(&self) -> Option<Phase> {
        match self {
            TransferOutcome::Completed { .. } => Some(Phase::Complete),
            TransferOutcome::Paused { .. } => None,
            TransferOutcome::Cancelled { .. } => Some(Phase::Cancelled),
            TransferOutcome::Failed { .. } => Some(Phase::Failed),
        }
    }
}

/// What to transfer, and where to put it.
#[derive(Debug)]
pub struct TransferRequest<'a> {
    /// Which download this run belongs to. Stamped on every event.
    pub id: DownloadId,
    /// Which attempt. Stale-attempt events are dropped by the reducer.
    pub attempt: Attempt,
    /// The URL handed to the transport verbatim.
    pub url: &'a str,
    /// Destination path; an existing file is opened once to supply the resume offset and receive appended bytes.
    pub destination: &'a Path,
    /// Stored validator used to make a resumed request conditional.
    pub validator: Option<Validator>,
    /// Previously recorded total used to detect a changed resource size.
    pub stored_total: Option<u64>,
    /// Coding of the partial file, which must match any appended response.
    pub partial_coding: ContentCoding,
}

impl<'a> TransferRequest<'a> {
    /// Creates a request with no stored validator or total and identity coding.
    #[must_use]
    pub fn new(id: DownloadId, attempt: Attempt, url: &'a str, destination: &'a Path) -> Self {
        Self {
            id,
            attempt,
            url,
            destination,
            validator: None,
            stored_total: None,
            partial_coding: ContentCoding::Identity,
        }
    }

    /// Attaches a stored validator; only a valid strong validator enables a conditional resume.
    #[must_use]
    pub fn with_validator(mut self, validator: Option<Validator>) -> Self {
        self.validator = validator;
        self
    }

    /// Attach the previously announced total.
    #[must_use]
    pub fn with_stored_total(mut self, total: Option<u64>) -> Self {
        self.stored_total = total;
        self
    }
}

/// Returns the regular-file length, or zero when metadata is unavailable or the destination is not a regular file.
///
/// A symlink destination reports zero rather than its target's length.
#[must_use]
pub fn partial_len(destination: &Path) -> u64 {
    fs::symlink_metadata(destination)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map_or(0, |m| m.len())
}

/// Returns a range plan for a nonempty regular destination, or `None` otherwise.
#[must_use]
pub fn resume_plan(destination: &Path, validator: Option<&Validator>) -> Option<RangeRequestPlan> {
    resume_plan_from_len(partial_len(destination), validator)
}

/// Runs a blocking transfer and emits exactly one terminal event or one pause event.
///
/// The returned outcome reports the same result and byte counts to the thread owner.
pub fn run_transfer(
    transport: &dyn DownloadTransport,
    request: &TransferRequest<'_>,
    control: &TransferControl,
    mut emit: impl FnMut(DownloadEvent),
) -> TransferOutcome {
    let id = request.id;
    let attempt = request.attempt;
    let dest = request.destination;

    // Bind the resume offset and all writes to one descriptor opened before transport startup, preventing later path substitution from redirecting writes.
    let (existing_file, existing_len) = match open_existing_destination(dest) {
        Ok(Some((file, len))) => (Some(file), len),
        Ok(None) => (None, 0),
        Err(err) => {
            return fail(TransferError::from_io(&err), dest, id, attempt, &mut emit);
        }
    };

    // A pre-start stop does not call the transport; cancellation passes only a previously opened destination to cleanup.
    match control.intent() {
        TransferIntent::Pause => {
            if existing_file
                .as_ref()
                .is_some_and(|file| !path_names_open_file(dest, file))
            {
                return fail(destination_changed_error(), dest, id, attempt, &mut emit);
            }
            let on_disk = existing_len;
            emit(DownloadEvent::Paused {
                id,
                attempt,
                received: on_disk,
            });
            return TransferOutcome::Paused {
                bytes_written: 0,
                bytes_on_disk: on_disk,
            };
        }
        TransferIntent::Cancel => {
            return cancelled(dest, id, attempt, existing_file, &mut emit);
        }
        TransferIntent::Run => {}
    }

    let plan = resume_plan_from_len(existing_len, request.validator.as_ref());

    let meta = match transport.start(request.url, plan.as_ref(), control.signal()) {
        Ok(meta) => meta,
        Err(err) => {
            let path_identity = existing_file
                .as_ref()
                .map(|file| path_names_open_file(dest, file));
            return stop_on_transport_error(
                err,
                StopContext {
                    dest,
                    id,
                    attempt,
                    bytes_written: 0,
                    bytes_on_disk: existing_len,
                    control,
                    partial_file: existing_file,
                    path_identity,
                },
                &mut emit,
            );
        }
    };

    emit(DownloadEvent::ResponseMeta {
        id,
        attempt,
        meta: meta.clone(),
    });

    let (start_offset, expected, truncate) = match plan.as_ref() {
        Some(plan) => {
            let response = RangeResponse {
                status: meta.status,
                content_range: meta.content_range.as_deref(),
                response_validator: meta.response_validator.as_ref(),
                content_length: meta.content_length,
                content_encoding: meta.content_encoding.as_deref(),
            };
            match decide_range_response(
                plan,
                request.stored_total,
                &request.partial_coding,
                &response,
            ) {
                RangeDecision::Resume {
                    append_offset,
                    expected,
                    ..
                } => (append_offset, expected, false),
                RangeDecision::RestartWithBody { expected, .. } => (0, expected, true),
                RangeDecision::DiscardAndRetry { reason, .. } => {
                    return fail(
                        TransferError::Range(format!("{reason:?}")),
                        dest,
                        id,
                        attempt,
                        &mut emit,
                    );
                }
                RangeDecision::Refuse { reason } => {
                    return fail(
                        TransferError::Range(format!("{reason:?}")),
                        dest,
                        id,
                        attempt,
                        &mut emit,
                    );
                }
            }
        }
        None => {
            if !(200..300).contains(&meta.status) {
                return fail(
                    TransferError::Transport {
                        category: TransportCategory::UnexpectedStatus,
                        detail: format!("unexpected status {}", meta.status),
                    },
                    dest,
                    id,
                    attempt,
                    &mut emit,
                );
            }
            let expected = ExpectedBody {
                len: meta.content_length,
                end_exclusive: meta.content_length,
                coding: ContentCoding::from_header(meta.content_encoding.as_deref()),
            };
            (0, expected, true)
        }
    };

    // Reuse the pre-start descriptor and delay creation until response acceptance, so startup or range rejection leaves no empty destination.
    let (mut file, created_entry) =
        match open_destination(dest, start_offset, truncate, existing_file) {
            Ok(opened) => opened,
            Err(err) => {
                return fail(err, dest, id, attempt, &mut emit);
            }
        };

    let mut body_bytes: u64 = 0;
    let mut on_disk: u64 = start_offset;

    loop {
        match control.intent() {
            TransferIntent::Pause => {
                // The partial file becomes the next run's resume offset, so it
                // has to be durable before it is reported as one.
                if let Err(err) = sync_destination(&file) {
                    drop(file);
                    return fail(TransferError::from_io(&err), dest, id, attempt, &mut emit);
                }
                if !path_names_open_file(dest, &file) {
                    drop(file);
                    return fail(destination_changed_error(), dest, id, attempt, &mut emit);
                }
                drop(file);
                emit(DownloadEvent::Paused {
                    id,
                    attempt,
                    received: on_disk,
                });
                return TransferOutcome::Paused {
                    bytes_written: body_bytes,
                    bytes_on_disk: on_disk,
                };
            }
            TransferIntent::Cancel => {
                return cancelled(dest, id, attempt, Some(file), &mut emit);
            }
            TransferIntent::Run => {}
        }

        match transport.read_chunk(control.signal()) {
            Ok(BodyChunk::Bytes(bytes)) => {
                if let Err(err) = file.write_all(&bytes) {
                    drop(file);
                    return fail(TransferError::from_io(&err), dest, id, attempt, &mut emit);
                }
                let n = bytes.len() as u64;
                body_bytes = body_bytes.saturating_add(n);
                on_disk = on_disk.saturating_add(n);
                emit(DownloadEvent::Progress {
                    id,
                    attempt,
                    received: on_disk,
                });
            }
            Ok(BodyChunk::Done) => break,
            Err(err) => {
                // Sync before classifying an error while pause is pending, because a cancellation-shaped transport error may become a resumable pause.
                if control.intent() == TransferIntent::Pause
                    && let Err(sync_err) = sync_destination(&file)
                {
                    drop(file);
                    return fail(
                        TransferError::from_io(&sync_err),
                        dest,
                        id,
                        attempt,
                        &mut emit,
                    );
                }
                let may_remove = path_names_open_file(dest, &file);
                return stop_on_transport_error(
                    err,
                    StopContext {
                        dest,
                        id,
                        attempt,
                        bytes_written: body_bytes,
                        bytes_on_disk: on_disk,
                        control,
                        partial_file: Some(file),
                        path_identity: Some(may_remove),
                    },
                    &mut emit,
                );
            }
        }
    }

    // Sync before reporting completion; `File::flush` does not establish on-disk durability.
    if let Err(err) = sync_destination(&file) {
        drop(file);
        return fail(TransferError::from_io(&err), dest, id, attempt, &mut emit);
    }
    // On Unix, this check requires the path to name the retained inode and accepts hard links to it; non-Unix performs only existence and descriptor checks.
    if !path_names_open_file(dest, &file) {
        drop(file);
        return fail(destination_changed_error(), dest, id, attempt, &mut emit);
    }

    // An unsized response may complete only as unverified; it cannot be classified as an exact byte match.
    let check = expected.check(body_bytes);
    match check {
        BodyCheck::Exact | BodyCheck::Unverifiable => {
            // A newly created completed entry requires a parent-directory sync in addition to syncing the file.
            if created_entry {
                if let Err(err) = sync_new_directory_entry(dest) {
                    drop(file);
                    return fail(TransferError::from_io(&err), dest, id, attempt, &mut emit);
                }
                if !path_names_open_file(dest, &file) {
                    drop(file);
                    return fail(destination_changed_error(), dest, id, attempt, &mut emit);
                }
            }
            drop(file);
            let verified = check == BodyCheck::Exact;
            emit(DownloadEvent::Finished { id, attempt });
            TransferOutcome::Completed {
                bytes_written: body_bytes,
                bytes_on_disk: on_disk,
                verified,
            }
        }
        BodyCheck::Short { expected, got } => fail(
            TransferError::ShortBody { expected, got },
            dest,
            id,
            attempt,
            &mut emit,
        ),
        BodyCheck::Overlong { expected, got } => fail(
            TransferError::OverlongBody { expected, got },
            dest,
            id,
            attempt,
            &mut emit,
        ),
    }
}

fn sync_destination(file: &fs::File) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(err) = test_sync_fault::take() {
        return Err(err);
    }
    file.sync_all()
}

// Reuse a pre-opened inode, truncate only after accepting the response, and refuse entries that appear during transport startup.
fn open_destination(
    dest: &Path,
    start_offset: u64,
    truncate: bool,
    existing: Option<fs::File>,
) -> Result<(fs::File, bool), TransferError> {
    if let Some(file) = existing {
        if !path_names_open_file(dest, &file) {
            return Err(destination_changed_error());
        }
        return Ok((
            finish_open(file, start_offset, truncate).map_err(|e| TransferError::from_io(&e))?,
            false,
        ));
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| TransferError::from_io(&e))?;
    Ok((
        finish_open(file, start_offset, truncate).map_err(|e| TransferError::from_io(&e))?,
        true,
    ))
}

// A final-component symlink must not be followed, or a resume could be redirected onto another
// inode; the open also stays non-blocking so a planted FIFO cannot stall it.
fn open_existing_destination(dest: &Path) -> std::io::Result<Option<(fs::File, u64)>> {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            enzui_security::platform::libc_o_nofollow()
                | enzui_security::platform::libc_o_nonblock(),
        );
    }
    let file = match options.open(dest) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "download destination is not a regular file",
        ));
    }
    Ok(Some((file, metadata.len())))
}

fn resume_plan_from_len(offset: u64, validator: Option<&Validator>) -> Option<RangeRequestPlan> {
    if offset == 0 {
        return None;
    }
    let conditional = validator
        .cloned()
        .and_then(|v| RangeRequestPlan::conditional(offset, v));
    Some(conditional.unwrap_or_else(|| RangeRequestPlan::unconditional(offset)))
}

#[cfg(unix)]
fn path_names_open_file(path: &Path, file: &fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(opened) = file.metadata() else {
        return false;
    };
    // Compare the pathname without following it, so a symlink substituted here is refused even
    // when it resolves to the retained inode.
    let Ok(current) = fs::symlink_metadata(path) else {
        return false;
    };
    opened.dev() == current.dev() && opened.ino() == current.ino()
}

#[cfg(not(unix))]
fn path_names_open_file(path: &Path, file: &fs::File) -> bool {
    // `std` exposes no portable stable file identity, so non-Unix checks only that the path and retained descriptor still exist.
    path.exists() && file.metadata().is_ok()
}

fn destination_changed_error() -> TransferError {
    TransferError::DestinationChanged {
        detail: "download destination path changed during transfer".into(),
    }
}

fn finish_open(mut file: fs::File, start_offset: u64, truncate: bool) -> std::io::Result<fs::File> {
    if truncate {
        // Delay truncation until response acceptance so a refused response leaves the pre-opened destination unchanged.
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        return Ok(file);
    }
    file.set_len(start_offset)?;
    file.seek(SeekFrom::Start(start_offset))?;
    Ok(file)
}

// Sync the resolved destination's parent, falling back to the lexical path when canonicalization fails.
fn sync_new_directory_entry(dest: &Path) -> std::io::Result<()> {
    let resolved = fs::canonicalize(dest);
    let path = resolved.as_deref().unwrap_or(dest);
    enzui_security::durable::sync_parent_dir(path)?;
    #[cfg(test)]
    test_after_parent_sync::run();
    Ok(())
}

// On Unix, remove a cancelled partial only while the path still names the retained descriptor.
fn cancelled(
    dest: &Path,
    id: DownloadId,
    attempt: Attempt,
    partial: Option<fs::File>,
    emit: &mut impl FnMut(DownloadEvent),
) -> TransferOutcome {
    let removed = match partial {
        Some(ref file) => enzui_security::tempstage::remove_if_path_names_file(dest, file).is_ok(),
        None => false,
    };
    drop(partial);
    emit(DownloadEvent::Cancelled { id, attempt });
    TransferOutcome::Cancelled {
        partial_removed: removed,
    }
}

fn fail(
    error: TransferError,
    dest: &Path,
    id: DownloadId,
    attempt: Attempt,
    emit: &mut impl FnMut(DownloadEvent),
) -> TransferOutcome {
    // Carry retryability as `FailureKind` so reducer behavior does not depend on rendered error text.
    emit(DownloadEvent::Failed {
        id,
        attempt,
        error: error.message(),
        kind: if error.is_retryable() {
            FailureKind::Transient
        } else {
            FailureKind::Blocked
        },
    });
    TransferOutcome::Failed {
        error,
        bytes_on_disk: partial_len(dest),
    }
}

struct StopContext<'a> {
    dest: &'a Path,
    id: DownloadId,
    attempt: Attempt,
    bytes_written: u64,
    bytes_on_disk: u64,
    control: &'a TransferControl,
    // Retained descriptor used to identity-check cancellation cleanup on Unix.
    partial_file: Option<fs::File>,
    // Cached result of the platform-specific path check; cancellation performs its own cleanup check.
    path_identity: Option<bool>,
}

// The shared signal interrupts both stops, so the owner intent decides whether the partial survives.
fn stop_on_transport_error(
    err: TransportError,
    ctx: StopContext<'_>,
    emit: &mut impl FnMut(DownloadEvent),
) -> TransferOutcome {
    let StopContext {
        dest,
        id,
        attempt,
        bytes_written,
        bytes_on_disk,
        control,
        partial_file,
        path_identity,
    } = ctx;
    match err {
        TransportError::Cancelled => match control.intent() {
            TransferIntent::Pause if path_identity == Some(false) => {
                drop(partial_file);
                fail(destination_changed_error(), dest, id, attempt, emit)
            }
            TransferIntent::Pause => {
                drop(partial_file);
                emit(DownloadEvent::Paused {
                    id,
                    attempt,
                    received: bytes_on_disk,
                });
                TransferOutcome::Paused {
                    bytes_written,
                    bytes_on_disk,
                }
            }
            _ => cancelled(dest, id, attempt, partial_file, emit),
        },
        TransportError::Unavailable { backend, hint } => {
            drop(partial_file);
            fail(
                TransferError::BackendUnavailable { backend, hint },
                dest,
                id,
                attempt,
                emit,
            )
        }
        TransportError::Timeout(phase) => {
            drop(partial_file);
            fail(
                TransferError::Transport {
                    category: TransportCategory::Timeout,
                    detail: phase.to_string(),
                },
                dest,
                id,
                attempt,
                emit,
            )
        }
        TransportError::Io(msg) => {
            drop(partial_file);
            fail(
                TransferError::Transport {
                    category: TransportCategory::Other,
                    detail: msg,
                },
                dest,
                id,
                attempt,
                emit,
            )
        }
        TransportError::HeaderLimit(msg) => {
            drop(partial_file);
            fail(
                TransferError::Transport {
                    category: TransportCategory::HeaderLimit,
                    detail: format!("response header limit exceeded: {msg}"),
                },
                dest,
                id,
                attempt,
                emit,
            )
        }
        TransportError::Blocked { reason } => {
            drop(partial_file);
            fail(
                TransferError::Transport {
                    category: TransportCategory::Blocked,
                    detail: format!("destination refused by URL policy: {reason}"),
                },
                dest,
                id,
                attempt,
                emit,
            )
        }
        TransportError::AlreadyActive => {
            drop(partial_file);
            fail(
                TransferError::Transport {
                    category: TransportCategory::AlreadyActive,
                    detail: "download transport is already driving a transfer".into(),
                },
                dest,
                id,
                attempt,
                emit,
            )
        }
    }
}

#[cfg(test)]
pub(crate) mod test_after_parent_sync {
    use std::cell::RefCell;

    thread_local! {
        static ACTION: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn run() {
        if let Some(action) = ACTION.with(|slot| slot.borrow_mut().take()) {
            action();
        }
    }
}

#[cfg(test)]
pub(crate) mod test_sync_fault {
    use std::cell::Cell;
    use std::io;

    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
    }

    pub(crate) fn take() -> Option<io::Error> {
        ARMED.with(|a| {
            if a.replace(false) {
                Some(io::Error::other("injected destination sync failure"))
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod resume_race_tests {
    use super::*;

    /// However a cancellation and a resume interleave, a cancelled control
    /// ends with its signal cancelled, so the transport always stops.
    #[test]
    fn a_cancel_racing_a_resume_stays_terminal() {
        for _ in 0..20_000 {
            let control = TransferControl::new();
            control.pause();
            let racer = control.clone();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let gate = std::sync::Arc::clone(&barrier);
            let cancel = std::thread::spawn(move || {
                gate.wait();
                racer.cancel();
            });
            barrier.wait();
            let _ = control.resume();
            cancel.join().expect("cancel thread");
            assert_eq!(control.intent(), TransferIntent::Cancel);
            assert!(
                control.signal().is_cancelled(),
                "a cancel was lost to a resume"
            );
        }
    }
}
