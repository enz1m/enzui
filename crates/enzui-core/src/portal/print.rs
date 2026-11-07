//! Print-portal requests over owned PDF or PostScript descriptors.
//!
//! Requests either prepare a new token or reuse a prior token. The document
//! descriptor remains owned through the exchange and is normalized
//! close-on-exec.

use std::os::fd::{AsRawFd, OwnedFd};

use super::parent::ParentHandle;

// Adopted print document descriptors must be close-on-exec; failure prevents backend submission.
#[allow(
    unsafe_code,
    reason = "`fcntl` on an adopted descriptor has no safe wrapper"
)]
fn normalize_document_cloexec(fd: &OwnedFd) -> Result<(), std::io::Error> {
    let raw = fd.as_raw_fd();
    // SAFETY: F_GETFD reads a flag word from the descriptor table and mutates
    // nothing; any int is a legal fd argument and an invalid one is reported
    // as -1/EBADF, never UB.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if flags & libc::FD_CLOEXEC != 0 {
        return Ok(());
    }
    // SAFETY: `F_SETFD` receives the flag word returned by `F_GETFD` plus `FD_CLOEXEC`; invalid descriptors report `EBADF`.
    let rc = unsafe { libc::fcntl(raw, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if rc == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrintPreflight {
    Ready,
    CloexecFailed,
}

/// A document format accepted by the print portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrintFormat {
    /// Portable Document Format.
    Pdf,
    /// PostScript.
    PostScript,
}

impl PrintFormat {
    /// The portal spelling used in `supported_output_file_formats`.
    #[must_use]
    pub fn portal_name(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::PostScript => "ps",
        }
    }
}

/// A portal-issued bearer token that permits printing without another preparation dialog.
///
/// Its `Debug` representation is redacted to prevent accidental disclosure.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrintToken(u32);

impl core::fmt::Debug for PrintToken {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PrintToken([redacted])")
    }
}

impl PrintToken {
    /// Wraps a token previously obtained outside this process.
    #[must_use]
    pub fn from_raw(token: u32) -> Self {
        Self(token)
    }

    /// Returns the portal's raw token value.
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }
}

/// What a successful print exchange produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrintReceipt {
    /// The token used by the print call, newly prepared or supplied by the caller.
    pub token: PrintToken,
}

/// An anonymous in-memory file holding `bytes`, positioned at byte zero.
fn document_fd(bytes: &[u8]) -> std::io::Result<OwnedFd> {
    let fd = rustix::fs::memfd_create("enzui-print", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut written = 0;
    while written < bytes.len() {
        written += rustix::io::write(&fd, &bytes[written..])?;
    }
    rustix::fs::seek(&fd, rustix::fs::SeekFrom::Start(0))?;
    Ok(fd)
}

/// A print operation that owns its document descriptor until the exchange ends.
#[derive(Debug)]
pub struct PrintRequest {
    pub(crate) document: OwnedFd,
    pub(crate) format: PrintFormat,
    pub(crate) token: Option<PrintToken>,
    pub(crate) title: String,
    pub(crate) parent: ParentHandle,
    // Close-on-exec normalization is unconditional; only consumption of its failure depends on `portal-dbus`.
    #[cfg_attr(not(feature = "portal-dbus"), allow(dead_code))]
    pub(crate) preflight: PrintPreflight,
}

impl PrintRequest {
    /// Creates a dialog-based request for a readable PDF or PostScript descriptor positioned at byte zero.
    #[must_use]
    pub fn with_dialog(document: OwnedFd, format: PrintFormat) -> Self {
        // Set close-on-exec when ownership transfers so concurrent process creation cannot inherit the document.
        let preflight = match normalize_document_cloexec(&document) {
            Ok(()) => PrintPreflight::Ready,
            Err(_) => PrintPreflight::CloexecFailed,
        };
        Self {
            document,
            format,
            token: None,
            title: String::new(),
            parent: ParentHandle::none(),
            preflight,
        }
    }

    /// A dialog-based request for `document`, the bytes of a PDF or
    /// PostScript file, held in an anonymous in-memory file.
    ///
    /// # Errors
    /// The in-memory file cannot be created or written.
    pub fn with_dialog_bytes(document: &[u8], format: PrintFormat) -> std::io::Result<Self> {
        Ok(Self::with_dialog(document_fd(document)?, format))
    }

    /// Creates a dialog-free request using an earlier portal token unchanged.
    #[must_use]
    pub fn with_token(document: OwnedFd, format: PrintFormat, token: PrintToken) -> Self {
        // Token-based requests retain the descriptor across the same process-spawn window.
        let preflight = match normalize_document_cloexec(&document) {
            Ok(()) => PrintPreflight::Ready,
            Err(_) => PrintPreflight::CloexecFailed,
        };
        Self {
            document,
            format,
            token: Some(token),
            title: String::new(),
            parent: ParentHandle::none(),
            preflight,
        }
    }

    /// Sets the portal dialog/job title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the parent handle; an absent handle requests an unparented dialog.
    #[must_use]
    pub fn parent(mut self, parent: ParentHandle) -> Self {
        self.parent = parent;
        self
    }

    /// The declared document format.
    #[must_use]
    pub fn format(&self) -> PrintFormat {
        self.format
    }

    /// Borrows the document descriptor without transferring or closing it.
    #[must_use]
    pub fn document(&self) -> &OwnedFd {
        &self.document
    }

    /// The prior token, if this request will skip `PreparePrint`.
    #[must_use]
    pub fn token(&self) -> Option<PrintToken> {
        self.token
    }

    // A close-on-exec adoption failure prevents the request from reaching a backend.
    #[cfg_attr(not(any(feature = "portal-dbus", test)), allow(dead_code))]
    pub(crate) fn preflight_failure(&self) -> Option<super::PortalOutcome<PrintReceipt>> {
        match self.preflight {
            PrintPreflight::Ready => None,
            PrintPreflight::CloexecFailed => Some(super::PortalOutcome::Failed),
        }
    }

    /// Starts the D-Bus print operation without waiting for user interaction.
    ///
    /// A descriptor preflight failure is returned as a terminal failure.
    ///
    /// # Errors
    ///
    /// A terminal failure when the document descriptor fails its preflight or the backend refuses.
    #[cfg(feature = "portal-dbus")]
    pub fn start(
        self,
        backend: &mut super::dbus_backend::DbusBackend,
    ) -> Result<PendingPrint, super::PortalOutcome<PrintReceipt>> {
        if let Some(failure) = self.preflight_failure() {
            return Err(failure);
        }
        backend.start_print(self).map(PendingPrint::new)
    }
}

/// A move-only live print operation sharing the common pending lifecycle.
///
/// Polling and settlement are mutually exclusive, and cancellation closes the wire request at most once.
#[cfg(feature = "portal-dbus")]
#[derive(Debug)]
pub struct PendingPrint {
    lifecycle: super::pending::PendingLifecycle<super::backend::RequestId>,
}

#[cfg(feature = "portal-dbus")]
impl PendingPrint {
    fn new(id: super::backend::RequestId) -> Self {
        Self {
            lifecycle: super::pending::PendingLifecycle::live(id),
        }
    }

    #[cfg(test)]
    fn test_clone(&self) -> Self {
        Self {
            lifecycle: self.lifecycle.clone(),
        }
    }

    #[cfg(test)]
    fn lifecycle_is_finished(&self) -> bool {
        self.lifecycle.is_finished()
    }

    fn poll_with(
        &mut self,
        poll: impl FnOnce(super::backend::RequestId) -> Option<super::PortalOutcome<PrintReceipt>>,
    ) -> Option<super::PortalOutcome<PrintReceipt>> {
        let id = self.lifecycle.try_claim_poll()?;

        let Some(outcome) = poll(id) else {
            self.lifecycle.release_poll();
            return self
                .lifecycle
                .outcome_reason()
                .map(super::PortalOutcome::Cancelled);
        };

        // Publish wire completion before resolving a racing cancellation to avoid closing a completed request.
        self.lifecycle.commit_terminal_reply();
        if self.lifecycle.cancel().complete() {
            Some(outcome)
        } else {
            self.lifecycle
                .outcome_reason()
                .map(super::PortalOutcome::Cancelled)
        }
    }

    /// Polls for the terminal typed outcome without blocking.
    pub fn poll(
        &mut self,
        backend: &mut super::dbus_backend::DbusBackend,
    ) -> Option<super::PortalOutcome<PrintReceipt>> {
        self.poll_with(|id| backend.poll_print(id))
    }

    /// Records cancellation; call [`settle`](Self::settle) to close the live
    /// portal request.
    pub fn cancel(
        &mut self,
        reason: super::CancelReason,
    ) -> Option<super::PortalOutcome<PrintReceipt>> {
        self.lifecycle
            .record_cancel(reason)
            .then_some(super::PortalOutcome::Cancelled(reason))
    }

    fn settle_with(
        &mut self,
        close: impl FnOnce(super::backend::RequestId, super::CancelReason),
    ) -> Option<super::CancelReason> {
        match self.lifecycle.claim_close()? {
            super::pending::CloseClaim::Wire(id, reason) => {
                close(id, reason);
                Some(reason)
            }
            super::pending::CloseClaim::NoWire(_) => None,
        }
    }

    /// Performs an owed `Request::Close`, at most once.
    pub fn settle(
        &mut self,
        backend: &mut super::dbus_backend::DbusBackend,
    ) -> Option<super::CancelReason> {
        self.settle_with(|id, reason| backend.close_print(id, reason))
    }
}

#[cfg(test)]
#[allow(unsafe_code, reason = "the tests build raw descriptors")]
mod tests {

    use super::*;

    #[cfg(feature = "portal-dbus")]
    #[test]
    fn print_close_claim_seals_lifecycle_before_backend_close() {
        let mut settler = PendingPrint::new(super::super::backend::RequestId::new(72));
        let mut poller = settler.test_clone();
        settler.cancel(super::super::CancelReason::ParentClosed);
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result = settler.settle_with(|_, _| {
                entered_tx.send(()).expect("close observer");
                resume_rx
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .expect("test must release blocked print close");
            });
            (result, settler)
        });

        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("settlement must reach backend close");
        assert!(
            poller.lifecycle_is_finished(),
            "the close obligation and Finished lifecycle claim must publish atomically"
        );
        let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let poll_count = std::sync::Arc::clone(&polls);
        assert_eq!(
            poller.poll_with(move |_| {
                poll_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                None
            }),
            None
        );
        assert_eq!(
            polls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a close claim must seal Finished before the backend call"
        );
        resume_tx.send(()).expect("release print close");
        let (result, _) = worker.join().expect("print close worker");
        assert_eq!(result, Some(super::super::CancelReason::ParentClosed));
    }

    #[test]
    fn bytes_become_a_document_read_from_the_start() {
        use std::io::Read as _;
        let request =
            super::PrintRequest::with_dialog_bytes(b"%PDF-1.7 tiny", super::PrintFormat::Pdf)
                .expect("an in-memory document");
        let mut read = String::new();
        std::fs::File::from(request.document)
            .read_to_string(&mut read)
            .expect("readable");
        assert_eq!(read, "%PDF-1.7 tiny");
    }
}
