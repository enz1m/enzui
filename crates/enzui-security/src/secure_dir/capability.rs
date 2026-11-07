//! Private-root policy over descriptor-relative filesystem operations.
//!
//! Entry operations use the retained descriptor even after pathname replacement.
//! Pathname reachability is checked separately by `SecureRoot::revalidate`;
//! callers decide when a moved root must invalidate their higher-level operation.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, Dir, Mode, OFlags};

use super::SecureDirRefusal;

/// A verified private directory bound to a retained descriptor.
///
/// Construct with [`SecureRoot::open`]; every entry-level op below is
/// descriptor-relative and cannot be redirected by a later pathname
/// replacement. Entry operations also refuse owner or writable-mode drift on
/// the retained directory. See the [directory contract](crate::secure_dir)
/// for the remaining pathname race.
#[derive(Debug)]
pub struct SecureRoot {
    /// Pathname the root was opened under. Kept for display, for the
    /// [`SecureRoot::revalidate`] mismatch signal, and for callers that log
    /// a diagnostic — never for opening entries.
    path: PathBuf,
    /// Retained `O_DIRECTORY | O_NOFOLLOW` descriptor on the verified inode.
    /// All entry ops use `*at` syscalls relative to this fd.
    fd: OwnedFd,
    // Pathname reachability is compared with this retained identity, not a new pathname open.
    dev: u64,
    ino: u64,
    owner_uid: u32,
}

impl SecureRoot {
    /// Verify `path`, open it with `O_DIRECTORY | O_NOFOLLOW`, and validate
    /// the descriptor — not the pathname — as the authoritative object.
    ///
    /// # The two-syscall race that this closes
    ///
    /// The pathname-view verification ([`super::ensure_private_dir`]) uses
    /// `symlink_metadata`; a moment later the openat sees the pathname a
    /// second time. Between those two syscalls a same-uid attacker (or a
    /// writable ancestor) can rename the verified leaf away and put a
    /// different — possibly wrong-owner, group/world-writable, or otherwise
    /// hostile — directory at the same pathname, and every check performed on
    /// the pathname view is by definition against the *first* inode. Without
    /// the fd-side re-check the resulting `SecureRoot` would be bound to that
    /// substituted inode.
    ///
    /// This constructor therefore treats the retained fd as authoritative and
    /// runs every leaf check a second time against `fstat(fd)`:
    ///
    /// 1. File type: `S_IFMT` on the fd must be `S_IFDIR`. `O_DIRECTORY`
    ///    already refused a non-directory in the kernel, but the explicit
    ///    check documents the invariant so the fd type is not silently trusted
    ///    to the open flag.
    /// 2. Owner: `st_uid` on the fd must equal the process's effective uid. A
    ///    wrong-owner substitution between `symlink_metadata` and `openat` is
    ///    refused with [`SecureDirRefusal::WrongOwner`] on the fd's uid, not
    ///    the pathname view's.
    /// 3. Mode: `st_mode & 0o022 == 0` on the fd. A group/world-writable
    ///    substitution is refused with [`SecureDirRefusal::GroupOrWorldWritable`].
    /// 4. Identity: the fd's `(st_dev, st_ino)` must equal the
    ///    `(dev, ino)` observed by the initial `symlink_metadata` check.
    ///    This catches a substitution even when the
    ///    replacement happens to be same-uid and 0700 — a same-uid attacker's
    ///    own private directory swapped in is still not the verified inode.
    ///    Refused with [`SecureDirRefusal::RootIdentityChanged`].
    ///
    /// Only after those four fd-side checks pass is the `SecureRoot` handed
    /// out. Every later op is bound to the fd's `(dev, ino)`; pathname
    /// [`SecureRoot::revalidate`] remains a fail-closed signal on top of that.
    ///
    /// # Errors
    ///
    /// [`SecureDirRefusal`] on the same conditions as
    /// [`super::ensure_private_dir`], plus the fd-side re-checks above.
    /// [`SecureDirRefusal::CreateFailed`] when the `O_DIRECTORY | O_NOFOLLOW`
    /// open fails for any other reason (`EACCES`, a race where the leaf was
    /// replaced with a symlink or non-directory between verification and open
    /// — `O_NOFOLLOW`/`O_DIRECTORY` refuse those in the kernel).
    pub fn open(path: &Path) -> Result<Self, SecureDirRefusal> {
        // Pathname-view verification. `create_private_chain` makes any
        // missing components at 0o700, then `symlink_metadata` refuses a
        // planted symlink, wrong-owner, or writable leaf; the returned
        // identity is the pair the openat below must reproduce.
        let verified = super::ensure_and_verify_leaf(path)?;

        // Open the verified path with O_DIRECTORY | O_NOFOLLOW. The kernel
        // refuses a symlink at the leaf via `O_NOFOLLOW` (ELOOP) and refuses
        // a non-directory via `O_DIRECTORY` (ENOTDIR); `O_CLOEXEC` keeps the
        // descriptor out of unrelated child processes.
        let fd = rustix::fs::openat(
            rustix::fs::CWD,
            &verified.path,
            OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::RDONLY,
            Mode::empty(),
        )
        .map_err(|_| SecureDirRefusal::CreateFailed)?;

        // The fd is now authoritative: every check that ran against the
        // pathname view runs a second time here, against the inode we
        // actually retained. This is what closes the two-syscall race the
        // module doc describes.
        verify_open_fd_against(&fd, &verified)?;

        // The identity we store on `Self` is the FD's, not the pathname's:
        // if for any reason they were *close but not equal* and got past the
        // check above, we would rather record what the fd actually points to
        // than what we hoped it would point to. In practice the check above
        // rejected any mismatch, so these are the same numbers.
        let stat = rustix::fs::fstat(&fd).map_err(|_| SecureDirRefusal::CreateFailed)?;
        let (dev, ino) = stat_dev_ino(&stat);

        Ok(Self {
            path: verified.path,
            fd,
            dev,
            ino,
            owner_uid: verified.our_uid,
        })
    }

    /// Test-only fd-side verifier.
    ///
    /// Runs the exact four checks that [`SecureRoot::open`]'s constructor
    /// runs — file type, owner uid, mode, `(dev, ino)` identity — against an
    /// arbitrary already-open directory fd, with the pathname-view identity
    /// the caller believed it verified. Tests use this to feed the check a
    /// wrong-owner fd (a system directory), a chmod'd-back-to-0o777 fd, or a
    /// fd against a same-uid decoy directory with a different inode, so the
    /// four checks are exercised in isolation on real disk without racing
    /// them against `symlink_metadata` in production.
    ///
    /// `expected_dev`/`expected_ino` supply the identity the pathname view
    /// *would have* observed. Passing the fd's own identity here makes the
    /// identity check vacuous, which is why the tests always pass the
    /// previously-observed pathname identity so the four checks stay
    /// independent.
    ///
    /// Hidden from rustdoc: this is a security-test seam, not a supported API.
    /// Callers who need descriptor-relative operations construct a
    /// [`SecureRoot`] with [`SecureRoot::open`], which runs the same checks
    /// on the constructor's own fd.
    #[doc(hidden)]
    pub fn verify_open_fd_for_test(
        fd: &std::os::fd::OwnedFd,
        expected_dev: u64,
        expected_ino: u64,
        our_uid: u32,
    ) -> Result<(), SecureDirRefusal> {
        verify_open_fd_against(
            fd,
            &super::VerifiedLeaf {
                path: PathBuf::new(),
                dev: expected_dev,
                ino: expected_ino,
                our_uid,
            },
        )
    }

    /// Open `path` with the exact `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC |
    /// O_RDONLY` flag set [`SecureRoot::open`] uses, for tests that need to
    /// hand [`SecureRoot::verify_open_fd_for_test`] an fd whose provenance
    /// matches the real constructor's fd.
    ///
    /// Hidden from rustdoc for the same reason as
    /// [`SecureRoot::verify_open_fd_for_test`]: a supported caller opens the
    /// root through [`SecureRoot::open`], not by producing a raw fd.
    #[doc(hidden)]
    pub fn openat_directory_for_test(path: &Path) -> std::io::Result<std::os::fd::OwnedFd> {
        rustix::fs::openat(
            rustix::fs::CWD,
            path,
            OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::RDONLY,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)
    }

    /// The pathname this root was opened under.
    ///
    /// Kept for display, for [`SecureRoot::revalidate`]'s mismatch signal, and
    /// for callers that log a diagnostic — not for opening entries. The only
    /// safe way to reach an entry is through the methods on `self`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The `(dev, ino)` captured from the retained descriptor at open time.
    #[must_use]
    pub const fn identity(&self) -> (u64, u64) {
        (self.dev, self.ino)
    }

    pub(crate) fn retained_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub(crate) fn verify_retained_policy(&self) -> io::Result<()> {
        let stat = rustix::fs::fstat(&self.fd).map_err(io::Error::from)?;
        verify_private_stat(&stat, self.owner_uid)
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error))
    }

    fn from_retained_directory(path: PathBuf, fd: OwnedFd, owner_uid: u32) -> io::Result<Self> {
        let stat = rustix::fs::fstat(&fd).map_err(io::Error::from)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "retained stage descriptor is not a directory",
            ));
        }
        let (dev, ino) = stat_dev_ino(&stat);
        Ok(Self {
            path,
            fd,
            dev,
            ino,
            owner_uid,
        })
    }

    /// Open a private child without resolving this root's pathname again.
    ///
    /// # Errors
    /// Refuses invalid basenames, symlinks, non-directories, foreign owners,
    /// group/other-writable children, and descriptor-relative I/O failures.
    pub fn open_subroot(&self, name: &OsStr) -> io::Result<Self> {
        self.verify_retained_policy()?;
        self.ordinary_directory()?
            .open_subroot(name)?
            .into_private()
    }

    /// Create or adopt a private child relative to the retained descriptor.
    ///
    /// New children request mode `0700`; existing children are verified without
    /// chmod. Failure can leave a newly created directory for explicit recovery.
    ///
    /// # Errors
    /// The same refusals as [`Self::open_subroot`], plus creation failures.
    pub fn ensure_subroot(&self, name: &OsStr) -> io::Result<Self> {
        self.verify_retained_policy()?;
        self.ordinary_directory()?
            .ensure_subroot(name)?
            .into_private()
    }

    /// Retain this directory without carrying its private-adoption policy.
    ///
    /// # Errors
    /// Returns a descriptor duplication or inspection failure.
    pub fn ordinary_directory(&self) -> io::Result<super::RetainedDir> {
        super::RetainedDir::from_fd(self.path.clone(), self.fd.try_clone()?)
    }

    pub(crate) fn verify_private_descriptor(path: PathBuf, fd: OwnedFd) -> io::Result<Self> {
        let stat = rustix::fs::fstat(&fd).map_err(io::Error::from)?;
        let our_uid = crate::runtime_dir::current_uid().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                SecureDirRefusal::OwnerUnverifiable,
            )
        })?;
        let verified = super::VerifiedLeaf {
            path: path.clone(),
            dev: stat.st_dev,
            ino: stat.st_ino,
            our_uid,
        };
        verify_open_fd_against(&fd, &verified)
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error))?;
        Self::from_retained_directory(path, fd, our_uid)
    }

    /// Check that the retained directory remains private and its original path
    /// still reaches it.
    ///
    /// Entry operations use the descriptor regardless of pathname reachability.
    /// A successful check is not a lock against a later metadata or pathname
    /// change.
    ///
    /// # Errors
    /// Returns the applicable private-directory refusal after owner or mode drift,
    /// or [`SecureDirRefusal::RootIdentityChanged`] for a missing, unreadable or
    /// substituted name, including a terminal symlink.
    pub fn revalidate(&self) -> Result<(), SecureDirRefusal> {
        let retained = rustix::fs::fstat(&self.fd).map_err(|_| SecureDirRefusal::CreateFailed)?;
        verify_private_stat(&retained, self.owner_uid)?;
        match rustix::fs::statat(rustix::fs::CWD, &self.path, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                let current = stat_dev_ino(&stat);
                if current == (self.dev, self.ino) {
                    Ok(())
                } else {
                    Err(SecureDirRefusal::RootIdentityChanged {
                        original: (self.dev, self.ino),
                        current: Some(current),
                    })
                }
            }
            Err(_) => Err(SecureDirRefusal::RootIdentityChanged {
                original: (self.dev, self.ino),
                current: None,
            }),
        }
    }

    /// Open an entry for reading without following a terminal symlink.
    ///
    /// Uses `openat(fd, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK)` relative to
    /// the retained root descriptor. `O_NONBLOCK` keeps the open from hanging
    /// inside the kernel on a planted FIFO before the caller can `fstat` the
    /// descriptor; it is inert for a regular file. The descriptor is only
    /// trustworthy once the caller confirms a regular file via `metadata()`.
    ///
    /// # Errors
    ///
    /// An I/O error when the directory no longer meets its policy, `name` contains a separator, or
    /// the open fails.
    pub fn open_entry_read(&self, name: &OsStr) -> io::Result<File> {
        self.verify_retained_policy()?;
        reject_path_separators(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        Ok(File::from(fd))
    }

    /// Exclusively create an entry with mode `0600`, subject to the process umask.
    ///
    /// The returned writable descriptor is relative to this retained root;
    /// occupied names of every type are refused without following symlinks.
    ///
    /// # Errors
    /// Returns a basename-validation or `openat` failure.
    pub fn create_entry_private(&self, name: &OsStr) -> io::Result<File> {
        self.verify_retained_policy()?;
        create_entry_private_at(self.retained_fd(), name)
    }

    /// Create an unnamed read/write file in the retained directory on Linux.
    ///
    /// The file has no directory entry, is private (`0600`, subject to umask),
    /// and is reclaimed when its last descriptor closes, including after a crash.
    /// Its descriptor is close-on-exec. Renaming or replacing the root pathname
    /// does not redirect creation.
    ///
    /// # Errors
    /// Returns an I/O error on retained-directory policy drift or when the
    /// filesystem does not support `O_TMPFILE`. There is no named fallback.
    #[cfg(target_os = "linux")]
    pub fn create_anonymous_file(&self) -> io::Result<File> {
        self.verify_retained_policy()?;
        rustix::fs::openat(
            &self.fd,
            ".",
            OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io::Error::from)
    }

    /// Observe an entry's device and inode through the retained root.
    ///
    /// Symlinks report their own identity, not their target's. Compare with
    /// [`open_file_identity`] while holding the originating descriptor open.
    ///
    /// # Errors
    /// Returns a basename-validation or no-follow `fstatat` failure.
    pub fn entry_identity(&self, name: &OsStr) -> io::Result<(u64, u64)> {
        self.verify_retained_policy()?;
        entry_identity_at(self.retained_fd(), name)
    }

    /// Unlink a non-directory name only when its current identity matches.
    ///
    /// A replacement is left untouched and returns `false`. Keep the descriptor
    /// that produced `identity` open to prevent inode-number reuse. POSIX has no
    /// unlink-by-inode: replacement between the check and unlink remains possible.
    ///
    /// # Errors
    /// Returns a basename, `fstatat` or `unlinkat` failure, including a missing name.
    pub fn unlink_entry_if_identity(&self, name: &OsStr, identity: (u64, u64)) -> io::Result<bool> {
        self.verify_retained_policy()?;
        unlink_entry_if_identity_at(self.retained_fd(), name, identity, AtFlags::empty())
    }

    /// Rename an entry within this root using `renameat`.
    ///
    /// Same source and destination directory fd, so the rename never crosses
    /// out of this root. Publication of an atomically-written cache entry
    /// goes through here.
    ///
    /// This helper renames a *name*: a caller publishing a staged inode must
    /// bind `from` to that inode with [`SecureRoot::entry_identity`] first, as
    /// [`crate::tempstage::RootTempStage::commit`] does.
    ///
    /// # Errors
    ///
    /// An I/O error when the directory no longer meets its policy, a name contains a separator, or
    /// the rename fails.
    pub fn rename_entry(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        self.verify_retained_policy()?;
        reject_path_separators(from)?;
        reject_path_separators(to)?;
        rustix::fs::renameat(&self.fd, from, &self.fd, to).map_err(io::Error::from)
    }

    /// Unlink an entry using `unlinkat`.
    ///
    /// Refuses a directory (`AtFlags::empty()` — not `AT_REMOVEDIR`), matching
    /// `unlink(2)` semantics: a directory substituted at an entry name is not
    /// removed by this helper, and neither is a symlink's target (the link
    /// itself is unlinked, its target untouched).
    ///
    /// # Errors
    ///
    /// An I/O error when the directory no longer meets its policy, `name` contains a separator, or
    /// the unlink fails.
    pub fn unlink_entry(&self, name: &OsStr) -> io::Result<()> {
        self.verify_retained_policy()?;
        reject_path_separators(name)?;
        rustix::fs::unlinkat(&self.fd, name, AtFlags::empty()).map_err(io::Error::from)
    }

    /// Update an entry's mtime without following a symlink.
    ///
    /// Opens the entry with `O_WRONLY | O_APPEND | O_NOFOLLOW | O_NONBLOCK`,
    /// confirms a regular file via `metadata()`, then calls
    /// `File::set_len(current_len)` — the same trick the former path-based code used
    /// to bump mtime without a write. A symlink substituted at the entry name
    /// is refused in the kernel; a non-regular substitution is refused by the
    /// metadata check, so this helper cannot truncate a FIFO, device, or
    /// directory. Failure is expected to be tolerated by callers — the LRU
    /// touch only affects eviction ordering, never data.
    ///
    /// # Errors
    ///
    /// An I/O error when the directory no longer meets its policy, `name` contains a separator or
    /// is not a regular file, or the update fails.
    pub fn touch_entry_mtime(&self, name: &OsStr) -> io::Result<()> {
        self.verify_retained_policy()?;
        reject_path_separators(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::WRONLY | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let file = File::from(fd);
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache entry is not a regular file",
            ));
        }
        file.set_len(meta.len())?;
        Ok(())
    }

    /// Yield entry names with signed mtime nanoseconds and byte sizes.
    ///
    /// Modification time remains signed so files before the Unix epoch retain
    /// their chronological order instead of wrapping into the far future.
    /// A fresh `openat(fd, ".", O_RDONLY | O_DIRECTORY)` gives a duplicate
    /// descriptor to the pinned inode; `rustix::fs::Dir::read_from` then owns
    /// that duplicate and iterates via `getdents`. `.` and `..` are dropped,
    /// and any name containing a path separator (impossible from `getdents`
    /// but defensively filtered) is dropped. Non-regular files are not
    /// filtered here — a caller's clear or eviction needs to observe them so
    /// the resulting `unlinkat` failure surfaces (a directory planted at an
    /// entry name is an EISDIR the caller reports to the user); the fd-relative
    /// entry opens carry their own `O_NOFOLLOW` + `is_file()` gate that
    /// refuses reads/writes on such objects.
    ///
    /// The mtime and size are captured by `fstatat(fd, name, AT_SYMLINK_NOFOLLOW)`
    /// so a substituted symlink at an entry name cannot bias the accounting.
    /// Directory-read failures are returned with root context. Once `getdents`
    /// has exposed a name, metadata failures are returned with that exact name
    /// and root rather than inventing zero metadata.
    ///
    /// There is one narrow race exception: `ENOENT` is retried once. If the
    /// second fd-relative stat also says the observed name is absent, that name
    /// disappeared between `getdents` and `statat` and is omitted from this
    /// snapshot. A later recreation belongs to the next census. The retry is
    /// important: a transient first `ENOENT` cannot hide an entry that is still
    /// present at the confirmation point. Every other error makes the whole
    /// census incomplete and is returned.
    ///
    /// # Errors
    ///
    /// An I/O error when the directory no longer meets its policy or cannot be read.
    pub fn entry_names(&self) -> io::Result<Vec<EntryStat>> {
        self.verify_retained_policy()?;
        self.entry_names_impl()
    }

    /// One entry's file size, from `fstatat` through the retained root fd.
    ///
    /// Cache eviction accounts a total-bytes budget; the size must be read
    /// through the pinned fd so a substituted pathname cannot bias the
    /// accounting.
    ///
    /// # Errors
    ///
    /// An I/O error when the directory no longer meets its policy, `name` contains a separator, or
    /// the entry cannot be inspected.
    pub fn entry_size(&self, name: &OsStr) -> io::Result<u64> {
        self.verify_retained_policy()?;
        reject_path_separators(name)?;
        let stat = rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| contextual_io("stat entry size", &self.path, Some(name), error))?;
        Ok(stat_size(&stat))
    }

    fn entry_names_impl(&self) -> io::Result<Vec<EntryStat>> {
        let dirfd = rustix::fs::openat(
            &self.fd,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| contextual_io("open directory census", &self.path, None, error))?;
        let mut dir = Dir::read_from(dirfd)
            .map_err(|error| contextual_io("start directory census", &self.path, None, error))?;
        let mut out: Vec<EntryStat> = Vec::new();
        while let Some(item) = dir.read() {
            #[cfg(test)]
            if let Some(error) = census_fault::take_iterator() {
                return Err(contextual_io(
                    "read directory entries",
                    &self.path,
                    None,
                    error,
                ));
            }

            let entry = item.map_err(|error| {
                contextual_io("read directory entries", &self.path, None, error)
            })?;
            let name_bytes = entry.file_name().to_bytes();
            if name_bytes == b"." || name_bytes == b".." {
                continue;
            }
            if name_bytes.contains(&b'/') {
                continue;
            }
            let name = OsString::from_vec(name_bytes.to_vec());
            let Some(stat) = self.stat_observed_entry(&name)? else {
                // Two consecutive ENOENT results are the sole omission policy:
                // the name genuinely disappeared after getdents observed it.
                continue;
            };
            out.push(EntryStat {
                name,
                mtime_nanos: stat_mtime_nanos(&stat),
                size: stat_size(&stat),
            });
        }
        Ok(out)
    }

    /// Stat a name already observed by `getdents`.
    ///
    /// A first `ENOENT` is not enough to omit it: retrying closes the useful,
    /// deterministic part of the lookup race (including a synthetic transient
    /// miss). Only a confirmed second `ENOENT` means the
    /// observed directory entry disappeared. Any other error invalidates the
    /// complete census.
    fn stat_observed_entry(&self, name: &OsStr) -> io::Result<Option<rustix::fs::Stat>> {
        match self.statat_for_census(name) {
            Ok(stat) => Ok(Some(stat)),
            Err(rustix::io::Errno::NOENT) => match self.statat_for_census(name) {
                Ok(stat) => Ok(Some(stat)),
                Err(rustix::io::Errno::NOENT) => Ok(None),
                Err(error) => Err(contextual_io(
                    "stat observed directory entry after ENOENT retry",
                    &self.path,
                    Some(name),
                    error,
                )),
            },
            Err(error) => Err(contextual_io(
                "stat observed directory entry",
                &self.path,
                Some(name),
                error,
            )),
        }
    }

    fn statat_for_census(&self, name: &OsStr) -> Result<rustix::fs::Stat, rustix::io::Errno> {
        #[cfg(test)]
        if let Some(error) = census_fault::take_stat(name) {
            return Err(error);
        }
        rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)
    }
}

pub(crate) fn create_entry_private_at(fd: BorrowedFd<'_>, name: &OsStr) -> io::Result<File> {
    reject_path_separators(name)?;
    let file = rustix::fs::openat(
        fd,
        name,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(io::Error::from)?;
    Ok(File::from(file))
}

pub(crate) fn entry_identity_at(fd: BorrowedFd<'_>, name: &OsStr) -> io::Result<(u64, u64)> {
    reject_path_separators(name)?;
    let stat = rustix::fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    Ok(stat_dev_ino(&stat))
}

pub(crate) fn unlink_entry_if_identity_at(
    fd: BorrowedFd<'_>,
    name: &OsStr,
    identity: (u64, u64),
    flags: AtFlags,
) -> io::Result<bool> {
    if entry_identity_at(fd, name)? != identity {
        return Ok(false);
    }
    rustix::fs::unlinkat(fd, name, flags).map_err(io::Error::from)?;
    Ok(true)
}

/// The `(device, inode)` an open descriptor denotes, from `fstat(fd)`.
///
/// The other half of [`SecureRoot::entry_identity`]: a name lookup can be
/// redirected by a substitution, an open descriptor cannot. Both sides go
/// through the same `rustix` `Stat` so the two `st_dev` values are directly
/// comparable on every backend this crate builds against.
///
/// # Errors
/// Any `fstat` failure.
pub fn open_file_identity(file: &File) -> io::Result<(u64, u64)> {
    let stat = rustix::fs::fstat(file).map_err(io::Error::from)?;
    Ok(stat_dev_ino(&stat))
}

/// Directory-entry accounting as observed through the retained root fd.
///
/// Returned by [`SecureRoot::entry_names`]; the cache uses this triple for
/// eviction ordering and total-bytes accounting without ever exposing a
/// pathname to the surrounding code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryStat {
    /// Basename of the entry within the retained root. Guaranteed by
    /// [`SecureRoot::entry_names`] to contain no path separator.
    pub name: OsString,
    /// Signed mtime as nanoseconds relative to the Unix epoch, from `fstatat`
    /// through the retained root fd (so a substituted pathname cannot redirect
    /// it). Negative values represent valid pre-epoch timestamps.
    pub mtime_nanos: i128,
    /// File size in bytes, from the same `fstatat`.
    pub size: u64,
}

/// The fd-authoritative leaf verifier called by [`SecureRoot::open`].
///
/// Runs the four checks from [`SecureRoot::open`]'s doc against the
/// `fstat(fd)` view of the retained descriptor, using the pathname-view
/// identity captured by [`super::ensure_and_verify_leaf`]:
///
/// 1. `st_mode & S_IFMT == S_IFDIR` — file type.
/// 2. `st_uid == verified.our_uid` — owner.
/// 3. `st_mode & 0o022 == 0` — no group/world write.
/// 4. `(st_dev, st_ino) == (verified.dev, verified.ino)` — no substitution
///    between the pathname verification and the openat that produced the fd.
///
/// Every arm produces a distinct typed refusal.
fn verify_open_fd_against(
    fd: &OwnedFd,
    verified: &super::VerifiedLeaf,
) -> Result<(), SecureDirRefusal> {
    let stat = rustix::fs::fstat(fd).map_err(|_| SecureDirRefusal::CreateFailed)?;
    verify_private_stat(&stat, verified.our_uid)?;

    let (fd_dev, fd_ino) = stat_dev_ino(&stat);
    if (fd_dev, fd_ino) != (verified.dev, verified.ino) {
        return Err(SecureDirRefusal::RootIdentityChanged {
            original: (verified.dev, verified.ino),
            current: Some((fd_dev, fd_ino)),
        });
    }

    Ok(())
}

fn verify_private_stat(stat: &rustix::fs::Stat, owner_uid: u32) -> Result<(), SecureDirRefusal> {
    let mode_bits = stat_mode(stat);
    if mode_bits & 0o170_000 != 0o040_000 {
        return Err(SecureDirRefusal::NotADirectory);
    }
    let dir_uid = stat_uid(stat);
    if dir_uid != owner_uid {
        return Err(SecureDirRefusal::WrongOwner {
            dir_uid,
            our_uid: owner_uid,
        });
    }
    let mode = mode_bits & 0o7777;
    if mode & 0o022 != 0 {
        return Err(SecureDirRefusal::GroupOrWorldWritable { mode });
    }
    Ok(())
}

/// Preserve an OS error's kind while adding the operation and exact retained
/// root/entry that made a complete census impossible.
fn contextual_io(
    operation: &str,
    root: &Path,
    entry: Option<&OsStr>,
    error: rustix::io::Errno,
) -> io::Error {
    let source = io::Error::from(error);
    let subject = entry.map_or_else(
        || root.display().to_string(),
        |name| root.join(name).display().to_string(),
    );
    io::Error::new(
        source.kind(),
        format!(
            "{operation} at {subject} (secure root {}): {source}",
            root.display()
        ),
    )
}

// The *at syscalls accept paths; only a single basename confines them to the retained parent.
pub(crate) fn reject_path_separators(name: &OsStr) -> io::Result<()> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.contains(&b'/')
        || bytes.contains(&0)
        || bytes == b"."
        || bytes == b".."
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry name must not contain a path separator",
        ));
    }
    Ok(())
}

// The next few helpers convert rustix's `Stat` field types (which differ across
// linux/glibc/musl and the linux_raw_sys backend) to the fixed-width numbers
// this module uses. Modification time widens into a signed `i128`; the other
// fields use the documented nonnegative filesystem values and fixed-width
// representations used by this capability.
fn stat_dev_ino(stat: &rustix::fs::Stat) -> (u64, u64) {
    (stat.st_dev, stat.st_ino)
}

/// Convert the kernel's signed `(seconds, nanoseconds)` pair without losing
/// pre-epoch ordering. Combining both fields in the wider signed domain also
/// defensively normalizes a non-canonical `st_mtime_nsec`: values outside
/// `0..1_000_000_000` carry into (or borrow from) the seconds component rather
/// than wrapping into a far-future unsigned timestamp.
fn stat_mtime_nanos(stat: &rustix::fs::Stat) -> i128 {
    const NANOS_PER_SECOND: i128 = 1_000_000_000;

    let seconds = i128::from(stat.st_mtime);
    let nanoseconds = i128::from(stat.st_mtime_nsec);
    let normalized_seconds = seconds + nanoseconds.div_euclid(NANOS_PER_SECOND);
    let normalized_nanoseconds = nanoseconds.rem_euclid(NANOS_PER_SECOND);
    normalized_seconds * NANOS_PER_SECOND + normalized_nanoseconds
}

fn stat_size(stat: &rustix::fs::Stat) -> u64 {
    u64::try_from(stat.st_size).unwrap_or(0)
}

fn stat_uid(stat: &rustix::fs::Stat) -> u32 {
    stat.st_uid
}

fn stat_mode(stat: &rustix::fs::Stat) -> u32 {
    stat.st_mode
}

/// Thread-local, unit-test-only outcomes for the two fallible census stages.
/// Product builds contain neither these switches nor their branches.
#[cfg(test)]
mod census_fault {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::{OsStr, OsString};

    thread_local! {
        static ITERATOR: RefCell<VecDeque<rustix::io::Errno>> = const { RefCell::new(VecDeque::new()) };
        static STAT: RefCell<VecDeque<(OsString, rustix::io::Errno)>> = const { RefCell::new(VecDeque::new()) };
    }

    pub(super) fn take_iterator() -> Option<rustix::io::Errno> {
        ITERATOR.with(|slot| slot.borrow_mut().pop_front())
    }

    pub(super) fn take_stat(name: &OsStr) -> Option<rustix::io::Errno> {
        STAT.with(|slot| {
            let mut outcomes = slot.borrow_mut();
            let index = outcomes.iter().position(|(armed, _)| armed == name)?;
            outcomes.remove(index).map(|(_, error)| error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let mut path = std::env::current_exe().expect("test executable path");
            path.pop();
            path.push(format!(
                "secure-root-census-{}-{}-{tag}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn anonymous_file_retains_root_without_creating_an_entry() {
        use std::io::{Read, Seek, SeekFrom, Write};
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let root = TestRoot::new("anonymous");
        let moved = TestRoot::new("anonymous-moved");
        let secure = SecureRoot::open(&root.0).unwrap();
        secure.create_entry_private(OsStr::new("control")).unwrap();
        assert_eq!(secure.entry_names().unwrap().len(), 1);
        std::fs::rename(&root.0, &moved.0).unwrap();
        std::fs::create_dir(&root.0).unwrap();
        let mut file = secure.create_anonymous_file().unwrap();
        let metadata = file.metadata().unwrap();
        assert_eq!(metadata.nlink(), 0, "anonymous files must have no name");
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert!(
            rustix::io::fcntl_getfd(&file)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
        let target = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
        assert!(
            target.starts_with(&moved.0),
            "creation must use the retained directory"
        );
        file.write_all(b"private buffer").unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"private buffer");
        assert_eq!(secure.entry_names().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
        std::fs::set_permissions(&moved.0, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            secure.create_anonymous_file().is_err(),
            "retained policy drift must refuse creation"
        );
    }

    #[test]
    fn retained_fd_census_includes_regular_and_non_regular_entries() {
        let root = TestRoot::new("positive");
        let secure = SecureRoot::open(&root.0).expect("open secure root");
        std::fs::write(root.0.join("regular.enzhc"), b"regular bytes").expect("seed file");
        std::fs::create_dir(root.0.join("directory.enzhc")).expect("seed directory");
        symlink("regular.enzhc", root.0.join("symlink.enzhc")).expect("seed symlink");

        let entries = secure.entry_names().expect("complete retained-fd census");
        let names: Vec<&OsStr> = entries.iter().map(|entry| entry.name.as_os_str()).collect();
        assert!(names.contains(&OsStr::new("regular.enzhc")));
        assert!(names.contains(&OsStr::new("directory.enzhc")));
        assert!(names.contains(&OsStr::new("symlink.enzhc")));
        assert!(
            entries
                .iter()
                .find(|entry| entry.name == OsStr::new("regular.enzhc"))
                .expect("regular entry metadata")
                .size
                > 0,
            "the regular-file positive control must have real, nonzero metadata"
        );
    }

    #[test]
    fn retained_root_mode_drift_blocks_every_entry_operation() {
        fn assert_denied<T>(result: io::Result<T>, operation: &str) {
            match result {
                Err(error) => assert_eq!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied,
                    "{operation}: {error}"
                ),
                Ok(_) => panic!("{operation} accepted a group/world-writable retained root"),
            }
        }

        let root = TestRoot::new("policy-drift");
        let secure = SecureRoot::open(&root.0).expect("open secure root");
        for name in [
            "read",
            "identity",
            "unlink-identity",
            "rename",
            "unlink",
            "touch",
        ] {
            std::fs::write(root.0.join(name), name.as_bytes()).expect("seed protected entry");
        }
        std::fs::create_dir(root.0.join("child")).expect("seed child directory");
        let unlink_identity = secure
            .entry_identity(OsStr::new("unlink-identity"))
            .expect("capture identity before permission drift");

        std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o777))
            .expect("make retained root unsafe");
        assert_eq!(
            secure.revalidate(),
            Err(SecureDirRefusal::GroupOrWorldWritable { mode: 0o777 })
        );
        assert_denied(secure.open_entry_read(OsStr::new("read")), "read");
        assert_denied(secure.create_entry_private(OsStr::new("created")), "create");
        assert_denied(secure.entry_identity(OsStr::new("identity")), "stat");
        assert_denied(
            secure.unlink_entry_if_identity(OsStr::new("unlink-identity"), unlink_identity),
            "identity unlink",
        );
        assert_denied(
            secure.rename_entry(OsStr::new("rename"), OsStr::new("renamed")),
            "rename",
        );
        assert_denied(secure.unlink_entry(OsStr::new("unlink")), "unlink");
        assert_denied(secure.touch_entry_mtime(OsStr::new("touch")), "touch");
        assert_denied(secure.entry_names(), "census");
        assert_denied(secure.entry_size(OsStr::new("read")), "size");
        assert_denied(secure.open_subroot(OsStr::new("child")), "open subroot");
        assert_denied(
            secure.ensure_subroot(OsStr::new("created-child")),
            "ensure subroot",
        );

        assert_eq!(std::fs::read(root.0.join("unlink")).unwrap(), b"unlink");
        assert_eq!(std::fs::read(root.0.join("rename")).unwrap(), b"rename");
        assert!(!root.0.join("renamed").exists());
        assert!(!root.0.join("created").exists());
        assert!(!root.0.join("created-child").exists());

        std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700))
            .expect("restore private root mode");
        assert_eq!(secure.revalidate(), Ok(()));
        secure
            .open_entry_read(OsStr::new("read"))
            .expect("restored root remains usable");
    }

    /// A real pre-epoch file must remain older than epoch zero and a modern
    /// positive timestamp at the retained-fd metadata boundary.
    ///
    /// SABOTAGE: cast `st_mtime` to `u64` before widening it in
    /// `stat_mtime_nanos`; this test fails because the pre-epoch entry becomes
    /// an enormous far-future value and sorts last.
    #[test]
    fn retained_fd_census_preserves_signed_pre_epoch_mtime_ordering() {
        let root = TestRoot::new("signed-mtime");
        let secure = SecureRoot::open(&root.0).expect("open secure root");
        let fixtures = [
            (
                "pre-epoch.enzhc",
                UNIX_EPOCH
                    .checked_sub(Duration::from_secs(1))
                    .expect("representable pre-epoch timestamp"),
            ),
            ("epoch.enzhc", UNIX_EPOCH),
            (
                "modern.enzhc",
                UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            ),
        ];
        for (name, modified) in fixtures {
            let path = root.0.join(name);
            std::fs::write(&path, name.as_bytes()).expect("seed timestamp fixture");
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open timestamp fixture")
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .expect("set requested modification time");
        }

        let mut entries = secure.entry_names().expect("complete retained-fd census");
        entries.sort_by_key(|entry| entry.mtime_nanos);
        let ordered: Vec<_> = entries
            .iter()
            .map(|entry| (entry.name.to_string_lossy().into_owned(), entry.mtime_nanos))
            .collect();
        assert_eq!(
            ordered,
            vec![
                ("pre-epoch.enzhc".to_owned(), -1_000_000_000),
                ("epoch.enzhc".to_owned(), 0),
                ("modern.enzhc".to_owned(), 1_700_000_000_000_000_000,),
            ],
            "the secure metadata boundary must report real signed chronology"
        );
    }
}
