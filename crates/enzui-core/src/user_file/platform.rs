use super::{
    Custody, ExpectedDestination, PublishFailure, RetainedDestination, RetainedMetadata, SaveError,
    SaveOperation, SavePolicy, StagedNaming,
};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags};
use rustix::process::{Gid, Uid};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

const MAX_SYMLINKS: usize = 40;
const MAX_NAME_ATTEMPTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            device: stat.st_dev,
            inode: stat.st_ino,
        }
    }

    fn of(file: &impl std::os::fd::AsFd) -> io::Result<Self> {
        rustix::fs::fstat(file)
            .map(|stat| Self::from_stat(&stat))
            .map_err(io::Error::from)
    }
}

struct Parent {
    fd: File,
}

impl Parent {
    fn open_from(base: Option<&Self>, path: &Path) -> io::Result<Self> {
        #[cfg(test)]
        if let Some(error) = fault::take_parent_open() {
            return Err(parent_open_error(error));
        }
        let start = if path.is_absolute() {
            rustix::fs::openat(
                rustix::fs::CWD,
                "/",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
        } else if let Some(base) = base {
            rustix::fs::openat(
                &base.fd,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
        } else {
            rustix::fs::openat(
                rustix::fs::CWD,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
        }
        .map_err(|error| parent_open_error(io::Error::from(error)))?;
        let mut current = File::from(start);
        for component in path.components() {
            use std::path::Component;
            let name = match component {
                Component::RootDir | Component::CurDir => continue,
                Component::ParentDir => OsStr::new(".."),
                Component::Normal(name) => name,
                Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "unsupported path prefix",
                    ));
                }
            };
            let next = rustix::fs::openat(
                &current,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| parent_open_error(io::Error::from(error)))?;
            current = File::from(next);
        }
        Ok(Self { fd: current })
    }

    fn duplicate(&self) -> io::Result<Self> {
        Self::open_from(Some(self), Path::new("."))
    }

    fn stat(&self, name: &OsStr) -> io::Result<rustix::fs::Stat> {
        rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)
    }

    fn open_path(&self, name: &OsStr) -> io::Result<(OwnedFd, rustix::fs::Stat)> {
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let stat = rustix::fs::fstat(&fd).map_err(io::Error::from)?;
        Ok((fd, stat))
    }

    fn create_unnamed(&self) -> io::Result<File> {
        #[cfg(test)]
        if let Some(error) = fault::take_tmpfile() {
            return Err(error);
        }
        let fd = rustix::fs::openat(
            &self.fd,
            ".",
            OFlags::TMPFILE | OFlags::WRONLY | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?;
        Ok(File::from(fd))
    }

    fn link_file(&self, file: &File, name: &OsStr) -> io::Result<()> {
        #[cfg(test)]
        let empty_path = if let Some(error) = fault::take_empty_link() {
            Err(error)
        } else {
            rustix::fs::linkat(file, "", &self.fd, name, AtFlags::EMPTY_PATH)
                .map_err(io::Error::from)
        };
        #[cfg(not(test))]
        let empty_path = rustix::fs::linkat(file, "", &self.fd, name, AtFlags::EMPTY_PATH)
            .map_err(io::Error::from);
        match empty_path {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                let source = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
                rustix::fs::linkat(
                    rustix::fs::CWD,
                    &source,
                    &self.fd,
                    name,
                    AtFlags::SYMLINK_FOLLOW,
                )
                .map_err(io::Error::from)
            }
            Err(error) => Err(error),
        }
    }

    fn exchange(&self, left: &OsStr, right: &OsStr) -> io::Result<()> {
        rustix::fs::renameat_with(&self.fd, left, &self.fd, right, RenameFlags::EXCHANGE)
            .map_err(io::Error::from)
    }

    fn unlink(&self, name: &OsStr) -> io::Result<()> {
        rustix::fs::unlinkat(&self.fd, name, AtFlags::empty()).map_err(io::Error::from)
    }

    fn sync(&self) -> io::Result<()> {
        #[cfg(test)]
        if let Some(error) = fault::take_parent_sync() {
            return Err(error);
        }
        self.fd.sync_all()
    }

    fn lock(&self) -> io::Result<ParentLock<'_>> {
        self.fd.lock()?;
        Ok(ParentLock(&self.fd))
    }
}

struct ParentLock<'a>(&'a File);

impl Drop for ParentLock<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

struct LinkGuard {
    parent: Parent,
    name: OsString,
    identity: Identity,
    held: OwnedFd,
}

impl LinkGuard {
    fn verify(&self) -> io::Result<()> {
        let held = Identity::of(&self.held)?;
        let named = self.parent.stat(&self.name)?;
        if FileType::from_raw_mode(named.st_mode) != FileType::Symlink
            || Identity::from_stat(&named) != self.identity
            || held != self.identity
        {
            return Err(identity_changed("selected symlink changed during save"));
        }
        Ok(())
    }
}

struct Existing {
    identity: Identity,
    held: OwnedFd,
    mode: u32,
    uid: u32,
    gid: u32,
}

enum InitialDestination {
    Existing(Existing),
    Missing,
}

struct Destination {
    parent: Parent,
    name: OsString,
    initial: InitialDestination,
    links: Vec<LinkGuard>,
}

impl Destination {
    fn resolve(path: &Path, policy: SavePolicy) -> io::Result<Self> {
        if path.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "user-file path has no file name",
            ));
        }
        let mut current = path.to_path_buf();
        let mut current_base = None;
        let mut links = Vec::new();
        for depth in 0..=MAX_SYMLINKS {
            let parent_path = current.parent().unwrap_or_else(|| Path::new("."));
            let name = current
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid path"))?
                .to_os_string();
            let parent = Parent::open_from(current_base.as_ref(), parent_path)?;
            let stat = match parent.stat(&name) {
                Ok(stat) => stat,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if links.is_empty()
                        && matches!(policy, SavePolicy::ReplaceOrCreate | SavePolicy::CreateNew)
                    {
                        return Ok(Self {
                            parent,
                            name,
                            initial: InitialDestination::Missing,
                            links,
                        });
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            if links.is_empty() && policy == SavePolicy::CreateNew {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "create-new destination already exists",
                ));
            }
            let kind = FileType::from_raw_mode(stat.st_mode);
            if kind == FileType::Symlink {
                if depth == MAX_SYMLINKS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "symbolic-link resolution limit exceeded",
                    ));
                }
                let (held, held_stat) = parent.open_path(&name)?;
                let identity = Identity::from_stat(&held_stat);
                if identity != Identity::from_stat(&stat) {
                    return Err(identity_changed("selected symlink changed while resolving"));
                }
                #[cfg(test)]
                fault::run_before_readlink();
                let target =
                    rustix::fs::readlinkat(&held, "", Vec::new()).map_err(io::Error::from)?;
                let target = PathBuf::from(OsString::from_vec(target.into_bytes()));
                current_base = if target.is_absolute() {
                    None
                } else {
                    Some(parent.duplicate()?)
                };
                current = target;
                links.push(LinkGuard {
                    parent,
                    name,
                    identity,
                    held,
                });
                continue;
            }
            if kind != FileType::RegularFile {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "user-file destination is not a regular file",
                ));
            }
            if stat.st_nlink != 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "user-file destination has multiple hard links",
                ));
            }
            let (held, held_stat) = parent.open_path(&name)?;
            if Identity::from_stat(&held_stat) != Identity::from_stat(&stat) {
                return Err(identity_changed("destination changed while opening"));
            }
            return Ok(Self {
                parent,
                name,
                initial: InitialDestination::Existing(Existing {
                    identity: Identity::from_stat(&held_stat),
                    held,
                    mode: held_stat.st_mode & 0o7777,
                    uid: held_stat.st_uid,
                    gid: held_stat.st_gid,
                }),
                links,
            });
        }
        unreachable!()
    }

    fn verify_initial(&self) -> io::Result<()> {
        for link in &self.links {
            link.verify()?;
        }
        match &self.initial {
            InitialDestination::Existing(existing) => {
                if Identity::of(&existing.held)? != existing.identity {
                    return Err(identity_changed("held destination identity changed"));
                }
                let stat = self.parent.stat(&self.name)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
                    || Identity::from_stat(&stat) != existing.identity
                    || stat.st_nlink != 1
                    || stat.st_mode & 0o7777 != existing.mode
                    || stat.st_uid != existing.uid
                    || stat.st_gid != existing.gid
                {
                    return Err(identity_changed("destination changed during save"));
                }
            }
            InitialDestination::Missing => match self.parent.stat(&self.name) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "destination appeared during save",
                    ));
                }
                Err(error) => return Err(error),
            },
        }
        Ok(())
    }

    fn verify_links(&self) -> io::Result<()> {
        for link in &self.links {
            link.verify()?;
        }
        Ok(())
    }

    fn metadata(&self, new_file_mode: u32) -> (u32, u32, u32) {
        match &self.initial {
            InitialDestination::Existing(existing) => (existing.mode, existing.uid, existing.gid),
            InitialDestination::Missing => (
                new_file_mode,
                rustix::process::geteuid().as_raw(),
                rustix::process::getegid().as_raw(),
            ),
        }
    }

    fn existing(&self) -> Option<&Existing> {
        match &self.initial {
            InitialDestination::Existing(existing) => Some(existing),
            InitialDestination::Missing => None,
        }
    }

    fn retained_metadata(&self) -> Option<RetainedMetadata> {
        self.existing().map(|existing| RetainedMetadata {
            mode: existing.mode,
            uid: existing.uid,
            gid: existing.gid,
        })
    }
}

struct Candidate<'a> {
    parent: &'a Parent,
    name: OsString,
    expected: Identity,
    active: bool,
}

impl Candidate<'_> {
    fn verify(&self) -> io::Result<()> {
        let stat = self.parent.stat(&self.name)?;
        if Identity::from_stat(&stat) != self.expected {
            return Err(identity_changed("publication name changed during save"));
        }
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        #[cfg(test)]
        fault::run_before_cleanup();
        self.verify()?;
        self.parent.unlink(&self.name)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.cleanup();
        }
    }
}

struct StagedWriter<'a>(&'a mut File);

impl Write for StagedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(test)]
        if let Some(error) = fault::take_write() {
            return Err(error);
        }
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

// A symbolic final component is refused rather than resolved: only an exact
// regular inode can authorize a later conditional replacement.
pub(super) fn open_retained(path: &Path, policy: SavePolicy) -> io::Result<RetainedDestination> {
    let destination = Destination::resolve(path, policy)?;
    if !destination.links.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "user-file destination is a symbolic link",
        ));
    }
    let metadata = destination.retained_metadata();
    let Destination {
        parent, initial, ..
    } = destination;
    let existing = match (initial, metadata) {
        (InitialDestination::Existing(existing), Some(metadata)) => {
            Some((File::from(existing.held), metadata))
        }
        _ => None,
    };
    Ok(RetainedDestination {
        existing,
        parent: parent.fd,
    })
}

fn before(operation: SaveOperation, source: impl Into<io::Error>) -> PublishFailure {
    PublishFailure {
        error: SaveError::before(operation, source),
        custody: Custody::Unchanged,
        staged: StagedNaming::NotPublished,
        unknown_staged: None,
    }
}

fn after(
    operation: SaveOperation,
    source: impl Into<io::Error>,
    custody: Custody,
    staged: StagedNaming,
) -> PublishFailure {
    PublishFailure {
        error: SaveError::after(operation, source),
        custody,
        staged,
        unknown_staged: None,
    }
}

// An `Unknown`-custody failure retains its staged descriptor so the caller
// can read the bytes it published even after their last name was removed.
fn retain_unknown_stage(mut failure: PublishFailure, stage: File) -> PublishFailure {
    failure.unknown_staged = Some(stage);
    failure
}

// Linux refuses to link an inode whose last name was removed, so a name can
// be recovered only while the staged inode still has one.
fn staged_naming(
    stage: &File,
    destination: &Destination,
    candidate: Option<&Candidate<'_>>,
) -> StagedNaming {
    let held = Identity::of(stage).ok();
    if let (Some(held), Ok(named)) = (held, destination.parent.stat(&destination.name))
        && held == Identity::from_stat(&named)
    {
        return StagedNaming::AtDestination;
    }
    if let (Some(held), Some(candidate)) = (held, candidate)
        && let Ok(named) = candidate.parent.stat(&candidate.name)
        && held == Identity::from_stat(&named)
    {
        return StagedNaming::Named(candidate.name.clone());
    }
    match link_random_candidate(&destination.parent, stage) {
        Ok(mut recovery) => {
            recovery.active = false;
            StagedNaming::Named(recovery.name.clone())
        }
        Err(_) => StagedNaming::Unnamed,
    }
}

#[derive(Clone, Copy)]
struct ExpectedIdentity {
    file: Option<Identity>,
    metadata: Option<RetainedMetadata>,
    parent: Identity,
}

pub(super) fn save_with(
    path: &Path,
    policy: SavePolicy,
    expected: Option<ExpectedDestination<'_>>,
    new_file_mode: u32,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> Result<File, PublishFailure> {
    // `StagedWriter` keeps the per-`write(2)` fault-injection seam the
    // buffered-writer callers already exercise. `save_with_file` fsyncs the
    // stage itself, so the wrapper's flush is only for user-buffered writers.
    save_with_file(path, policy, expected, new_file_mode, |stage: &mut File| {
        let mut output = StagedWriter(stage);
        write(&mut output)?;
        output.flush()
    })
}

pub(super) fn save_with_file(
    path: &Path,
    policy: SavePolicy,
    expected: Option<ExpectedDestination<'_>>,
    new_file_mode: u32,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<File, PublishFailure> {
    let destination = Destination::resolve(path, policy)
        .map_err(|error| before(classify_resolution(&error), error))?;
    let expected = match expected {
        Some(expected) => Some(ExpectedIdentity {
            file: expected
                .file
                .map(Identity::of)
                .transpose()
                .map_err(|error| before(SaveOperation::InspectDestination, error))?,
            metadata: expected.metadata,
            parent: Identity::of(expected.parent)
                .map_err(|error| before(SaveOperation::OpenParent, error))?,
        }),
        None => None,
    };
    if let Some(expected) = expected {
        let current_parent = Identity::of(&destination.parent.fd)
            .map_err(|error| before(SaveOperation::OpenParent, error))?;
        if !destination.links.is_empty()
            || current_parent != expected.parent
            || destination.existing().map(|existing| existing.identity) != expected.file
        {
            return Err(before(
                SaveOperation::InspectDestination,
                identity_changed("destination no longer matches its reservation"),
            ));
        }
        if expected.metadata.is_some() && destination.retained_metadata() != expected.metadata {
            return Err(before(
                SaveOperation::InspectDestination,
                identity_changed("destination mode or ownership changed after retention"),
            ));
        }
    }
    if policy == SavePolicy::ReplaceExisting && destination.existing().is_none() {
        return Err(before(
            SaveOperation::InspectDestination,
            io::Error::new(
                io::ErrorKind::NotFound,
                "replace destination does not exist",
            ),
        ));
    }
    let mut stage = destination
        .parent
        .create_unnamed()
        .map_err(|error| before(SaveOperation::CreateStage, error))?;
    // `File` has no user-space buffer, so the direct `&mut File` writer
    // owns its own flush semantics; the dyn-`Write` wrapper flushes its
    // `StagedWriter` before returning here, and `sync_stage` follows below.
    write(&mut stage).map_err(|error| before(SaveOperation::Write, error))?;
    let (mode, uid, gid) = destination.metadata(new_file_mode);
    apply_metadata(&stage, mode, uid, gid)
        .map_err(|error| before(SaveOperation::PreserveMetadata, error))?;
    sync_stage(&stage).map_err(|error| before(SaveOperation::SyncFile, error))?;
    #[cfg(test)]
    fault::run_after_sync();

    let _lock = destination
        .parent
        .lock()
        .map_err(|error| before(SaveOperation::Revalidate, error))?;
    destination
        .verify_initial()
        .map_err(|error| before(SaveOperation::Revalidate, error))?;
    let expected_parent = expected.map(|expected| expected.parent);
    let verify_parent = || match expected_parent {
        Some(expected_parent) => verify_path_parent(path, expected_parent),
        None => Ok(()),
    };
    verify_parent().map_err(|error| before(SaveOperation::Revalidate, error))?;

    if destination.existing().is_none() {
        #[cfg(test)]
        fault::run_before_publish();
        #[cfg(test)]
        if let Some(error) = fault::take_rename() {
            return Err(before(SaveOperation::Publish, error));
        }
        destination
            .parent
            .link_file(&stage, &destination.name)
            .map_err(|error| before(SaveOperation::Publish, error))?;
        if let Err(error) = verify_published(&stage, &destination).and_then(|()| verify_parent()) {
            let staged = staged_naming(&stage, &destination, None);
            return Err(retain_unknown_stage(
                after(SaveOperation::Revalidate, error, Custody::Unknown, staged),
                stage,
            ));
        }
        if let Err(error) = destination.parent.sync() {
            return Err(after(
                SaveOperation::SyncParent,
                error,
                Custody::Staged(stage),
                StagedNaming::AtDestination,
            ));
        }
        #[cfg(test)]
        fault::run_after_parent_sync();
        if let Err(error) = verify_published(&stage, &destination).and_then(|()| verify_parent()) {
            let staged = staged_naming(&stage, &destination, None);
            return Err(retain_unknown_stage(
                after(SaveOperation::Revalidate, error, Custody::Unknown, staged),
                stage,
            ));
        }
        #[cfg(test)]
        fault::run_before_return();
        return Ok(stage);
    }

    let mut candidate = link_random_candidate(&destination.parent, &stage)
        .map_err(|error| before(SaveOperation::Publish, error))?;
    candidate
        .verify()
        .map_err(|error| before(SaveOperation::Revalidate, error))?;
    destination
        .verify_initial()
        .map_err(|error| before(SaveOperation::Revalidate, error))?;
    #[cfg(test)]
    fault::run_before_exchange();
    candidate
        .verify()
        .map_err(|error| before(SaveOperation::Revalidate, error))?;
    destination
        .verify_initial()
        .map_err(|error| before(SaveOperation::Revalidate, error))?;
    #[cfg(test)]
    fault::run_after_exchange_checks();
    #[cfg(test)]
    if let Some(error) = fault::take_rename() {
        return Err(before(SaveOperation::Publish, error));
    }
    destination
        .parent
        .exchange(&candidate.name, &destination.name)
        .map_err(|error| before(SaveOperation::Publish, error))?;
    #[cfg(test)]
    fault::run_after_exchange();

    let old = destination
        .existing()
        .expect("replace path captured an inode");
    candidate.expected = old.identity;
    if let Err(error) = verify_published(&stage, &destination)
        .and_then(|()| candidate.verify())
        .and_then(|()| verify_parent())
    {
        let rollback = rollback_exchange(&destination, &candidate);
        // The displaced inodes stay linked at the candidate name rather than
        // being unlinked by an identity the failed validation disproved.
        candidate.active = false;
        let staged = staged_naming(&stage, &destination, Some(&candidate));
        let failure = match rollback {
            Ok(()) => match destination.verify_initial() {
                Ok(()) => after(SaveOperation::Revalidate, error, Custody::Unchanged, staged),
                Err(_) => retain_unknown_stage(
                    after(SaveOperation::Revalidate, error, Custody::Unknown, staged),
                    stage,
                ),
            },
            Err(rollback) => retain_unknown_stage(
                after(
                    SaveOperation::Revalidate,
                    io::Error::new(
                        rollback.kind(),
                        format!(
                            "publication identity failed and rollback failed: {error}; {rollback}"
                        ),
                    ),
                    Custody::Unknown,
                    staged,
                ),
                stage,
            ),
        };
        return Err(failure);
    }
    if let Err(error) = candidate.cleanup() {
        return Err(after(
            SaveOperation::Revalidate,
            error,
            Custody::Staged(stage),
            StagedNaming::AtDestination,
        ));
    }
    if let Err(error) = destination.parent.sync() {
        return Err(after(
            SaveOperation::SyncParent,
            error,
            Custody::Staged(stage),
            StagedNaming::AtDestination,
        ));
    }
    #[cfg(test)]
    fault::run_after_parent_sync();
    if let Err(error) = verify_published(&stage, &destination).and_then(|()| verify_parent()) {
        let staged = staged_naming(&stage, &destination, None);
        return Err(retain_unknown_stage(
            after(SaveOperation::Revalidate, error, Custody::Unknown, staged),
            stage,
        ));
    }
    #[cfg(test)]
    fault::run_before_return();
    Ok(stage)
}

fn rollback_exchange(destination: &Destination, candidate: &Candidate<'_>) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = fault::take_rollback() {
        return Err(error);
    }
    destination
        .parent
        .exchange(&candidate.name, &destination.name)
}

fn verify_path_parent(path: &Path, expected: Identity) -> io::Result<()> {
    let parent = Parent::open_from(None, path.parent().unwrap_or_else(|| Path::new(".")))?;
    if Identity::of(&parent.fd)? != expected {
        return Err(identity_changed(
            "destination parent no longer matches its reservation",
        ));
    }
    Ok(())
}

fn link_random_candidate<'a>(parent: &'a Parent, file: &File) -> io::Result<Candidate<'a>> {
    let identity = Identity::of(file)?;
    for _ in 0..MAX_NAME_ATTEMPTS {
        let name = random_candidate_name()?;
        #[cfg(test)]
        fault::run_before_create(&name);
        match parent.link_file(file, &name) {
            Ok(()) => {
                return Ok(Candidate {
                    parent,
                    name,
                    expected: identity,
                    active: true,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "random publication-name retry budget exhausted",
    ))
}

fn random_candidate_name() -> io::Result<OsString> {
    #[cfg(test)]
    if let Some(name) = fault::take_random_name() {
        return Ok(name);
    }
    let bytes = enzui_security::random::random_array::<16>()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut name = String::from(".enzui-save-");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(name, "{byte:02x}");
    }
    Ok(OsString::from(name))
}

fn classify_resolution(error: &io::Error) -> SaveOperation {
    if error.to_string().contains("parent directory") {
        SaveOperation::OpenParent
    } else {
        SaveOperation::InspectDestination
    }
}

fn apply_metadata(file: &File, mode: u32, uid: u32, gid: u32) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = fault::take_metadata() {
        return Err(error);
    }
    #[cfg(test)]
    {
        fault::record_metadata("fchown");
        fault::record_fchown_args(uid, gid);
    }
    #[cfg(test)]
    if let Some(result) = fault::take_fchown() {
        result?;
    } else {
        rustix::fs::fchown(file, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
            .map_err(io::Error::from)?;
    }
    #[cfg(not(test))]
    rustix::fs::fchown(file, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
        .map_err(io::Error::from)?;
    #[cfg(test)]
    fault::record_metadata("fchmod");
    rustix::fs::fchmod(file, Mode::from_raw_mode(mode)).map_err(io::Error::from)
}

fn sync_stage(file: &File) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = fault::take_file_sync() {
        return Err(error);
    }
    file.sync_all()
}

fn verify_published(file: &File, destination: &Destination) -> io::Result<()> {
    destination.verify_links()?;
    let held = Identity::of(file)?;
    let named = destination.parent.stat(&destination.name)?;
    if held != Identity::from_stat(&named) {
        return Err(identity_changed("published destination identity changed"));
    }
    Ok(())
}

fn parent_open_error(error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("open parent directory: {error}"))
}

fn identity_changed(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
pub(crate) mod fault {
    use std::cell::{Cell, RefCell};
    use std::ffi::{OsStr, OsString};
    use std::io;

    type NameAction = Box<dyn FnOnce(&OsStr)>;

    thread_local! {
        static PARENT_OPEN: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static METADATA: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static WRITE: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static FILE_SYNC: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static RENAME: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static ROLLBACK: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static PARENT_SYNC: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static TMPFILE: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static EMPTY_LINK: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static FCHOWN: RefCell<Option<Result<(), io::ErrorKind>>> = const { RefCell::new(None) };
        static RANDOM_NAMES: RefCell<Vec<OsString>> = const { RefCell::new(Vec::new()) };
        static METADATA_CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
        static FCHOWN_ARGS: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
        static BEFORE_CREATE: RefCell<Option<NameAction>> = const { RefCell::new(None) };
        static AFTER_SYNC: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static BEFORE_READLINK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static BEFORE_PUBLISH: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static BEFORE_EXCHANGE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static AFTER_EXCHANGE_CHECKS: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static AFTER_EXCHANGE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static BEFORE_RETURN: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static BEFORE_CLEANUP: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static AFTER_PARENT_SYNC: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    fn make(kind: io::ErrorKind, label: &str) -> io::Error {
        io::Error::new(kind, format!("injected {label} failure"))
    }
    pub(super) fn take_parent_open() -> Option<io::Error> {
        PARENT_OPEN.with(|s| s.take().map(|k| make(k, "parent-open")))
    }
    pub(super) fn take_metadata() -> Option<io::Error> {
        METADATA.with(|s| s.take().map(|k| make(k, "metadata")))
    }
    pub(super) fn take_write() -> Option<io::Error> {
        WRITE.with(|s| s.take().map(|k| make(k, "write")))
    }
    pub(super) fn take_file_sync() -> Option<io::Error> {
        FILE_SYNC.with(|s| s.take().map(|k| make(k, "file-sync")))
    }
    pub(super) fn take_rename() -> Option<io::Error> {
        RENAME.with(|s| s.take().map(|k| make(k, "publish")))
    }
    pub(super) fn take_rollback() -> Option<io::Error> {
        ROLLBACK.with(|s| s.take().map(|k| make(k, "rollback")))
    }
    pub(super) fn take_parent_sync() -> Option<io::Error> {
        PARENT_SYNC.with(|s| s.take().map(|k| make(k, "parent-sync")))
    }
    pub(super) fn take_tmpfile() -> Option<io::Error> {
        TMPFILE.with(|s| s.take().map(|k| make(k, "O_TMPFILE")))
    }
    pub(super) fn take_empty_link() -> Option<io::Error> {
        EMPTY_LINK.with(|s| s.take().map(|k| make(k, "AT_EMPTY_PATH")))
    }
    pub(super) fn take_fchown() -> Option<io::Result<()>> {
        FCHOWN.with(|s| {
            s.borrow_mut()
                .take()
                .map(|r| r.map_err(|k| make(k, "fchown")))
        })
    }
    pub(super) fn take_random_name() -> Option<OsString> {
        RANDOM_NAMES.with(|s| s.borrow_mut().pop())
    }
    pub(super) fn record_metadata(call: &'static str) {
        METADATA_CALLS.with(|s| s.borrow_mut().push(call));
    }
    pub(super) fn record_fchown_args(uid: u32, gid: u32) {
        FCHOWN_ARGS.with(|s| s.borrow_mut().push((uid, gid)));
    }
    pub(super) fn run_before_create(name: &OsStr) {
        BEFORE_CREATE.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a(name);
            }
        });
    }
    pub(super) fn run_after_sync() {
        AFTER_SYNC.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_before_readlink() {
        BEFORE_READLINK.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_before_publish() {
        BEFORE_PUBLISH.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_before_exchange() {
        BEFORE_EXCHANGE.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_before_return() {
        BEFORE_RETURN.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_after_exchange_checks() {
        AFTER_EXCHANGE_CHECKS.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_after_exchange() {
        AFTER_EXCHANGE.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_before_cleanup() {
        BEFORE_CLEANUP.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
    pub(super) fn run_after_parent_sync() {
        AFTER_PARENT_SYNC.with(|s| {
            if let Some(a) = s.borrow_mut().take() {
                a();
            }
        });
    }
}
