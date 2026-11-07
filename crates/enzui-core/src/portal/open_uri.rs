//! Cancellable OpenURI operations that preserve the caller’s URI bytes.
//!
//! Callers own scheme policy and confirmation. Cancelling a started request creates one close obligation; deferred cancellation settles without backend access, and dropping the result consumer does not cancel.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::activation::ActivationContext;
use crate::portal::backend::{BackendError, RequestId};
use crate::portal::cancel::PortalCancel;
use crate::portal::parent::{ParentHandle, ParentResolution, PortalParent};
use crate::portal::pending::{ClaimStart, CloseClaim, PendingLifecycle};
use crate::portal::{CancelReason, CompletionSignal, PortalOutcome};

pub mod desktop_launcher;

/// A syntactically valid absolute URI preserved byte-for-byte.
///
/// Scheme policy remains the caller’s responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpenUri(String);

impl OpenUri {
    /// Encodes an absolute local path as a `file://` URI without lossy conversion.
    ///
    /// Forward slashes and RFC 3986 unreserved bytes remain literal; all other bytes use uppercase percent escapes.
    ///
    /// # Errors
    ///
    /// Returns [`FilePathUriError::Relative`] for a relative path, [`FilePathUriError::NotUtf8`] when a non-Unix path cannot be represented losslessly, or [`FilePathUriError::EncodedUri`] if the generated URI fails validation.
    pub fn from_file_path(path: &Path) -> Result<Self, FilePathUriError> {
        if !path.is_absolute() {
            return Err(FilePathUriError::Relative);
        }

        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStrExt;
            path.as_os_str().as_bytes()
        };
        #[cfg(not(unix))]
        let bytes = path.to_str().ok_or(FilePathUriError::NotUtf8)?.as_bytes();

        let mut uri = String::with_capacity("file://".len() + bytes.len());
        uri.push_str("file://");
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for &byte in bytes {
            if byte == b'/'
                || byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'.' | b'_' | b'~')
            {
                uri.push(char::from(byte));
            } else {
                uri.push('%');
                uri.push(char::from(HEX[(byte >> 4) as usize]));
                uri.push(char::from(HEX[(byte & 0x0f) as usize]));
            }
        }

        // Revalidate generated output so future parser tightening fails closed.
        Self::parse(uri).map_err(FilePathUriError::EncodedUri)
    }

    /// Parses an absolute RFC 3986 URI without normalization.
    ///
    /// # Errors
    ///
    /// Returns [`OpenUriParseError`] for an invalid scheme, whitespace or control bytes, malformed percent escapes, or bytes outside the accepted URI set.
    pub fn parse(uri: impl Into<String>) -> Result<Self, OpenUriParseError> {
        let uri = uri.into();
        if uri.is_empty() {
            return Err(OpenUriParseError::Empty);
        }
        let Some((scheme, _)) = uri.split_once(':') else {
            return Err(OpenUriParseError::MissingScheme);
        };
        let mut chars = scheme.chars();
        if !chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
            || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        {
            return Err(OpenUriParseError::InvalidScheme);
        }
        let bytes = uri.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte.is_ascii_control() || byte.is_ascii_whitespace() {
                return Err(OpenUriParseError::WhitespaceOrControl);
            }
            if byte == b'%' {
                let Some(pair) = bytes.get(index + 1..index + 3) else {
                    return Err(OpenUriParseError::BadPercentEscape);
                };
                if !pair.iter().all(u8::is_ascii_hexdigit) {
                    return Err(OpenUriParseError::BadPercentEscape);
                }
                index += 3;
                continue;
            }
            let unreserved =
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
            let reserved = matches!(
                byte,
                b':' | b'/'
                    | b'?'
                    | b'#'
                    | b'['
                    | b']'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
            );
            if !unreserved && !reserved {
                return Err(OpenUriParseError::InvalidCharacter);
            }
            index += 1;
        }
        Ok(Self(uri))
    }

    /// Returns the exact string supplied to [`parse`](Self::parse).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the local filesystem path this URI names, if it names one.
    ///
    /// `Some` only for a `file:` URI whose authority is empty or `localhost`
    /// and which carries no query or fragment — the shape
    /// [`from_file_path`](Self::from_file_path) produces, round-tripped. A
    /// remote authority, a relative reference, or a malformed escape returns
    /// `None`, so a caller cannot mistake another host's file for a local one.
    ///
    /// Transports need this because the desktop portal opens a local file from
    /// a file descriptor (`OpenFile`) and not from a `file:` URI: passing
    /// the URI instead reaches the portal and fails there, which looks to the
    /// user like a file manager that cannot open anything.
    #[must_use]
    pub fn local_file_path(&self) -> Option<std::path::PathBuf> {
        let rest = self
            .0
            .split_once(':')
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
            .map(|(_, rest)| rest)?;
        let rest = rest.strip_prefix("//")?;
        if rest.contains(['?', '#']) {
            return None;
        }
        let (authority, path) = rest.split_at(rest.find('/')?);
        if !(authority.is_empty() || authority.eq_ignore_ascii_case("localhost")) {
            return None;
        }
        let bytes = percent_decode(path)?;
        if bytes.contains(&0) {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(
                bytes,
            )))
        }
        #[cfg(not(unix))]
        {
            String::from_utf8(bytes).ok().map(std::path::PathBuf::from)
        }
    }
}

/// Decodes RFC 3986 percent escapes. `None` on a malformed escape.
fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = bytes.get(index + 1..index + 3)?;
            let hex = std::str::from_utf8(pair).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(out)
}

impl std::fmt::Display for OpenUri {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why an [`OpenUri`] string was not valid URI syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenUriParseError {
    /// The input was empty.
    Empty,
    /// No `scheme:` prefix was present.
    MissingScheme,
    /// The scheme did not follow RFC 3986 syntax.
    InvalidScheme,
    /// The input contained an ASCII whitespace or control byte.
    WhitespaceOrControl,
    /// A percent sign was not followed by exactly two hexadecimal digits.
    BadPercentEscape,
    /// A byte was outside the RFC 3986 URI character set.
    InvalidCharacter,
}

impl std::fmt::Display for OpenUriParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "URI is empty",
            Self::MissingScheme => "URI has no scheme",
            Self::InvalidScheme => "URI scheme is invalid",
            Self::WhitespaceOrControl => "URI contains whitespace or a control byte",
            Self::BadPercentEscape => "URI contains a malformed percent escape",
            Self::InvalidCharacter => "URI contains a character that must be percent-encoded",
        })
    }
}

impl std::error::Error for OpenUriParseError {}

/// Why a local filesystem path could not be represented as an [`OpenUri`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilePathUriError {
    /// The path is relative and therefore has no stable OpenURI identity.
    Relative,
    /// The platform path could not be represented without a lossy conversion.
    /// Unix paths are byte strings and never take this arm.
    NotUtf8,
    /// The encoded URI was rejected by the URI syntax validator.
    EncodedUri(OpenUriParseError),
}

impl std::fmt::Display for FilePathUriError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Relative => formatter.write_str("file path is not absolute"),
            Self::NotUtf8 => formatter.write_str("file path cannot be encoded without data loss"),
            Self::EncodedUri(error) => write!(formatter, "encoded file URI is invalid: {error}"),
        }
    }
}

impl std::error::Error for FilePathUriError {}

/// An already-open regular file whose exact descriptor is handed to the portal.
///
/// The descriptor is confirmed to name a regular file and to be duplicable
/// close-on-exec at construction, so a transport can pass the caller's exact
/// authority to `org.freedesktop.portal.OpenURI.OpenFile` without reopening a
/// pathname. Renaming or replacing the file on disk afterwards cannot
/// substitute different bytes: the retained descriptor still names the original
/// file, which is the guarantee a `file://` URI cannot make.
///
/// Clones share one underlying descriptor.
#[derive(Clone)]
pub struct RetainedFile {
    file: Arc<std::fs::File>,
}

impl RetainedFile {
    /// Retains ownership of an already-open regular file.
    ///
    /// # Errors
    ///
    /// Returns [`RetainedFileError::Stat`] when the descriptor cannot be
    /// queried, [`RetainedFileError::NotRegularFile`] when it does not name a
    /// regular file, or [`RetainedFileError::Duplicate`] when it cannot be
    /// duplicated close-on-exec.
    pub fn new(file: std::fs::File) -> Result<Self, RetainedFileError> {
        Self::from_arc(Arc::new(file))
    }

    /// Retains an already-open regular file shared through an [`Arc`].
    ///
    /// # Errors
    ///
    /// Identical to [`RetainedFile::new`].
    pub fn from_arc(file: Arc<std::fs::File>) -> Result<Self, RetainedFileError> {
        let metadata = file.metadata().map_err(|_| RetainedFileError::Stat)?;
        if !metadata.file_type().is_file() {
            return Err(RetainedFileError::NotRegularFile);
        }
        // A close-on-exec duplicate must succeed before wire admission; `try_clone`
        // is `F_DUPFD_CLOEXEC` on Unix and rejects a non-duplicable descriptor.
        drop(file.try_clone().map_err(|_| RetainedFileError::Duplicate)?);
        Ok(Self { file })
    }

    /// Returns the stable identity of the retained descriptor.
    ///
    /// The identity names the file the descriptor already holds open and does
    /// not change when the pathname is later renamed or replaced. Computing it
    /// does not read file contents.
    #[must_use]
    pub fn identity(&self) -> FileIdentity {
        FileIdentity::of(&self.file)
    }

    /// The shared descriptor, for a transport that passes it on.
    #[cfg(feature = "portal-dbus")]
    pub(crate) fn descriptor(&self) -> Arc<std::fs::File> {
        Arc::clone(&self.file)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn raw(&self) -> &std::fs::File {
        &self.file
    }
}

/// An already-open directory whose exact descriptor the desktop opens, as
/// [`RetainedFile`] is for a regular file.
///
/// Inside a sandbox the descriptor itself goes to the OpenURI portal; outside
/// one the desktop launcher opens the directory's path, after checking that
/// the path still names the retained directory, so a directory renamed or
/// replaced since it was opened is refused rather than opening another one.
///
/// Clones share one underlying descriptor.
#[derive(Clone)]
pub struct RetainedDirectory {
    directory: Arc<std::fs::File>,
}

impl RetainedDirectory {
    /// Retains ownership of an already-open directory.
    ///
    /// # Errors
    ///
    /// Returns [`RetainedFileError::Stat`] when the descriptor cannot be
    /// queried, [`RetainedFileError::NotDirectory`] when it does not name a
    /// directory, or [`RetainedFileError::Duplicate`] when it cannot be
    /// duplicated close-on-exec.
    pub fn new(directory: std::fs::File) -> Result<Self, RetainedFileError> {
        Self::from_arc(Arc::new(directory))
    }

    /// Retains an already-open directory shared through an [`Arc`].
    ///
    /// # Errors
    ///
    /// Identical to [`RetainedDirectory::new`].
    pub fn from_arc(directory: Arc<std::fs::File>) -> Result<Self, RetainedFileError> {
        let metadata = directory.metadata().map_err(|_| RetainedFileError::Stat)?;
        if !metadata.file_type().is_dir() {
            return Err(RetainedFileError::NotDirectory);
        }
        drop(
            directory
                .try_clone()
                .map_err(|_| RetainedFileError::Duplicate)?,
        );
        Ok(Self { directory })
    }

    /// Returns the stable identity of the retained descriptor; see
    /// [`RetainedFile::identity`].
    #[must_use]
    pub fn identity(&self) -> FileIdentity {
        FileIdentity::of(&self.directory)
    }

    /// The shared descriptor, for a transport that passes it on.
    #[cfg(feature = "portal-dbus")]
    pub(crate) fn descriptor(&self) -> Arc<std::fs::File> {
        Arc::clone(&self.directory)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn raw(&self) -> &std::fs::File {
        &self.directory
    }
}

impl std::fmt::Debug for RetainedDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RetainedDirectory")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for RetainedFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Diagnostics never expose the descriptor number, path, or contents.
        formatter
            .debug_struct("RetainedFile")
            .finish_non_exhaustive()
    }
}

/// The stable identity of a retained descriptor, independent of its pathname.
///
/// Two [`RetainedFile`] values share an identity when their descriptors hold the
/// same underlying file open, even after the path is renamed or replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of(file: &std::fs::File) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(metadata) = file.metadata() {
                return Self {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                };
            }
        }
        #[cfg(not(unix))]
        let _ = file;
        Self {
            device: 0,
            inode: 0,
        }
    }
}

/// Why an already-open file could not be retained as an OpenURI target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetainedFileError {
    /// The descriptor's metadata could not be queried.
    Stat,
    /// The descriptor does not name a regular file.
    NotRegularFile,
    /// The descriptor does not name a directory.
    NotDirectory,
    /// The descriptor could not be duplicated close-on-exec.
    Duplicate,
}

impl std::fmt::Display for RetainedFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Stat => "retained file descriptor metadata is unavailable",
            Self::NotRegularFile => "retained file descriptor is not a regular file",
            Self::NotDirectory => "retained directory descriptor is not a directory",
            Self::Duplicate => "retained file descriptor cannot be duplicated close-on-exec",
        })
    }
}

impl std::error::Error for RetainedFileError {}

/// A terminal OpenURI transport reply.
///
/// `Denied` represents explicit permission refusal; portal response code `2` maps to [`Self::Failed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenUriReply {
    /// The desktop accepted the request.
    Success,
    /// The user dismissed the portal interaction.
    UserCancelled,
    /// The portal or bus explicitly denied permission.
    Denied,
    /// The portal disappeared or was unavailable after the request started.
    Unavailable,
    /// The interface is present but OpenURI is unsupported.
    Unsupported,
    /// A transport, protocol, or unspecified portal failure occurred.
    Failed,
}

impl OpenUriReply {
    fn into_outcome(self) -> PortalOutcome<()> {
        match self {
            Self::Success => PortalOutcome::Completed(()),
            Self::UserCancelled => PortalOutcome::Cancelled(CancelReason::User),
            Self::Denied => PortalOutcome::Denied,
            Self::Unavailable => PortalOutcome::Unavailable,
            Self::Unsupported => PortalOutcome::Unsupported,
            Self::Failed => PortalOutcome::Failed,
        }
    }
}

/// Nonblocking transport for correlated OpenURI requests.
///
/// A reply or close must apply only to its exact [`RequestId`].
pub trait OpenUriBackend {
    /// Starts an OpenURI request with the exact parsed URI and encoded parent.
    ///
    /// # Errors
    ///
    /// A [`BackendError`] when the request cannot start.
    fn start_open_uri(
        &mut self,
        uri: &OpenUri,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError>;

    /// Starts OpenURI with compositor context when the portal supports it.
    ///
    /// # Errors
    ///
    /// As [`Self::start_open_uri`].
    fn start_open_uri_with_activation(
        &mut self,
        uri: &OpenUri,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let _ = activation;
        self.start_open_uri(uri, parent)
    }

    /// Starts an OpenURI request that opens the caller's exact retained file.
    ///
    /// The transport passes the retained descriptor to
    /// `org.freedesktop.portal.OpenURI.OpenFile` and never reopens a pathname,
    /// so name substitution between validation and backend open cannot launch
    /// different bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Unsupported`] for a backend without retained-file
    /// support.
    fn start_open_file(
        &mut self,
        file: &RetainedFile,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let _ = (file, parent, activation);
        Err(BackendError::Unsupported)
    }

    /// Starts a request that opens the caller's retained directory in the
    /// file manager.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Unsupported`] for a backend without
    /// retained-directory support.
    fn start_open_directory(
        &mut self,
        directory: &RetainedDirectory,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let _ = (directory, parent, activation);
        Err(BackendError::Unsupported)
    }

    /// Starts a request that shows the caller's retained file in its folder,
    /// selected where the file manager supports it.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Unsupported`] for a backend that cannot show a
    /// containing folder.
    fn start_open_containing_folder(
        &mut self,
        file: &RetainedFile,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let _ = (file, parent, activation);
        Err(BackendError::Unsupported)
    }

    /// Returns this exact request's reply, if one has arrived.
    fn poll_open_uri(&mut self, id: RequestId) -> Option<OpenUriReply>;

    /// Closes this exact live portal request, idempotently.
    fn close_open_uri(&mut self, id: RequestId, reason: CancelReason);

    /// Returns the event signal for availability and reply changes, if supported.
    fn open_uri_completion_signal(&self) -> Option<CompletionSignal> {
        None
    }

    /// A one-line diagnostic description.
    fn describe_open_uri(&self) -> String;
}

/// An unavailable feature-free backend that never starts a subprocess.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableOpenUriBackend;

impl OpenUriBackend for UnavailableOpenUriBackend {
    fn start_open_uri(
        &mut self,
        _uri: &OpenUri,
        _parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        Err(BackendError::Unavailable)
    }

    fn start_open_file(
        &mut self,
        _file: &RetainedFile,
        _parent: &ParentHandle,
        _activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        Err(BackendError::Unavailable)
    }

    fn start_open_containing_folder(
        &mut self,
        _file: &RetainedFile,
        _parent: &ParentHandle,
        _activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        Err(BackendError::Unavailable)
    }

    fn poll_open_uri(&mut self, _id: RequestId) -> Option<OpenUriReply> {
        None
    }

    fn close_open_uri(&mut self, _id: RequestId, _reason: CancelReason) {}

    fn describe_open_uri(&self) -> String {
        "OpenURI unavailable (build without portal-dbus)".into()
    }
}

/// A request for the desktop to open one URI.
#[derive(Debug)]
pub struct OpenUriRequest {
    uri: OpenUri,
    parent: PortalParent,
    parent_handle: ParentHandle,
    activation: Option<ActivationContext>,
}

impl OpenUriRequest {
    /// Builds a request from a parsed URI and an explicit parent policy.
    #[must_use]
    pub fn new(uri: OpenUri, parent: PortalParent) -> Self {
        Self {
            uri,
            parent,
            parent_handle: ParentHandle::none(),
            activation: None,
        }
    }

    /// Carries compositor activation context to OpenURI version 4 or newer.
    #[must_use]
    pub fn with_activation(mut self, activation: ActivationContext) -> Self {
        self.activation = Some(activation);
        self
    }

    /// Sets the already-exported portal parent handle.
    #[must_use]
    pub fn with_parent_handle(mut self, parent_handle: ParentHandle) -> Self {
        self.parent_handle = parent_handle;
        self
    }

    /// Returns the exact URI this request will send.
    #[must_use]
    pub fn uri(&self) -> &OpenUri {
        &self.uri
    }

    /// Checks parent liveness, then starts or defers this OpenURI operation.
    ///
    /// A stale requested parent returns [`PortalOutcome::ParentUnavailable`] before backend access; initialization and queue pressure retain the request for retry.
    ///
    /// # Errors
    ///
    /// [`PortalOutcome::ParentUnavailable`] for a dead parent window, and the backend's terminal
    /// refusal.
    pub fn start(
        mut self,
        backend: &mut dyn OpenUriBackend,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Result<PendingOpenUri, PortalOutcome<()>> {
        if matches!(
            self.parent.resolve(is_live),
            ParentResolution::Unavailable(_)
        ) {
            return Err(PortalOutcome::ParentUnavailable);
        }
        if self
            .activation
            .as_ref()
            .is_some_and(|context| context.is_expired(std::time::Instant::now()))
        {
            self.activation = None;
        }
        match backend.start_open_uri_with_activation(
            &self.uri,
            &self.parent_handle,
            self.activation.as_ref(),
        ) {
            Ok(id) => {
                let pending = PendingOpenUri::started(id);
                attach_wake(&pending, backend);
                Ok(pending)
            }
            Err(BackendError::Initialising | BackendError::ResourceExhausted) => {
                let pending = PendingOpenUri::deferred(
                    OpenTarget::Uri(self.uri),
                    self.parent_handle,
                    self.activation,
                );
                attach_wake(&pending, backend);
                Ok(pending)
            }
            Err(error) => Err(outcome_for_backend_error(error)),
        }
    }
}

/// A request for the desktop to open one already-open local file.
///
/// The retained descriptor is carried from construction through deferred
/// queueing and backend start, so the portal opens the exact file the caller
/// validated rather than whatever a pathname resolves to at open time.
#[derive(Debug)]
pub struct OpenFileRequest {
    file: RetainedFile,
    parent: PortalParent,
    parent_handle: ParentHandle,
    activation: Option<ActivationContext>,
    containing_folder: bool,
}

impl OpenFileRequest {
    /// Builds a request from a retained file and an explicit parent policy.
    #[must_use]
    pub fn new(file: RetainedFile, parent: PortalParent) -> Self {
        Self {
            file,
            parent,
            parent_handle: ParentHandle::none(),
            activation: None,
            containing_folder: false,
        }
    }

    /// Builds a request that shows the file in its folder instead of opening
    /// it ("Show in Files"), through `OpenURI.OpenDirectory` or the file
    /// manager's `ShowItems`.
    #[must_use]
    pub fn containing_folder(file: RetainedFile, parent: PortalParent) -> Self {
        Self {
            containing_folder: true,
            ..Self::new(file, parent)
        }
    }

    /// Carries compositor activation context to OpenURI version 4 or newer.
    #[must_use]
    pub fn with_activation(mut self, activation: ActivationContext) -> Self {
        self.activation = Some(activation);
        self
    }

    /// Sets the already-exported portal parent handle.
    #[must_use]
    pub fn with_parent_handle(mut self, parent_handle: ParentHandle) -> Self {
        self.parent_handle = parent_handle;
        self
    }

    /// Returns the retained file this request will open.
    #[must_use]
    pub fn file(&self) -> &RetainedFile {
        &self.file
    }

    /// Checks parent liveness, then starts or defers this OpenFile operation.
    ///
    /// A stale requested parent returns [`PortalOutcome::ParentUnavailable`] before backend access; initialization and queue pressure retain the request, and its descriptor, for retry.
    ///
    /// # Errors
    ///
    /// [`PortalOutcome::ParentUnavailable`] for a dead parent window, and the backend's terminal
    /// refusal.
    pub fn start(
        mut self,
        backend: &mut dyn OpenUriBackend,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Result<PendingOpenUri, PortalOutcome<()>> {
        if matches!(
            self.parent.resolve(is_live),
            ParentResolution::Unavailable(_)
        ) {
            return Err(PortalOutcome::ParentUnavailable);
        }
        if self
            .activation
            .as_ref()
            .is_some_and(|context| context.is_expired(std::time::Instant::now()))
        {
            self.activation = None;
        }
        let started = if self.containing_folder {
            backend.start_open_containing_folder(
                &self.file,
                &self.parent_handle,
                self.activation.as_ref(),
            )
        } else {
            backend.start_open_file(&self.file, &self.parent_handle, self.activation.as_ref())
        };
        match started {
            Ok(id) => {
                let pending = PendingOpenUri::started(id);
                attach_wake(&pending, backend);
                Ok(pending)
            }
            Err(BackendError::Initialising | BackendError::ResourceExhausted) => {
                let target = if self.containing_folder {
                    OpenTarget::Folder(self.file)
                } else {
                    OpenTarget::File(self.file)
                };
                let pending = PendingOpenUri::deferred(target, self.parent_handle, self.activation);
                attach_wake(&pending, backend);
                Ok(pending)
            }
            Err(error) => Err(outcome_for_backend_error(error)),
        }
    }
}

/// A request for the desktop to open one already-open local directory in the
/// file manager.
///
/// The retained descriptor is carried through deferred queueing and backend
/// start as an [`OpenFileRequest`]'s file is, with the same parent checks,
/// cancellation and completion; [`OpenFileRequest::containing_folder`] is the
/// request that shows a *file* in its folder.
#[derive(Debug)]
pub struct OpenDirectoryRequest {
    directory: RetainedDirectory,
    parent: PortalParent,
    parent_handle: ParentHandle,
    activation: Option<ActivationContext>,
}

impl OpenDirectoryRequest {
    /// Builds a request from a retained directory and an explicit parent policy.
    #[must_use]
    pub fn new(directory: RetainedDirectory, parent: PortalParent) -> Self {
        Self {
            directory,
            parent,
            parent_handle: ParentHandle::none(),
            activation: None,
        }
    }

    /// Carries compositor activation context to OpenURI version 4 or newer.
    #[must_use]
    pub fn with_activation(mut self, activation: ActivationContext) -> Self {
        self.activation = Some(activation);
        self
    }

    /// Sets the already-exported portal parent handle.
    #[must_use]
    pub fn with_parent_handle(mut self, parent_handle: ParentHandle) -> Self {
        self.parent_handle = parent_handle;
        self
    }

    /// Returns the retained directory this request will open.
    #[must_use]
    pub fn directory(&self) -> &RetainedDirectory {
        &self.directory
    }

    /// Checks parent liveness, then starts or defers this request, as
    /// [`OpenFileRequest::start`] does.
    ///
    /// # Errors
    ///
    /// [`PortalOutcome::ParentUnavailable`] for a dead parent window, and the
    /// backend's terminal refusal.
    pub fn start(
        mut self,
        backend: &mut dyn OpenUriBackend,
        is_live: impl Fn(iced_core::window::Id) -> bool,
    ) -> Result<PendingOpenUri, PortalOutcome<()>> {
        if matches!(
            self.parent.resolve(is_live),
            ParentResolution::Unavailable(_)
        ) {
            return Err(PortalOutcome::ParentUnavailable);
        }
        if self
            .activation
            .as_ref()
            .is_some_and(|context| context.is_expired(std::time::Instant::now()))
        {
            self.activation = None;
        }
        match backend.start_open_directory(
            &self.directory,
            &self.parent_handle,
            self.activation.as_ref(),
        ) {
            Ok(id) => {
                let pending = PendingOpenUri::started(id);
                attach_wake(&pending, backend);
                Ok(pending)
            }
            Err(BackendError::Initialising | BackendError::ResourceExhausted) => {
                let pending = PendingOpenUri::deferred(
                    OpenTarget::Directory(self.directory),
                    self.parent_handle,
                    self.activation,
                );
                attach_wake(&pending, backend);
                Ok(pending)
            }
            Err(error) => Err(outcome_for_backend_error(error)),
        }
    }
}

fn attach_wake(pending: &PendingOpenUri, backend: &dyn OpenUriBackend) {
    if let Some(signal) = backend.open_uri_completion_signal() {
        pending.lifecycle.attach_wake(signal);
    }
}

fn outcome_for_backend_error(error: BackendError) -> PortalOutcome<()> {
    match error {
        BackendError::Unavailable => PortalOutcome::Unavailable,
        BackendError::Unsupported => PortalOutcome::Unsupported,
        BackendError::NotDeclared => PortalOutcome::NotDeclared,
        BackendError::Denied => PortalOutcome::Denied,
        BackendError::Failed | BackendError::ResourceExhausted => PortalOutcome::Failed,
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

/// The target a deferred OpenURI request replays when it starts.
///
/// A retained file keeps its descriptor alive here until the start succeeds.
#[derive(Debug, Clone)]
enum OpenTarget {
    Uri(OpenUri),
    File(RetainedFile),
    Folder(RetainedFile),
    Directory(RetainedDirectory),
}

type OpenUriRetry = (OpenTarget, ParentHandle, Option<ActivationContext>);

/// A live or deferred OpenURI operation.
///
/// Clones share one request identity, retained retry payload, terminal state, and exactly-once close obligation.
#[derive(Debug, Clone)]
pub struct PendingOpenUri {
    lifecycle: PendingLifecycle<RequestId>,
    retry: Arc<Mutex<Option<OpenUriRetry>>>,
    #[cfg(test)]
    poll_claim_gate: Option<PollClaimGate>,
}

impl PendingOpenUri {
    fn started(id: RequestId) -> Self {
        Self {
            lifecycle: PendingLifecycle::live(id),
            retry: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            poll_claim_gate: None,
        }
    }

    fn deferred(
        target: OpenTarget,
        parent: ParentHandle,
        activation: Option<ActivationContext>,
    ) -> Self {
        Self {
            lifecycle: PendingLifecycle::no_wire(),
            retry: Arc::new(Mutex::new(Some((target, parent, activation)))),
            #[cfg(test)]
            poll_claim_gate: None,
        }
    }

    fn retry_take(&self) -> Option<OpenUriRetry> {
        self.retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn retry_restore(&self, payload: OpenUriRetry) {
        *self
            .retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(payload);
    }

    #[cfg(test)]
    fn cancel_ref(&self) -> &PortalCancel {
        self.lifecycle.cancel()
    }

    /// The correlated wire request ID, once started.
    #[must_use]
    pub fn request_id(&self) -> Option<RequestId> {
        self.lifecycle.wire_id()
    }

    /// Returns whether no terminal result has been recorded.
    ///
    /// A cancelled operation may return `false` while still requiring [`Self::settle`].
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.lifecycle.is_pending()
    }

    /// Returns whether this operation is pending or still owes settlement.
    ///
    /// It becomes false after reply consumption or after [`Self::settle`] claims the close obligation.
    #[must_use]
    pub fn is_unsettled(&self) -> bool {
        // Treat a still-pending terminal writer as unsettled after lifecycle sealing.
        self.is_pending() || !self.lifecycle.is_finished()
    }

    /// A `Send + Sync` handle for explicit application cancellation.
    #[must_use]
    pub fn cancel_handle(&self) -> PortalCancel {
        self.lifecycle.cancel_handle()
    }

    /// The winning cancellation reason, if cancellation ended the operation.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.lifecycle.outcome_reason()
    }

    /// Records explicit cancellation and returns its outcome only if this call wins.
    ///
    /// Call [`Self::settle`] to perform any owed wire Close.
    pub fn cancel(&mut self, reason: CancelReason) -> Option<PortalOutcome<()>> {
        self.lifecycle
            .record_cancel(reason)
            .then_some(PortalOutcome::Cancelled(reason))
    }

    /// Retries a deferred start or polls this request for one terminal outcome.
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
    pub fn poll(&mut self, backend: &mut dyn OpenUriBackend) -> Option<PortalOutcome<()>> {
        if !self.lifecycle.is_pending() {
            return None;
        }

        #[cfg(test)]
        if let Some(gate) = &self.poll_claim_gate {
            gate.pause();
        }

        // Claim deferred start under the lifecycle lock so clones cannot start twice or race settlement.
        match self.lifecycle.try_claim_start() {
            ClaimStart::Claimed => {
                let (target, parent, mut activation) = self
                    .retry_take()
                    .expect("NoWire deferred request always carries a retry payload");
                if activation
                    .as_ref()
                    .is_some_and(|context| context.is_expired(std::time::Instant::now()))
                {
                    activation = None;
                }
                let started = match &target {
                    OpenTarget::Uri(uri) => {
                        backend.start_open_uri_with_activation(uri, &parent, activation.as_ref())
                    }
                    OpenTarget::File(file) => {
                        backend.start_open_file(file, &parent, activation.as_ref())
                    }
                    OpenTarget::Folder(file) => {
                        backend.start_open_containing_folder(file, &parent, activation.as_ref())
                    }
                    OpenTarget::Directory(directory) => {
                        backend.start_open_directory(directory, &parent, activation.as_ref())
                    }
                };
                match started {
                    Ok(id) => {
                        self.lifecycle.publish_started(id);
                        return self
                            .lifecycle
                            .outcome_reason()
                            .map(PortalOutcome::Cancelled);
                    }
                    Err(BackendError::Initialising | BackendError::ResourceExhausted) => {
                        // Restore the target, parent, and live context before republishing `NoWire`.
                        self.retry_restore((target, parent, activation));
                        self.lifecycle.publish_deferred();
                        return self
                            .lifecycle
                            .outcome_reason()
                            .map(PortalOutcome::Cancelled);
                    }
                    Err(error) => {
                        self.lifecycle.publish_terminal_no_wire();
                        if self.lifecycle.cancel().complete() {
                            return Some(outcome_for_backend_error(error));
                        }
                        return self
                            .lifecycle
                            .outcome_reason()
                            .map(PortalOutcome::Cancelled);
                    }
                }
            }
            ClaimStart::CancelledBeforeStart | ClaimStart::Locked | ClaimStart::Finished => {
                return None;
            }
            ClaimStart::AlreadyLive => {}
        }

        let id = self.lifecycle.try_claim_poll()?;
        let Some(reply) = backend.poll_open_uri(id) else {
            self.lifecycle.release_poll();
            return self
                .lifecycle
                .outcome_reason()
                .map(PortalOutcome::Cancelled);
        };

        self.lifecycle.commit_terminal_reply();
        if !self.lifecycle.cancel().complete() {
            return self
                .lifecycle
                .outcome_reason()
                .map(PortalOutcome::Cancelled);
        }
        Some(reply.into_outcome())
    }

    /// Performs an owed wire Close at most once.
    ///
    /// Returns the cancellation reason for a live request close, or `None` when no wire request existed or no close remains.
    pub fn settle(&mut self, backend: &mut dyn OpenUriBackend) -> Option<CancelReason> {
        match self.lifecycle.claim_close()? {
            CloseClaim::Wire(id, reason) => {
                backend.close_open_uri(id, reason);
                Some(reason)
            }
            // Seal no-wire cancellation without calling the backend. Dropping the
            // retained target here releases any held descriptor exactly once,
            // because `claim_close` yields the no-wire obligation only once.
            CloseClaim::NoWire(_) => {
                self.retry_take();
                None
            }
        }
    }
}

/// Deterministic host-independent OpenURI backend.
#[derive(Debug, Default)]
pub struct MockOpenUriBackend {
    next_id: u64,
    start_error: Option<BackendError>,
    replies: HashMap<RequestId, VecDeque<OpenUriReply>>,
    /// Every started URI request, including its byte-exact URI and parent handle.
    pub started: Vec<(RequestId, OpenUri, ParentHandle)>,
    /// Every started retained-file request, with the descriptor's identity and parent.
    ///
    /// The identity is derived without reading file contents.
    pub started_files: Vec<(RequestId, FileIdentity, ParentHandle)>,
    /// Every effective close, with the exact correlated request ID.
    pub closed: Vec<(RequestId, CancelReason)>,
}

impl MockOpenUriBackend {
    /// Creates an available backend with no scripted replies.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a backend that refuses every start with `error`.
    #[must_use]
    pub fn failing(error: BackendError) -> Self {
        Self {
            start_error: Some(error),
            ..Self::default()
        }
    }

    /// Scripts a reply for one exact request ID.
    pub fn script(&mut self, id: RequestId, reply: OpenUriReply) {
        self.replies.entry(id).or_default().push_back(reply);
    }

    /// Number of effective closes for one request.
    #[must_use]
    pub fn close_count(&self, id: RequestId) -> usize {
        self.closed
            .iter()
            .filter(|(closed, _)| *closed == id)
            .count()
    }

    fn is_started(&self, id: RequestId) -> bool {
        self.started.iter().any(|(started, _, _)| *started == id)
            || self
                .started_files
                .iter()
                .any(|(started, _, _)| *started == id)
    }
}

impl OpenUriBackend for MockOpenUriBackend {
    fn start_open_uri(
        &mut self,
        uri: &OpenUri,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        if let Some(error) = self.start_error {
            return Err(error);
        }
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        self.started.push((id, uri.clone(), parent.clone()));
        Ok(id)
    }

    fn start_open_file(
        &mut self,
        file: &RetainedFile,
        parent: &ParentHandle,
        _activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        if let Some(error) = self.start_error {
            return Err(error);
        }
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        self.started_files
            .push((id, file.identity(), parent.clone()));
        Ok(id)
    }

    fn start_open_directory(
        &mut self,
        directory: &RetainedDirectory,
        parent: &ParentHandle,
        _activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        if let Some(error) = self.start_error {
            return Err(error);
        }
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        self.started_files
            .push((id, directory.identity(), parent.clone()));
        Ok(id)
    }

    fn poll_open_uri(&mut self, id: RequestId) -> Option<OpenUriReply> {
        if !self.is_started(id) {
            return None;
        }
        self.replies.get_mut(&id).and_then(VecDeque::pop_front)
    }

    fn close_open_uri(&mut self, id: RequestId, reason: CancelReason) {
        if !self.is_started(id) || self.closed.iter().any(|(closed, _)| *closed == id) {
            return;
        }
        self.closed.push((id, reason));
        self.replies.remove(&id);
    }

    fn describe_open_uri(&self) -> String {
        format!(
            "mock OpenURI — {} started, {} closed",
            self.started.len() + self.started_files.len(),
            self.closed.len()
        )
    }
}

#[cfg(test)]
mod tests;
