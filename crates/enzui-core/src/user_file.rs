//! Atomic publication for files selected and owned by the user.
//!
//! Unlike [`crate::atomic_state`], this module neither creates parent
//! directories nor imposes private-state permissions. Existing regular files
//! keep their mode and ownership, new files use `0644`, and a final symlink is
//! retained while its regular target is replaced. The strict contract refuses
//! dangling symlinks, non-regular files, and multiply-linked destinations.
//!
//! [`save`] and [`save_with`] are one-shot: they resolve the pathname each call
//! and honor the final-symlink contract. When a caller must parse one document
//! and later replace exactly the inode it parsed, [`DocumentAuthority`] retains
//! the destination inode and its parent directory by descriptor, offers a
//! bounded read that never reopens the pathname, and conditionally replaces only
//! the retained identity. The retained authority refuses a symbolic-link final
//! component rather than binding a target that a later read cannot prove stable,
//! so a caller that needs the final-symlink contract uses the one-shot path.
//! [`DocumentAuthority::create_new`] and
//! [`DocumentAuthority::create_new_with`] are the strict Save-As shape: they
//! resolve and retain the parent directory under the same secure policy,
//! publish the staged inode atomically with no-clobber semantics, verify the
//! destination name reaches the staged inode, and hand back authority to that
//! exact inode without a pathname reopen window.

use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) const NEW_DOCUMENT_MODE: u32 = 0o644;
pub(crate) const NEW_STATE_MODE: u32 = 0o600;

/// Whether an atomic save may replace or create its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePolicy {
    /// Require one existing regular destination and replace it.
    ReplaceExisting,
    /// Replace an existing regular destination or create a new file.
    ReplaceOrCreate,
    /// Create a new file and fail if any entry already has the selected name.
    CreateNew,
}

/// The operation at which a save failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SaveOperation {
    /// Opening and retaining a parent directory.
    OpenParent,
    /// Resolving or validating the selected destination.
    InspectDestination,
    /// Creating the unnamed staged inode.
    CreateStage,
    /// Applying the destination's ownership and mode to the stage.
    PreserveMetadata,
    /// Running the caller's writer.
    Write,
    /// Synchronizing the complete staged file.
    SyncFile,
    /// Atomically publishing the staged inode.
    Publish,
    /// Synchronizing the retained parent directory.
    SyncParent,
    /// Revalidating identities after a blocking operation.
    Revalidate,
    /// Refusing a replacement because the generation counter cannot advance
    /// without repeating a value. Never reached by the one-shot save entry
    /// points, which keep no generation.
    GenerationExhausted,
}

impl fmt::Display for SaveOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::OpenParent => "open parent directory",
            Self::InspectDestination => "inspect destination",
            Self::CreateStage => "create staging file",
            Self::PreserveMetadata => "preserve destination metadata",
            Self::GenerationExhausted => "generation counter exhausted",
            Self::Write => "write staging file",
            Self::SyncFile => "sync staging file",
            Self::Publish => "publish staging file",
            Self::SyncParent => "sync parent directory",
            Self::Revalidate => "revalidate file identity",
        };
        f.write_str(name)
    }
}

/// An atomic user-file save failure.
#[derive(Debug)]
pub enum SaveError {
    /// This call did not publish its staged bytes.
    PrePublication {
        /// The failed operation.
        operation: SaveOperation,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Publication occurred, but current pathname identity or durability could
    /// not be confirmed.
    PublishedButNotDurable {
        /// The failed operation.
        operation: SaveOperation,
        /// The underlying I/O error.
        source: io::Error,
    },
}

impl SaveError {
    pub(crate) fn before(operation: SaveOperation, source: impl Into<io::Error>) -> Self {
        Self::PrePublication {
            operation,
            source: source.into(),
        }
    }

    pub(crate) fn after(operation: SaveOperation, source: impl Into<io::Error>) -> Self {
        Self::PublishedButNotDurable {
            operation,
            source: source.into(),
        }
    }

    /// Whether a publication syscall completed before the failure.
    #[must_use]
    pub fn was_published(&self) -> bool {
        matches!(self, Self::PublishedButNotDurable { .. })
    }

    /// The operation that failed.
    #[must_use]
    pub fn operation(&self) -> SaveOperation {
        match self {
            Self::PrePublication { operation, .. }
            | Self::PublishedButNotDurable { operation, .. } => *operation,
        }
    }

    /// The underlying I/O error.
    #[must_use]
    pub fn io_error(&self) -> &io::Error {
        match self {
            Self::PrePublication { source, .. } | Self::PublishedButNotDurable { source, .. } => {
                source
            }
        }
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrePublication { operation, source } => {
                write!(f, "user file was not published: {operation}: {source}")
            }
            Self::PublishedButNotDurable { operation, source } => write!(
                f,
                "user-file publication occurred but current identity or durability is uncertain: {operation}: {source}"
            ),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.io_error())
    }
}

/// Atomically save `bytes` according to `policy`.
///
/// New files are created with mode `0644`. An existing regular file keeps its
/// permission bits, owner, and group. See [`save_with`] for the full refusal
/// and durability contract.
///
/// # Errors
/// Returns [`SaveError::PrePublication`] before this call publishes its bytes,
/// or [`SaveError::PublishedButNotDurable`] after publication occurred but
/// current pathname identity or durability could not be confirmed.
pub fn save(path: &Path, policy: SavePolicy, bytes: &[u8]) -> Result<(), SaveError> {
    save_with(path, policy, |output| output.write_all(bytes))
}

/// Atomically publishes bytes streamed by `write` according to `policy`.
///
/// On Linux, publication uses an unnamed staged inode and retained directory
/// handles, so ancestor renames cannot redirect it. Existing targets retain mode
/// and ownership; new files use `0644`. Cooperating saves are serialized, but
/// non-cooperating namespace changes can leave pathname identity or durability
/// uncertain after publication and may leave a random recovery entry.
///
/// The callback runs only after destination validation and must write the complete
/// file. Callback errors publish nothing. Non-Linux targets and filesystems without
/// `O_TMPFILE` fail before invoking it.
///
/// # Errors
/// Refuses incompatible policies, dangling or unstable final symlinks, intermediate
/// symlinks, non-regular destinations, and destinations with multiple hard links.
/// Callback, metadata, synchronization, and publication errors retain their
/// [`SaveOperation`] and publication phase.
///
/// # Panics
/// A callback panic propagates without publishing the staged bytes.
pub fn save_with(
    path: &Path,
    policy: SavePolicy,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> Result<(), SaveError> {
    platform::save_with(path, policy, None, NEW_DOCUMENT_MODE, write)
        .map(|_| ())
        .map_err(|failure| failure.error)
}

// `Staged` carries the published inode itself, so a caller can rotate custody
// to it without resolving the destination name again. `Unknown` means the
// current pathname identity could not be proven; the staged descriptor, when one
// still exists, travels separately in `PublishFailure::unknown_staged` so a
// repeated-publication caller can retain and read it rather than drop it.
pub(crate) enum Custody {
    Unchanged,
    Staged(File),
    Unknown,
}

// Where the bytes this call staged are named when it returns. `Named` holds an
// entry in the destination's parent directory; `Unnamed` records that another
// writer removed the published name, which Linux will not let us re-link.
pub(crate) enum StagedNaming {
    NotPublished,
    AtDestination,
    Named(std::ffi::OsString),
    Unnamed,
}

pub(crate) struct PublishFailure {
    pub(crate) error: SaveError,
    pub(crate) custody: Custody,
    pub(crate) staged: StagedNaming,
    // The staged inode retained when publication identity is `Unknown` and its
    // descriptor still exists, so a repeated-publication caller can adopt it into
    // lost custody and read it even after its last name was removed. The one-shot
    // save entry points discard it with the rest of the custody information.
    pub(crate) unknown_staged: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetainedMetadata {
    pub(crate) mode: u32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

#[cfg(unix)]
pub(crate) fn normalize_and_capture_document_metadata(file: &File) -> io::Result<RetainedMetadata> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    file.set_permissions(std::fs::Permissions::from_mode(NEW_DOCUMENT_MODE))?;
    let metadata = file.metadata()?;
    let mode = metadata.mode() & 0o7777;
    if mode != NEW_DOCUMENT_MODE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reserved document mode changed during creation",
        ));
    }
    Ok(RetainedMetadata {
        mode,
        uid: metadata.uid(),
        gid: metadata.gid(),
    })
}

#[cfg(not(unix))]
pub(crate) fn normalize_and_capture_document_metadata(
    _file: &File,
) -> io::Result<RetainedMetadata> {
    Ok(RetainedMetadata {
        mode: 0,
        uid: 0,
        gid: 0,
    })
}

// `existing` is `None` for a destination that does not exist yet.
pub(crate) struct RetainedDestination {
    pub(crate) existing: Option<(File, RetainedMetadata)>,
    pub(crate) parent: File,
}

// `metadata` is compared against the destination resolved at publication time,
// so a mode or ownership change made after retention refuses the publication
// instead of being preserved into the newly published inode.
pub(crate) struct ExpectedDestination<'a> {
    pub(crate) file: Option<&'a File>,
    pub(crate) metadata: Option<RetainedMetadata>,
    pub(crate) parent: &'a File,
}

pub(crate) fn open_retained(path: &Path, policy: SavePolicy) -> io::Result<RetainedDestination> {
    platform::open_retained(path, policy)
}

pub(crate) fn publish_retained(
    path: &Path,
    expected: ExpectedDestination<'_>,
    new_file_mode: u32,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> Result<File, PublishFailure> {
    let policy = if expected.file.is_some() {
        SavePolicy::ReplaceExisting
    } else {
        SavePolicy::CreateNew
    };
    platform::save_with(path, policy, Some(expected), new_file_mode, write)
}

pub(crate) fn save_with_expected_destination(
    path: &Path,
    expected_file: &File,
    expected_metadata: RetainedMetadata,
    expected_parent: &File,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> Result<(), SaveError> {
    publish_retained(
        path,
        ExpectedDestination {
            file: Some(expected_file),
            metadata: Some(expected_metadata),
            parent: expected_parent,
        },
        NEW_DOCUMENT_MODE,
        write,
    )
    .map(|_| ())
    .map_err(|failure| failure.error)
}

/// The device and inode of a user document held by descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentIdentity {
    device: u64,
    inode: u64,
}

impl DocumentIdentity {
    /// The device number reported by the held descriptor.
    #[must_use]
    pub fn device(&self) -> u64 {
        self.device
    }

    /// The inode number reported by the held descriptor.
    #[must_use]
    pub fn inode(&self) -> u64 {
        self.inode
    }
}

impl fmt::Display for DocumentIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.device, self.inode)
    }
}

/// Which inode an authority owns after a failed conditional replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplaceCustody {
    /// Nothing this call staged survives at the destination name, and the
    /// authority still owns the identity it owned before the call. Another
    /// writer may have replaced that identity since, which the next replacement
    /// refuses.
    Unchanged,
    /// The staged inode is published at the destination name and the authority
    /// now owns it, so replacement may continue; only durability or a later
    /// validation failed.
    Rotated,
    /// No identity could be proven current for the destination name. The
    /// authority replaces nothing further, and the caller must decide what the
    /// pathname now holds before opening a new authority.
    /// [`ReplaceError::staged_bytes`] reports where the staged bytes are named.
    Lost,
}

/// Where the bytes a failed replacement staged are named when it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StagedBytes {
    /// Never published and never named; the destination is untouched.
    Discarded,
    /// Named by the destination itself.
    AtDestination,
    /// Named by this path in the destination's parent directory, either the
    /// replacement's own staging name left in place or a recovery link made from
    /// the retained staged inode. The parent may itself have been replaced, in
    /// which case this path no longer resolves to that directory.
    AtPath(PathBuf),
    /// Named nowhere. Another writer removed the name this call published, and
    /// Linux refuses to link an inode whose last name was removed, so these
    /// bytes cannot be recovered from the retained descriptor.
    Unnamed,
}

/// A failed conditional replacement, the custody the authority retains after
/// it, and where the staged bytes are named.
#[derive(Debug)]
pub struct ReplaceError {
    save: SaveError,
    custody: ReplaceCustody,
    staged: StagedBytes,
}

impl ReplaceError {
    /// Which inode the authority owns after this failure.
    #[must_use]
    pub fn custody(&self) -> ReplaceCustody {
        self.custody
    }

    /// Where the bytes this call staged are named.
    ///
    /// [`StagedBytes::Unnamed`] is the one outcome in which a completed
    /// replacement leaves no path holding those bytes.
    #[must_use]
    pub fn staged_bytes(&self) -> &StagedBytes {
        &self.staged
    }

    /// Whether a publication syscall completed before the failure.
    #[must_use]
    pub fn was_published(&self) -> bool {
        self.save.was_published()
    }

    /// The operation that failed.
    #[must_use]
    pub fn operation(&self) -> SaveOperation {
        self.save.operation()
    }

    /// The underlying I/O error.
    #[must_use]
    pub fn io_error(&self) -> &io::Error {
        self.save.io_error()
    }
}

impl fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let custody = match self.custody {
            ReplaceCustody::Unchanged => "custody unchanged",
            ReplaceCustody::Rotated => "custody rotated to the published inode",
            ReplaceCustody::Lost => "custody lost",
        };
        write!(f, "{}: {custody}, ", self.save)?;
        match &self.staged {
            StagedBytes::Discarded => f.write_str("staged bytes discarded"),
            StagedBytes::AtDestination => f.write_str("staged bytes at the destination name"),
            StagedBytes::AtPath(path) => write!(f, "staged bytes named {}", path.display()),
            StagedBytes::Unnamed => f.write_str("staged bytes named nowhere"),
        }
    }
}

impl std::error::Error for ReplaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.save)
    }
}

/// Snapshot of the retained document's content read without reopening its
/// pathname.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentSnapshot {
    /// Content read from the retained inode.
    Current {
        /// Bytes read, at most `cap` bytes long.
        bytes: Vec<u8>,
        /// The retained inode contained more bytes than `cap`.
        overflow: bool,
        /// Identity of the inode the bytes were read from.
        identity: DocumentIdentity,
    },
    /// Custody is lost; no read or further replacement is possible.
    Lost,
}

/// Error reading from the retained inode.
#[derive(Debug)]
pub enum ReadError {
    /// Metadata changed between the pre-read and post-read inspection, so the
    /// bytes may be a hybrid of two or more in-place writes. Detection is a size
    /// and timestamp comparison, so a size-preserving rewrite is observable only
    /// as finely as the filesystem timestamps it.
    ConcurrentMutation,
    /// The cap cannot represent an overflow-detection buffer (`cap + 1`
    /// overflows `usize`) or the allocation was refused by the system.
    CapOverflow,
    /// I/O error during the read or metadata inspection.
    Io(io::Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConcurrentMutation => {
                f.write_str("in-place mutation detected during bounded read")
            }
            Self::CapOverflow => f.write_str("cap too large for overflow-detection buffer"),
            Self::Io(e) => write!(f, "read from retained inode: {e}"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ConcurrentMutation | Self::CapOverflow => None,
            Self::Io(e) => Some(e),
        }
    }
}

/// The outcome of reading one descriptor an authority retains after custody is
/// lost.
#[derive(Debug)]
pub enum HandleRead {
    /// Bounded content read from the retained descriptor.
    Read {
        /// Bytes read, at most `cap` bytes long.
        bytes: Vec<u8>,
        /// The retained inode contained more bytes than `cap`.
        overflow: bool,
        /// Identity of the retained inode.
        identity: DocumentIdentity,
    },
    /// The descriptor is still retained but could not be read.
    Unreadable(ReadError),
}

/// Bounded reconciliation of the descriptors an authority still owns after
/// custody is lost.
///
/// Reading both halves lets a caller determine, without reopening the pathname,
/// whether a replacement published: the previous inode holds the pre-replacement
/// bytes, and the staged inode holds the bytes this authority tried to publish,
/// each with its own [`DocumentIdentity`].
#[derive(Debug)]
pub struct LostReconciliation {
    previous: Option<HandleRead>,
    staged: Option<HandleRead>,
}

impl LostReconciliation {
    /// Read of the inode retained from before the failed replacement, if the
    /// authority still owns it.
    #[must_use]
    pub fn previous(&self) -> Option<&HandleRead> {
        self.previous.as_ref()
    }

    /// Read of the inode this authority staged, if it still owns it. Present even
    /// when another writer removed the staged inode's last name, since the read
    /// reaches it through the retained descriptor.
    #[must_use]
    pub fn staged(&self) -> Option<&HandleRead> {
        self.staged.as_ref()
    }
}

// A descriptor retained after custody is lost, with the identity observed for it
// when one was available; a `None` identity is recomputed from the descriptor at
// read time.
struct RetainedHandle {
    file: File,
    identity: Option<DocumentIdentity>,
}

/// The staged descriptor retained after a create-new publication whose
/// destination-name identity cannot be proven.
///
/// Linux refuses to link an inode whose last name was removed, so a caller who
/// finds the destination pointing elsewhere cannot recover these bytes by
/// pathname. The retained descriptor reaches the staged inode without a
/// pathname reopen, so [`read_bounded`](Self::read_bounded) can still return
/// the bytes this call wrote. Dropping the handle releases the descriptor.
pub struct LostCreateStage {
    handle: RetainedHandle,
}

impl fmt::Debug for LostCreateStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LostCreateStage")
            .field("identity", &self.handle.identity)
            .finish()
    }
}

impl LostCreateStage {
    /// Reads up to `cap` bytes from the retained staged descriptor.
    ///
    /// The read reaches the staged inode through the descriptor rather than the
    /// pathname, so it succeeds even after another writer removed the last name.
    /// The same [`pread(2)`](https://man7.org/linux/man-pages/man2/pread.2.html)
    /// and mutation-detection contract as
    /// [`DocumentAuthority::read_bounded`] applies.
    #[must_use]
    pub fn read_bounded(&self, cap: usize) -> HandleRead {
        read_retained_handle(&self.handle, cap)
    }
}

/// A create-new publication failure, typed by what the caller can still do
/// about the bytes it staged.
///
/// A failure never discards a published descriptor: durability faults return a
/// `DocumentAuthority` for the exact inode this call published, and an
/// unprovable pathname identity retains the staged descriptor in
/// [`LostCreateStage`] so its bytes remain readable even after their last name
/// was removed.
#[derive(Debug)]
#[non_exhaustive]
pub enum CreateDocumentError {
    /// Publication did not occur; no file was created and the destination name
    /// is untouched.
    NotPublished {
        /// The failed operation.
        operation: SaveOperation,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// Publication completed and the exact published inode is retained in
    /// `authority`. Only durability or a late validation failed, so the caller
    /// may continue with [`DocumentAuthority::read_bounded`] and
    /// [`DocumentAuthority::replace`] on the returned authority.
    PublishedButNotDurable {
        /// The failed operation.
        operation: SaveOperation,
        /// The underlying I/O error.
        source: io::Error,
        /// Authority for the exact inode this call published, ready for
        /// bounded reads and conditional replacements.
        authority: DocumentAuthority,
    },
    /// Publication occurred but the destination name cannot be proven to
    /// denote the staged inode. The staged descriptor is retained in
    /// `staged_handle` so the caller can read the bytes without reopening the
    /// pathname; no `DocumentAuthority` is returned because identity is
    /// unprovable and every later replacement would be refused.
    Lost {
        /// The failed operation.
        operation: SaveOperation,
        /// The underlying I/O error.
        source: io::Error,
        /// Where the staged bytes are named when this error returns.
        staged: StagedBytes,
        /// The staged inode retained by descriptor so its bytes remain
        /// readable through it.
        staged_handle: LostCreateStage,
    },
}

impl CreateDocumentError {
    /// Whether a publication syscall completed before the failure.
    #[must_use]
    pub fn was_published(&self) -> bool {
        matches!(
            self,
            Self::PublishedButNotDurable { .. } | Self::Lost { .. }
        )
    }

    /// The operation that failed.
    #[must_use]
    pub fn operation(&self) -> SaveOperation {
        match self {
            Self::NotPublished { operation, .. }
            | Self::PublishedButNotDurable { operation, .. }
            | Self::Lost { operation, .. } => *operation,
        }
    }

    /// The underlying I/O error.
    #[must_use]
    pub fn io_error(&self) -> &io::Error {
        match self {
            Self::NotPublished { source, .. }
            | Self::PublishedButNotDurable { source, .. }
            | Self::Lost { source, .. } => source,
        }
    }
}

impl fmt::Display for CreateDocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPublished { operation, source } => {
                write!(
                    f,
                    "create-new document was not published: {operation}: {source}"
                )
            }
            Self::PublishedButNotDurable {
                operation, source, ..
            } => write!(
                f,
                "create-new publication occurred but durability or identity is uncertain: {operation}: {source}, custody retained"
            ),
            Self::Lost {
                operation,
                source,
                staged,
                ..
            } => {
                write!(
                    f,
                    "create-new publication cannot be proven current: {operation}: {source}, "
                )?;
                match staged {
                    StagedBytes::Discarded => f.write_str("staged bytes discarded"),
                    StagedBytes::AtDestination => {
                        f.write_str("staged bytes at the destination name")
                    }
                    StagedBytes::AtPath(path) => write!(f, "staged bytes named {}", path.display()),
                    StagedBytes::Unnamed => f.write_str("staged bytes named nowhere"),
                }
            }
        }
    }
}

impl std::error::Error for CreateDocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.io_error())
    }
}

enum DocumentHeld {
    Current {
        file: File,
        identity: DocumentIdentity,
        metadata: RetainedMetadata,
    },
    // Retains the exact descriptors for every inode this authority still owns —
    // the previous inode, the staged inode, or both — so a failed replacement
    // never drops the only handle to bytes the caller may still need. Dropping
    // the authority releases them.
    Lost {
        old: Option<RetainedHandle>,
        staged: Option<RetainedHandle>,
    },
}

/// Conditional replacement authority over one user-selected document.
///
/// The authority is the open destination inode and its open parent directory,
/// never the pathname: [`open_existing`](Self::open_existing) resolves the
/// pathname once and [`create_new`](Self::create_new) publishes a fresh inode
/// under a strict no-clobber policy; [`read_bounded`](Self::read_bounded) and
/// [`replace`](Self::replace) address the retained descriptors afterwards, so
/// no pathname reopen happens between the read a caller parses and the write it
/// publishes, nor between publication and the authority the caller receives
/// from `create_new`. A successful replacement rotates custody to the inode it
/// just published, like [`crate::state_authority::StateAuthority`]. Custody is
/// not shareable — the type is deliberately not `Clone` — so a second writer
/// opens its own authority and is refused once this one has published.
///
/// Unlike a private state file, a document keeps the mode, owner, and group it
/// was opened with; a replacement carries them forward and refuses if they
/// changed since. The final component must be a single-linked regular file: a
/// symbolic link, a directory, a multiply-linked file, or a symlinked parent
/// component is refused rather than resolved. Ancestor renames cannot redirect a
/// replacement, since publication addresses the retained parent descriptor.
///
/// A failed replacement never drops the descriptors it still owns. When
/// publication identity becomes unprovable, custody is lost but the previous
/// inode and the staged inode remain open, so
/// [`lost_snapshots`](Self::lost_snapshots) can still read both to reconcile
/// whether the write published. The generation counter never reuses or saturates
/// a value: once it cannot advance, [`replace`](Self::replace) refuses before
/// staging, without running the writer.
///
/// ```no_run
/// use enzui_core::user_file::DocumentAuthority;
/// use std::path::Path;
///
/// # fn main() -> std::io::Result<()> {
/// let mut document = DocumentAuthority::open_existing(Path::new("/home/user/notes.txt"))?;
/// let _snapshot = document.read_bounded(1 << 20).expect("bounded read");
/// let published = document.replace(b"edited contents").expect("conditional replace");
/// assert_eq!(document.identity(), Some(published));
/// # Ok(())
/// # }
/// ```
pub struct DocumentAuthority {
    path: PathBuf,
    parent: File,
    held: DocumentHeld,
    generation: u64,
}

impl fmt::Debug for DocumentAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentAuthority")
            .field("path", &self.path)
            .field("identity", &self.identity())
            .field("generation", &self.generation)
            .finish()
    }
}

impl DocumentAuthority {
    /// Atomically creates a new user document from `bytes` and retains authority
    /// to the exact inode it published.
    ///
    /// See [`create_new_with`](Self::create_new_with) for the full refusal,
    /// durability, and custody contract.
    ///
    /// # Errors
    ///
    /// Returns [`CreateDocumentError::NotPublished`] before publication,
    /// [`CreateDocumentError::PublishedButNotDurable`] carrying a
    /// `DocumentAuthority` for the published inode when only durability or a
    /// late validation failed, and [`CreateDocumentError::Lost`] retaining the
    /// staged descriptor when identity is unprovable.
    pub fn create_new(path: &Path, bytes: &[u8]) -> Result<Self, CreateDocumentError> {
        Self::create_new_with(path, |file| file.write_all(bytes))
    }

    /// Atomically creates a new user document with bytes streamed by `writer`
    /// and retains authority to the exact inode it published.
    ///
    /// The parent directory is retained by descriptor under the same secure
    /// user-file policy [`open_existing`](Self::open_existing) uses, with the
    /// final component required to be absent. A symbolic link, a directory, a
    /// non-regular entry, or an existing entry at the final component is
    /// refused, and no numeric-suffix fallback happens inside the framework.
    /// A symbolic link among the parent components is refused rather than
    /// resolved.
    ///
    /// Publication is atomic and no-clobber: the writer runs into an unnamed
    /// `O_TMPFILE` inode created in the retained parent, the staged inode is
    /// fsynced, and it is linked at the destination name only if the name is
    /// still absent. The retained parent directory is fsynced after publication
    /// and the destination-name lookup is re-verified against the staged inode
    /// before success; the returned [`DocumentAuthority`] retains that exact
    /// staged/published inode by descriptor together with the retained parent,
    /// so no pathname reopen occurs between publication and the authority the
    /// caller receives. The generation counter starts at `1` (nonzero), so a
    /// caller correlating generations across authorities cannot alias a
    /// generation-zero authority produced by [`open_existing`](Self::open_existing).
    /// The returned authority supports [`read_bounded`](Self::read_bounded),
    /// [`identity`](Self::identity), and
    /// [`replace`](Self::replace)/[`replace_with`](Self::replace_with)
    /// immediately.
    ///
    /// New files use mode `0644`. Requires Linux.
    ///
    /// # Errors
    ///
    /// Returns [`CreateDocumentError::NotPublished`] for any failure — parent
    /// resolution, a refused existing entry, callback failure, staging,
    /// metadata, sync, or the no-clobber link — that leaves the destination
    /// name untouched. Returns
    /// [`CreateDocumentError::PublishedButNotDurable`] carrying a
    /// `DocumentAuthority` for the exact published inode when only the retained
    /// parent directory's fsync failed. Returns [`CreateDocumentError::Lost`]
    /// retaining the staged descriptor when the destination-name identity
    /// cannot be proven current after publication (another writer
    /// removed/substituted the published name); the staged inode is still
    /// readable through the retained descriptor.
    ///
    /// # Panics
    ///
    /// A callback panic is caught before publication and reported as
    /// [`CreateDocumentError::NotPublished`] with
    /// [`SaveOperation::Write`]; nothing is left named at the destination.
    /// Oversized-write policy belongs to the caller's writer just like
    /// [`save_with`], and staged bytes and descriptors remain bounded by the
    /// writer's behavior.
    pub fn create_new_with(
        path: &Path,
        writer: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> Result<Self, CreateDocumentError> {
        // Retain the exact parent directory and require the final component to
        // be absent under the secure user-file resolution policy.
        let retained = open_retained(path, SavePolicy::CreateNew).map_err(|error| {
            CreateDocumentError::NotPublished {
                operation: classify_create_error(&error),
                source: error,
            }
        })?;
        debug_assert!(
            retained.existing.is_none(),
            "CreateNew resolution must not carry an existing destination",
        );
        let parent = retained.parent;

        // Catch a writer panic before publication so the caller receives a typed
        // `NotPublished` variant rather than an unwinding panic mid-publish; the
        // writer runs strictly before `link_file`, so no name reaches the
        // destination on panic.
        let mut panic_payload: Option<Box<dyn std::any::Any + Send>> = None;
        let outcome = platform::save_with_file(
            path,
            SavePolicy::CreateNew,
            Some(ExpectedDestination {
                file: None,
                metadata: None,
                parent: &parent,
            }),
            NEW_DOCUMENT_MODE,
            |stage: &mut File| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                move || writer(stage),
            )) {
                Ok(result) => result,
                Err(payload) => {
                    panic_payload = Some(payload);
                    Err(io::Error::other("document writer panicked"))
                }
            },
        );

        match outcome {
            Ok(published) => match document_identity(&published) {
                Ok((identity, metadata)) => Ok(Self {
                    path: path.to_path_buf(),
                    parent,
                    held: DocumentHeld::Current {
                        file: published,
                        identity,
                        metadata,
                    },
                    // The generation counter is nonzero out of the gate so a
                    // caller comparing generations across authorities cannot
                    // false-match a fresh `open_existing` at generation zero.
                    generation: 1,
                }),
                Err(error) => Err(CreateDocumentError::Lost {
                    operation: SaveOperation::Revalidate,
                    source: error,
                    staged: StagedBytes::AtDestination,
                    staged_handle: LostCreateStage {
                        handle: RetainedHandle {
                            file: published,
                            identity: None,
                        },
                    },
                }),
            },
            Err(failure) => {
                if let Some(payload) = panic_payload {
                    // Cleanup already happened via stage/candidate drops during
                    // the buffered publish. Convert the caught unwind to a
                    // typed pre-publication error without resuming the panic;
                    // the payload is discarded here.
                    debug_assert!(
                        !failure.error.was_published(),
                        "writer panic must not have crossed the publish barrier",
                    );
                    drop(payload);
                    return Err(CreateDocumentError::NotPublished {
                        operation: SaveOperation::Write,
                        source: io::Error::other("document writer panicked before publication"),
                    });
                }
                Err(create_error_from_failure(path, parent, failure))
            }
        }
    }

    /// Retains an existing user document and its parent directory by descriptor.
    ///
    /// # Errors
    ///
    /// Refuses a missing destination, a symbolic-link final component, a
    /// non-regular file, a multiply-linked file, and any symbolic link among the
    /// parent components. Requires Linux.
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        let retained = open_retained(path, SavePolicy::ReplaceExisting)?;
        let (file, metadata) = retained.existing.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "user-file authority requires an existing regular file",
            )
        })?;
        let (identity, _) = document_identity(&file)?;
        Ok(Self {
            path: path.to_path_buf(),
            parent: retained.parent,
            held: DocumentHeld::Current {
                file,
                identity,
                metadata,
            },
            generation: 0,
        })
    }

    /// The pathname this authority was opened from.
    ///
    /// It is a diagnostic label: reading and replacement address the retained
    /// descriptors, not this path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The identity this authority currently owns, for generation correlation.
    ///
    /// `None` only after custody is lost.
    #[must_use]
    pub fn identity(&self) -> Option<DocumentIdentity> {
        match &self.held {
            DocumentHeld::Current { identity, .. } => Some(*identity),
            DocumentHeld::Lost { .. } => None,
        }
    }

    /// How many times custody has rotated to a newly published inode.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether custody was lost, which refuses every later replacement.
    #[must_use]
    pub fn has_lost_custody(&self) -> bool {
        matches!(self.held, DocumentHeld::Lost { .. })
    }

    /// Reads up to `cap` bytes from the retained inode without reopening its
    /// pathname.
    ///
    /// Returns [`DocumentSnapshot::Current`] with the bounded content, an
    /// overflow flag, and the inode identity while custody is held. The read uses
    /// `pread(2)` so repeated calls see the same content without a shared-offset
    /// surprise. After custody is lost there is no single current inode, so this
    /// returns [`DocumentSnapshot::Lost`]; the descriptors the authority still
    /// retains are read through [`lost_snapshots`](Self::lost_snapshots).
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::CapOverflow`] when `cap + 1` overflows `usize` or the
    /// system refuses the allocation, [`ReadError::ConcurrentMutation`] when
    /// metadata observed through the held descriptor changed across the read, and
    /// [`ReadError::Io`] for any I/O failure, including an environment without
    /// `/proc/self/fd`, through which the read reaches the retained inode without
    /// its pathname.
    pub fn read_bounded(&self, cap: usize) -> Result<DocumentSnapshot, ReadError> {
        match &self.held {
            DocumentHeld::Lost { .. } => Ok(DocumentSnapshot::Lost),
            DocumentHeld::Current { file, identity, .. } => {
                let (bytes, overflow) = read_bounded_bytes(file, *identity, cap)?;
                Ok(DocumentSnapshot::Current {
                    bytes,
                    overflow,
                    identity: *identity,
                })
            }
        }
    }

    /// Reads the descriptors this authority still retains after custody is lost.
    ///
    /// Returns `None` while custody is held. After a failed replacement it
    /// returns each retained inode's bounded content and identity — the previous
    /// inode, the staged inode, or both — so a caller can reconcile whether the
    /// replacement published without reopening the pathname. Each half reads up
    /// to `cap` bytes with the same `pread(2)` and mutation-detection contract as
    /// [`read_bounded`](Self::read_bounded).
    #[must_use]
    pub fn lost_snapshots(&self, cap: usize) -> Option<LostReconciliation> {
        match &self.held {
            DocumentHeld::Current { .. } => None,
            DocumentHeld::Lost { old, staged } => Some(LostReconciliation {
                previous: old.as_ref().map(|handle| read_retained_handle(handle, cap)),
                staged: staged
                    .as_ref()
                    .map(|handle| read_retained_handle(handle, cap)),
            }),
        }
    }

    /// Conditionally replaces the document with `bytes`.
    ///
    /// # Errors
    ///
    /// Returns a [`ReplaceError`] whose [`custody`](ReplaceError::custody)
    /// reports what this authority owns afterwards.
    pub fn replace(&mut self, bytes: &[u8]) -> Result<DocumentIdentity, ReplaceError> {
        self.replace_with(|output| output.write_all(bytes))
    }

    /// Conditionally replaces the document with bytes streamed by `write`.
    ///
    /// The replacement publishes only if the destination name still denotes the
    /// retained inode, its retained mode/owner/group, and the retained parent
    /// directory; any of those changing since the authority opened is a refusal
    /// rather than a clobber. The callback runs after that validation and before
    /// any name exists, so its failure publishes nothing. On success the returned
    /// identity is the inode now published, and this authority owns it.
    ///
    /// # Errors
    ///
    /// Returns a [`ReplaceError`] for a substituted destination, parent, or
    /// staging name, for a destination whose mode or ownership changed after
    /// retention, for callback and filesystem failures, and for a durability
    /// failure after publication. When the generation counter cannot advance
    /// without repeating a value the call refuses before staging, without running
    /// the writer and without publishing, reporting
    /// [`SaveOperation::GenerationExhausted`] with
    /// [`ReplaceCustody::Unchanged`]. [`ReplaceError::custody`] distinguishes an
    /// untouched destination from a rotated one and from lost custody, and
    /// [`ReplaceError::staged_bytes`] reports where the staged bytes are named. A
    /// failure never drops the descriptors the authority still owns; after lost
    /// custody they are read through [`lost_snapshots`](Self::lost_snapshots).
    ///
    /// # Panics
    ///
    /// A callback panic propagates without publishing the staged bytes.
    pub fn replace_with(
        &mut self,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<DocumentIdentity, ReplaceError> {
        // Refuse before staging so an exhausted counter never runs the writer or
        // publishes, and a generation value is never reused or saturated.
        let Some(next_generation) = self.generation.checked_add(1) else {
            return Err(ReplaceError {
                save: SaveError::before(SaveOperation::GenerationExhausted, generation_exhausted()),
                custody: ReplaceCustody::Unchanged,
                staged: StagedBytes::Discarded,
            });
        };
        let expected = match &self.held {
            DocumentHeld::Current { file, metadata, .. } => ExpectedDestination {
                file: Some(file),
                metadata: Some(*metadata),
                parent: &self.parent,
            },
            DocumentHeld::Lost { .. } => {
                return Err(ReplaceError {
                    save: SaveError::before(
                        SaveOperation::InspectDestination,
                        io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "document authority cannot prove which inode its destination names",
                        ),
                    ),
                    custody: ReplaceCustody::Lost,
                    staged: StagedBytes::Discarded,
                });
            }
        };
        let outcome = publish_retained(&self.path, expected, NEW_DOCUMENT_MODE, write);
        // The borrow held by `expected` ends with `publish_retained`, so the
        // previous inode can be moved out and either superseded on success or
        // retained in lost custody on an uncertain failure.
        let (old_file, old_identity, metadata) = match std::mem::replace(
            &mut self.held,
            DocumentHeld::Lost {
                old: None,
                staged: None,
            },
        ) {
            DocumentHeld::Current {
                file,
                identity,
                metadata,
            } => (file, identity, metadata),
            DocumentHeld::Lost { .. } => unreachable!("replace requires current custody"),
        };
        match outcome {
            Ok(published) => match document_identity(&published) {
                Ok((identity, _)) => {
                    self.held = DocumentHeld::Current {
                        file: published,
                        identity,
                        metadata,
                    };
                    self.generation = next_generation;
                    Ok(identity)
                }
                Err(error) => {
                    self.held = lost_custody(old_file, old_identity, Some(published));
                    Err(ReplaceError {
                        save: SaveError::after(SaveOperation::Revalidate, error),
                        custody: ReplaceCustody::Lost,
                        staged: StagedBytes::AtDestination,
                    })
                }
            },
            Err(failure) => {
                let staged_bytes = staged_bytes_of(&self.path, &failure.staged);
                let staged_at_destination = matches!(failure.staged, StagedNaming::AtDestination);
                let (custody, error) = match failure.custody {
                    Custody::Unchanged => {
                        self.held = DocumentHeld::Current {
                            file: old_file,
                            identity: old_identity,
                            metadata,
                        };
                        (ReplaceCustody::Unchanged, failure.error)
                    }
                    // Rotation may only be claimed when the staged inode is the
                    // destination name, not by convention.
                    Custody::Staged(published) if staged_at_destination => {
                        match document_identity(&published) {
                            Ok((identity, _)) => {
                                self.held = DocumentHeld::Current {
                                    file: published,
                                    identity,
                                    metadata,
                                };
                                self.generation = next_generation;
                                (ReplaceCustody::Rotated, failure.error)
                            }
                            Err(_) => {
                                self.held = lost_custody(old_file, old_identity, Some(published));
                                (ReplaceCustody::Lost, failure.error)
                            }
                        }
                    }
                    Custody::Staged(published) => {
                        self.held = lost_custody(old_file, old_identity, Some(published));
                        (ReplaceCustody::Lost, failure.error)
                    }
                    Custody::Unknown => {
                        self.held = lost_custody(old_file, old_identity, failure.unknown_staged);
                        (ReplaceCustody::Lost, failure.error)
                    }
                };
                Err(ReplaceError {
                    save: error,
                    custody,
                    staged: staged_bytes,
                })
            }
        }
    }
}

fn generation_exhausted() -> io::Error {
    io::Error::other("document generation counter exhausted")
}

// Mirrors `platform::classify_resolution` for errors returned by
// `open_retained`, so a parent-directory refusal is reported as
// `SaveOperation::OpenParent` and a destination refusal as
// `SaveOperation::InspectDestination`.
fn classify_create_error(error: &io::Error) -> SaveOperation {
    if error.to_string().contains("parent directory") {
        SaveOperation::OpenParent
    } else {
        SaveOperation::InspectDestination
    }
}

// Interprets a `PublishFailure` from `platform::save_with_file` for a
// create-new call. A durability failure that still owns the published inode
// hands it back inside a `DocumentAuthority`; an unprovable identity retains
// the staged descriptor inside `LostCreateStage`; every other failure leaves
// the destination untouched and is reported as `NotPublished`.
fn create_error_from_failure(
    path: &Path,
    parent: File,
    failure: PublishFailure,
) -> CreateDocumentError {
    let staged_bytes = staged_bytes_of(path, &failure.staged);
    let (operation, source) = match failure.error {
        SaveError::PrePublication { operation, source }
        | SaveError::PublishedButNotDurable { operation, source } => (operation, source),
    };
    match failure.custody {
        Custody::Unchanged => CreateDocumentError::NotPublished { operation, source },
        Custody::Staged(published) => match document_identity(&published) {
            Ok((identity, metadata)) => CreateDocumentError::PublishedButNotDurable {
                operation,
                source,
                authority: DocumentAuthority {
                    path: path.to_path_buf(),
                    parent,
                    held: DocumentHeld::Current {
                        file: published,
                        identity,
                        metadata,
                    },
                    generation: 1,
                },
            },
            // A rotated stage whose identity cannot be re-read still holds the
            // published bytes, so it moves into lost custody rather than being
            // dropped in the error.
            Err(identity_error) => CreateDocumentError::Lost {
                operation: SaveOperation::Revalidate,
                source: identity_error,
                staged: staged_bytes,
                staged_handle: LostCreateStage {
                    handle: RetainedHandle {
                        file: published,
                        identity: None,
                    },
                },
            },
        },
        Custody::Unknown => {
            // Every `Unknown`-custody path in `platform::save_with_file`
            // attaches its staged descriptor through `retain_unknown_stage`.
            let staged_file = failure
                .unknown_staged
                .expect("Unknown-custody publication must retain its staged descriptor");
            CreateDocumentError::Lost {
                operation,
                source,
                staged: staged_bytes,
                staged_handle: LostCreateStage {
                    handle: RetainedHandle {
                        file: staged_file,
                        identity: None,
                    },
                },
            }
        }
    }
}

fn staged_bytes_of(path: &Path, staged: &StagedNaming) -> StagedBytes {
    match staged {
        StagedNaming::NotPublished => StagedBytes::Discarded,
        StagedNaming::AtDestination => StagedBytes::AtDestination,
        StagedNaming::Named(name) => StagedBytes::AtPath(path.with_file_name(name)),
        StagedNaming::Unnamed => StagedBytes::Unnamed,
    }
}

// A failed replacement retains the previous inode with its observed identity and
// the staged inode when one exists; the staged identity is recomputed at read
// time from the descriptor.
fn lost_custody(
    old_file: File,
    old_identity: DocumentIdentity,
    staged: Option<File>,
) -> DocumentHeld {
    DocumentHeld::Lost {
        old: Some(RetainedHandle {
            file: old_file,
            identity: Some(old_identity),
        }),
        staged: staged.map(|file| RetainedHandle {
            file,
            identity: None,
        }),
    }
}

fn read_retained_handle(handle: &RetainedHandle, cap: usize) -> HandleRead {
    let identity = match handle.identity {
        Some(identity) => identity,
        None => match document_identity(&handle.file) {
            Ok((identity, _)) => identity,
            Err(error) => return HandleRead::Unreadable(ReadError::Io(error)),
        },
    };
    match read_bounded_bytes(&handle.file, identity, cap) {
        Ok((bytes, overflow)) => HandleRead::Read {
            bytes,
            overflow,
            identity,
        },
        Err(error) => HandleRead::Unreadable(error),
    }
}

#[cfg(unix)]
fn document_identity(file: &File) -> io::Result<(DocumentIdentity, RetainedMetadata)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok((
        DocumentIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        RetainedMetadata {
            mode: metadata.mode() & 0o7777,
            uid: metadata.uid(),
            gid: metadata.gid(),
        },
    ))
}

#[cfg(not(unix))]
fn document_identity(_file: &File) -> io::Result<(DocumentIdentity, RetainedMetadata)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "user-file authority requires a Unix filesystem",
    ))
}

// Metadata snapshot for detecting in-place mutation across a bounded read.
#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct InodeSnapshot {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
    mode: u32,
    uid: u32,
    gid: u32,
}

#[cfg(unix)]
fn snapshot_inode(file: &File) -> io::Result<InodeSnapshot> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok(InodeSnapshot {
        dev: m.dev(),
        ino: m.ino(),
        size: m.size(),
        mtime: m.mtime(),
        mtime_nsec: m.mtime_nsec(),
        ctime: m.ctime(),
        ctime_nsec: m.ctime_nsec(),
        mode: m.mode(),
        uid: m.uid(),
        gid: m.gid(),
    })
}

// The retained destination is held `O_PATH` and a rotated staged inode
// write-only, so bytes come from a `/proc/self/fd` reopen of the inode the
// authority holds, never from whatever the destination name now denotes.
#[cfg(unix)]
fn readable_view(file: &File, identity: DocumentIdentity) -> io::Result<File> {
    use std::os::fd::AsRawFd;

    let view = File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    let (reopened, _) = document_identity(&view)?;
    // A `/proc` that is not procfs is the one substitution this pathname admits.
    if reopened != identity {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reopened descriptor is not the retained document inode",
        ));
    }
    Ok(view)
}

// Returns the bounded bytes and whether the inode held more than `cap`; the
// caller attaches the inode identity so both the single-inode and lost-custody
// readers share one read path.
#[cfg(unix)]
fn read_bounded_bytes(
    file: &File,
    identity: DocumentIdentity,
    cap: usize,
) -> Result<(Vec<u8>, bool), ReadError> {
    use std::os::unix::fs::FileExt;

    let buf_len = cap.checked_add(1).ok_or(ReadError::CapOverflow)?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(buf_len)
        .map_err(|_| ReadError::CapOverflow)?;
    buf.resize(buf_len, 0u8);

    let view = readable_view(file, identity).map_err(ReadError::Io)?;
    let pre = snapshot_inode(&view).map_err(ReadError::Io)?;

    let mut filled = 0usize;
    while filled < buf_len {
        let offset = u64::try_from(filled).map_err(|_| ReadError::CapOverflow)?;
        match view.read_at(&mut buf[filled..], offset) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadError::Io(e)),
        }
    }

    let post = snapshot_inode(&view).map_err(ReadError::Io)?;
    if pre != post {
        return Err(ReadError::ConcurrentMutation);
    }

    let overflow = filled > cap;
    buf.truncate(filled.min(cap));

    Ok((buf, overflow))
}

#[cfg(not(unix))]
fn read_bounded_bytes(
    _file: &File,
    _identity: DocumentIdentity,
    _cap: usize,
) -> Result<(Vec<u8>, bool), ReadError> {
    Err(ReadError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "bounded read requires a Unix filesystem",
    )))
}

#[cfg(target_os = "linux")]
pub(crate) mod platform;

#[cfg(not(target_os = "linux"))]
pub(crate) mod platform {
    use super::{
        Custody, ExpectedDestination, PublishFailure, RetainedDestination, SaveError,
        SaveOperation, SavePolicy, StagedNaming,
    };
    use std::io::{self, Write};
    use std::path::Path;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "strict atomic user-file publication requires Linux",
        )
    }

    pub(super) fn open_retained(
        _path: &Path,
        _policy: SavePolicy,
    ) -> io::Result<RetainedDestination> {
        Err(unsupported())
    }

    pub(super) fn save_with(
        _path: &Path,
        _policy: SavePolicy,
        _expected: Option<ExpectedDestination<'_>>,
        _new_file_mode: u32,
        _write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<std::fs::File, PublishFailure> {
        Err(PublishFailure {
            error: SaveError::before(SaveOperation::InspectDestination, unsupported()),
            custody: Custody::Unchanged,
            staged: StagedNaming::NotPublished,
            unknown_staged: None,
        })
    }

    pub(super) fn save_with_file(
        _path: &Path,
        _policy: SavePolicy,
        _expected: Option<ExpectedDestination<'_>>,
        _new_file_mode: u32,
        _write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    ) -> Result<std::fs::File, PublishFailure> {
        Err(PublishFailure {
            error: SaveError::before(SaveOperation::InspectDestination, unsupported()),
            custody: Custody::Unchanged,
            staged: StagedNaming::NotPublished,
            unknown_staged: None,
        })
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests;
