//! Repeated, identity-bound publication and bounded reading of one private
//! state file.
//!
//! A [`StateAuthority`] retains its destination inode and parent directory by
//! descriptor. Each publication stages the new bytes in an unnamed inode,
//! conditionally replaces only the retained destination identity, and rotates
//! custody to the inode it just published, so the next publication never
//! re-derives authority from the pathname. A destination, parent, or staging
//! name substituted by another writer is refused rather than overwritten or
//! adopted.
//!
//! [`read_bounded`](StateAuthority::read_bounded) reads the retained inode
//! through a read-only view derived from the held descriptor rather than from
//! the destination pathname, compares metadata before and after the read, and
//! refuses a result when in-place mutation is detected.
//!
//! ```no_run
//! use enzui_core::state_authority::StateAuthority;
//! use std::path::Path;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut state = StateAuthority::open_or_create(Path::new("/run/user/1000/session"))?;
//! let first = state.publish(b"generation one").expect("first publication");
//! let second = state.publish(b"generation two").expect("second publication");
//! assert_ne!(first, second);
//! assert_eq!(state.identity(), Some(second));
//! # Ok(())
//! # }
//! ```
//!
//! State stays owner-only: a new file is created `0600`, opening refuses a
//! destination that grants group or other access (except
//! [`StateAuthority::open_or_create_tightening`], which clears group/other
//! access while preserving owner permissions), and publication refuses a
//! mode or ownership change made after the authority opened rather than
//! carrying it into the newly published inode.
//!
//! When another writer removes the last name of a published inode, recovery
//! linking can fail and [`StagedBytes::Unnamed`] reports no recovered name.
//! This authority returns no retained handle for those staged bytes. Other
//! recovery outcomes report destination or staging names, but an
//! [`StagedBytes::AtPath`] path may no longer resolve if its retained parent was
//! replaced. A failure before publication does not overwrite the destination.
//!
//! Opening and publication require Linux; every open refuses on other targets.
//! Use [`crate::atomic_state`] when unconditional replacement of a pathname is
//! wanted, and [`crate::user_file`] for one-shot publication of a user-selected
//! document.

use crate::user_file::{
    Custody, ExpectedDestination, NEW_STATE_MODE, RetainedMetadata, SaveError, SaveOperation,
    SavePolicy, StagedNaming,
};
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

// Any group or other access, plus the set-user-ID, set-group-ID and sticky bits.
const FORBIDDEN_STATE_MODE_BITS: u32 = 0o7077;

/// The device and inode of a state file held by descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateIdentity {
    device: u64,
    inode: u64,
}

impl StateIdentity {
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

impl fmt::Display for StateIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.device, self.inode)
    }
}

/// Which inode an authority owns after a failed publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PublishCustody {
    /// Nothing this call staged survives at the destination name, and the
    /// authority still owns the identity it owned before the call. That
    /// identity may since have been replaced by another writer, which the next
    /// publication refuses.
    Unchanged,
    /// The staged inode is published at the destination name and the authority
    /// now owns it, so publication may continue; only durability or a later
    /// validation failed.
    Rotated,
    /// No identity could be proven current for the destination name. The
    /// authority publishes nothing further, and the caller must decide what the
    /// pathname now holds before opening a new authority.
    /// [`PublishError::staged_bytes`] reports where the staged bytes are named.
    Lost,
}

/// Where the bytes a failed publication staged are named when it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StagedBytes {
    /// Publication did not complete; no staged descriptor or recovery name is returned.
    Discarded,
    /// Named by the destination itself.
    AtDestination,
    /// Named by this path in the destination's parent directory, either the
    /// publication's own staging name left in place or a recovery link made
    /// from the retained staged inode. The parent may itself have been
    /// replaced, in which case this path no longer resolves to that directory.
    AtPath(PathBuf),
    /// No surviving name could be found or recovered for the staged bytes.
    /// Removing the published inode's last name prevents recovery linking;
    /// this API returns no retained descriptor for reading those bytes.
    Unnamed,
}

/// A failed publication, the custody the authority retains after it, and where
/// the staged bytes are named.
#[derive(Debug)]
pub struct PublishError {
    save: SaveError,
    custody: PublishCustody,
    staged: StagedBytes,
}

impl PublishError {
    /// Which inode the authority owns after this failure.
    #[must_use]
    pub fn custody(&self) -> PublishCustody {
        self.custody
    }

    /// Where the bytes this call staged are named.
    ///
    /// [`StagedBytes::Unnamed`] reports that no surviving name was found or
    /// recovered. [`StagedBytes::AtPath`] can name a retained parent that is
    /// no longer reachable through the original directory pathname.
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

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let custody = match self.custody {
            PublishCustody::Unchanged => "custody unchanged",
            PublishCustody::Rotated => "custody rotated to the published inode",
            PublishCustody::Lost => "custody lost",
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

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.save)
    }
}

/// Snapshot of the retained state file's content, structurally distinguishing
/// the three custody states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodySnapshot {
    /// The authority retained an absent destination and has not itself
    /// published. The pathname may have been occupied since opening; only the
    /// first publication performs the real no-clobber validation.
    Absent,
    /// Content read from the retained inode without reopening its pathname.
    Current {
        /// Bytes read, at most `cap` bytes long.
        bytes: Vec<u8>,
        /// The retained inode contained more bytes than `cap`.
        overflow: bool,
        /// Identity of the inode the bytes were read from.
        identity: StateIdentity,
    },
    /// Custody is lost; no read or further publication is possible.
    Lost,
}

/// Error reading from the retained inode.
#[derive(Debug)]
pub enum ReadError {
    /// Metadata changed between pre-read and post-read inspection, so the
    /// bytes may combine in-place writes. Detection compares inode identity,
    /// size, mode, ownership, and modification/change timestamps. A content
    /// rewrite leaving those fields unchanged is not detected.
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

enum Held {
    Absent,
    Current {
        file: File,
        identity: StateIdentity,
        metadata: RetainedMetadata,
    },
    Lost,
}

/// Repeated publication authority over one state file.
///
/// Held destination and parent descriptors bind the identities a publication
/// may replace. Later pathname checks refuse substitutions rather than adopting
/// them; successful publication rotates custody to the new inode.
/// The type is not `Clone`. Authorities retaining the same inode become stale
/// when another authority replaces it; an authority opened afterward can retain
/// the new inode.
pub struct StateAuthority {
    path: PathBuf,
    parent: File,
    held: Held,
    generation: u64,
}

impl fmt::Debug for StateAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateAuthority")
            .field("path", &self.path)
            .field("identity", &self.identity())
            .field("generation", &self.generation)
            .finish()
    }
}

impl StateAuthority {
    /// Retains an existing state file and its parent directory by descriptor.
    ///
    /// # Errors
    ///
    /// Refuses a missing destination, a symbolic link, a non-regular file, a
    /// multiply-linked file, any symbolic link among the parent components, and
    /// a destination whose mode grants group or other access or carries a
    /// set-user-ID, set-group-ID, or sticky bit. Opening requires Linux and can
    /// fail on filesystem access or descriptor inspection.
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        Self::open(path, SavePolicy::ReplaceExisting, false)
    }

    /// Retains the parent directory of a destination that must not exist yet.
    ///
    /// The first publication creates the destination without clobbering an
    /// entry that appears first.
    ///
    /// # Errors
    ///
    /// Refuses an occupied name, including one occupied by a symbolic link, and
    /// any symbolic link among the parent components.
    pub fn create_new(path: &Path) -> io::Result<Self> {
        Self::open(path, SavePolicy::CreateNew, false)
    }

    /// Retains an existing state file, or the parent of a destination that does
    /// not exist yet.
    ///
    /// # Errors
    ///
    /// Refuses the same destinations as [`open_existing`](Self::open_existing)
    /// apart from absence, which yields a create-on-first-publication authority.
    pub fn open_or_create(path: &Path) -> io::Result<Self> {
        Self::open(path, SavePolicy::ReplaceOrCreate, false)
    }

    /// Open or create authority while clearing existing group/other access bits.
    ///
    /// Tightening preserves owner permission bits, so `0644` becomes `0600`
    /// and `0444` becomes `0400`. It changes the existing inode in place through
    /// a verified descriptor reopen and does not roll back that mode change
    /// if a later inspection fails.
    ///
    /// # Errors
    ///
    /// Still refuses set-user-ID, set-group-ID and sticky bits, and every
    /// destination [`open_existing`](Self::open_existing) refuses apart from
    /// absence and group or other access.
    pub fn open_or_create_tightening(path: &Path) -> io::Result<Self> {
        Self::open(path, SavePolicy::ReplaceOrCreate, true)
    }

    /// [`open_existing`](Self::open_existing) with the permission tightening of
    /// [`open_or_create_tightening`](Self::open_or_create_tightening).
    ///
    /// # Errors
    ///
    /// Refuses a missing destination and every destination
    /// [`open_or_create_tightening`](Self::open_or_create_tightening) refuses.
    pub fn open_existing_tightening(path: &Path) -> io::Result<Self> {
        Self::open(path, SavePolicy::ReplaceExisting, true)
    }

    fn open(path: &Path, policy: SavePolicy, tighten: bool) -> io::Result<Self> {
        let retained = crate::user_file::open_retained(path, policy)?;
        let held = match retained.existing {
            Some((file, mut metadata)) => {
                if tighten && metadata.mode & 0o7000 == 0 && metadata.mode & 0o077 != 0 {
                    // O_PATH cannot chmod; reopen and verify the same inode before changing mode.
                    let owner_only = metadata.mode & 0o700;
                    let (identity, _) = retained_state(&file)?;
                    let view = readable_view(&file, identity)?;
                    rustix::fs::fchmod(&view, rustix::fs::Mode::from_raw_mode(owner_only))
                        .map_err(io::Error::from)?;
                    metadata.mode = owner_only;
                }
                if metadata.mode & FORBIDDEN_STATE_MODE_BITS != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "state file is not owner-only",
                    ));
                }
                let (identity, _) = retained_state(&file)?;
                Held::Current {
                    file,
                    identity,
                    metadata,
                }
            }
            None => Held::Absent,
        };
        Ok(Self {
            path: path.to_path_buf(),
            parent: retained.parent,
            held,
            generation: 0,
        })
    }

    /// The pathname this authority was opened from.
    ///
    /// It is a diagnostic label: publication addresses the retained descriptors,
    /// not this path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The identity this authority currently owns.
    ///
    /// `None` before a create-on-first-publication authority has published, and
    /// after custody is lost.
    #[must_use]
    pub fn identity(&self) -> Option<StateIdentity> {
        match &self.held {
            Held::Current { identity, .. } => Some(*identity),
            Held::Absent | Held::Lost => None,
        }
    }

    /// Saturating count of custody rotations to newly published inodes.
    /// Starts at zero, including when opening an existing file.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether custody was lost, which refuses every later publication.
    #[must_use]
    pub fn has_lost_custody(&self) -> bool {
        matches!(self.held, Held::Lost)
    }

    /// Reads up to `cap` bytes from the retained inode without reopening its
    /// pathname.
    ///
    /// Returns [`CustodySnapshot::Absent`] when the authority retained an
    /// absent destination and has not itself published,
    /// [`CustodySnapshot::Lost`] after custody is lost, and
    /// [`CustodySnapshot::Current`] with the bounded content and an overflow
    /// flag otherwise. Absent and lost snapshots allocate no read buffer.
    /// Reads start at byte zero using `pread(2)` and do not share a file offset;
    /// an unchanged inode yields the same content across repeated calls.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::CapOverflow`] when `cap + 1` overflows `usize` or
    /// the system refuses the allocation.
    /// Returns [`ReadError::ConcurrentMutation`] when metadata observed through
    /// the held descriptor changed between the pre-read and post-read
    /// inspection, refusing possibly hybrid bytes rather than returning them.
    /// Returns [`ReadError::Io`] for any I/O failure, including a retained inode
    /// the process may not read and an environment without `/proc/self/fd`,
    /// through which the read reaches that inode without its pathname.
    pub fn read_bounded(&self, cap: usize) -> Result<CustodySnapshot, ReadError> {
        let (file, identity) = match &self.held {
            Held::Absent => return Ok(CustodySnapshot::Absent),
            Held::Lost => return Ok(CustodySnapshot::Lost),
            Held::Current { file, identity, .. } => (file, *identity),
        };
        read_bounded_from(file, identity, cap)
    }

    /// Publishes `bytes` as the next generation of this state file.
    ///
    /// # Errors
    ///
    /// Returns a [`PublishError`] whose [`custody`](PublishError::custody)
    /// reports what this authority owns afterwards.
    pub fn publish(&mut self, bytes: &[u8]) -> Result<StateIdentity, PublishError> {
        self.publish_with(|output| output.write_all(bytes))
    }

    /// Publishes bytes streamed by `write` as the next generation.
    ///
    /// A new destination is created owner-only (`0600`); a replacement carries
    /// forward the mode, owner, and group retained when the authority opened,
    /// and refuses if they changed since, so a state file is never published
    /// with permissions widened behind the writer's back. The callback runs
    /// after validation and before any name exists, so its failure publishes
    /// nothing. On success the returned identity is the inode now published at
    /// the destination name, and this authority owns it.
    ///
    /// # Errors
    ///
    /// Returns a [`PublishError`] for a substituted destination, parent, or
    /// staging name, for a destination whose mode or ownership changed after
    /// retention, for callback and filesystem failures, and for a durability
    /// failure after publication. [`PublishError::custody`] reports whether the
    /// original authority remains, rotated to the staged inode, or was lost;
    /// [`PublishError::staged_bytes`] reports where the staged bytes are named.
    ///
    /// # Panics
    ///
    /// A callback panic propagates without publishing the staged bytes.
    pub fn publish_with(
        &mut self,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<StateIdentity, PublishError> {
        let expected = match &self.held {
            Held::Current { file, metadata, .. } => ExpectedDestination {
                file: Some(file),
                metadata: Some(*metadata),
                parent: &self.parent,
            },
            Held::Absent => ExpectedDestination {
                file: None,
                metadata: None,
                parent: &self.parent,
            },
            Held::Lost => {
                return Err(PublishError {
                    save: SaveError::before(
                        SaveOperation::InspectDestination,
                        io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "state authority cannot prove which inode its destination names",
                        ),
                    ),
                    custody: PublishCustody::Lost,
                    staged: StagedBytes::Discarded,
                });
            }
        };
        let outcome =
            crate::user_file::publish_retained(&self.path, expected, NEW_STATE_MODE, write);
        match outcome {
            Ok(published) => match self.rotate(published) {
                Ok(identity) => Ok(identity),
                Err(error) => Err(PublishError {
                    save: SaveError::after(SaveOperation::Revalidate, error),
                    custody: PublishCustody::Lost,
                    staged: StagedBytes::AtDestination,
                }),
            },
            Err(failure) => {
                let staged = match failure.staged {
                    StagedNaming::NotPublished => StagedBytes::Discarded,
                    StagedNaming::AtDestination => StagedBytes::AtDestination,
                    StagedNaming::Named(name) => {
                        StagedBytes::AtPath(self.path.with_file_name(name))
                    }
                    StagedNaming::Unnamed => StagedBytes::Unnamed,
                };
                let custody = match failure.custody {
                    Custody::Unchanged => PublishCustody::Unchanged,
                    Custody::Staged(published) => match self.rotate(published) {
                        Ok(_) => PublishCustody::Rotated,
                        Err(_) => PublishCustody::Lost,
                    },
                    Custody::Unknown => {
                        self.held = Held::Lost;
                        PublishCustody::Lost
                    }
                };
                Err(PublishError {
                    save: failure.error,
                    custody,
                    staged,
                })
            }
        }
    }

    fn rotate(&mut self, published: File) -> io::Result<StateIdentity> {
        let carried = match &self.held {
            Held::Current { metadata, .. } => Some(*metadata),
            Held::Absent | Held::Lost => None,
        };
        match retained_state(&published) {
            Ok((identity, observed)) => {
                // Carry the applied baseline so a later permission change cannot be adopted.
                let metadata = carried.unwrap_or(RetainedMetadata {
                    mode: NEW_STATE_MODE,
                    uid: observed.uid,
                    gid: observed.gid,
                });
                self.held = Held::Current {
                    file: published,
                    identity,
                    metadata,
                };
                self.generation = self.generation.saturating_add(1);
                Ok(identity)
            }
            Err(error) => {
                self.held = Held::Lost;
                Err(error)
            }
        }
    }
}

#[cfg(unix)]
fn retained_state(file: &File) -> io::Result<(StateIdentity, RetainedMetadata)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok((
        StateIdentity {
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
fn retained_state(_file: &File) -> io::Result<(StateIdentity, RetainedMetadata)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "state authority requires a Unix filesystem",
    ))
}

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

// Reopen the retained O_PATH or write-only inode for reads, avoiding its destination name.
#[cfg(unix)]
fn readable_view(file: &File, identity: StateIdentity) -> io::Result<File> {
    use std::os::fd::AsRawFd;

    let view = File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    let (reopened, _) = retained_state(&view)?;
    // A `/proc` that is not procfs is the one substitution this pathname admits.
    if reopened != identity {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reopened descriptor is not the retained state inode",
        ));
    }
    Ok(view)
}

#[cfg(unix)]
fn read_bounded_from(
    file: &File,
    identity: StateIdentity,
    cap: usize,
) -> Result<CustodySnapshot, ReadError> {
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
        match view.read_at(&mut buf[filled..], filled as u64) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadError::Io(e)),
        }
    }

    #[cfg(test)]
    read_fault::fire_between_stats();

    let post = snapshot_inode(&view).map_err(ReadError::Io)?;
    if pre != post {
        return Err(ReadError::ConcurrentMutation);
    }

    let overflow = filled > cap;
    buf.truncate(filled.min(cap));

    Ok(CustodySnapshot::Current {
        bytes: buf,
        overflow,
        identity,
    })
}

#[cfg(not(unix))]
fn read_bounded_from(
    _file: &File,
    _identity: StateIdentity,
    _cap: usize,
) -> Result<CustodySnapshot, ReadError> {
    Err(ReadError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "bounded read requires a Unix filesystem",
    )))
}

#[cfg(test)]
mod read_fault {
    use std::cell::RefCell;

    thread_local! {
        static BETWEEN_STATS: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn fire_between_stats() {
        BETWEEN_STATS.with(|s| {
            if let Some(action) = s.borrow_mut().take() {
                action();
            }
        });
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests;
