//! No-clobber quarantine of a regular file into unpredictable sibling names.
//!
//! The operation retains the source inode and parent directory. Every namespace
//! mutation is an exclusive link or `RENAME_NOREPLACE`; an entry raced into a
//! known name can be moved into custody but is never unlinked. Returned custody
//! identifies which paths were proved to name the retained source inode.

#[cfg(target_os = "linux")]
use std::ffi::{OsStr, OsString};
use std::fmt;
#[cfg(target_os = "linux")]
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// How many fresh recovery names are tried before giving up.
pub const MAX_QUARANTINE_ATTEMPTS: u32 = 16;

/// Device-and-inode identity retained by a caller that already read the source.
///
/// This is distinct from quarantine's own descriptor identity: the expected
/// form lets a caller refuse a replacement that arrived after its read but
/// before quarantine opened the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpectedSourceIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl ExpectedSourceIdentity {
    /// The identity of the file the caller read, from its device and inode numbers.
    #[cfg(unix)]
    #[must_use]
    pub const fn from_dev_ino(device: u64, inode: u64) -> Self {
        Self { device, inode }
    }

    /// The device number.
    #[cfg(unix)]
    #[must_use]
    pub const fn device(self) -> u64 {
        self.device
    }

    /// The inode number.
    #[cfg(unix)]
    #[must_use]
    pub const fn inode(self) -> u64 {
        self.inode
    }

    /// An identity on a platform without device and inode numbers.
    #[cfg(not(unix))]
    #[must_use]
    pub const fn unsupported() -> Self {
        Self {}
    }

    /// Whether this platform can bind quarantine to a file identity.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        cfg!(unix)
    }

    #[cfg(target_os = "linux")]
    const fn file_identity(self) -> FileIdentity {
        (self.device, self.inode)
    }
}

/// Whether a custody path was proved to name the retained source, another
/// inode, or could not be verified through the original parent pathname.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CustodyIdentity {
    /// The path names the retained source inode.
    RetainedSource,
    /// The path names another inode, such as one raced into the name.
    Other,
    /// The path could not be checked through the original parent.
    Unverified,
}

/// A retained path paired with its verified relationship to the source inode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyPath {
    path: PathBuf,
    identity: CustodyIdentity,
}

impl CustodyPath {
    /// The retained path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the path was proved to name.
    #[must_use]
    pub const fn identity(&self) -> CustodyIdentity {
        self.identity
    }
}

/// Bounded custody for source, publication, and moved names retained by one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCustody {
    paths: Vec<CustodyPath>,
}

impl RecoveryCustody {
    fn one(path: CustodyPath) -> Self {
        Self { paths: vec![path] }
    }

    fn push(&mut self, path: CustodyPath) {
        self.paths.push(path);
    }

    fn mark_unverified(&mut self) {
        for path in &mut self.paths {
            path.identity = CustodyIdentity::Unverified;
        }
    }

    /// Every retained path, with what each was proved to name.
    #[must_use]
    pub fn paths(&self) -> &[CustodyPath] {
        &self.paths
    }

    /// The retained paths proved to name the source inode.
    pub fn source_paths(&self) -> impl Iterator<Item = &Path> {
        self.paths.iter().filter_map(|entry| {
            (entry.identity == CustodyIdentity::RetainedSource).then_some(entry.path.as_path())
        })
    }
}

/// Filesystem step associated with a quarantine failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuarantineOperation {
    /// Opening the source's parent directory.
    OpenParent,
    /// Opening the source file.
    OpenSource,
    /// Generating an unpredictable recovery name.
    GenerateName,
    /// Linking the source under its recovery name.
    Publish,
    /// Moving the original name into custody.
    MoveToCustody,
    /// Comparing a path's inode with the source's.
    VerifyIdentity,
    /// Flushing the parent directory to disk.
    SyncParent,
}

impl fmt::Display for QuarantineOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OpenParent => "open parent",
            Self::OpenSource => "open source",
            Self::GenerateName => "generate recovery name",
            Self::Publish => "publish recovery name",
            Self::MoveToCustody => "move name into custody",
            Self::VerifyIdentity => "verify file identity",
            Self::SyncParent => "synchronize parent",
        })
    }
}

/// Bounded diagnostic for an operating-system failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineFailure {
    operation: QuarantineOperation,
    kind: io::ErrorKind,
    message: String,
}

impl QuarantineFailure {
    fn io(operation: QuarantineOperation, error: &io::Error) -> Self {
        Self::message(operation, error.kind(), &error.to_string())
    }

    fn message(operation: QuarantineOperation, kind: io::ErrorKind, message: &str) -> Self {
        let mut message = message.to_owned();
        if message.len() > 512 {
            let mut end = 512;
            while !message.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            message.truncate(end);
        }
        Self {
            operation,
            kind,
            message,
        }
    }

    /// The step that failed.
    #[must_use]
    pub const fn operation(&self) -> QuarantineOperation {
        self.operation
    }

    /// The operating system's error kind.
    #[must_use]
    pub const fn kind(&self) -> io::ErrorKind {
        self.kind
    }
}

impl fmt::Display for QuarantineFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.operation, self.message)
    }
}

impl std::error::Error for QuarantineFailure {}

/// Pre-publication refusal; no source or destination name was mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuarantineRefusal {
    /// This platform cannot quarantine by file identity.
    Unsupported,
    /// The source path has no usable file name.
    InvalidSourcePath,
    /// The source is a symlink.
    Symlink,
    /// The source is a directory.
    Directory,
    /// The source is not a regular file.
    SpecialFile,
    /// The operating system refused a step before anything moved.
    Failure(QuarantineFailure),
}

impl fmt::Display for QuarantineRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("identity-bound quarantine is unsupported"),
            Self::InvalidSourcePath => formatter.write_str("source path has no usable file name"),
            Self::Symlink => formatter.write_str("source is a symlink"),
            Self::Directory => formatter.write_str("source is a directory"),
            Self::SpecialFile => formatter.write_str("source is not a regular file"),
            Self::Failure(failure) => failure.fmt(formatter),
        }
    }
}

impl std::error::Error for QuarantineRefusal {}

/// Typed completion, refusal, durability uncertainty, or partial custody.
///
/// `custody` records every relevant name retained, published, or moved and never
/// identifies source bytes without a successful inode comparison. A
/// `source_removed` value only describes movement of the retained source inode,
/// not a foreign entry raced into the original pathname.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuarantineOutcome {
    /// The source moved under a recovery name, durably.
    Moved {
        /// Every name retained, published or moved.
        custody: RecoveryCustody,
    },
    /// Every recovery name tried was taken; nothing moved.
    CollisionExhausted {
        /// The path the caller asked to quarantine.
        source_path: PathBuf,
        /// How many recovery names were tried.
        attempts: u32,
    },
    /// The source is not the file the caller read; nothing moved.
    SourceMismatch {
        /// The path the caller asked to quarantine.
        source_path: PathBuf,
        /// The identity the caller read.
        expected: ExpectedSourceIdentity,
        /// The identity found at the path, if one could be read.
        observed: Option<ExpectedSourceIdentity>,
    },
    /// Another file took the source's name mid-move; the source still has a
    /// verified recovery name, and nothing was unlinked.
    SourceReplaced {
        /// The path the caller asked to quarantine.
        source_path: PathBuf,
        /// Every name retained, published or moved.
        custody: RecoveryCustody,
    },
    /// The names moved, but flushing them to disk failed.
    DurabilityUncertain {
        /// The path the caller asked to quarantine.
        source_path: PathBuf,
        /// Every name retained, published or moved.
        custody: RecoveryCustody,
        /// Whether the source inode left its original name.
        source_removed: bool,
        /// The step that failed, and why.
        failure: QuarantineFailure,
    },
    /// A step failed after some names had moved.
    PartialCustody {
        /// The path the caller asked to quarantine.
        source_path: PathBuf,
        /// Every name retained, published or moved.
        custody: RecoveryCustody,
        /// Whether the source inode left its original name.
        source_removed: bool,
        /// The step that failed, and why.
        failure: QuarantineFailure,
    },
    /// Refused before any name was touched.
    Refused {
        /// The path the caller asked to quarantine.
        source_path: PathBuf,
        /// Why it was refused.
        reason: QuarantineRefusal,
    },
}

/// Quarantine a regular file without overwriting or unlinking any raced entry.
///
/// Linux `RENAME_NOREPLACE` moves each observed pathname into a fresh custody
/// name before identity is decided. A raced foreign inode therefore remains
/// named and is reported as [`CustodyIdentity::Other`]. The normal result keeps
/// two hard links to the retained source: the recovery publication and the
/// source-removal custody name. Keeping both is what avoids an identity-check
/// followed by a racy pathname unlink.
#[must_use]
pub fn quarantine_sibling(source: impl AsRef<Path>) -> QuarantineOutcome {
    quarantine_sibling_impl(source.as_ref(), None)
}

/// Quarantine only if the opened source still has the identity retained by a prior reader.
///
/// The comparison uses the newly opened source descriptor before any staging
/// link or namespace mutation. A pathname pre-check followed by
/// [`quarantine_sibling`] does not provide this contract.
#[must_use]
pub fn quarantine_sibling_expected(
    source: impl AsRef<Path>,
    expected: ExpectedSourceIdentity,
) -> QuarantineOutcome {
    quarantine_sibling_impl(source.as_ref(), Some(expected))
}

#[cfg(target_os = "linux")]
fn quarantine_sibling_impl(
    source: &Path,
    expected: Option<ExpectedSourceIdentity>,
) -> QuarantineOutcome {
    let source_path = source.to_path_buf();
    let Some(source_name) = source.file_name() else {
        return refused(source_path, QuarantineRefusal::InvalidSourcePath);
    };
    if source_name.is_empty() || source_name == OsStr::new(".") || source_name == OsStr::new("..") {
        return refused(source_path, QuarantineRefusal::InvalidSourcePath);
    }

    let parent = match RetainedParent::open(&normalized_parent(source)) {
        Ok(parent) => parent,
        Err(error) => return refused_io(source_path, QuarantineOperation::OpenParent, error),
    };

    #[cfg(any(test, feature = "fault-injection"))]
    fault::run_after_parent_open(source);

    let retained = match RetainedSource::open(&parent, source_name) {
        Ok(source) => source,
        Err(SourceOpenError::Refused(reason)) => return refused(source_path, reason),
        Err(SourceOpenError::Io(error)) => {
            if let Some(expected) = expected
                && error.kind() == io::ErrorKind::NotFound
            {
                return QuarantineOutcome::SourceMismatch {
                    source_path,
                    expected,
                    observed: None,
                };
            }
            return refused_io(source_path, QuarantineOperation::OpenSource, error);
        }
    };
    if let Some(expected) = expected
        && retained.identity != expected.file_identity()
    {
        return QuarantineOutcome::SourceMismatch {
            source_path,
            expected,
            observed: Some(ExpectedSourceIdentity::from_dev_ino(
                retained.identity.0,
                retained.identity.1,
            )),
        };
    }
    if let Err(error) = parent.validate_reachable() {
        return refused_io(source_path, QuarantineOperation::OpenParent, error);
    }

    let staging_name = match publish_staging(&parent, &retained, source_name, source) {
        Ok(name) => name,
        Err(outcome) => return outcome,
    };
    let staging_path = source.with_file_name(&staging_name);
    if let Err(error) = parent.verify_entry(&staging_name, retained.identity) {
        return partial_one(
            source_path,
            false,
            parent.custody(&staging_name, staging_path, retained.identity),
            QuarantineOperation::VerifyIdentity,
            error,
        );
    }

    #[cfg(any(test, feature = "fault-injection"))]
    fault::run_after_recovery_identity(&staging_path);

    let recovery_name = match move_to_new_name(
        &parent,
        &staging_name,
        source_name,
        "recovery",
        source,
        retained.identity,
    ) {
        Ok(name) => name,
        Err(outcome) => return outcome,
    };
    let recovery_path = source.with_file_name(&recovery_name);
    let mut custody = parent.custody(&recovery_name, recovery_path, retained.identity);
    if custody.paths[0].identity != CustodyIdentity::RetainedSource {
        return QuarantineOutcome::PartialCustody {
            source_path,
            custody,
            source_removed: false,
            failure: identity_failure("recovery custody does not name the retained source"),
        };
    }
    if let Err(error) = parent.validate_reachable() {
        custody.mark_unverified();
        return QuarantineOutcome::PartialCustody {
            source_path,
            custody,
            source_removed: false,
            failure: QuarantineFailure::io(QuarantineOperation::OpenParent, &error),
        };
    }
    if let Err(error) = parent.sync(SyncPhase::BeforeSourceMove) {
        return durability_outcome(
            &parent,
            retained.identity,
            source_path,
            custody,
            false,
            error,
        );
    }

    match parent.entry_identity(source_name) {
        Ok(identity) if identity == retained.identity => {}
        Ok(_) => {
            parent.refresh(&mut custody, retained.identity);
            return source_replaced_outcome(source_path, custody);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            parent.refresh(&mut custody, retained.identity);
            return source_replaced_outcome(source_path, custody);
        }
        Err(error) => {
            parent.refresh(&mut custody, retained.identity);
            return QuarantineOutcome::PartialCustody {
                source_path,
                custody,
                source_removed: false,
                failure: QuarantineFailure::io(QuarantineOperation::VerifyIdentity, &error),
            };
        }
    }

    #[cfg(any(test, feature = "fault-injection"))]
    fault::run_after_source_identity(source);

    let source_custody_name = match move_to_new_name(
        &parent,
        source_name,
        source_name,
        "source-custody",
        source,
        retained.identity,
    ) {
        Ok(name) => name,
        Err(QuarantineOutcome::PartialCustody {
            custody: moved,
            failure,
            ..
        }) => {
            parent.refresh(&mut custody, retained.identity);
            if failure.kind() == io::ErrorKind::NotFound {
                return source_replaced_outcome(source_path, custody);
            }
            for entry in moved.paths {
                custody.push(entry);
            }
            return QuarantineOutcome::PartialCustody {
                source_path,
                custody,
                source_removed: false,
                failure,
            };
        }
        Err(_) => {
            parent.refresh(&mut custody, retained.identity);
            return source_replaced_outcome(source_path, custody);
        }
    };
    let source_custody_path = source.with_file_name(&source_custody_name);
    let source_custody_entry =
        parent.classify(&source_custody_name, source_custody_path, retained.identity);
    let moved_source = source_custody_entry.identity == CustodyIdentity::RetainedSource;
    custody.push(source_custody_entry);
    if !moved_source {
        parent.refresh(&mut custody, retained.identity);
        return source_replaced_outcome(source_path, custody);
    }

    if let Err(error) = parent.sync(SyncPhase::AfterSourceMove) {
        return durability_outcome(
            &parent,
            retained.identity,
            source_path,
            custody,
            true,
            error,
        );
    }
    parent.refresh(&mut custody, retained.identity);
    if parent.validate_reachable().is_err()
        || custody
            .paths
            .iter()
            .any(|entry| entry.identity != CustodyIdentity::RetainedSource)
    {
        return QuarantineOutcome::PartialCustody {
            source_path,
            custody,
            source_removed: true,
            failure: identity_failure("final recovery custody could not be verified"),
        };
    }
    QuarantineOutcome::Moved { custody }
}

#[cfg(target_os = "linux")]
fn publish_staging(
    parent: &RetainedParent,
    retained: &RetainedSource,
    source_name: &OsStr,
    source_path: &Path,
) -> Result<OsString, QuarantineOutcome> {
    for attempt in 1..=MAX_QUARANTINE_ATTEMPTS {
        let name = random_sibling_name(source_name, "recovery-stage").map_err(|error| {
            refused_io(
                source_path.to_path_buf(),
                QuarantineOperation::GenerateName,
                io::Error::other(error.to_string()),
            )
        })?;
        #[cfg(any(test, feature = "fault-injection"))]
        fault::run_before_publish(&source_path.with_file_name(&name));
        match parent.publish(&retained.file, &name) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if attempt == MAX_QUARANTINE_ATTEMPTS {
                    return Err(QuarantineOutcome::CollisionExhausted {
                        source_path: source_path.to_path_buf(),
                        attempts: MAX_QUARANTINE_ATTEMPTS,
                    });
                }
            }
            Err(error) => {
                return Err(refused_io(
                    source_path.to_path_buf(),
                    QuarantineOperation::Publish,
                    error,
                ));
            }
        }
    }
    Err(QuarantineOutcome::CollisionExhausted {
        source_path: source_path.to_path_buf(),
        attempts: MAX_QUARANTINE_ATTEMPTS,
    })
}

#[cfg(target_os = "linux")]
fn move_to_new_name(
    parent: &RetainedParent,
    from: &OsStr,
    source_name: &OsStr,
    label: &str,
    source_path: &Path,
    source_identity: FileIdentity,
) -> Result<OsString, QuarantineOutcome> {
    for _ in 0..MAX_QUARANTINE_ATTEMPTS {
        let name = random_sibling_name(source_name, label).map_err(|error| {
            QuarantineOutcome::PartialCustody {
                source_path: source_path.to_path_buf(),
                custody: parent.custody(from, source_path.with_file_name(from), source_identity),
                source_removed: false,
                failure: QuarantineFailure::message(
                    QuarantineOperation::GenerateName,
                    io::ErrorKind::Other,
                    &error.to_string(),
                ),
            }
        })?;
        match parent.rename_new(from, &name) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(QuarantineOutcome::PartialCustody {
                    source_path: source_path.to_path_buf(),
                    custody: parent.custody(
                        from,
                        source_path.with_file_name(from),
                        source_identity,
                    ),
                    source_removed: false,
                    failure: QuarantineFailure::io(QuarantineOperation::MoveToCustody, &error),
                });
            }
        }
    }
    Err(QuarantineOutcome::PartialCustody {
        source_path: source_path.to_path_buf(),
        custody: parent.custody(from, source_path.with_file_name(from), source_identity),
        source_removed: false,
        failure: QuarantineFailure::message(
            QuarantineOperation::MoveToCustody,
            io::ErrorKind::AlreadyExists,
            "custody-name collision limit reached",
        ),
    })
}

#[cfg(target_os = "linux")]
fn source_replaced_outcome(source_path: PathBuf, custody: RecoveryCustody) -> QuarantineOutcome {
    if custody
        .paths
        .iter()
        .any(|entry| entry.identity == CustodyIdentity::RetainedSource)
    {
        QuarantineOutcome::SourceReplaced {
            source_path,
            custody,
        }
    } else {
        QuarantineOutcome::PartialCustody {
            source_path,
            custody,
            source_removed: false,
            failure: identity_failure("source replacement left no verified recovery path"),
        }
    }
}

#[cfg(target_os = "linux")]
fn durability_outcome(
    parent: &RetainedParent,
    source_identity: FileIdentity,
    source_path: PathBuf,
    mut custody: RecoveryCustody,
    source_removed: bool,
    error: io::Error,
) -> QuarantineOutcome {
    parent.refresh(&mut custody, source_identity);
    let failure = QuarantineFailure::io(QuarantineOperation::SyncParent, &error);
    if custody
        .paths
        .iter()
        .all(|entry| entry.identity == CustodyIdentity::RetainedSource)
    {
        QuarantineOutcome::DurabilityUncertain {
            source_path,
            custody,
            source_removed,
            failure,
        }
    } else {
        QuarantineOutcome::PartialCustody {
            source_path,
            custody,
            source_removed,
            failure,
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn quarantine_sibling_impl(
    source: &Path,
    _expected: Option<ExpectedSourceIdentity>,
) -> QuarantineOutcome {
    refused(source.to_path_buf(), QuarantineRefusal::Unsupported)
}

fn refused(source_path: PathBuf, reason: QuarantineRefusal) -> QuarantineOutcome {
    QuarantineOutcome::Refused {
        source_path,
        reason,
    }
}

fn refused_io(
    source_path: PathBuf,
    operation: QuarantineOperation,
    error: io::Error,
) -> QuarantineOutcome {
    refused(
        source_path,
        QuarantineRefusal::Failure(QuarantineFailure::io(operation, &error)),
    )
}

fn identity_failure(message: &'static str) -> QuarantineFailure {
    QuarantineFailure::message(
        QuarantineOperation::VerifyIdentity,
        io::ErrorKind::PermissionDenied,
        message,
    )
}

#[cfg(target_os = "linux")]
fn normalized_parent(source: &Path) -> PathBuf {
    match source.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(target_os = "linux")]
fn random_sibling_name(
    source_name: &OsStr,
    label: &str,
) -> Result<OsString, crate::random::RandomError> {
    use std::fmt::Write as _;

    let random = crate::random::random_array::<16>()?;
    let mut suffix = String::new();
    suffix.push('.');
    suffix.push_str(label);
    suffix.push('-');
    for byte in random {
        let _ = write!(suffix, "{byte:02x}");
    }
    let mut name = source_name.to_os_string();
    name.push(suffix);
    Ok(name)
}

#[cfg(target_os = "linux")]
type FileIdentity = (u64, u64);

#[cfg(target_os = "linux")]
struct RetainedSource {
    file: File,
    identity: FileIdentity,
}

#[cfg(target_os = "linux")]
enum SourceOpenError {
    Refused(QuarantineRefusal),
    Io(io::Error),
}

#[cfg(target_os = "linux")]
impl RetainedSource {
    fn open(parent: &RetainedParent, name: &OsStr) -> Result<Self, SourceOpenError> {
        use rustix::fs::{FileType, Mode, OFlags};

        let fd = rustix::fs::openat(
            &parent.file,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .map_err(SourceOpenError::Io)?;
        let stat = rustix::fs::fstat(&fd)
            .map_err(io::Error::from)
            .map_err(SourceOpenError::Io)?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile => Ok(Self {
                file: File::from(fd),
                identity: (stat.st_dev, stat.st_ino),
            }),
            FileType::Symlink => Err(SourceOpenError::Refused(QuarantineRefusal::Symlink)),
            FileType::Directory => Err(SourceOpenError::Refused(QuarantineRefusal::Directory)),
            _ => Err(SourceOpenError::Refused(QuarantineRefusal::SpecialFile)),
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
enum SyncPhase {
    BeforeSourceMove,
    AfterSourceMove,
}

#[cfg(target_os = "linux")]
struct RetainedParent {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
}

#[cfg(target_os = "linux")]
impl RetainedParent {
    fn open(path: &Path) -> io::Result<Self> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags};

        let named = rustix::fs::statat(rustix::fs::CWD, path, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        if FileType::from_raw_mode(named.st_mode) != FileType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source parent is not a directory",
            ));
        }
        let fd = rustix::fs::openat(
            rustix::fs::CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let held = rustix::fs::fstat(&fd).map_err(io::Error::from)?;
        let identity = (held.st_dev, held.st_ino);
        if identity != (named.st_dev, named.st_ino) {
            return Err(identity_error("source parent changed while it was opened"));
        }
        Ok(Self {
            path: path.to_path_buf(),
            file: File::from(fd),
            identity,
        })
    }

    fn validate_reachable(&self) -> io::Result<()> {
        use rustix::fs::{AtFlags, FileType};

        let stat = rustix::fs::statat(rustix::fs::CWD, &self.path, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory
            && (stat.st_dev, stat.st_ino) == self.identity
        {
            Ok(())
        } else {
            Err(identity_error("source parent pathname was replaced"))
        }
    }

    fn publish(&self, source: &File, name: &OsStr) -> io::Result<()> {
        use std::os::fd::AsRawFd;

        let destination = Path::new("/proc/self/fd")
            .join(self.file.as_raw_fd().to_string())
            .join(name);
        crate::tempstage::link_new_by_identity(source, &destination)
    }

    fn rename_new(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        rustix::fs::renameat_with(
            &self.file,
            from,
            &self.file,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)
    }

    fn entry_identity(&self, name: &OsStr) -> io::Result<FileIdentity> {
        let stat = rustix::fs::statat(&self.file, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        Ok((stat.st_dev, stat.st_ino))
    }

    fn verify_entry(&self, name: &OsStr, identity: FileIdentity) -> io::Result<()> {
        if self.entry_identity(name)? == identity {
            Ok(())
        } else {
            Err(identity_error(
                "recovery name does not identify the retained source",
            ))
        }
    }

    fn classify(&self, name: &OsStr, path: PathBuf, source: FileIdentity) -> CustodyPath {
        let identity = match self.entry_identity(name) {
            Ok(identity) if identity == source => CustodyIdentity::RetainedSource,
            Ok(_) => CustodyIdentity::Other,
            Err(_) => CustodyIdentity::Unverified,
        };
        CustodyPath { path, identity }
    }

    fn custody(&self, name: &OsStr, path: PathBuf, source: FileIdentity) -> RecoveryCustody {
        let mut custody = RecoveryCustody::one(self.classify(name, path, source));
        self.refresh(&mut custody, source);
        custody
    }

    fn refresh(&self, custody: &mut RecoveryCustody, source: FileIdentity) {
        for entry in &mut custody.paths {
            let Some(name) = entry.path.file_name() else {
                entry.identity = CustodyIdentity::Unverified;
                continue;
            };
            entry.identity = match self.entry_identity(name) {
                Ok(identity) if identity == source => CustodyIdentity::RetainedSource,
                Ok(_) => CustodyIdentity::Other,
                Err(_) => CustodyIdentity::Unverified,
            };
        }
        if self.validate_reachable().is_err() {
            custody.mark_unverified();
        }
    }

    fn sync(&self, _phase: SyncPhase) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        if let Some(error) = fault::take_sync_failure(_phase) {
            return Err(error);
        }
        self.file.sync_all()
    }
}

#[cfg(target_os = "linux")]
fn partial_one(
    source_path: PathBuf,
    source_removed: bool,
    custody: RecoveryCustody,
    operation: QuarantineOperation,
    error: io::Error,
) -> QuarantineOutcome {
    QuarantineOutcome::PartialCustody {
        source_path,
        custody,
        source_removed,
        failure: QuarantineFailure::io(operation, &error),
    }
}

#[cfg(target_os = "linux")]
fn identity_error(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

/// Deterministic race and durability controls available only with fault injection.
#[cfg(all(target_os = "linux", any(test, feature = "fault-injection")))]
pub mod fault {
    use std::cell::{Cell, RefCell};
    use std::io;
    use std::path::Path;

    use super::SyncPhase;

    type PathAction = Box<dyn FnMut(&Path)>;

    thread_local! {
        static AFTER_PARENT_OPEN: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static BEFORE_PUBLISH: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static AFTER_RECOVERY_IDENTITY: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static AFTER_SOURCE_IDENTITY: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static SYNC_BEFORE_SOURCE: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static SYNC_AFTER_SOURCE: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
    }

    /// Runs after parent retention and before the source is opened.
    pub fn arm_after_parent_open(action: Box<dyn FnMut(&Path)>) {
        AFTER_PARENT_OPEN.with(|slot| *slot.borrow_mut() = Some(action));
    }

    /// Runs after candidate selection and before each exclusive publication.
    pub fn arm_before_publish(action: Box<dyn FnMut(&Path)>) {
        BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = Some(action));
    }

    /// Runs after recovery identity observation and before its no-clobber move.
    pub fn arm_after_recovery_identity(action: Box<dyn FnMut(&Path)>) {
        AFTER_RECOVERY_IDENTITY.with(|slot| *slot.borrow_mut() = Some(action));
    }

    /// Runs after source identity observation and before its no-clobber move.
    pub fn arm_after_source_identity(action: Box<dyn FnMut(&Path)>) {
        AFTER_SOURCE_IDENTITY.with(|slot| *slot.borrow_mut() = Some(action));
    }

    /// Fails the publication durability barrier before source movement.
    pub fn arm_sync_failure_before_source(kind: io::ErrorKind) {
        SYNC_BEFORE_SOURCE.with(|slot| slot.set(Some(kind)));
    }

    /// Fails the source-removal durability barrier after source movement.
    pub fn arm_sync_failure_after_source(kind: io::ErrorKind) {
        SYNC_AFTER_SOURCE.with(|slot| slot.set(Some(kind)));
    }

    /// Whether a quarantine control remains armed on this thread.
    #[must_use]
    pub fn is_armed() -> bool {
        AFTER_PARENT_OPEN.with(|slot| slot.borrow().is_some())
            || BEFORE_PUBLISH.with(|slot| slot.borrow().is_some())
            || AFTER_RECOVERY_IDENTITY.with(|slot| slot.borrow().is_some())
            || AFTER_SOURCE_IDENTITY.with(|slot| slot.borrow().is_some())
            || SYNC_BEFORE_SOURCE.with(|slot| slot.get().is_some())
            || SYNC_AFTER_SOURCE.with(|slot| slot.get().is_some())
    }

    /// Disarms all quarantine controls on this thread.
    pub fn clear() {
        AFTER_PARENT_OPEN.with(|slot| *slot.borrow_mut() = None);
        BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = None);
        AFTER_RECOVERY_IDENTITY.with(|slot| *slot.borrow_mut() = None);
        AFTER_SOURCE_IDENTITY.with(|slot| *slot.borrow_mut() = None);
        SYNC_BEFORE_SOURCE.with(|slot| slot.set(None));
        SYNC_AFTER_SOURCE.with(|slot| slot.set(None));
    }

    pub(super) fn run_after_parent_open(path: &Path) {
        AFTER_PARENT_OPEN.with(|slot| {
            if let Some(mut action) = slot.borrow_mut().take() {
                action(path);
            }
        });
    }

    pub(super) fn run_before_publish(path: &Path) {
        BEFORE_PUBLISH.with(|slot| {
            if let Some(action) = slot.borrow_mut().as_mut() {
                action(path);
            }
        });
    }

    pub(super) fn run_after_recovery_identity(path: &Path) {
        AFTER_RECOVERY_IDENTITY.with(|slot| {
            if let Some(mut action) = slot.borrow_mut().take() {
                action(path);
            }
        });
    }

    pub(super) fn run_after_source_identity(path: &Path) {
        AFTER_SOURCE_IDENTITY.with(|slot| {
            if let Some(mut action) = slot.borrow_mut().take() {
                action(path);
            }
        });
    }

    pub(super) fn take_sync_failure(phase: SyncPhase) -> Option<io::Error> {
        let kind = match phase {
            SyncPhase::BeforeSourceMove => SYNC_BEFORE_SOURCE.with(std::cell::Cell::take),
            SyncPhase::AfterSourceMove => SYNC_AFTER_SOURCE.with(std::cell::Cell::take),
        }?;
        Some(io::Error::new(
            kind,
            "injected quarantine directory sync failure",
        ))
    }
}
