//! Crash-recoverable relocation of a closed SQLite database without overwriting destination names.
//!
//! The source database must be checkpointed and all connections closed. The protocol retains every source member and the destination directory by descriptor, publishes each retained source inode with SQLite sidecars before the main file, syncs the destination, and verifies that its original pathname still reaches it before identity-bound source cleanup. During recovery, a remaining source name is removed only when its destination counterpart has the same inode. A failed post-link validation removes the destination only while it still names the retained source inode.
//!
//! Only `-wal`, `-shm`, `-journal`, and valid `-mj<hex>` sidecars are included, under fixed scan budgets. Symlinks, non-regular source files, changed source, destination-entry, or destination-directory identities, same-directory renames, conflicting destination database-family entries, cross-filesystem moves, and platforms without the required descriptor-relative guarantees are refused. There is no copy fallback.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use enzui_security::secure_dir::{RetainedDir, open_file_identity};
use enzui_security::tempstage::remove_if_path_names_file;

// Raw `EXDEV` code used by the migration's Unix targets to classify cross-filesystem hard-link failures.
const EXDEV: i32 = 18;

/// Which filesystem operation reported an [`MigrationError::Io`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationOp {
    /// Reading a path's own metadata (`lstat`).
    Stat,
    /// Listing a directory to derive the database file set.
    ReadDir,
    /// `link(2)` — creating a destination name.
    Link,
    /// `unlink(2)` — removing a source name.
    Unlink,
    /// `fsync(2)` on a directory.
    SyncDir,
}

impl MigrationOp {
    /// A short name for messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stat => "stat",
            Self::ReadDir => "read directory",
            Self::Link => "link",
            Self::Unlink => "unlink",
            Self::SyncDir => "fsync directory",
        }
    }
}

/// Errors and typed refusals produced while planning or executing migration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// A source or destination path was relative. Both must be absolute.
    NotAbsolute {
        /// The offending path.
        path: PathBuf,
        /// True for the destination, false for the source.
        destination: bool,
    },
    /// A containing directory is missing, is not a directory, or could not be resolved.
    DirectoryMissing {
        /// The unusable containing directory.
        path: PathBuf,
        /// True for the destination, false for the source.
        destination: bool,
        /// The rendered OS error or validation reason.
        message: String,
    },
    /// The source main path exists but is not a regular file and is refused.
    SourceNotRegularFile {
        /// The offending path.
        path: PathBuf,
        /// What it is instead.
        found: String,
    },
    /// A selected sidecar is not a regular file; symlinks are rejected without following them.
    SourceSidecarNotRegularFile {
        /// The sidecar path itself (never its symlink target).
        path: PathBuf,
    },
    /// A source path no longer names the retained source descriptor, so execution refuses to touch its replacement.
    SourceIdentityChanged {
        /// The source path whose identity changed.
        path: PathBuf,
        /// Whether the path is the main database or one of its sidecars.
        role: MigrationSourceRole,
    },
    /// The destination main path is occupied by another inode.
    ///
    /// Preflight or a racing `link` reports this refusal without overwriting or moving data.
    DestinationExists {
        /// The destination path that is occupied.
        path: PathBuf,
    },
    /// A destination sidecar exists but is not linked to its source counterpart, so migration is refused.
    StaleDestinationSidecar {
        /// The stale sidecar.
        path: PathBuf,
    },
    /// A newly published destination name no longer denotes the retained source inode; its current entry is preserved.
    DestinationIdentityChanged {
        /// The destination name whose publication identity became uncertain.
        path: PathBuf,
    },
    /// Source and destination are on different filesystems; migration is refused without copying.
    CrossDevice {
        /// The source directory.
        source_dir: PathBuf,
        /// The destination directory.
        destination_dir: PathBuf,
    },
    /// Source and destination use different filenames in the same canonical directory, an unsupported in-place rename.
    SameDirectory {
        /// The canonical directory both paths resolve to.
        directory: PathBuf,
        /// The source filename.
        source_name: PathBuf,
        /// The destination filename.
        destination_name: PathBuf,
    },
    /// The destination pathname no longer resolves to the directory retained during preparation.
    DestinationDirectoryChanged {
        /// The destination directory pathname that no longer has the retained identity.
        path: PathBuf,
    },
    /// The platform cannot retain and address a destination directory by descriptor.
    DirectoryIdentityUnsupported {
        /// The destination directory for which the guarantee is unavailable.
        path: PathBuf,
    },
    /// A database-family scan exceeded a fixed budget, so no migration plan was produced.
    ScanBudgetExceeded {
        /// The directory whose entries were being inspected.
        directory: PathBuf,
        /// Which independent scan budget was exhausted.
        budget: MigrationScanBudget,
        /// The documented maximum accepted by that budget.
        limit: usize,
        /// The first count that could not be accepted (`limit + 1`).
        attempted: usize,
    },
    /// A filesystem operation failed.
    Io {
        /// Which operation.
        op: MigrationOp,
        /// The path it was applied to.
        path: PathBuf,
        /// The `io::ErrorKind`, for programmatic handling.
        kind: io::ErrorKind,
        /// The rendered OS error.
        message: String,
    },
    /// Execution was aborted by a fault hook immediately before a step.
    FaultInjected {
        /// The step that was about to run.
        step: String,
        /// The label the injector supplied.
        label: String,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |destination: &bool| {
            if *destination {
                "destination"
            } else {
                "source"
            }
        };
        match self {
            Self::NotAbsolute { path, destination } => write!(
                f,
                "{} path {} is relative; an absolute path is required",
                side(destination),
                path.display()
            ),
            Self::DirectoryMissing {
                path,
                destination,
                message,
            } => write!(
                f,
                "{} directory {} is unusable: {message}",
                side(destination),
                path.display()
            ),
            Self::SourceNotRegularFile { path, found } => write!(
                f,
                "source database {} is a {found}, not a regular file",
                path.display()
            ),
            Self::SourceSidecarNotRegularFile { path } => write!(
                f,
                "source SQLite sidecar {} is not a regular file",
                path.display()
            ),
            Self::SourceIdentityChanged { path, role } => write!(
                f,
                "source {role} {} changed after migration planning; refusing to touch its replacement",
                path.display()
            ),
            Self::DestinationExists { path } => write!(
                f,
                "destination {} already exists; refusing to overwrite a database",
                path.display()
            ),
            Self::StaleDestinationSidecar { path } => write!(
                f,
                "stale sidecar {} at the destination would corrupt the migrated \
                 database; refusing before moving anything",
                path.display()
            ),
            Self::DestinationIdentityChanged { path } => write!(
                f,
                "destination {} changed after publication; preserving its current entry",
                path.display()
            ),
            Self::CrossDevice {
                source_dir,
                destination_dir,
            } => write!(
                f,
                "{} and {} are on different filesystems; cross-filesystem \
                 migration is refused (no reviewed durable-copy protocol)",
                source_dir.display(),
                destination_dir.display()
            ),
            Self::SameDirectory {
                directory,
                source_name,
                destination_name,
            } => write!(
                f,
                "source {} and destination {} are in the same directory {}; \
                 an in-place rename is outside this relocation protocol's \
                 reviewed two-directory durability contract",
                source_name.display(),
                destination_name.display(),
                directory.display()
            ),
            Self::DestinationDirectoryChanged { path } => write!(
                f,
                "destination directory {} changed after migration planning; \
                 refusing to remove source files",
                path.display()
            ),
            Self::DirectoryIdentityUnsupported { path } => write!(
                f,
                "destination directory identity cannot be retained on this \
                 platform for {}",
                path.display()
            ),
            Self::ScanBudgetExceeded {
                directory,
                budget,
                limit,
                attempted,
            } => write!(
                f,
                "database migration scan of {} exhausted the {} budget: \
                 attempted {attempted}, limit {limit}",
                directory.display(),
                budget.as_str()
            ),
            Self::Io {
                op,
                path,
                kind: _,
                message,
            } => write!(f, "{} {}: {message}", op.as_str(), path.display()),
            Self::FaultInjected { step, label } => {
                write!(f, "fault injected before `{step}`: {label}")
            }
        }
    }
}

impl std::error::Error for MigrationError {}

impl MigrationError {
    fn io(op: MigrationOp, path: &Path, e: &io::Error) -> Self {
        Self::Io {
            op,
            path: path.to_path_buf(),
            kind: e.kind(),
            message: e.to_string(),
        }
    }

    // Classify `link` EEXIST as no-overwrite refusal and EXDEV as cross-device refusal even when metadata preflight matched.
    fn from_link_failure(from: &Path, to: &Path, is_main: bool, e: &io::Error) -> Self {
        if e.kind() == io::ErrorKind::AlreadyExists {
            return if is_main {
                Self::DestinationExists {
                    path: to.to_path_buf(),
                }
            } else {
                Self::StaleDestinationSidecar {
                    path: to.to_path_buf(),
                }
            };
        }
        if e.raw_os_error() == Some(EXDEV) {
            return Self::CrossDevice {
                source_dir: parent_of(from),
                destination_dir: parent_of(to),
            };
        }
        Self::io(MigrationOp::Link, to, e)
    }
}

fn parent_of(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_path_buf()
}

/// The role of a source name whose retained filesystem identity changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationSourceRole {
    /// The main SQLite database file.
    Main,
    /// A SQLite-owned WAL, shared-memory file, rollback journal, or master
    /// journal.
    Sidecar,
}

impl fmt::Display for MigrationSourceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Main => f.write_str("database"),
            Self::Sidecar => f.write_str("SQLite sidecar"),
        }
    }
}

/// One ordered filesystem operation in a migration plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationStep {
    /// Phase A: hard-link a sidecar to the destination (never overwrites).
    LinkSidecar {
        /// Source name.
        from: PathBuf,
        /// Destination name.
        to: PathBuf,
    },
    /// Hard-links the main database after all sidecars, making the destination openable.
    LinkMain {
        /// Source name.
        from: PathBuf,
        /// Destination name.
        to: PathBuf,
    },
    /// Syncs destination-directory entries before any source name is removed, including on resumed plans.
    SyncDestinationDir(PathBuf),
    /// Removes the source main file before source sidecars.
    UnlinkSourceMain(PathBuf),
    /// Phase C: remove a source sidecar.
    UnlinkSourceSidecar(PathBuf),
    /// Phase C, last: `fsync` the source directory.
    SyncSourceDir(PathBuf),
}

impl fmt::Display for MigrationStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LinkSidecar { from, to } => {
                write!(f, "link sidecar {} -> {}", from.display(), to.display())
            }
            Self::LinkMain { from, to } => {
                write!(f, "link main {} -> {}", from.display(), to.display())
            }
            Self::SyncDestinationDir(p) => write!(f, "fsync destination dir {}", p.display()),
            Self::UnlinkSourceMain(p) => write!(f, "unlink source main {}", p.display()),
            Self::UnlinkSourceSidecar(p) => write!(f, "unlink source sidecar {}", p.display()),
            Self::SyncSourceDir(p) => write!(f, "fsync source dir {}", p.display()),
        }
    }
}

/// What a completed [`MigrationPlan::execute`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// No source database or recoverable source sidecars remain.
    NothingToMigrate,
    /// Source and destination are the same canonical pathname.
    ///
    /// A distinct hard-linked destination is instead treated as interrupted migration state.
    AlreadyInPlace,
    /// The database was relocated.
    Migrated {
        /// How many names were created at the destination.
        linked: usize,
        /// How many names were removed at the source.
        unlinked: usize,
        /// Whether this execution completed an interrupted migration.
        resumed: bool,
    },
}

/// Maximum entries inspected independently in each source or destination directory scan.
pub const MAX_DATABASE_DIRECTORY_ENTRIES: usize = 4_096;

/// Maximum SQLite-owned sidecars retained in one database-family scan.
pub const MAX_DATABASE_FAMILY_MEMBERS: usize = 256;

/// The independent deterministic budget exhausted by a database-family scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationScanBudget {
    /// Total directory entries inspected, whether relevant or not.
    DirectoryEntries,
    /// SQLite-owned sidecar names accepted as database-family members.
    DatabaseFamilyMembers,
}

impl MigrationScanBudget {
    /// Stable human-readable policy name used in migration refusal messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectoryEntries => "directory entries inspected",
            Self::DatabaseFamilyMembers => "accepted database-family members",
        }
    }
}

/// Returns whether `suffix` is `-wal`, `-shm`, `-journal`, or `-mj` followed by one or more ASCII hexadecimal digits.
///
/// Other similarly prefixed names are not SQLite sidecars and are never migrated.
#[must_use]
pub fn is_sqlite_sidecar_suffix(suffix: &str) -> bool {
    is_sqlite_sidecar_suffix_bytes(suffix.as_bytes())
}

fn is_sqlite_sidecar_suffix_bytes(suffix: &[u8]) -> bool {
    match suffix {
        b"-wal" | b"-shm" | b"-journal" => true,
        _ => match suffix.strip_prefix(b"-mj") {
            Some(hex) => !hex.is_empty() && hex.iter().all(u8::is_ascii_hexdigit),
            None => false,
        },
    }
}

/// Returns SQLite-owned sidecars of `main`, sorted for deterministic plans.
///
/// The main file and unrelated siblings are excluded.
///
/// # Errors
///
/// Returns [`MigrationError::Io`] for a directory-read failure or [`MigrationError::ScanBudgetExceeded`] before retaining an over-budget result.
pub fn database_file_set(main: &Path) -> Result<Vec<PathBuf>, MigrationError> {
    let dir = parent_of(main);
    let Some(stem) = main.file_name() else {
        return Ok(Vec::new());
    };
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(MigrationError::io(MigrationOp::ReadDir, &dir, &e)),
    };
    let stem = stem.as_bytes();
    let mut inspected = 0usize;
    let mut out = Vec::new();
    for entry in entries {
        inspected += 1;
        if inspected > MAX_DATABASE_DIRECTORY_ENTRIES {
            return Err(MigrationError::ScanBudgetExceeded {
                directory: dir,
                budget: MigrationScanBudget::DirectoryEntries,
                limit: MAX_DATABASE_DIRECTORY_ENTRIES,
                attempted: inspected,
            });
        }
        let entry = entry.map_err(|e| MigrationError::io(MigrationOp::ReadDir, &dir, &e))?;
        let name = entry.file_name();
        let Some(suffix) = name.as_bytes().strip_prefix(stem) else {
            continue;
        };
        if is_sqlite_sidecar_suffix_bytes(suffix) {
            if out.len() >= MAX_DATABASE_FAMILY_MEMBERS {
                return Err(MigrationError::ScanBudgetExceeded {
                    directory: dir,
                    budget: MigrationScanBudget::DatabaseFamilyMembers,
                    limit: MAX_DATABASE_FAMILY_MEMBERS,
                    attempted: out.len() + 1,
                });
            }
            out.push(dir.join(name));
        }
    }
    out.sort();
    Ok(out)
}

// Device-and-inode identity retained to detect pathname replacement between planning and execution.
type FileIdentity = (u64, u64);

fn metadata_identity(md: &fs::Metadata) -> FileIdentity {
    (md.dev(), md.ino())
}

#[derive(Clone)]
struct RetainedSource {
    path: PathBuf,
    identity: FileIdentity,
    role: MigrationSourceRole,
    handle: Arc<File>,
}

impl fmt::Debug for RetainedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetainedSource")
            .field("path", &self.path)
            .field("identity", &self.identity)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RetainedSource {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.identity == other.identity && self.role == other.role
    }
}

impl Eq for RetainedSource {}

impl RetainedSource {
    #[cfg(target_os = "linux")]
    fn open(path: &Path, role: MigrationSourceRole) -> Result<Self, MigrationError> {
        use rustix::fs::{Mode, OFlags};

        let metadata = fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                MigrationError::SourceIdentityChanged {
                    path: path.to_path_buf(),
                    role,
                }
            } else {
                MigrationError::io(MigrationOp::Stat, path, &error)
            }
        })?;
        ensure_regular_source(path, role, &metadata)?;
        let identity = metadata_identity(&metadata);
        let fd = rustix::fs::openat(
            rustix::fs::CWD,
            path,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .map_err(|error| MigrationError::io(MigrationOp::Stat, path, &error))?;
        let handle = Arc::new(File::from(fd));
        let retained_metadata = handle
            .metadata()
            .map_err(|error| MigrationError::io(MigrationOp::Stat, path, &error))?;
        let retained_identity = open_file_identity(handle.as_ref())
            .map_err(|error| MigrationError::io(MigrationOp::Stat, path, &error))?;
        if !retained_metadata.file_type().is_file() || retained_identity != identity {
            return Err(MigrationError::SourceIdentityChanged {
                path: path.to_path_buf(),
                role,
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            identity,
            role,
            handle,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn open(path: &Path, _role: MigrationSourceRole) -> Result<Self, MigrationError> {
        Err(MigrationError::DirectoryIdentityUnsupported {
            path: parent_of(path),
        })
    }

    fn validate_path(&self) -> Result<(), MigrationError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(MigrationError::SourceIdentityChanged {
                    path: self.path.clone(),
                    role: self.role,
                });
            }
            Err(error) => {
                return Err(MigrationError::io(MigrationOp::Stat, &self.path, &error));
            }
        };
        if !metadata.file_type().is_file() || metadata_identity(&metadata) != self.identity {
            return Err(MigrationError::SourceIdentityChanged {
                path: self.path.clone(),
                role: self.role,
            });
        }
        Ok(())
    }
}

fn ensure_regular_source(
    path: &Path,
    role: MigrationSourceRole,
    metadata: &fs::Metadata,
) -> Result<(), MigrationError> {
    if metadata.file_type().is_file() {
        return Ok(());
    }
    if role == MigrationSourceRole::Sidecar {
        return Err(MigrationError::SourceSidecarNotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let found = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.file_type().is_dir() {
        "directory"
    } else {
        "special file"
    };
    Err(MigrationError::SourceNotRegularFile {
        path: path.to_path_buf(),
        found: found.to_owned(),
    })
}

fn identity(path: &Path) -> Result<Option<FileIdentity>, MigrationError> {
    match fs::symlink_metadata(path) {
        Ok(md) => Ok(Some(metadata_identity(&md))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(MigrationError::io(MigrationOp::Stat, path, &e)),
    }
}

fn source_directory_device(dir: &Path) -> Result<u64, MigrationError> {
    let md = fs::metadata(dir).map_err(|e| MigrationError::DirectoryMissing {
        path: dir.to_path_buf(),
        destination: false,
        message: e.to_string(),
    })?;
    if !md.is_dir() {
        return Err(MigrationError::DirectoryMissing {
            path: dir.to_path_buf(),
            destination: false,
            message: "not a directory".to_owned(),
        });
    }
    Ok(md.dev())
}

// Canonicalize the containing directory for equivalent-path detection.
fn canonical_dir(dir: &Path, destination: bool) -> Result<PathBuf, MigrationError> {
    fs::canonicalize(dir).map_err(|e| MigrationError::DirectoryMissing {
        path: dir.to_path_buf(),
        destination,
        message: e.to_string(),
    })
}

#[derive(Clone, Debug)]
struct RetainedDestinationDirectory {
    path: PathBuf,
    identity: FileIdentity,
    handle: RetainedDir,
}

impl PartialEq for RetainedDestinationDirectory {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.identity == other.identity
    }
}

impl Eq for RetainedDestinationDirectory {}

impl RetainedDestinationDirectory {
    #[cfg(target_os = "linux")]
    fn open(path: &Path) -> Result<Self, MigrationError> {
        let handle = RetainedDir::open(path).map_err(|error| MigrationError::DirectoryMissing {
            path: path.to_path_buf(),
            destination: true,
            message: error.to_string(),
        })?;
        let retained = Self {
            path: path.to_path_buf(),
            identity: handle.identity(),
            handle,
        };
        retained.validate_reachable()?;
        Ok(retained)
    }

    #[cfg(not(target_os = "linux"))]
    fn open(path: &Path) -> Result<Self, MigrationError> {
        Err(MigrationError::DirectoryIdentityUnsupported {
            path: path.to_path_buf(),
        })
    }

    fn validate_reachable(&self) -> Result<(), MigrationError> {
        match fs::metadata(&self.path) {
            Ok(metadata) if metadata.is_dir() && metadata_identity(&metadata) == self.identity => {
                Ok(())
            }
            Ok(_) | Err(_) => Err(MigrationError::DestinationDirectoryChanged {
                path: self.path.clone(),
            }),
        }
    }

    #[cfg(target_os = "linux")]
    fn link_from(
        &self,
        source: &RetainedSource,
        to: &Path,
        is_main: bool,
    ) -> Result<(), MigrationError> {
        let Some(name) = to.file_name() else {
            return Err(MigrationError::io(
                MigrationOp::Link,
                to,
                &io::Error::new(io::ErrorKind::InvalidInput, "destination has no filename"),
            ));
        };
        self.validate_reachable()?;
        self.handle
            .link_file_noreplace(source.handle.as_ref(), name)
            .map_err(|error| MigrationError::from_link_failure(&source.path, to, is_main, &error))
    }

    #[cfg(not(target_os = "linux"))]
    fn link_from(
        &self,
        _source: &RetainedSource,
        _to: &Path,
        _is_main: bool,
    ) -> Result<(), MigrationError> {
        Err(MigrationError::DirectoryIdentityUnsupported {
            path: self.path.clone(),
        })
    }

    #[cfg(target_os = "linux")]
    fn rollback_entry_if_source(
        &self,
        path: &Path,
        source: &RetainedSource,
    ) -> Result<bool, MigrationError> {
        let Some(name) = path.file_name() else {
            return Ok(false);
        };
        match self
            .handle
            .remove_file_if_identity(name, source.handle.as_ref())
        {
            Ok(removed) => Ok(removed),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(MigrationError::io(MigrationOp::Unlink, path, &error)),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn rollback_entry_if_source(
        &self,
        _path: &Path,
        _source: &RetainedSource,
    ) -> Result<bool, MigrationError> {
        Err(MigrationError::DirectoryIdentityUnsupported {
            path: self.path.clone(),
        })
    }

    #[cfg(target_os = "linux")]
    fn entry_identity(&self, path: &Path) -> Result<Option<FileIdentity>, MigrationError> {
        let Some(name) = path.file_name() else {
            return Ok(None);
        };
        match self.handle.entry_identity(name) {
            Ok(identity) => Ok(Some(identity)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(MigrationError::io(MigrationOp::Stat, path, &error)),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn entry_identity(&self, _path: &Path) -> Result<Option<FileIdentity>, MigrationError> {
        Err(MigrationError::DirectoryIdentityUnsupported {
            path: self.path.clone(),
        })
    }

    #[cfg(target_os = "linux")]
    fn database_file_set(&self, main: &Path) -> Result<Vec<PathBuf>, MigrationError> {
        let Some(stem) = main.file_name() else {
            return Ok(Vec::new());
        };
        let mut entries = rustix::fs::Dir::read_from(&self.handle)
            .map_err(io::Error::from)
            .map_err(|error| MigrationError::io(MigrationOp::ReadDir, &self.path, &error))?;
        let stem = stem.as_bytes();
        let mut inspected = 0usize;
        let mut out = Vec::new();
        while let Some(entry) = entries.read() {
            let entry = entry
                .map_err(io::Error::from)
                .map_err(|error| MigrationError::io(MigrationOp::ReadDir, &self.path, &error))?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            inspected += 1;
            if inspected > MAX_DATABASE_DIRECTORY_ENTRIES {
                return Err(MigrationError::ScanBudgetExceeded {
                    directory: self.path.clone(),
                    budget: MigrationScanBudget::DirectoryEntries,
                    limit: MAX_DATABASE_DIRECTORY_ENTRIES,
                    attempted: inspected,
                });
            }
            let Some(suffix) = name.strip_prefix(stem) else {
                continue;
            };
            if is_sqlite_sidecar_suffix_bytes(suffix) {
                if out.len() >= MAX_DATABASE_FAMILY_MEMBERS {
                    return Err(MigrationError::ScanBudgetExceeded {
                        directory: self.path.clone(),
                        budget: MigrationScanBudget::DatabaseFamilyMembers,
                        limit: MAX_DATABASE_FAMILY_MEMBERS,
                        attempted: out.len() + 1,
                    });
                }
                out.push(self.path.join(std::ffi::OsString::from_vec(name.to_vec())));
            }
        }
        out.sort();
        Ok(out)
    }

    #[cfg(not(target_os = "linux"))]
    fn database_file_set(&self, _main: &Path) -> Result<Vec<PathBuf>, MigrationError> {
        Err(MigrationError::DirectoryIdentityUnsupported {
            path: self.path.clone(),
        })
    }

    fn sync(&self) -> Result<(), MigrationError> {
        self.handle
            .sync_all()
            .map_err(|error| MigrationError::io(MigrationOp::SyncDir, &self.path, &error))?;
        self.validate_reachable()
    }
}

/// An inspectable ordered migration plan.
///
/// Preparation mutates nothing and classifies interrupted prior execution before returning exact operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationPlan {
    source_main: PathBuf,
    destination_main: PathBuf,
    destination_directory: Option<RetainedDestinationDirectory>,
    steps: Vec<MigrationStep>,
    sources: Vec<RetainedSource>,
    no_op: Option<MigrationOutcome>,
    resumed: bool,
}

/// Optional label returned by a fault hook to abort immediately before a step.
///
/// The hook may mutate the filesystem before returning, allowing race injection.
pub type FaultVerdict = Option<String>;

/// An identity-sensitive point reached within a migration operation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationIdentityFaultPoint {
    /// The source pathname was validated and publication has not started.
    SourceValidatedBeforePublication,
    /// A destination name was created and its inode has not been validated.
    DestinationCreatedBeforeValidation,
    /// The source pathname was validated and identity-bound cleanup has not started.
    SourceValidatedBeforeCleanup,
}

impl MigrationPlan {
    /// Classifies source and destination state and builds a plan without mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed [`MigrationError`] for invalid paths, unsafe filesystem state, unavailable directory-identity guarantees, exhausted scan budgets, cross-device placement, or conflicting destination database-family names.
    pub fn prepare(
        source_main: impl AsRef<Path>,
        destination_main: impl AsRef<Path>,
    ) -> Result<Self, MigrationError> {
        let source_main = source_main.as_ref().to_path_buf();
        let destination_main = destination_main.as_ref().to_path_buf();

        for (path, destination) in [(&source_main, false), (&destination_main, true)] {
            if !path.is_absolute() {
                return Err(MigrationError::NotAbsolute {
                    path: path.clone(),
                    destination,
                });
            }
        }

        let source_dir = parent_of(&source_main);
        let destination_dir = parent_of(&destination_main);

        // Require the destination directory to exist; creating it belongs to the app-data resolver.
        let destination_canonical = canonical_dir(&destination_dir, true)?;
        let source_canonical = match canonical_dir(&source_dir, false) {
            Ok(dir) => dir,
            Err(MigrationError::DirectoryMissing { .. }) => {
                return Ok(Self::no_op_plan(
                    &source_main,
                    &destination_main,
                    MigrationOutcome::NothingToMigrate,
                ));
            }
            Err(e) => return Err(e),
        };

        // Detect equivalent paths first; reject different filenames in one canonical directory as unsupported in-place rename.
        if source_canonical == destination_canonical {
            let source_name = source_main.file_name();
            let destination_name = destination_main.file_name();
            if source_name == destination_name {
                return Ok(Self::no_op_plan(
                    &source_main,
                    &destination_main,
                    MigrationOutcome::AlreadyInPlace,
                ));
            }
            return Err(MigrationError::SameDirectory {
                directory: source_canonical,
                source_name: PathBuf::from(source_name.unwrap_or_default()),
                destination_name: PathBuf::from(destination_name.unwrap_or_default()),
            });
        }

        let destination_directory = RetainedDestinationDirectory::open(&destination_dir)?;
        let source_identity = identity(&source_main)?;
        let destination_identity = destination_directory.entry_identity(&destination_main)?;
        let source_files = database_file_set(&source_main)?;

        if source_identity.is_none() {
            // With no source main, recover only residual source sidecars linked to destination counterparts; otherwise return no work.
            let mut steps = Vec::new();
            let mut sources = Vec::new();
            if destination_identity.is_some() {
                for file in &source_files {
                    let destination_sidecar = counterpart(file, &source_main, &destination_main);
                    let Some(candidate_identity) = identity(file)? else {
                        continue;
                    };
                    if destination_directory.entry_identity(&destination_sidecar)?
                        != Some(candidate_identity)
                    {
                        continue;
                    }
                    let source = RetainedSource::open(file, MigrationSourceRole::Sidecar)?;
                    if source.identity != candidate_identity
                        || destination_directory.entry_identity(&destination_sidecar)?
                            != Some(source.identity)
                    {
                        continue;
                    }
                    steps.push(MigrationStep::UnlinkSourceSidecar(file.clone()));
                    sources.push(source);
                }
            }
            if steps.is_empty() {
                return Ok(Self::no_op_plan(
                    &source_main,
                    &destination_main,
                    MigrationOutcome::NothingToMigrate,
                ));
            }
            // Re-sync the complete destination before removing source names that may have reappeared after a lost source-directory sync.
            steps.insert(0, MigrationStep::SyncDestinationDir(destination_dir));
            steps.push(MigrationStep::SyncSourceDir(source_dir));
            destination_directory.validate_reachable()?;
            return Ok(Self {
                source_main,
                destination_main,
                destination_directory: Some(destination_directory),
                steps,
                sources,
                no_op: None,
                resumed: true,
            });
        }

        let source = RetainedSource::open(&source_main, MigrationSourceRole::Main)?;
        let source_identity = source.identity;

        // Inspect each scanned sidecar without following it and retain the opened inode through execution.
        let mut sources = Vec::with_capacity(source_files.len() + 1);
        sources.push(source);
        for file in &source_files {
            sources.push(RetainedSource::open(file, MigrationSourceRole::Sidecar)?);
        }

        let source_device = source_directory_device(&source_dir)?;
        let destination_device = destination_directory.identity.0;
        if source_device != destination_device {
            return Err(MigrationError::CrossDevice {
                source_dir,
                destination_dir,
            });
        }

        // An existing destination name is accepted only when it has its source counterpart's inode.
        let mut resumed = false;
        for stale in destination_directory.database_file_set(&destination_main)? {
            let source_counterpart = counterpart(&stale, &destination_main, &source_main);
            let destination_sidecar_identity = destination_directory.entry_identity(&stale)?;
            let source_sidecar_identity = sources
                .iter()
                .find(|source| source.path == source_counterpart)
                .map(|source| source.identity);
            if matches!(
                (destination_sidecar_identity, source_sidecar_identity),
                (Some(destination), Some(source)) if destination == source
            ) {
                resumed = true;
                continue;
            }
            return Err(MigrationError::StaleDestinationSidecar { path: stale });
        }
        let mut link_main = true;
        if let Some(dst) = destination_identity {
            if dst == source_identity {
                // When the main link already exists, recreate missing sidecars and repeat destination sync before cleanup.
                link_main = false;
                resumed = true;
            } else {
                return Err(MigrationError::DestinationExists {
                    path: destination_main,
                });
            }
        }

        let mut steps = Vec::new();
        for file in &source_files {
            let to = counterpart(file, &source_main, &destination_main);
            if destination_directory.entry_identity(&to)?.is_some() {
                continue;
            }
            steps.push(MigrationStep::LinkSidecar {
                from: file.clone(),
                to,
            });
        }
        if link_main {
            steps.push(MigrationStep::LinkMain {
                from: source_main.clone(),
                to: destination_main.clone(),
            });
        }
        // Always sync the complete destination before unlinking; a resumed run cannot assume an earlier directory sync reached stable storage.
        steps.push(MigrationStep::SyncDestinationDir(destination_dir));
        steps.push(MigrationStep::UnlinkSourceMain(source_main.clone()));
        for file in &source_files {
            steps.push(MigrationStep::UnlinkSourceSidecar(file.clone()));
        }
        steps.push(MigrationStep::SyncSourceDir(source_dir));
        destination_directory.validate_reachable()?;

        Ok(Self {
            source_main,
            destination_main,
            destination_directory: Some(destination_directory),
            steps,
            sources,
            no_op: None,
            resumed,
        })
    }

    fn no_op_plan(source_main: &Path, destination_main: &Path, outcome: MigrationOutcome) -> Self {
        Self {
            source_main: source_main.to_path_buf(),
            destination_main: destination_main.to_path_buf(),
            destination_directory: None,
            steps: Vec::new(),
            sources: Vec::new(),
            no_op: Some(outcome),
            resumed: false,
        }
    }

    /// The source main path this plan was prepared for.
    #[must_use]
    pub fn source_main(&self) -> &Path {
        &self.source_main
    }

    /// The destination main path this plan was prepared for.
    #[must_use]
    pub fn destination_main(&self) -> &Path {
        &self.destination_main
    }

    /// Returns the filesystem operations in execution order.
    #[must_use]
    pub fn steps(&self) -> &[MigrationStep] {
        &self.steps
    }

    /// Returns this plan’s no-op outcome, if it will not touch the filesystem.
    #[must_use]
    pub fn no_op(&self) -> Option<MigrationOutcome> {
        self.no_op
    }

    /// True when this plan finishes an earlier interrupted migration.
    #[must_use]
    pub fn resumes_interrupted_migration(&self) -> bool {
        self.resumed
    }

    /// Executes the plan in order.
    ///
    /// # Errors
    ///
    /// Returns the first step failure. A newly linked destination is rolled back only while it still names the retained source inode; completed prior steps remain classifiable by a later [`Self::prepare`].
    pub fn execute(self) -> Result<MigrationOutcome, MigrationError> {
        let mut before_step = |_: &MigrationStep| None;
        let mut identity_fault = |_: MigrationIdentityFaultPoint, _: &MigrationStep| {};
        self.execute_inner(&mut before_step, &mut identity_fault)
    }

    /// Executes the plan while calling `fault` immediately before each step.
    ///
    /// The hook may inspect or mutate filesystem state. Returning a label aborts before the step; otherwise execution continues from the resulting state.
    ///
    /// # Errors
    ///
    /// Returns [`MigrationError::FaultInjected`] for a supplied label or the first error produced while executing a step.
    pub fn execute_with_faults(
        self,
        fault: &mut dyn FnMut(&MigrationStep) -> FaultVerdict,
    ) -> Result<MigrationOutcome, MigrationError> {
        let mut identity_fault = |_: MigrationIdentityFaultPoint, _: &MigrationStep| {};
        self.execute_inner(fault, &mut identity_fault)
    }

    /// Executes with a callback inside identity-sensitive publication and cleanup windows.
    ///
    /// # Errors
    ///
    /// Returns the first error produced while executing a step.
    #[doc(hidden)]
    pub fn execute_with_identity_faults(
        self,
        identity_fault: &mut dyn FnMut(MigrationIdentityFaultPoint, &MigrationStep),
    ) -> Result<MigrationOutcome, MigrationError> {
        let mut before_step = |_: &MigrationStep| None;
        self.execute_inner(&mut before_step, identity_fault)
    }

    fn execute_inner(
        self,
        before_step: &mut dyn FnMut(&MigrationStep) -> FaultVerdict,
        identity_fault: &mut dyn FnMut(MigrationIdentityFaultPoint, &MigrationStep),
    ) -> Result<MigrationOutcome, MigrationError> {
        if let Some(outcome) = self.no_op {
            return Ok(outcome);
        }
        let mut linked = 0usize;
        let mut unlinked = 0usize;
        for step in &self.steps {
            if let Some(label) = before_step(step) {
                return Err(MigrationError::FaultInjected {
                    step: step.to_string(),
                    label,
                });
            }
            match step {
                MigrationStep::LinkSidecar { from, to } => {
                    let source = self.revalidate_source(from, MigrationSourceRole::Sidecar)?;
                    identity_fault(
                        MigrationIdentityFaultPoint::SourceValidatedBeforePublication,
                        step,
                    );
                    self.retained_destination_directory()?
                        .link_from(source, to, false)?;
                    identity_fault(
                        MigrationIdentityFaultPoint::DestinationCreatedBeforeValidation,
                        step,
                    );
                    self.validate_created_link(source, to)?;
                    linked += 1;
                }
                MigrationStep::LinkMain { from, to } => {
                    let source = self.revalidate_source(from, MigrationSourceRole::Main)?;
                    identity_fault(
                        MigrationIdentityFaultPoint::SourceValidatedBeforePublication,
                        step,
                    );
                    self.retained_destination_directory()?
                        .link_from(source, to, true)?;
                    identity_fault(
                        MigrationIdentityFaultPoint::DestinationCreatedBeforeValidation,
                        step,
                    );
                    self.validate_created_link(source, to)?;
                    linked += 1;
                }
                MigrationStep::SyncDestinationDir(_) => {
                    self.retained_destination_directory()?.sync()?;
                }
                MigrationStep::SyncSourceDir(dir) => {
                    sync_dir(dir)?;
                }
                MigrationStep::UnlinkSourceMain(path) => {
                    let source = self.revalidate_source(path, MigrationSourceRole::Main)?;
                    identity_fault(
                        MigrationIdentityFaultPoint::SourceValidatedBeforeCleanup,
                        step,
                    );
                    if self.unlink_retained(source)? {
                        unlinked += 1;
                    }
                }
                MigrationStep::UnlinkSourceSidecar(path) => {
                    let source = self.revalidate_source(path, MigrationSourceRole::Sidecar)?;
                    identity_fault(
                        MigrationIdentityFaultPoint::SourceValidatedBeforeCleanup,
                        step,
                    );
                    if self.unlink_retained(source)? {
                        unlinked += 1;
                    }
                }
            }
        }
        self.retained_destination_directory()?
            .validate_reachable()?;
        Ok(MigrationOutcome::Migrated {
            linked,
            unlinked,
            resumed: self.resumed,
        })
    }

    fn retained_destination_directory(
        &self,
    ) -> Result<&RetainedDestinationDirectory, MigrationError> {
        self.destination_directory.as_ref().ok_or_else(|| {
            MigrationError::DirectoryIdentityUnsupported {
                path: parent_of(&self.destination_main),
            }
        })
    }

    fn retained_source(
        &self,
        path: &Path,
        role: MigrationSourceRole,
    ) -> Result<&RetainedSource, MigrationError> {
        self.sources
            .iter()
            .find(|source| source.path == path && source.role == role)
            .ok_or_else(|| MigrationError::SourceIdentityChanged {
                path: path.to_path_buf(),
                role,
            })
    }

    fn revalidate_source(
        &self,
        path: &Path,
        role: MigrationSourceRole,
    ) -> Result<&RetainedSource, MigrationError> {
        let source = self.retained_source(path, role)?;
        source.validate_path()?;
        Ok(source)
    }

    fn validate_created_link(
        &self,
        source: &RetainedSource,
        destination: &Path,
    ) -> Result<(), MigrationError> {
        let destination_directory = self.retained_destination_directory()?;
        let validation = (|| {
            if destination_directory.entry_identity(destination)? != Some(source.identity) {
                return Err(MigrationError::DestinationIdentityChanged {
                    path: destination.to_path_buf(),
                });
            }
            destination_directory.validate_reachable()
        })();
        match validation {
            Ok(()) => Ok(()),
            Err(error) => {
                // Preserve the publication-validation error; conditional rollback cannot make the name trustworthy.
                let _ = destination_directory.rollback_entry_if_source(destination, source);
                Err(error)
            }
        }
    }

    fn unlink_retained(&self, source: &RetainedSource) -> Result<bool, MigrationError> {
        self.retained_destination_directory()?
            .validate_reachable()?;
        match remove_if_path_names_file(&source.path, source.handle.as_ref()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                match source.validate_path() {
                    Err(MigrationError::SourceIdentityChanged { .. }) => {
                        Err(MigrationError::SourceIdentityChanged {
                            path: source.path.clone(),
                            role: source.role,
                        })
                    }
                    Err(validation_error) => Err(validation_error),
                    Ok(()) => Err(MigrationError::io(
                        MigrationOp::Unlink,
                        &source.path,
                        &error,
                    )),
                }
            }
            Err(error) => Err(MigrationError::io(
                MigrationOp::Unlink,
                &source.path,
                &error,
            )),
        }
    }
}

/// Prepares and executes database relocation.
///
/// The database and all connections must already be closed.
///
/// # Errors
///
/// Returns any planning or execution [`MigrationError`].
pub fn migrate_database(
    source_main: impl AsRef<Path>,
    destination_main: impl AsRef<Path>,
) -> Result<MigrationOutcome, MigrationError> {
    MigrationPlan::prepare(source_main, destination_main)?.execute()
}

// Sync a directory so its created or removed names become durable.
fn sync_dir(dir: &Path) -> Result<(), MigrationError> {
    let handle = File::open(dir).map_err(|e| MigrationError::io(MigrationOp::SyncDir, dir, &e))?;
    handle
        .sync_all()
        .map_err(|e| MigrationError::io(MigrationOp::SyncDir, dir, &e))
}

fn counterpart(file: &Path, from_main: &Path, to_main: &Path) -> PathBuf {
    let file_name = file.file_name().map(OsStrExt::as_bytes).unwrap_or_default();
    let from_name = from_main
        .file_name()
        .map(OsStrExt::as_bytes)
        .unwrap_or_default();
    let to_name = to_main
        .file_name()
        .map(OsStrExt::as_bytes)
        .unwrap_or_default();
    let suffix = file_name.strip_prefix(from_name).unwrap_or_default();
    let mut counterpart_name = Vec::with_capacity(to_name.len() + suffix.len());
    counterpart_name.extend_from_slice(to_name);
    counterpart_name.extend_from_slice(suffix);
    parent_of(to_main).join(std::ffi::OsString::from_vec(counterpart_name))
}
