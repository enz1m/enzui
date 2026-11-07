//! Typed, versioned application-state persistence.
//!
//! File writes are atomic and may be prepared separately from blocking I/O; an
//! in-memory backend supports isolated tests. [`LoadController`] runs blocking
//! loads off-thread with at most one active worker and rejects stale completions.
//! Loads distinguish absence from refusal and decode failure. A refusal protects
//! existing bytes until explicit replacement authorization, and schema versions
//! match exactly.
//!
//! [`Persistable`] is an explicit opt-in for disk-safe values. Downstream crates
//! cannot implement it directly for [`enzui_security::secret::Secret`], though
//! they may deliberately expose secret bytes through a local wrapper.

use crate::atomic_state;
use iced_futures::futures::channel::oneshot;
use iced_runtime::Task;
use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

// Same-thread reentry detection for execute_inner and load.
thread_local! {
    static PERSIST_LOAD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static PERSIST_SAVE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

struct DepthGuard {
    cell: &'static std::thread::LocalKey<std::cell::Cell<u32>>,
}

impl DepthGuard {
    fn enter(cell: &'static std::thread::LocalKey<std::cell::Cell<u32>>) -> Self {
        cell.with(|c| c.set(c.get().wrapping_add(1)));
        Self { cell }
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.cell.with(|c| c.set(c.get().wrapping_sub(1)));
    }
}

// Clears active_save on drop so a panic in save_raw does not permanently
// block subsequent saves and loads.
struct ActiveSaveGuard {
    ordering: Arc<SaveOrdering>,
}

impl Drop for ActiveSaveGuard {
    fn drop(&mut self) {
        {
            let mut state = self.ordering.lock();
            state.active_save = false;
        }
        self.ordering.state_changed.notify_all();
    }
}

// Publishes the load-block decision and decrements active_loads on drop,
// including during panic unwind.
struct ActiveLoadGuard {
    ordering: Arc<SaveOrdering>,
    blocked: bool,
}

impl Drop for ActiveLoadGuard {
    fn drop(&mut self) {
        {
            let mut state = self.ordering.lock();
            SaveOrdering::publish_load_block(&mut state, self.blocked);
            state.active_loads -= 1;
        }
        self.ordering.state_changed.notify_all();
    }
}

/// Which part of an XDG descendant candidate was invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XdgPathPart {
    /// The one-component application namespace.
    Application,
    /// The relative object path below the application namespace.
    Object,
}

/// Why an XDG descendant candidate was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XdgPathRejection {
    /// The candidate was empty.
    Empty,
    /// The candidate contained a NUL byte.
    InteriorNul,
    /// The application was not exactly one ordinary component, or the object
    /// contained a non-normal component.
    NonNormalComponent,
}

/// Error returned when an application or object path is not a safe XDG descendant.
///
/// Resolver entry points use `Ok(None)` for missing roots; this error is returned
/// before filesystem I/O for input that could escape the selected root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XdgPathError {
    part: XdgPathPart,
    rejection: XdgPathRejection,
    candidate: PathBuf,
}

impl XdgPathError {
    /// Which input was invalid.
    #[must_use]
    pub fn part(&self) -> XdgPathPart {
        self.part
    }

    /// Why it was invalid.
    #[must_use]
    pub fn rejection(&self) -> XdgPathRejection {
        self.rejection
    }

    /// The rejected value.
    #[must_use]
    pub fn candidate(&self) -> &Path {
        &self.candidate
    }
}

impl std::fmt::Display for XdgPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let part = match self.part {
            XdgPathPart::Application => "application namespace",
            XdgPathPart::Object => "object path",
        };
        let reason = match self.rejection {
            XdgPathRejection::Empty => "must not be empty",
            XdgPathRejection::InteriorNul => "must not contain a NUL byte",
            XdgPathRejection::NonNormalComponent => {
                "must contain only ordinary relative path components"
            }
        };
        write!(f, "invalid XDG {part} {:?}: {reason}", self.candidate)
    }
}

impl std::error::Error for XdgPathError {}

/// A validated, one-component application namespace below an XDG root.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct XdgApplication(String);

impl XdgApplication {
    /// Validate one non-empty [`Component::Normal`] application component.
    ///
    /// # Errors
    ///
    /// Returns [`XdgPathError`] when the candidate is empty, contains a NUL,
    /// or is not exactly one normal component.
    pub fn new(candidate: impl Into<String>) -> Result<Self, XdgPathError> {
        let candidate = candidate.into();
        validate_xdg_descendant(Path::new(&candidate), XdgPathPart::Application, true)?;
        Ok(Self(candidate))
    }

    /// The validated application component.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated non-empty relative object path below an XDG application.
///
/// Nested paths are supported, but every component must be
/// [`Component::Normal`]. Roots, prefixes, `.` and `..` are refused.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct XdgRelativePath(PathBuf);

impl XdgRelativePath {
    /// Validate a relative object path.
    ///
    /// # Errors
    ///
    /// Returns [`XdgPathError`] when the candidate is empty, contains a NUL,
    /// or contains any component other than [`Component::Normal`].
    pub fn new(candidate: impl AsRef<Path>) -> Result<Self, XdgPathError> {
        let candidate = candidate.as_ref();
        validate_xdg_descendant(candidate, XdgPathPart::Object, false)?;
        Ok(Self(candidate.to_path_buf()))
    }

    /// The validated relative path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

// Validate every appended XDG descendant lexically, checking encoded `.` before
// `components()` normalizes it; do not canonicalize or follow symlinks.
fn validate_xdg_descendant(
    candidate: &Path,
    part: XdgPathPart,
    exactly_one: bool,
) -> Result<(), XdgPathError> {
    let bytes = candidate.as_os_str().as_encoded_bytes();
    let is_separator =
        |byte: u8| byte == std::path::MAIN_SEPARATOR as u8 || (cfg!(windows) && byte == b'/');
    let has_explicit_dot = bytes
        .split(|byte| is_separator(*byte))
        .any(|component| component == b"." || component == b"..");
    let components = candidate.components().collect::<Vec<_>>();
    let rejection = if bytes.is_empty() {
        Some(XdgPathRejection::Empty)
    } else if bytes.contains(&0) {
        Some(XdgPathRejection::InteriorNul)
    } else if has_explicit_dot
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
        || (exactly_one && components.len() != 1)
    {
        Some(XdgPathRejection::NonNormalComponent)
    } else {
        None
    };
    match rejection {
        Some(rejection) => Err(XdgPathError {
            part,
            rejection,
            candidate: candidate.to_path_buf(),
        }),
        None => Ok(()),
    }
}

fn validated_xdg_inputs(
    app: &str,
    object: impl AsRef<Path>,
) -> Result<(XdgApplication, XdgRelativePath), XdgPathError> {
    Ok((XdgApplication::new(app)?, XdgRelativePath::new(object)?))
}

/// XDG directory family selected by the lifetime and ownership of persisted data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistenceKind {
    /// `$XDG_CONFIG_HOME` (preferences).
    Config,
    /// `$XDG_STATE_HOME` (session state).
    State,
    /// `$XDG_CACHE_HOME` (discardable cache).
    Cache,
}

impl PersistenceKind {
    /// The XDG environment variable this kind reads first.
    #[must_use]
    pub fn env_var(self) -> &'static str {
        match self {
            Self::Config => "XDG_CONFIG_HOME",
            Self::State => "XDG_STATE_HOME",
            Self::Cache => "XDG_CACHE_HOME",
        }
    }

    /// Returns the XDG default path relative to `$HOME`.
    ///
    /// Config uses `.config`, state uses `.local/state`, and cache uses `.cache`.
    #[must_use]
    pub fn fallback_relative(self) -> &'static str {
        match self {
            Self::Config => ".config",
            Self::State => ".local/state",
            Self::Cache => ".cache",
        }
    }
}

/// Resolves validated application paths from captured XDG roots.
///
/// A kind-specific root takes precedence over `$HOME` plus the XDG default.
/// Relative roots are ignored per the XDG Base Directory specification, so a
/// relative kind root falls through to an absolute `$HOME`. If neither usable
/// root exists, resolution returns `Ok(None)`. Resolution reads no environment
/// variables and rejects invalid descendants with [`XdgPathError`].
#[derive(Clone, Debug, Default)]
pub struct XdgResolver {
    config: Option<PathBuf>,
    state: Option<PathBuf>,
    cache: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl XdgResolver {
    /// Creates a resolver from optional explicit roots; `None` means unset.
    ///
    /// ```
    /// use enzui_core::persist::{PersistenceKind, XdgResolver};
    /// let resolver = XdgResolver::new(Some("/config"), None::<&str>, None::<&str>, Some("/home/u"));
    /// assert_eq!(resolver.resolve(PersistenceKind::Config, "app", "settings")
    ///     .unwrap().unwrap().to_str(), Some("/config/app/settings"));
    /// ```
    #[must_use]
    pub fn new(
        config: Option<impl Into<PathBuf>>,
        state: Option<impl Into<PathBuf>>,
        cache: Option<impl Into<PathBuf>>,
        home: Option<impl Into<PathBuf>>,
    ) -> Self {
        Self {
            config: config.map(Into::into),
            state: state.map(Into::into),
            cache: cache.map(Into::into),
            home: home.map(Into::into),
        }
    }

    /// Captures the current process XDG and home environment into a resolver.
    #[must_use]
    pub fn from_env() -> Self {
        fn non_empty(key: &str) -> Option<PathBuf> {
            let val = std::env::var_os(key)?;
            if val.is_empty() {
                None
            } else {
                Some(PathBuf::from(val))
            }
        }
        Self {
            config: non_empty("XDG_CONFIG_HOME"),
            state: non_empty("XDG_STATE_HOME"),
            cache: non_empty("XDG_CACHE_HOME"),
            home: non_empty("HOME"),
        }
    }

    /// Resolve an XDG-correct path for the given kind, app name and file.
    ///
    /// Returns `Ok(None)` when there is no usable absolute root.
    ///
    /// # Errors
    ///
    /// Returns [`XdgPathError`] when either descendant input is invalid,
    /// before any root is joined.
    pub fn resolve(
        &self,
        kind: PersistenceKind,
        app: &str,
        file: impl AsRef<Path>,
    ) -> Result<Option<PathBuf>, XdgPathError> {
        let (app, file) = validated_xdg_inputs(app, file)?;
        Ok(self.resolve_descendant(kind, &app, &file))
    }

    /// Resolve a path from inputs that have already passed XDG descendant
    /// validation.
    #[must_use]
    pub fn resolve_descendant(
        &self,
        kind: PersistenceKind,
        app: &XdgApplication,
        file: &XdgRelativePath,
    ) -> Option<PathBuf> {
        let kind_root = match kind {
            PersistenceKind::Config => &self.config,
            PersistenceKind::State => &self.state,
            PersistenceKind::Cache => &self.cache,
        };
        // A relative root would otherwise resolve under the process CWD.
        if let Some(dir) = kind_root.as_ref().filter(|p| p.is_absolute()) {
            let mut p = dir.clone();
            p.push(app.as_str());
            p.push(file.as_path());
            return Some(p);
        }
        if let Some(home) = self.home.as_ref().filter(|p| p.is_absolute()) {
            let mut p = home.clone();
            p.push(kind.fallback_relative());
            p.push(app.as_str());
            p.push(file.as_path());
            return Some(p);
        }
        None
    }

    /// Resolves a state path under the state root or `$HOME/.local/state`.
    ///
    /// Returns `Ok(None)` with no usable absolute root and [`XdgPathError`] for invalid descendants.
    ///
    /// # Errors
    ///
    /// [`XdgPathError`] when `file` is not a valid descendant path.
    pub fn resolve_state(
        &self,
        app: &str,
        file: impl AsRef<Path>,
    ) -> Result<Option<PathBuf>, XdgPathError> {
        self.resolve(PersistenceKind::State, app, file)
    }

    /// The home directory, if set.
    #[must_use]
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }
}

/// Resolves an XDG path from the current process environment.
///
/// Returns `Ok(None)` when there is no usable absolute root.
///
/// # Errors
/// Returns [`XdgPathError`] for an invalid application or object descendant.
pub fn xdg_path(
    kind: PersistenceKind,
    app: &str,
    file: impl AsRef<Path>,
) -> Result<Option<PathBuf>, XdgPathError> {
    XdgResolver::from_env().resolve(kind, app, file)
}

/// Serialization contract for values explicitly approved for file persistence.
///
/// Implementations provide a complete body, decode it, and supply a first-run
/// fallback. Downstream crates cannot implement this trait directly for
/// [`enzui_security::secret::Secret`] because both trait and type are foreign; a
/// local wrapper may opt in only by explicitly exposing the secret bytes.
pub trait Persistable: Send + 'static {
    /// Serialise to a string body.  The body is what
    /// [`atomic_state::save_atomic`] writes to disk, so it must be a
    /// complete, self-contained document (JSON, TOML, etc.).
    ///
    /// # Errors
    ///
    /// A message when the value cannot be serialized.
    fn persist_serialize(&self) -> Result<String, String>;

    /// Deserializes a body produced by [`Self::persist_serialize`].
    ///
    /// Implementations that keep a schema version across additive changes must accept
    /// older bodies that omit newly added fields.
    ///
    /// # Errors
    ///
    /// A message when `body` is not a valid document.
    fn persist_deserialize(body: &str) -> Result<Self, String>
    where
        Self: Sized;

    /// The default value used when no persisted state exists (first launch).
    fn persist_default() -> Self
    where
        Self: Sized;
}

/// Category of persistence failure for user-visible handling.
///
/// Match this independently of the detailed diagnostic string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PersistErrorKind {
    /// The OS refused the write — permission denied or a read-only
    /// filesystem.
    Permission,
    /// The target directory does not exist.
    MissingDestination,
    /// No space or write quota remains for the destination.
    StorageExhausted,
    /// The value could not be serialized to its persistence format.
    Serialization,
    /// The store refused to replace persisted state it could not load; the
    /// existing bytes are kept until replacement is explicitly authorized.
    Protected,
    /// An I/O error or state refusal that does not map to another variant.
    Unknown,
}

impl std::fmt::Display for PersistErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.safe_label())
    }
}

impl PersistErrorKind {
    fn safe_label(self) -> &'static str {
        match self {
            Self::Permission => "Permission denied",
            Self::MissingDestination => "Destination not found",
            Self::StorageExhausted => "No space left on device",
            Self::Serialization => "Could not prepare data for saving",
            Self::Protected => "Saving is blocked to protect existing saved data",
            Self::Unknown => "Save failed",
        }
    }
}

/// Persistence error with a stable category and full diagnostic text.
///
/// [`Self::user_summary`] is bounded and safe for UI surfaces;
/// [`Self::diagnostic`] and `Display` preserve the original payload for logs.
#[derive(Clone, Debug)]
pub struct PersistError {
    kind: PersistErrorKind,
    diagnostic: String,
}

impl PartialEq for PersistError {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.diagnostic == other.diagnostic
    }
}

impl Eq for PersistError {}

impl PersistError {
    fn classified(kind: PersistErrorKind, diagnostic: impl Into<String>) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.into(),
        }
    }

    /// Create a new error from a message.
    ///
    /// Classifies as [`PersistErrorKind::Unknown`] — callers that know
    /// the category should use [`serialization`](Self::serialization) or
    /// let the `From<io::Error>` impl classify automatically.
    pub fn new(msg: impl Into<String>) -> Self {
        Self::classified(PersistErrorKind::Unknown, msg)
    }

    /// Creates a [`PersistErrorKind::Serialization`] error from a diagnostic message.
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::classified(PersistErrorKind::Serialization, msg)
    }

    fn replacement_not_authorized() -> Self {
        Self::classified(
            PersistErrorKind::Protected,
            "persisted state was not loaded; explicit replacement authorization is required",
        )
    }

    fn reentry() -> Self {
        Self::classified(
            PersistErrorKind::Unknown,
            "persistence operation re-entered from a backend or token callback; \
             the caller may retry after the callback returns",
        )
    }

    fn frame_too_large(frame_bytes: Option<u64>, limit: RawByteLimit) -> Self {
        let diagnostic = match frame_bytes {
            Some(frame_bytes) => format!(
                "serialized state is too large: {frame_bytes} bytes exceeds {}",
                limit.bytes()
            ),
            None => "serialized state length overflow".to_owned(),
        };
        Self::serialization(diagnostic)
    }

    /// The error classification.
    #[must_use]
    pub fn kind(&self) -> PersistErrorKind {
        self.kind
    }

    /// Returns the original diagnostic text for logs.
    ///
    /// It may contain control characters or sensitive system detail and is not safe
    /// for direct display.
    #[must_use]
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    /// Returns localized category text without diagnostics, capped at 256 non-control characters.
    #[must_use]
    pub fn user_summary(&self) -> String {
        let key = match self.kind {
            PersistErrorKind::Permission => "enzui-persist-permission-denied",
            PersistErrorKind::MissingDestination => "enzui-persist-destination-missing",
            PersistErrorKind::StorageExhausted => "enzui-persist-storage-full",
            PersistErrorKind::Serialization => "enzui-persist-serialization",
            PersistErrorKind::Protected => "enzui-persist-protected",
            PersistErrorKind::Unknown => "enzui-persist-failed",
        };
        crate::localization::safe_text(key)
    }

    /// Returns the original diagnostic text.
    #[must_use]
    pub fn message(&self) -> &str {
        self.diagnostic()
    }
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.diagnostic)
    }
}

impl std::error::Error for PersistError {}

impl From<io::Error> for PersistError {
    fn from(e: io::Error) -> Self {
        let kind = classify_io_error(&e);
        Self::classified(kind, e.to_string())
    }
}

// Use the shared I/O classifier, but keep `NotADirectory` as Unknown because it
// does not mean a missing save destination.
fn classify_io_error(e: &io::Error) -> PersistErrorKind {
    match crate::io_class::IoFailureCategory::classify(e) {
        crate::io_class::IoFailureCategory::Permission => PersistErrorKind::Permission,
        crate::io_class::IoFailureCategory::NotFound => PersistErrorKind::MissingDestination,
        crate::io_class::IoFailureCategory::StorageExhausted => PersistErrorKind::StorageExhausted,
        crate::io_class::IoFailureCategory::NotADirectory
        | crate::io_class::IoFailureCategory::Unknown => PersistErrorKind::Unknown,
    }
}

/// Raw backend load result that distinguishes verified data, true absence, and refusal.
///
/// Callers must not infer absence with `Path::exists`, which hides metadata
/// failures and dangling symlinks.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackendLoad {
    /// A verified frame body.
    Loaded(String),
    /// No state file exists; a first save is safe.
    Absent,
    /// State was present or its absence could not be established safely.
    Refused {
        /// The atomic loader's machine-readable refusal class.
        reason: atomic_state::StateRefusal,
        /// Detailed text for logs; unsafe for direct user display.
        diagnostic: String,
    },
}

/// Failure to derive a complete-frame persistence limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RawByteLimitError {
    /// Framing plus the maximum body length overflowed `u64`.
    LengthOverflow,
    /// The complete frame would exceed the framework hard ceiling.
    ExceedsFrameworkMaximum {
        /// Requested complete-frame bytes.
        framed_bytes: u64,
        /// Framework hard ceiling in bytes.
        framework_max: u64,
    },
}

impl std::fmt::Display for RawByteLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthOverflow => f.write_str("framed persistence length overflow"),
            Self::ExceedsFrameworkMaximum {
                framed_bytes,
                framework_max,
            } => write!(
                f,
                "framed persistence length {framed_bytes} exceeds framework maximum {framework_max}"
            ),
        }
    }
}

impl std::error::Error for RawByteLimitError {}

/// Maximum complete framed-file size accepted by a persistence backend.
///
/// The limit includes the `atomic_state` header as well as the serialized body.
/// Files exactly at the limit are accepted. Production [`FileBackend`] callers
/// must select a limit for their own domain instead of inheriting the framework
/// ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawByteLimit(u64);

impl RawByteLimit {
    const FRAMEWORK_MAX: Self = Self(atomic_state::MAX_STATE_FILE_BYTES);
    const FRAME_FIXED_BYTES: u64 = (b"enzui-iced ".len() + b" v".len() + b"\nbody\n".len()) as u64;

    /// Creates an inclusive complete-frame byte limit.
    ///
    /// Values above [`atomic_state::MAX_STATE_FILE_BYTES`] are clamped to that
    /// hard ceiling.
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        if bytes > Self::FRAMEWORK_MAX.0 {
            Self::FRAMEWORK_MAX
        } else {
            Self(bytes)
        }
    }

    /// Derives an inclusive frame limit from the maximum serialized body size.
    ///
    /// The framing calculation includes the format identifier and the exact
    /// decimal width of `version`.
    ///
    /// # Errors
    ///
    /// Returns [`RawByteLimitError::LengthOverflow`] on arithmetic overflow and
    /// [`RawByteLimitError::ExceedsFrameworkMaximum`] above the hard ceiling.
    pub const fn for_body(
        format_id: &str,
        version: u32,
        maximum_body_bytes: u64,
    ) -> Result<Self, RawByteLimitError> {
        let format_bytes_wide = format_id.len() as u128;
        if format_bytes_wide > u64::MAX as u128 {
            return Err(RawByteLimitError::LengthOverflow);
        }
        let format_bytes = format_bytes_wide as u64;
        let Some(framed_bytes) = maximum_body_bytes.checked_add(Self::FRAME_FIXED_BYTES) else {
            return Err(RawByteLimitError::LengthOverflow);
        };
        let Some(framed_bytes) = framed_bytes.checked_add(format_bytes) else {
            return Err(RawByteLimitError::LengthOverflow);
        };
        let Some(framed_bytes) = framed_bytes.checked_add(decimal_digits(version)) else {
            return Err(RawByteLimitError::LengthOverflow);
        };
        if framed_bytes > Self::FRAMEWORK_MAX.0 {
            return Err(RawByteLimitError::ExceedsFrameworkMaximum {
                framed_bytes,
                framework_max: Self::FRAMEWORK_MAX.0,
            });
        }
        Ok(Self(framed_bytes))
    }

    /// Returns the inclusive complete-frame limit in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }
}

const fn decimal_digits(mut value: u32) -> u64 {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

/// Injectable raw persistence backend.
///
/// [`FileBackend`] performs atomic disk I/O; [`InMemoryBackend`] provides
/// process-local storage for tests.
pub trait PersistBackend: Send + Sync + 'static {
    /// Load the raw body without collapsing refusal into absence.
    fn load_raw(&self, format_id: &str, version: u32) -> BackendLoad;

    /// Writes the raw body atomically, blocking the calling thread until completion.
    ///
    /// # Errors
    ///
    /// A [`PersistError`] when the write fails.
    fn save_raw(&self, format_id: &str, version: u32, body: &str) -> Result<(), PersistError>;

    /// The inclusive byte limit for a complete framed document.
    ///
    /// The default keeps existing non-file test backends compatible. A backend
    /// that delegates to a bounded backend must return the delegate's limit.
    fn raw_byte_limit(&self) -> RawByteLimit {
        RawByteLimit::FRAMEWORK_MAX
    }
}

/// Filesystem backend using atomic replacement and typed, versioned loads.
///
/// An interrupted write leaves the previous file intact. Loads and saves use
/// the same caller-selected complete-frame byte limit; reads also refuse
/// symlinks, special files, invalid headers, and unverified absence.
#[derive(Clone, Debug)]
pub struct FileBackend {
    path: PathBuf,
    raw_byte_limit: RawByteLimit,
}

impl FileBackend {
    /// Creates a backend for an explicit file path and domain-sized byte limit.
    pub fn new(path: impl Into<PathBuf>, raw_byte_limit: RawByteLimit) -> Self {
        Self {
            path: path.into(),
            raw_byte_limit,
        }
    }

    /// Creates a backend at the XDG path selected from the process environment.
    ///
    /// Returns `Ok(None)` when there is no usable absolute root.
    ///
    /// # Errors
    /// Returns [`XdgPathError`] for invalid descendants.
    pub fn xdg(
        kind: PersistenceKind,
        app: &str,
        file: impl AsRef<Path>,
        raw_byte_limit: RawByteLimit,
    ) -> Result<Option<Self>, XdgPathError> {
        Ok(xdg_path(kind, app, file)?.map(|path| Self::new(path, raw_byte_limit)))
    }

    /// Creates an XDG backend using an explicit [`XdgResolver`].
    ///
    /// # Errors
    /// Returns [`XdgPathError`] for invalid descendants.
    pub fn xdg_with_resolver(
        resolver: &XdgResolver,
        kind: PersistenceKind,
        app: &str,
        file: impl AsRef<Path>,
        raw_byte_limit: RawByteLimit,
    ) -> Result<Option<Self>, XdgPathError> {
        Ok(resolver
            .resolve(kind, app, file)?
            .map(|path| Self::new(path, raw_byte_limit)))
    }

    /// The resolved file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The inclusive byte limit used by both loads and saves.
    #[must_use]
    pub const fn raw_byte_limit(&self) -> RawByteLimit {
        self.raw_byte_limit
    }
}

impl PersistBackend for FileBackend {
    fn load_raw(&self, format_id: &str, version: u32) -> BackendLoad {
        let (loaded, reason) =
            atomic_state::load_bounded(&self.path, format_id, version, self.raw_byte_limit.bytes());
        match (loaded, reason) {
            (atomic_state::StateLoad::Loaded(body), None) => BackendLoad::Loaded(body),
            (
                atomic_state::StateLoad::UsedDefaults { .. },
                Some(atomic_state::StateRefusal::Absent),
            ) => BackendLoad::Absent,
            (atomic_state::StateLoad::UsedDefaults { note }, Some(reason)) => {
                BackendLoad::Refused {
                    reason,
                    diagnostic: note,
                }
            }
            // Treat a missing refusal reason conservatively; guessing `Absent` would authorize replacement of existing data.
            (atomic_state::StateLoad::UsedDefaults { note }, None) => BackendLoad::Refused {
                reason: atomic_state::StateRefusal::ReadFailed,
                diagnostic: format!("atomic loader omitted refusal reason: {note}"),
            },
            (atomic_state::StateLoad::Loaded(_), Some(reason)) => BackendLoad::Refused {
                reason,
                diagnostic: "atomic loader returned a body with a refusal reason".into(),
            },
        }
    }

    fn save_raw(&self, format_id: &str, version: u32, body: &str) -> Result<(), PersistError> {
        let frame_bytes = atomic_state::framed_len(format_id, version, body);
        if frame_bytes.is_none_or(|bytes| bytes > self.raw_byte_limit.bytes()) {
            return Err(PersistError::frame_too_large(
                frame_bytes,
                self.raw_byte_limit,
            ));
        }
        atomic_state::save_atomic(&self.path, format_id, version, body)?;
        Ok(())
    }

    fn raw_byte_limit(&self) -> RawByteLimit {
        self.raw_byte_limit
    }
}

/// Thread-safe, process-local persistence backend for tests.
#[derive(Clone, Debug, Default)]
pub struct InMemoryBackend {
    inner: Arc<Mutex<HashMap<String, InMemoryEntry>>>,
}

#[derive(Clone, Debug)]
struct InMemoryEntry {
    version: u32,
    body: String,
}

impl InMemoryBackend {
    /// Create a fresh, empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a copy of the stored body for `format_id`, if present.
    pub fn peek(&self, format_id: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(format_id)
            .map(|e| e.body.clone())
    }
}

impl PersistBackend for InMemoryBackend {
    fn load_raw(&self, format_id: &str, version: u32) -> BackendLoad {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = guard.get(format_id) else {
            return BackendLoad::Absent;
        };
        // Match versions exactly so the in-memory backend refuses the same older and
        // newer frames as the file backend.
        if entry.version != version {
            return BackendLoad::Refused {
                reason: atomic_state::StateRefusal::UnexpectedHeader,
                diagnostic: format!(
                    "stored version {} does not match reader version {version}",
                    entry.version
                ),
            };
        }
        BackendLoad::Loaded(entry.body.clone())
    }

    fn save_raw(&self, format_id: &str, version: u32, body: &str) -> Result<(), PersistError> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                format_id.to_owned(),
                InMemoryEntry {
                    version,
                    body: body.to_owned(),
                },
            );
        Ok(())
    }
}

/// Prepared, blocking persistence operation suitable for off-thread execution.
///
/// Operations from one store are published in preparation order: once a newer
/// operation enters publication, an older one becomes a successful no-op even
/// if the newer backend call later errors or panics.
pub struct SaveOp {
    backend: Arc<dyn PersistBackend>,
    format_id: String,
    version: u32,
    body: String,
    sequence: u64,
    ordering: Arc<SaveOrdering>,
    admitted_epoch: u64,
}

// The lock protects only metadata transitions; extension-point calls
// (backend I/O, serializers, defaults) execute outside.
#[derive(Debug, Default)]
struct StoreOpState {
    next_sequence: u64,
    // Never rolled back: a post-publication error leaves older bodies
    // superseded.
    highest_attempted: u64,
    write_blocked: bool,
    block_epoch: u64,
    // Saves wait for active_loads == 0 before entering the backend.
    active_loads: u32,
    // Serializes backend writes; loads wait for active_save == false.
    active_save: bool,
}

#[derive(Debug)]
struct SaveOrdering {
    state: Mutex<StoreOpState>,
    state_changed: Condvar,
}

impl Default for SaveOrdering {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            state_changed: Condvar::new(),
        }
    }
}

impl SaveOrdering {
    fn lock(&self) -> MutexGuard<'_, StoreOpState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn admit(&self) -> Result<(u64, u64), PersistError> {
        let mut state = self.lock();
        if state.write_blocked {
            return Err(PersistError::replacement_not_authorized());
        }
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .expect("persistence save sequence overflow");
        Ok((state.next_sequence, state.block_epoch))
    }

    fn reserve(&self) -> (u64, u64) {
        let mut state = self.lock();
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .expect("persistence save sequence overflow");
        (state.next_sequence, state.block_epoch)
    }

    fn accept_reservation(&self, admitted_epoch: u64) -> Result<(), PersistError> {
        let state = self.lock();
        if state.write_blocked || state.block_epoch != admitted_epoch {
            return Err(PersistError::replacement_not_authorized());
        }
        Ok(())
    }

    // Sticky: later successful or absent loads cannot clear a block.
    fn publish_load_block(state: &mut StoreOpState, blocked: bool) {
        if blocked {
            state.block_epoch = state.block_epoch.wrapping_add(1);
            state.write_blocked = true;
        }
    }

    fn supersede_all(&self) {
        let mut state = self.lock();
        while state.active_save {
            if PERSIST_SAVE_DEPTH.with(std::cell::Cell::get) > 0 {
                break;
            }
            state = self
                .state_changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.next_sequence > state.highest_attempted {
            state.highest_attempted = state.next_sequence;
        }
    }
}

// Whether a save wrote its body or was superseded by a newer attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveOpOutcome {
    Published,
    Superseded,
}

impl SaveOp {
    /// Executes the prepared save, blocking until its backend write completes.
    ///
    /// Newer attempts supersede older operations from the same store. Saves
    /// wait for in-progress loads so a returned refusal cannot be followed by
    /// an older save replacing its bytes.
    ///
    /// # Errors
    ///
    /// A [`PersistError`] when a newer save superseded this one or the write fails.
    pub fn execute(self) -> Result<(), PersistError> {
        self.execute_inner().map(|_outcome| ())
    }

    fn execute_inner(self) -> Result<SaveOpOutcome, PersistError> {
        {
            let mut state = self.ordering.lock();
            while state.active_loads > 0 {
                if PERSIST_LOAD_DEPTH.with(std::cell::Cell::get) > 0 {
                    return Err(PersistError::reentry());
                }
                state = self
                    .ordering
                    .state_changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            while state.active_save {
                if PERSIST_SAVE_DEPTH.with(std::cell::Cell::get) > 0 {
                    return Err(PersistError::reentry());
                }
                state = self
                    .ordering
                    .state_changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if self.sequence <= state.highest_attempted {
                return Ok(SaveOpOutcome::Superseded);
            }
            if state.write_blocked || state.block_epoch != self.admitted_epoch {
                return Err(PersistError::replacement_not_authorized());
            }
            state.highest_attempted = self.sequence;
            state.active_save = true;
        }

        let _save_guard = ActiveSaveGuard {
            ordering: Arc::clone(&self.ordering),
        };
        let _depth = DepthGuard::enter(&PERSIST_SAVE_DEPTH);
        let result = self
            .backend
            .save_raw(&self.format_id, self.version, &self.body);
        drop(_depth);
        drop(_save_guard);

        result.map(|()| SaveOpOutcome::Published)
    }
}

const _: () = {
    fn _assert_send<T: Send + 'static>() {}
    fn _check() {
        _assert_send::<SaveOp>();
    }
};

/// Typed load result preserving the distinction between loaded data, absence,
/// atomic-loader refusal, and deserialization failure.
///
/// Failure variants carry a fallback and write-protect the store until explicit
/// replacement authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "inspect the load outcome before choosing a startup fallback"]
#[non_exhaustive]
pub enum LoadOutcome<T> {
    /// A matching frame was successfully deserialized.
    Loaded(T),
    /// No file exists, so the supplied value is the normal first-run default.
    Absent {
        /// The application's default value.
        default: T,
    },
    /// The atomic loader refused the file before deserialization.
    Refused {
        /// A safe startup fallback. It has not replaced the stored bytes.
        fallback: T,
        /// Machine-readable refusal classification from the atomic loader.
        reason: atomic_state::StateRefusal,
        /// Detailed text for logs; unsafe for direct user display.
        diagnostic: String,
    },
    /// The frame was valid, but [`Persistable::persist_deserialize`] rejected
    /// its body.
    DeserializeFailed {
        /// A safe startup fallback. It has not replaced the stored bytes.
        fallback: T,
        /// Detailed decoder text for logs; unsafe for direct user display.
        diagnostic: String,
    },
}

impl<T> LoadOutcome<T> {
    /// Borrow the loaded value or startup fallback.
    #[must_use]
    pub fn value(&self) -> &T {
        match self {
            Self::Loaded(value) => value,
            Self::Absent { default } => default,
            Self::Refused { fallback, .. } | Self::DeserializeFailed { fallback, .. } => fallback,
        }
    }

    /// Consume the outcome and return the loaded value or startup fallback.
    ///
    /// This deliberately does not authorize writing that fallback. A store
    /// remains protected after either failure variant.
    #[must_use]
    pub fn into_value(self) -> T {
        match self {
            Self::Loaded(value) => value,
            Self::Absent { default } => default,
            Self::Refused { fallback, .. } | Self::DeserializeFailed { fallback, .. } => fallback,
        }
    }

    /// User-safe load feedback, separate from the detailed diagnostic.
    #[must_use]
    pub fn user_summary(&self) -> Option<String> {
        match self {
            Self::Loaded(_) | Self::Absent { .. } => None,
            Self::Refused { .. } => {
                Some(crate::localization::safe_text("enzui-persist-open-failed"))
            }
            Self::DeserializeFailed { .. } => Some(crate::localization::safe_text(
                "enzui-persist-decode-failed",
            )),
        }
    }

    /// Returns detailed load failure text for logs; unsafe for direct user display.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Refused { diagnostic, .. } | Self::DeserializeFailed { diagnostic, .. } => {
                Some(diagnostic)
            }
            Self::Loaded(_) | Self::Absent { .. } => None,
        }
    }
}

/// Typed persistence store with exact schema-version matching.
///
/// Loads distinguish absence, refusal, and decode failure. Failure fallbacks do
/// not authorize writes; call [`Self::authorize_replacement`] only after an
/// explicit reset or migration decision. Saves are ordered per store.
pub struct Store<T: Persistable> {
    backend: Arc<dyn PersistBackend>,
    format_id: String,
    version: u32,
    raw_byte_limit: RawByteLimit,
    save_ordering: Arc<SaveOrdering>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Persistable> Clone for Store<T> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            format_id: self.format_id.clone(),
            version: self.version,
            raw_byte_limit: self.raw_byte_limit,
            save_ordering: Arc::clone(&self.save_ordering),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T: Persistable> Store<T> {
    /// Creates a store for a backend, stable format identifier, and exact schema version.
    pub fn new(backend: impl PersistBackend, format_id: impl Into<String>, version: u32) -> Self {
        let raw_byte_limit = backend.raw_byte_limit();
        Self {
            backend: Arc::new(backend),
            format_id: format_id.into(),
            version,
            raw_byte_limit,
            save_ordering: Arc::new(SaveOrdering::default()),
            _marker: std::marker::PhantomData,
        }
    }

    /// Loads state while preserving absence and failure classifications.
    ///
    /// Refusal or decoding failure returns a fallback and atomically write-protects
    /// the store against prepared saves until explicit authorization.
    pub fn load(&self) -> LoadOutcome<T> {
        {
            let mut state = self.save_ordering.lock();
            while state.active_save {
                // Load-from-save reentry: the same thread runs save_raw.
                if PERSIST_SAVE_DEPTH.with(std::cell::Cell::get) > 0 {
                    break;
                }
                state = self
                    .save_ordering
                    .state_changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            state.active_loads += 1;
        }

        let mut load_guard = ActiveLoadGuard {
            ordering: Arc::clone(&self.save_ordering),
            blocked: false,
        };
        let _depth = DepthGuard::enter(&PERSIST_LOAD_DEPTH);
        let (outcome, blocked) = match self.backend.load_raw(&self.format_id, self.version) {
            BackendLoad::Loaded(body) => {
                let frame_bytes = atomic_state::framed_len(&self.format_id, self.version, &body);
                if frame_bytes.is_none_or(|bytes| bytes > self.raw_byte_limit.bytes()) {
                    (
                        LoadOutcome::Refused {
                            fallback: T::persist_default(),
                            reason: atomic_state::StateRefusal::TooLarge,
                            diagnostic: PersistError::frame_too_large(
                                frame_bytes,
                                self.raw_byte_limit,
                            )
                            .diagnostic,
                        },
                        true,
                    )
                } else {
                    match T::persist_deserialize(&body) {
                        Ok(value) => (LoadOutcome::Loaded(value), false),
                        Err(diagnostic) => (
                            LoadOutcome::DeserializeFailed {
                                fallback: T::persist_default(),
                                diagnostic,
                            },
                            true,
                        ),
                    }
                }
            }
            BackendLoad::Absent => (
                LoadOutcome::Absent {
                    default: T::persist_default(),
                },
                false,
            ),
            BackendLoad::Refused { reason, diagnostic } => (
                LoadOutcome::Refused {
                    fallback: T::persist_default(),
                    reason,
                    diagnostic,
                },
                true,
            ),
        };

        load_guard.blocked = blocked;
        drop(_depth);
        drop(load_guard);
        outcome
    }

    /// Allows saves prepared after this call to replace state protected by a failed load.
    ///
    /// Operations admitted before the refusal remain refused.
    pub fn authorize_replacement(&self) {
        self.save_ordering.lock().write_blocked = false;
    }

    /// Returns whether saves are blocked by an unacknowledged load refusal or decode failure.
    #[must_use]
    pub fn is_write_blocked(&self) -> bool {
        self.save_ordering.lock().write_blocked
    }

    /// Serializes a value into a prepared save without performing backend I/O.
    ///
    /// Returns an error on serialization failure or while replacement is not authorized.
    ///
    /// # Errors
    ///
    /// A [`PersistError`] when the value cannot be serialized or the save cannot be ordered.
    pub fn prepare_save(&self, value: &T) -> Result<SaveOp, PersistError> {
        let body = self.prepare_save_body(value)?;
        let (sequence, admitted_epoch) = self.save_ordering.admit()?;
        Ok(self.save_op(body, sequence, admitted_epoch))
    }

    fn reserve_save_order(&self) -> (u64, u64) {
        self.save_ordering.reserve()
    }

    fn prepare_reserved_save(
        &self,
        value: &T,
        sequence: u64,
        admitted_epoch: u64,
    ) -> Result<SaveOp, PersistError> {
        let body = self.prepare_save_body(value)?;
        self.save_ordering.accept_reservation(admitted_epoch)?;
        Ok(self.save_op(body, sequence, admitted_epoch))
    }

    fn prepare_save_body(&self, value: &T) -> Result<String, PersistError> {
        let body = value
            .persist_serialize()
            .map_err(PersistError::serialization)?;
        let frame_bytes = atomic_state::framed_len(&self.format_id, self.version, &body);
        if frame_bytes.is_none_or(|bytes| bytes > self.raw_byte_limit.bytes()) {
            return Err(PersistError::frame_too_large(
                frame_bytes,
                self.raw_byte_limit,
            ));
        }
        Ok(body)
    }

    fn save_op(&self, body: String, sequence: u64, admitted_epoch: u64) -> SaveOp {
        SaveOp {
            backend: Arc::clone(&self.backend),
            format_id: self.format_id.clone(),
            version: self.version,
            body,
            sequence,
            ordering: Arc::clone(&self.save_ordering),
            admitted_epoch,
        }
    }

    /// Save synchronously.  Convenience for exit flushes where there is no
    /// runtime to dispatch a `Task` to.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare_save`], and a [`PersistError`] when the write fails.
    pub fn save_blocking(&self, value: &T) -> Result<(), PersistError> {
        self.prepare_save(value)?.execute()
    }

    /// The format id this store was created with.
    pub fn format_id(&self) -> &str {
        &self.format_id
    }

    /// The schema version this store was created with.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The inclusive complete-frame byte limit enforced before decode and save.
    #[must_use]
    pub const fn raw_byte_limit(&self) -> RawByteLimit {
        self.raw_byte_limit
    }

    /// Supersedes every previously admitted operation, waiting for any
    /// in-progress backend call to finish first.
    pub(crate) fn supersede_admitted(&self) {
        self.save_ordering.supersede_all();
    }
}

/// Identifies one asynchronous load request for diagnostics.
///
/// Settlement also carries a private allocation identity, so wrapping this
/// display counter cannot make an old completion current again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadGeneration(u64);

impl LoadGeneration {
    /// Returns the process-local diagnostic sequence number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug)]
struct LoadStamp {
    generation: LoadGeneration,
    identity: Arc<()>,
}

/// Failure to start or finish an off-thread persistence load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AsyncLoadError {
    /// A worker already occupies this controller's load capacity.
    Busy {
        /// The active worker's diagnostic generation.
        active_generation: LoadGeneration,
    },
    /// The operating system refused to create the load thread.
    Spawn {
        /// The class of the thread creation error.
        kind: io::ErrorKind,
    },
    /// The worker stopped without returning its typed load outcome.
    WorkerStopped,
}

impl std::fmt::Display for AsyncLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { active_generation } => write!(
                f,
                "persistence load worker generation {} is still active",
                active_generation.get()
            ),
            Self::Spawn { kind } => write!(f, "persistence load worker could not start: {kind}"),
            Self::WorkerStopped => f.write_str("persistence load worker stopped without a result"),
        }
    }
}

impl std::error::Error for AsyncLoadError {}

/// Completion emitted by [`LoadController::begin`].
///
/// Pass this value to [`LoadController::settle`] instead of extracting its
/// outcome directly; only the controller can establish that the generation is
/// still current.
pub struct LoadCompletion<T> {
    stamp: LoadStamp,
    result: Result<LoadOutcome<T>, AsyncLoadError>,
}

impl<T> std::fmt::Debug for LoadCompletion<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadCompletion")
            .field("generation", &self.stamp.generation)
            .field("finished", &self.result.is_ok())
            .finish_non_exhaustive()
    }
}

impl<T> LoadCompletion<T> {
    /// The request generation carried by this completion.
    #[must_use]
    pub const fn generation(&self) -> LoadGeneration {
        self.stamp.generation
    }
}

/// Result of generation-checking an asynchronous load completion.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "only Applied state may replace application state"]
pub enum LoadSettlement<T> {
    /// The current request completed and may replace the published loading state.
    Applied(LoadOutcome<T>),
    /// The current worker failed before producing a load classification.
    WorkerFailed(AsyncLoadError),
    /// A superseded request completed; its value was discarded.
    DiscardedStale {
        /// The discarded request's diagnostic generation.
        generation: LoadGeneration,
    },
}

/// Generation owner for responsive persistence startup.
///
/// [`Self::begin`] admits at most one blocking worker and returns an iced task
/// that only awaits its one-shot result. Publish a loading state before returning
/// that task. [`Self::invalidate`] makes the result stale without releasing the
/// worker slot; [`Self::settle`] releases that slot only for its exact completion.
#[derive(Debug)]
pub struct LoadController {
    generation: u64,
    identity: Arc<()>,
    active: Option<LoadStamp>,
}

impl Default for LoadController {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadController {
    /// Creates an idle generation owner.
    #[must_use]
    pub fn new() -> Self {
        Self {
            generation: 0,
            identity: Arc::new(()),
            active: None,
        }
    }

    fn advance(&mut self) -> LoadStamp {
        self.generation = self.generation.wrapping_add(1);
        self.identity = Arc::new(());
        LoadStamp {
            generation: LoadGeneration(self.generation),
            identity: Arc::clone(&self.identity),
        }
    }

    /// Starts one off-thread load and marks its generation current.
    ///
    /// The returned task performs no filesystem or decoding work when polled;
    /// it only awaits the dedicated worker's one-shot completion.
    ///
    /// # Errors
    ///
    /// Returns [`AsyncLoadError::Busy`] while another worker remains active, or
    /// [`AsyncLoadError::Spawn`] when the OS refuses the worker thread.
    pub fn begin<T: Persistable>(
        &mut self,
        store: &Store<T>,
    ) -> Result<Task<LoadCompletion<T>>, AsyncLoadError> {
        self.begin_with_spawn(store, |worker| {
            std::thread::Builder::new()
                .name("enzui-persist-load".into())
                .spawn(worker)
                .map(drop)
        })
    }

    fn begin_with_spawn<T: Persistable>(
        &mut self,
        store: &Store<T>,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
    ) -> Result<Task<LoadCompletion<T>>, AsyncLoadError> {
        if let Some(active) = &self.active {
            return Err(AsyncLoadError::Busy {
                active_generation: active.generation,
            });
        }
        let stamp = self.advance();
        self.active = Some(stamp.clone());
        let worker_stamp = stamp.clone();
        let stopped_stamp = stamp;
        let store = Store::clone(store);
        let (sender, receiver) = oneshot::channel();
        let worker = Box::new(move || {
            let completion = LoadCompletion {
                stamp: worker_stamp,
                result: Ok(store.load()),
            };
            let _ = sender.send(completion);
        });
        if let Err(error) = spawn(worker) {
            self.active = None;
            return Err(AsyncLoadError::Spawn { kind: error.kind() });
        }

        Ok(Task::perform(receiver, move |result| {
            result.unwrap_or(LoadCompletion {
                stamp: stopped_stamp,
                result: Err(AsyncLoadError::WorkerStopped),
            })
        }))
    }

    /// Makes the active result stale without releasing its worker slot.
    pub fn invalidate(&mut self) -> LoadGeneration {
        self.advance().generation
    }

    /// Retires the matching active worker and applies its result only if current.
    pub fn settle<T>(&mut self, completion: LoadCompletion<T>) -> LoadSettlement<T> {
        let owns_active_slot = self
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(&active.identity, &completion.stamp.identity));
        if !owns_active_slot {
            return LoadSettlement::DiscardedStale {
                generation: completion.stamp.generation,
            };
        }
        self.active = None;
        if !Arc::ptr_eq(&self.identity, &completion.stamp.identity) {
            return LoadSettlement::DiscardedStale {
                generation: completion.stamp.generation,
            };
        }
        match completion.result {
            Ok(outcome) => LoadSettlement::Applied(outcome),
            Err(error) => LoadSettlement::WorkerFailed(error),
        }
    }

    /// Whether this controller's single worker slot is occupied.
    #[must_use]
    pub const fn is_loading(&self) -> bool {
        self.active.is_some()
    }
}

/// Value wrapper whose revision changes whenever mutable access is granted.
///
/// It implements `Deref` but not `DerefMut`; [`Self::get_mut`] is the only
/// mutable door and bumps unconditionally, favoring an extra save over a lost
/// one. [`Self::revision`] is suitable as a [`SaveGate`] token.
#[derive(Clone, Debug, Default)]
pub struct Tracked<V> {
    value: V,
    revision: u64,
}

impl<V> Tracked<V> {
    /// Wrap `value` at revision 0.
    pub const fn new(value: V) -> Self {
        Self { value, revision: 0 }
    }

    /// The current revision, suitable for [`SaveGate::reserve_save_if_changed`].
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Borrows the wrapped value immutably.
    pub const fn get(&self) -> &V {
        &self.value
    }

    /// Bumps the revision and returns the only mutable reference exposed by this type.
    pub fn get_mut(&mut self) -> &mut V {
        self.revision = self.revision.wrapping_add(1);
        &mut self.value
    }

    /// Replaces the value and bumps the revision.
    pub fn set(&mut self, value: V) {
        self.revision = self.revision.wrapping_add(1);
        self.value = value;
    }

    /// Consume the wrapper, returning the inner value and dropping the
    /// revision.
    pub fn into_inner(self) -> V {
        self.value
    }
}

impl<V> std::ops::Deref for Tracked<V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.value
    }
}

// Do not implement `DerefMut`; mutable access that bypasses `get_mut` would
// allow changes without advancing the revision.

/// Skips materialization and persistence while a cheap change token is unchanged.
///
/// [`Self::reserve_save_if_changed`] admits a changed token without building or
/// serializing its value. A token is committed only after
/// [`GatedSaveOp::execute`] publishes its body. Failures, supersession, panics,
/// and drops make the token retryable on the next reservation attempt.
pub struct SaveGate<T: Persistable, Tok: PartialEq + Clone + Send + 'static> {
    store: Store<T>,
    shared: Arc<Mutex<GateShared<Tok>>>,
    local_committed: Option<Tok>,
    local_committed_gen: u64,
    local_pending: Vec<(Tok, u64)>,
}

// The caller-owned cache keeps token Clone and PartialEq implementations out
// of this mutex; completed operations transfer tokens through `committed`.
struct GateShared<Tok> {
    committed: Option<Tok>,
    committed_generation: u64,
    generation: u64,
    // Generations retired by execute/drop; drained by sync_local.
    retired: Vec<u64>,
}

impl<Tok> GateShared<Tok> {
    fn new() -> Self {
        Self {
            committed: None,
            committed_generation: 0,
            generation: 0,
            retired: Vec::new(),
        }
    }
}

/// A changed-token reservation whose model construction and serialization are deferred.
///
/// Send the plan to the blocking owner and call [`Self::prepare`] or
/// [`Self::prepare_and_execute`]. Dropping it retires the pending generation,
/// making the same token retryable after the gate next synchronizes.
#[must_use = "a gated save plan must be prepared or deliberately dropped"]
pub struct GatedSavePlan<T: Persistable, Tok: PartialEq + Clone + Send + 'static> {
    store: Store<T>,
    shared: Arc<Mutex<GateShared<Tok>>>,
    generation: u64,
    save_sequence: u64,
    admitted_epoch: u64,
    token: Option<Tok>,
    finalized: bool,
}

impl<T: Persistable, Tok: PartialEq + Clone + Send + 'static> GatedSavePlan<T, Tok> {
    /// Builds and serializes the reserved value without performing backend I/O.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when serialization fails or replacement is
    /// no longer authorized.
    ///
    /// # Panics
    ///
    /// Propagates a panic from `build` or serialization; the reservation is
    /// retired while unwinding.
    pub fn prepare(
        self,
        build: impl FnOnce() -> T + Send + 'static,
    ) -> Result<GatedSaveOp<Tok>, PersistError> {
        self.prepare_with(build)
    }

    /// Builds, serializes, and executes the reserved save on the current thread.
    ///
    /// # Errors
    ///
    /// Returns a persistence error from serialization, admission, or the backend.
    ///
    /// # Panics
    ///
    /// Propagates a panic from `build`, serialization, or the backend; the
    /// reservation is retired while unwinding.
    pub fn prepare_and_execute(
        self,
        build: impl FnOnce() -> T + Send + 'static,
    ) -> Result<(), PersistError> {
        self.prepare(build)?.execute()
    }

    fn prepare_with(mut self, build: impl FnOnce() -> T) -> Result<GatedSaveOp<Tok>, PersistError> {
        let value = build();
        let op =
            self.store
                .prepare_reserved_save(&value, self.save_sequence, self.admitted_epoch)?;
        let token = self
            .token
            .take()
            .expect("GatedSavePlan token present while preparing");
        self.finalized = true;
        Ok(GatedSaveOp {
            inner: Some(op),
            shared: Arc::clone(&self.shared),
            generation: self.generation,
            token: Some(token),
            finalized: false,
        })
    }
}

impl<T: Persistable, Tok: PartialEq + Clone + Send + 'static> Drop for GatedSavePlan<T, Tok> {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.retired.push(self.generation);
    }
}

/// A prepared persistence operation that commits or retires its gate token.
///
/// Dropping without calling [`execute`](Self::execute) makes the same token
/// retryable. Panics during execution also retire its generation.
pub struct GatedSaveOp<Tok: Send + 'static> {
    inner: Option<SaveOp>,
    shared: Arc<Mutex<GateShared<Tok>>>,
    generation: u64,
    token: Option<Tok>,
    finalized: bool,
}

impl<Tok: PartialEq + Clone + Send + 'static> GatedSaveOp<Tok> {
    /// Executes the prepared save, blocking until its backend write completes.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when publication is refused or the backend fails.
    ///
    /// # Panics
    ///
    /// Propagates a panic from the backend; the generation is retired while unwinding.
    pub fn execute(mut self) -> Result<(), PersistError> {
        let superseded = {
            let shared = self
                .shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.generation < shared.committed_generation
        };
        if superseded {
            return Ok(());
        }

        let op = self
            .inner
            .take()
            .expect("GatedSaveOp::execute called after move");
        match op.execute_inner() {
            Ok(SaveOpOutcome::Published) => {
                let token = self
                    .token
                    .take()
                    .expect("GatedSaveOp token present for published op");
                // Take any displaced committed token out so its Drop runs
                // after the lock is released.
                let _displaced = {
                    let mut shared = self
                        .shared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let displaced = if self.generation > shared.committed_generation {
                        let old = shared.committed.take();
                        shared.committed = Some(token);
                        shared.committed_generation = self.generation;
                        old
                    } else {
                        Some(token)
                    };
                    shared.retired.push(self.generation);
                    self.finalized = true;
                    displaced
                };
                Ok(())
            }
            Ok(SaveOpOutcome::Superseded) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<Tok: Send + 'static> Drop for GatedSaveOp<Tok> {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.retired.push(self.generation);
    }
}

const _: () = {
    fn _assert_send<T: Send + 'static>() {}
    fn _check<T: Persistable>() {
        _assert_send::<GatedSavePlan<T, u64>>();
        _assert_send::<GatedSaveOp<u64>>();
    }
};

impl<T: Persistable, Tok: PartialEq + Clone + Send + 'static> SaveGate<T, Tok> {
    /// Wraps a store with no token remembered, so the first token is admitted.
    pub fn new(store: Store<T>) -> Self {
        Self {
            store,
            shared: Arc::new(Mutex::new(GateShared::new())),
            local_committed: None,
            local_committed_gen: 0,
            local_pending: Vec::new(),
        }
    }

    /// Wraps a store with `token` already marked as persisted.
    ///
    /// This avoids a redundant initial write after [`Store::load`] when the loaded value is unchanged.
    pub fn with_baseline(store: Store<T>, token: Tok) -> Self {
        Self {
            store,
            shared: Arc::new(Mutex::new(GateShared::new())),
            local_committed: Some(token),
            local_committed_gen: 0,
            local_pending: Vec::new(),
        }
    }

    /// Marks a token as already persisted without writing.
    ///
    /// Waits for any in-progress backend call to finish, then supersedes
    /// every previously admitted operation so none can write afterward.
    pub fn rebaseline(&mut self, token: Tok) {
        self.store.supersede_admitted();
        // Take any lingering committed token out so its Drop runs after
        // the lock is released.
        let (_displaced, rebase_gen) = {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A `u64` generation never wraps in practice.
            shared.generation = shared.generation.wrapping_add(1);
            let g = shared.generation;
            let old = shared.committed.take();
            shared.committed_generation = g;
            shared.retired.clear();
            (old, g)
        };
        self.local_committed = Some(token);
        self.local_committed_gen = rebase_gen;
        self.local_pending.clear();
    }

    fn sync_local(&mut self) {
        // Extract under the lock; assign after release so the previous
        // local_committed Drop runs outside the gate mutex.
        let (new_token, new_gen, retired) = {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let token = if shared.committed_generation > self.local_committed_gen {
                (shared.committed.take(), shared.committed_generation)
            } else {
                (None, self.local_committed_gen)
            };
            let retired = if shared.retired.is_empty() {
                Vec::new()
            } else {
                std::mem::take(&mut shared.retired)
            };
            (token.0, token.1, retired)
        };
        if new_gen > self.local_committed_gen {
            if let Some(token) = new_token {
                self.local_committed = Some(token);
            }
            self.local_committed_gen = new_gen;
        }
        if !retired.is_empty() {
            self.local_pending.retain(|(_, g)| !retired.contains(g));
        }
    }

    /// Reserves a save when `token` differs from committed and pending tokens.
    ///
    /// This caller-side step does not construct `T`, serialize it, or call the
    /// backend. Move the returned [`GatedSavePlan`] to a blocking owner; dropping
    /// it makes the exact token eligible again after the next call synchronizes.
    pub fn reserve_save_if_changed(&mut self, token: Tok) -> Option<GatedSavePlan<T, Tok>> {
        self.sync_local();

        if self.local_committed.as_ref() == Some(&token) {
            return None;
        }
        if self.local_pending.iter().any(|(tok, _)| *tok == token) {
            return None;
        }

        let store = self.store.clone();
        let shared = Arc::clone(&self.shared);
        let (save_sequence, admitted_epoch) = store.reserve_save_order();
        let generation = {
            let mut shared = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            shared.generation = shared.generation.checked_add(1)?;
            shared.generation
        };
        let pending_token = token.clone();
        let plan = GatedSavePlan {
            store,
            shared,
            generation,
            save_sequence,
            admitted_epoch,
            token: Some(token),
            finalized: false,
        };
        self.local_pending.push((pending_token, generation));
        Some(plan)
    }

    /// Prepares a save only when `token` differs from committed and pending tokens.
    ///
    /// This compatibility method builds and serializes on the current thread.
    /// Use [`Self::reserve_save_if_changed`] to defer that work to a blocking owner.
    ///
    /// # Errors
    ///
    /// A [`PersistError`] when the value cannot be serialized or the save cannot be ordered.
    pub fn prepare_save_if_changed(
        &mut self,
        token: Tok,
        build: impl FnOnce() -> T,
    ) -> Result<Option<GatedSaveOp<Tok>>, PersistError> {
        self.reserve_save_if_changed(token)
            .map(|plan| plan.prepare_with(build))
            .transpose()
    }

    /// Returns the wrapped store for direct load and metadata access.
    pub fn store(&self) -> &Store<T> {
        &self.store
    }
}

#[cfg(test)]
mod tests;
