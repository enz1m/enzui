use super::*;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use crate::secure_dir::capability::{create_entry_private_at, unlink_entry_if_identity_at};
use crate::tempstage::{link_new_at_by_identity, sync_staged, unique_temp_file_name, write_staged};

/// A retained entry and parent; dropping it closes descriptors but never deletes data.
///
/// Symlinks are retained as links, not followed. Names can still be replaced by
/// other writers, so removal and publication always recheck the held identity.
#[derive(Debug)]
pub struct RetainedEntry {
    parent: RetainedDir,
    name: OsString,
    file: Arc<File>,
    identity: (u64, u64),
}

/// Locations retained after a failed move, for explicit recovery or reporting.
///
/// These are identity-bound capabilities, not promises that the names remain
/// reachable. None of the variants removes data when dropped.
#[derive(Debug)]
#[must_use]
pub enum MoveCustody {
    /// No destination was published; the original entry is retained.
    Source(RetainedEntry),
    /// The move or source unlink succeeded; destination durability may have failed.
    Destination(RetainedEntry),
    /// A copy was published but source removal did not succeed.
    Both {
        /// The original inode and parent, retained for a retry.
        source: RetainedEntry,
        /// The copied inode and parent, retained for recovery.
        destination: RetainedEntry,
    },
    /// Rename succeeded but the destination name failed its identity check.
    Uncertain {
        /// The original name and retained inode; it may no longer be reachable here.
        source: RetainedEntry,
        /// The attempted destination name bound to that same retained inode.
        destination: RetainedEntry,
    },
}

/// A move failure carrying the descriptors needed to avoid pathname-only recovery.
#[derive(Debug)]
pub struct MoveFailure {
    /// The I/O or identity-check failure; raw `EXDEV` is retained by rename-only moves.
    pub error: io::Error,
    /// Retained source/destination custody at the failure boundary, boxed so
    /// a failed move's `Err` stays small.
    pub custody: Box<MoveCustody>,
}

impl std::fmt::Display for MoveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}
impl std::error::Error for MoveFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl RetainedDir {
    /// Retain one entry without following symlinks or opening devices for I/O.
    ///
    /// Linux `O_PATH` permits directories and symlinks for same-filesystem moves;
    /// only regular files can later use the copy fallback.
    ///
    /// # Errors
    /// Returns a basename/open/stat error or an identity mismatch.
    pub fn retain_entry(&self, name: &OsStr) -> io::Result<RetainedEntry> {
        reject_path_separators(name)?;
        let observed = entry_identity_at(self.fd(), name)?;
        let fd = rustix::fs::openat(
            self.fd(),
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let file = File::from(fd);
        let identity = open_file_identity(&file)?;
        if identity != observed {
            return Err(identity_changed());
        }
        Ok(RetainedEntry {
            parent: self.clone(),
            name: name.to_os_string(),
            file: Arc::new(file),
            identity,
        })
    }
}

impl RetainedEntry {
    pub(super) fn metadata(&self) -> io::Result<std::fs::Metadata> {
        self.file.metadata()
    }

    pub(super) fn open_directory(&self) -> io::Result<RetainedDir> {
        let fd = rustix::fs::openat(
            &*self.file,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        RetainedDir::from_fd(self.parent.path.join(&self.name), fd)
    }

    /// The basename within the retained parent, for diagnostics.
    #[must_use]
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// The parent descriptor used for entry operations.
    #[must_use]
    pub fn parent(&self) -> &RetainedDir {
        &self.parent
    }

    /// The device and inode kept alive by this capability.
    #[must_use]
    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }

    /// Check the current name against the retained descriptor, without following links.
    ///
    /// # Errors
    /// Returns a stat failure or `PermissionDenied` for a substituted name.
    pub fn verify(&self) -> io::Result<()> {
        if entry_identity_at(self.parent.fd(), &self.name)? != self.identity {
            return Err(identity_changed());
        }
        Ok(())
    }

    /// Remove this identity's name, or leave a replacement untouched and return `false`.
    ///
    /// Directories must be empty. This does not sync the parent. The descriptor
    /// stays open across the check/unlink window documented by the module.
    ///
    /// # Errors
    /// Returns stat/unlink errors, including a missing name or nonempty directory.
    pub fn remove_if_identity(&self) -> io::Result<bool> {
        let flags = if self.file.metadata()?.is_dir() {
            AtFlags::REMOVEDIR
        } else {
            AtFlags::empty()
        };
        unlink_entry_if_identity_at(self.parent.fd(), &self.name, self.identity, flags)
    }

    /// Move between retained directories without overwriting any destination entry.
    ///
    /// Files, symlinks and directories retain their inode. Both parents are
    /// synced on success. There is no copy fallback in this operation.
    ///
    /// # Errors
    /// Returns typed custody on identity, rename or durability failure; an occupied
    /// destination is `AlreadyExists`, and cross-filesystem placement is `EXDEV`.
    pub fn rename_noreplace(
        self,
        destination: &RetainedDir,
        name: &OsStr,
    ) -> Result<Self, MoveFailure> {
        if let Err(error) = reject_path_separators(name).and_then(|()| self.verify()) {
            return Err(self.source_failure(error));
        }
        let renamed = self.at(destination, name);
        let result = rename_noreplace(&self, destination, name);
        if let Err(error) = result {
            return Err(self.source_failure(error));
        }
        #[cfg(test)]
        let observation = fault::run(fault::Point::AfterRename).and_then(|()| renamed.verify());
        #[cfg(not(test))]
        let observation = renamed.verify();
        if let Err(error) = observation {
            return Err(MoveFailure {
                error,
                custody: Box::new(MoveCustody::Uncertain {
                    source: self,
                    destination: renamed,
                }),
            });
        }
        if let Err(error) = sync_destination(destination).and_then(|()| sync_source(&self.parent)) {
            return Err(MoveFailure {
                error,
                custody: Box::new(MoveCustody::Destination(renamed)),
            });
        }
        if let Err(error) = renamed.verify() {
            return Err(MoveFailure {
                error,
                custody: Box::new(MoveCustody::Uncertain {
                    source: self,
                    destination: renamed,
                }),
            });
        }
        Ok(renamed)
    }

    /// Rename without clobbering, copying a regular file only after kernel `EXDEV`.
    ///
    /// Copying stages bytes privately, syncs the data, publishes the held inode
    /// with a no-clobber link and syncs the destination before source removal.
    /// Rwx bits and timestamps are preserved; set-ID/sticky bits, ownership,
    /// ACLs and xattrs are not. The source must be quiescent; concurrent mutation is not a snapshot.
    ///
    /// # Errors
    /// Returns [`MoveFailure`] with retained custody. Cross-device directories,
    /// symlinks and special files are `Unsupported`, with source custody intact.
    pub fn move_noreplace(
        self,
        destination: &RetainedDir,
        name: &OsStr,
    ) -> Result<Self, MoveFailure> {
        match self.rename_noreplace(destination, name) {
            Err(failure)
                if failure.error.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error()) =>
            {
                match *failure.custody {
                    MoveCustody::Source(source) => source.copy_remove(destination, name),
                    custody => Err(MoveFailure {
                        error: failure.error,
                        custody: Box::new(custody),
                    }),
                }
            }
            outcome => outcome,
        }
    }

    fn at(&self, parent: &RetainedDir, name: &OsStr) -> Self {
        Self {
            parent: parent.clone(),
            name: name.to_os_string(),
            file: Arc::clone(&self.file),
            identity: self.identity,
        }
    }

    fn source_failure(self, error: io::Error) -> MoveFailure {
        MoveFailure {
            error,
            custody: Box::new(MoveCustody::Source(self)),
        }
    }

    fn copy_remove(self, destination: &RetainedDir, name: &OsStr) -> Result<Self, MoveFailure> {
        let source_stamp = match self.file.metadata() {
            Ok(metadata) => content_stamp(&metadata),
            Err(error) => return Err(self.source_failure(error)),
        };
        let copied = match self.copy_publish(destination, name) {
            Ok(copied) => copied,
            Err(error) => return Err(self.source_failure(error)),
        };
        let before_remove = || -> io::Result<()> {
            sync_destination(destination)?;
            #[cfg(test)]
            fault::run(fault::Point::BeforeRemove)?;
            copied.verify()?;
            self.verify()?;
            if content_stamp(&self.file.metadata()?) != source_stamp {
                return Err(identity_changed());
            }
            if !self.remove_if_identity()? {
                return Err(identity_changed());
            }
            Ok(())
        };
        if let Err(error) = before_remove() {
            return Err(MoveFailure {
                error,
                custody: Box::new(MoveCustody::Both {
                    source: self,
                    destination: copied,
                }),
            });
        }
        if let Err(error) = sync_source(&self.parent) {
            return Err(MoveFailure {
                error,
                custody: Box::new(MoveCustody::Destination(copied)),
            });
        }
        Ok(copied)
    }

    fn copy_publish(&self, destination: &RetainedDir, name: &OsStr) -> io::Result<Self> {
        self.verify()?;
        let initial = self.file.metadata()?;
        if !initial.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cross-device copy requires a regular file",
            ));
        }
        let fd = rustix::fs::openat(
            self.parent.fd(),
            &self.name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let mut input = File::from(fd);
        if open_file_identity(&input)? != self.identity {
            return Err(identity_changed());
        }
        let mut stage = CopyStage::create(destination, name)?;
        let mut remaining = initial.len();
        let mut buffer = vec![0_u8; 64 * 1024];
        while remaining != 0 {
            let limit = usize::try_from(remaining.min(64 * 1024))
                .map_err(|_| io::Error::other("copy buffer length overflow"))?;
            let count = input.read(&mut buffer[..limit])?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "source shrank during copy",
                ));
            }
            write_staged(&mut stage.file, &buffer[..count])?;
            remaining -=
                u64::try_from(count).map_err(|_| io::Error::other("copy count overflow"))?;
        }
        stage.file.flush()?;
        stage
            .file
            .set_permissions(std::fs::Permissions::from_mode(initial.mode() & 0o777))?;
        stage.file.set_times(
            std::fs::FileTimes::new()
                .set_accessed(initial.accessed()?)
                .set_modified(initial.modified()?),
        )?;
        sync_staged(&stage.file)?;
        #[cfg(test)]
        fault::run(fault::Point::AfterCopy)?;
        self.verify()?;
        if content_stamp(&initial) != content_stamp(&input.metadata()?) {
            return Err(identity_changed());
        }
        let published = Self {
            parent: destination.clone(),
            name: name.to_os_string(),
            identity: stage.identity,
            file: Arc::new(stage.file.try_clone()?),
        };
        link_new_at_by_identity(&stage.file, destination.fd(), name)?;
        // Publication transfers custody before any fallible directory barrier or cleanup.
        Ok(published)
    }
}

fn content_stamp(metadata: &std::fs::Metadata) -> (u64, i64, i64, i64, i64) {
    (
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn rename_noreplace(
    source: &RetainedEntry,
    destination: &RetainedDir,
    name: &OsStr,
) -> io::Result<()> {
    #[cfg(test)]
    fault::run(fault::Point::Rename)?;
    rustix::fs::renameat_with(
        source.parent.fd(),
        &source.name,
        destination.fd(),
        name,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

fn sync_destination(destination: &RetainedDir) -> io::Result<()> {
    #[cfg(test)]
    fault::run(fault::Point::DestinationSync)?;
    destination.sync_all()
}

fn sync_source(source: &RetainedDir) -> io::Result<()> {
    #[cfg(test)]
    fault::run(fault::Point::SourceSync)?;
    source.sync_all()
}

struct CopyStage {
    parent: RetainedDir,
    name: OsString,
    file: File,
    identity: (u64, u64),
}

impl CopyStage {
    fn create(parent: &RetainedDir, destination: &OsStr) -> io::Result<Self> {
        let name = unique_temp_file_name(destination);
        let file = create_entry_private_at(parent.fd(), &name)?;
        let identity = open_file_identity(&file)?;
        let stage = Self {
            parent: parent.clone(),
            name,
            file,
            identity,
        };
        stage
            .file
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(stage)
    }
}

impl Drop for CopyStage {
    fn drop(&mut self) {
        let _ = unlink_entry_if_identity_at(
            self.parent.fd(),
            &self.name,
            self.identity,
            AtFlags::empty(),
        );
    }
}

#[cfg(test)]
pub(super) mod fault {
    use std::cell::RefCell;
    use std::io;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Point {
        Rename,
        AfterCopy,
        DestinationSync,
        SourceSync,
        BeforeRemove,
        AfterRename,
    }
    type Hook = Box<dyn FnMut(Point) -> io::Result<()>>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = RefCell::new(None); }

    pub(crate) fn run(point: Point) -> io::Result<()> {
        HOOK.with(|slot| match slot.borrow_mut().as_mut() {
            Some(hook) => hook(point),
            None => Ok(()),
        })
    }
}
