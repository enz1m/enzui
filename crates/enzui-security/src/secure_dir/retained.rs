//! Descriptor-relative operations on ordinary directories, without an ownership or mode policy.
//!
//! Retained descriptors remain authoritative after pathname replacement. Initial
//! opens trust ancestors but refuse a symlink at the leaf; subroot operations
//! accept only basenames and never resolve those ancestors again. Paths are
//! diagnostic, not authority. Use `SecureRoot` when private adoption is required.
//!
//! Linux moves use kernel no-clobber rename. Only `EXDEV` enables regular-file
//! copy fallback; it publishes a synced held inode before removing the source.
//! Sources must be quiescent: identity checks are not content snapshots. Copy
//! preserves rwx bits and timestamps, not set-ID/sticky bits, ownership, ACLs or xattrs.
//! Rename and unlink still have a check-to-syscall window: POSIX has neither
//! rename-by-inode nor unlink-by-inode. Callers must exclude hostile writers
//! during that window. Failed moves report retained custody, never erase copies
//! automatically, and do not constitute a multi-file Trash transaction.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{AtFlags, Mode, OFlags};

use super::SecureRoot;
use super::capability::{entry_identity_at, reject_path_separators};
use super::open_file_identity;

/// An ordinary directory pinned by descriptor, including group/world-writable directories.
///
/// Clones share the descriptor. Neither opening nor dropping changes permissions
/// or removes entries; writable directories do not imply exclusive custody.
#[derive(Debug, Clone)]
pub struct RetainedDir {
    path: PathBuf,
    file: Arc<File>,
    identity: (u64, u64),
}

impl RetainedDir {
    /// Open an existing directory without following a terminal symlink.
    ///
    /// # Errors
    /// Returns an open/stat failure or `PermissionDenied` if the name changes
    /// between the initial no-follow observation and descriptor retention.
    pub fn open(path: &Path) -> io::Result<Self> {
        let observed = rustix::fs::statat(rustix::fs::CWD, path, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        let fd = rustix::fs::openat(
            rustix::fs::CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let directory = Self::from_fd(path.to_path_buf(), fd)?;
        if directory.identity != (observed.st_dev, observed.st_ino) {
            return Err(identity_changed());
        }
        Ok(directory)
    }

    pub(crate) fn from_fd(path: PathBuf, fd: OwnedFd) -> io::Result<Self> {
        let file = File::from(fd);
        if !file.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a directory",
            ));
        }
        let identity = open_file_identity(&file)?;
        Ok(Self {
            path,
            file: Arc::new(file),
            identity,
        })
    }

    pub(crate) fn fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }

    pub(crate) fn into_private(self) -> io::Result<SecureRoot> {
        SecureRoot::verify_private_descriptor(self.path, self.file.as_fd().try_clone_to_owned()?)
    }

    /// The original pathname, for diagnostics only.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The retained directory's device and inode.
    #[must_use]
    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }

    /// Open one child directory relative to the retained parent.
    ///
    /// # Errors
    /// Refuses empty, NUL, traversal and multi-component names, symlinks,
    /// non-directories, changed identities and descriptor-relative I/O failures.
    pub fn open_subroot(&self, name: &OsStr) -> io::Result<Self> {
        reject_path_separators(name)?;
        let observed = entry_identity_at(self.fd(), name)?;
        let fd = rustix::fs::openat(
            self.fd(),
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let child = Self::from_fd(self.path.join(name), fd)?;
        if child.identity != observed {
            return Err(identity_changed());
        }
        Ok(child)
    }

    /// Exclusively create and retain one child, requesting mode `0700`.
    ///
    /// The process umask may restrict the mode further. No chmod is attempted;
    /// an open failure can leave the newly created directory for recovery.
    ///
    /// # Errors
    /// Returns `AlreadyExists` for occupied names, or a basename/create/open failure.
    pub fn create_subroot(&self, name: &OsStr) -> io::Result<Self> {
        reject_path_separators(name)?;
        rustix::fs::mkdirat(self.fd(), name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
            .map_err(io::Error::from)?;
        self.open_subroot(name)
    }

    /// Create a missing child or adopt an existing ordinary directory without chmod.
    ///
    /// # Errors
    /// Propagates [`Self::create_subroot`] failures except a creation collision,
    /// which must pass [`Self::open_subroot`]'s no-follow checks.
    pub fn ensure_subroot(&self, name: &OsStr) -> io::Result<Self> {
        match self.create_subroot(name) {
            Ok(child) => Ok(child),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => self.open_subroot(name),
            Err(error) => Err(error),
        }
    }

    /// Observe an entry's device and inode without following symlinks.
    ///
    /// # Errors
    /// Returns a basename or descriptor-relative stat failure.
    pub fn entry_identity(&self, name: &OsStr) -> io::Result<(u64, u64)> {
        entry_identity_at(self.fd(), name)
    }

    /// Remove a non-directory name only while it denotes the held file.
    ///
    /// The file stays open across the check/unlink window. A replacement is
    /// left untouched and returns `false`; symlinks are never followed.
    ///
    /// # Errors
    /// Returns descriptor, basename, stat or unlink failures.
    pub fn remove_file_if_identity(&self, name: &OsStr, file: &File) -> io::Result<bool> {
        super::capability::unlink_entry_if_identity_at(
            self.fd(),
            name,
            open_file_identity(file)?,
            AtFlags::empty(),
        )
    }

    /// Publish a held inode at a new name relative to this directory.
    ///
    /// # Errors
    /// Returns `AlreadyExists` for occupied names, a basename failure, or a
    /// `linkat` failure. Procfs must be available and the inode must remain linked.
    #[cfg(target_os = "linux")]
    pub fn link_file_noreplace(&self, file: &File, name: &OsStr) -> io::Result<()> {
        reject_path_separators(name)?;
        crate::tempstage::link_new_at_by_identity(file, self.fd(), name)
    }

    /// Exclusively create the regular file `name`, requesting mode `0644`
    /// before the umask. A symlink at `name` is never followed.
    ///
    /// # Errors
    /// Returns `AlreadyExists` for an occupied name, or a basename/open failure.
    pub fn create_file_new(&self, name: &OsStr) -> io::Result<File> {
        reject_path_separators(name)?;
        let fd = rustix::fs::openat(
            self.fd(),
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .map_err(io::Error::from)?;
        Ok(File::from(fd))
    }

    /// Set this directory's permission bits through the retained descriptor.
    ///
    /// # Errors
    /// Returns the `fchmod` failure.
    pub fn set_mode(&self, mode: u32) -> io::Result<()> {
        rustix::fs::fchmod(self.fd(), Mode::from_raw_mode(mode)).map_err(io::Error::from)
    }

    /// Remove the child directory `name` and everything beneath it, only while
    /// the name still denotes `identity`.
    ///
    /// Descendants are opened without following symlinks, and each one is
    /// removed only while its name still denotes what was observed: a subtree
    /// renamed or replaced meanwhile is left alone, as is its ancestor. Removal
    /// stops after 64 levels or 65 536 entries. `Ok(true)` only when the whole
    /// tree and the name are gone.
    ///
    /// # Errors
    /// Returns a basename failure, or an open/stat/unlink failure of the child
    /// itself.
    pub fn remove_tree_if_identity(&self, name: &OsStr, identity: (u64, u64)) -> io::Result<bool> {
        reject_path_separators(name)?;
        crate::tempstage::remove_tree_if_identity(self.fd(), name, identity, &self.path.join(name))
    }

    /// Flush directory-entry changes through the retained descriptor.
    ///
    /// # Errors
    /// Returns the filesystem's directory `fsync` error.
    pub fn sync_all(&self) -> io::Result<()> {
        self.file.sync_all()
    }
}

impl AsFd for RetainedDir {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd()
    }
}

fn identity_changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "entry no longer names the retained inode",
    )
}

#[cfg(target_os = "linux")]
mod moves;
#[cfg(target_os = "linux")]
pub use moves::{MoveCustody, MoveFailure, RetainedEntry};

#[cfg(target_os = "linux")]
mod enumerate;
#[cfg(target_os = "linux")]
pub use enumerate::EnumerationLimits;

#[cfg(target_os = "linux")]
mod measure;
#[cfg(target_os = "linux")]
pub use measure::TreeSizeLimits;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests;
