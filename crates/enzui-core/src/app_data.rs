//! Resolution and creation of per-application durable data directories.
//!
//! Absolute `$XDG_DATA_HOME` takes precedence over `$HOME/.local/share`; unusable roots produce an error rather than a relative path. Application ids and child paths are confined, and ensured app directories are private on Unix.

use crate::persist::{XdgPathError, XdgRelativePath};
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// The XDG spec's default data home, relative to `$HOME`.
pub const DATA_HOME_FALLBACK_RELATIVE: &str = ".local/share";

/// A validated single path segment naming an application data directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppId(String);

impl AppId {
    /// Validates an application identifier.
    ///
    /// # Errors
    ///
    /// Returns [`AppDataError::InvalidAppId`] for an empty, dotted, hidden, NUL-containing, or multi-component path.
    pub fn new(id: impl Into<String>) -> Result<Self, AppDataError> {
        let id = id.into();
        let reason = if id.is_empty() {
            Some(AppIdRejection::Empty)
        } else if id.contains('\0') {
            Some(AppIdRejection::InteriorNul)
        } else if id.contains('/') || id.contains(std::path::MAIN_SEPARATOR) {
            // Checked before the dot rules so `../../etc` is reported as the
            // multi-segment traversal it is, not merely as a leading dot.
            Some(AppIdRejection::Separator)
        } else if id == "." || id == ".." {
            Some(AppIdRejection::DotSegment)
        } else if id.starts_with('.') {
            Some(AppIdRejection::LeadingDot)
        } else if Path::new(&id).components().count() != 1
            || !matches!(
                Path::new(&id).components().next(),
                Some(Component::Normal(_))
            )
        {
            Some(AppIdRejection::Separator)
        } else {
            None
        };
        match reason {
            Some(reason) => Err(AppDataError::InvalidAppId { id, reason }),
            None => Ok(Self(id)),
        }
    }

    /// The validated segment.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an [`AppId`] candidate was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppIdRejection {
    /// The empty string, which would select the base data directory itself.
    Empty,
    /// `.` or `..` — a traversal, not a name.
    DotSegment,
    /// Contains a separator or is not one normal platform path component.
    Separator,
    /// Starts with `.`, which this application-id policy disallows.
    LeadingDot,
    /// Contains an interior NUL byte.
    InteriorNul,
}

impl AppIdRejection {
    /// A short human-readable reason, for logs and error text.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "must not be empty",
            Self::DotSegment => "must not be `.` or `..`",
            Self::Separator => "must be a single path segment (no separators)",
            Self::LeadingDot => "must not start with `.`",
            Self::InteriorNul => "must not contain a NUL byte",
        }
    }
}

/// Failures while resolving or creating an application data directory.
///
/// Child-path validation returns [`XdgPathError`] instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppDataError {
    /// The application identifier is not one ordinary path segment.
    InvalidAppId {
        /// The rejected candidate, echoed for the message.
        id: String,
        /// Which rule it broke.
        reason: AppIdRejection,
    },
    /// No usable absolute data-home root is available.
    NoDataHome {
        /// True when `$XDG_DATA_HOME` was set but ignored because it was
        /// relative (the XDG spec's rule), so the message can say so.
        data_home_was_relative: bool,
        /// True when `$HOME` was set but ignored because it was relative.
        home_was_relative: bool,
    },
    /// The application directory could not be created or validated.
    Create {
        /// The path whose creation or validation failed.
        path: PathBuf,
        /// The OS error, rendered (`io::Error` is not `Clone`/`PartialEq`).
        message: String,
    },
}

impl fmt::Display for AppDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAppId { id, reason } => {
                write!(f, "invalid application id {id:?}: {}", reason.as_str())
            }
            Self::NoDataHome {
                data_home_was_relative,
                home_was_relative,
            } => {
                write!(
                    f,
                    "no XDG data directory: $XDG_DATA_HOME is {} and $HOME is {}",
                    if *data_home_was_relative {
                        "relative (invalid per the XDG spec, ignored)"
                    } else {
                        "unset or empty"
                    },
                    if *home_was_relative {
                        "relative (invalid, ignored)"
                    } else {
                        "unset or empty"
                    }
                )
            }
            Self::Create { path, message } => {
                write!(f, "cannot create {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for AppDataError {}

/// Resolves an application data directory from injectable XDG and home roots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppDataResolver {
    data_home: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl AppDataResolver {
    /// Build from explicit roots. `None` means "not set".
    #[must_use]
    pub fn new(data_home: Option<impl Into<PathBuf>>, home: Option<impl Into<PathBuf>>) -> Self {
        Self {
            data_home: data_home.map(Into::into),
            home: home.map(Into::into),
        }
    }

    /// Snapshots `$XDG_DATA_HOME` and `$HOME` from the process environment.
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
            data_home: non_empty("XDG_DATA_HOME"),
            home: non_empty("HOME"),
        }
    }

    /// Returns the usable base data directory without an application segment.
    ///
    /// # Errors
    ///
    /// Returns [`AppDataError::NoDataHome`] when neither configured root is usable.
    pub fn base(&self) -> Result<PathBuf, AppDataError> {
        if let Some(dir) = self.data_home.as_ref().filter(|p| p.is_absolute()) {
            return Ok(dir.clone());
        }
        if let Some(home) = self.home.as_ref().filter(|p| p.is_absolute()) {
            return Ok(home.join(DATA_HOME_FALLBACK_RELATIVE));
        }
        Err(AppDataError::NoDataHome {
            data_home_was_relative: self.data_home.is_some(),
            home_was_relative: self.home.is_some(),
        })
    }

    /// Resolves the application directory without filesystem access.
    ///
    /// # Errors
    ///
    /// Returns [`AppDataError::NoDataHome`] when neither root is usable.
    pub fn resolve(&self, app: &AppId) -> Result<PathBuf, AppDataError> {
        Ok(self.base()?.join(app.as_str()))
    }

    /// Resolves and creates the application directory.
    ///
    /// On Unix, a new leaf is mode `0o700`; an existing leaf must be a non-symlink directory with no group or other permissions and, when the current UID is available, matching ownership.
    ///
    /// # Errors
    ///
    /// Returns [`AppDataError::NoDataHome`] for unusable roots or [`AppDataError::Create`] when creation or validation fails.
    pub fn ensure(&self, app: &AppId) -> Result<AppDataDir, AppDataError> {
        self.ensure_with(app, false)
    }

    /// [`Self::ensure`], except that an existing leaf this user owns which
    /// still grants group or other access is made private (`0o700`) instead
    /// of refused, as [`StateAuthority::open_existing_tightening`] does for a
    /// state file. Earlier builds created app directories with the umask's
    /// `0o755`; without this an app refuses its own data on every launch.
    ///
    /// The mode is changed through a handle opened without following
    /// symlinks and checked to be the directory that was inspected, so a
    /// swapped path is refused, never tightened. Another owner's directory,
    /// a symlink or a non-directory is refused exactly as by `ensure`.
    ///
    /// [`StateAuthority::open_existing_tightening`]: crate::state_authority::StateAuthority::open_existing_tightening
    ///
    /// # Errors
    ///
    /// As [`Self::ensure`], and [`AppDataError::Create`] when the mode cannot
    /// be tightened.
    pub fn ensure_tightening(&self, app: &AppId) -> Result<AppDataDir, AppDataError> {
        self.ensure_with(app, true)
    }

    fn ensure_with(&self, app: &AppId, tighten: bool) -> Result<AppDataDir, AppDataError> {
        let path = self.resolve(app)?;
        ensure_parent_chain(&path)?;
        create_private_leaf(&path, tighten)?;
        // Inspect the final path without following symlinks; `create_dir_all` accepts a symlink to a directory.
        let file_type = std::fs::symlink_metadata(&path)
            .map_err(|e| AppDataError::Create {
                path: path.clone(),
                message: e.to_string(),
            })?
            .file_type();
        if file_type.is_symlink() {
            return Err(AppDataError::Create {
                path,
                message: "exists but is a symlink, not a directory".to_owned(),
            });
        }
        if !file_type.is_dir() {
            return Err(AppDataError::Create {
                path,
                message: "exists but is not a directory".to_owned(),
            });
        }
        Ok(AppDataDir { path })
    }
}

// Preserve platform-default permissions on shared XDG parent directories.
fn ensure_parent_chain(path: &Path) -> Result<(), AppDataError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| AppDataError::Create {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
    }
    Ok(())
}

// Create a mode-0700 leaf, or adopt only a non-symlink directory with no group/other bits and, when the current UID is available, matching ownership.
fn create_private_leaf(path: &Path, tighten: bool) -> Result<(), AppDataError> {
    #[cfg(unix)]
    {
        create_private_leaf_unix(path, tighten)
    }
    #[cfg(not(unix))]
    {
        let _ = tighten;
        std::fs::create_dir_all(path).map_err(|e| AppDataError::Create {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }
}

#[cfg(unix)]
fn create_private_leaf_unix(path: &Path, tighten: bool) -> Result<(), AppDataError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let refuse = |message: String| AppDataError::Create {
        path: path.to_path_buf(),
        message,
    };

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {
            // Force mode 0o700 so the result does not depend on the process umask.
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| refuse(e.to_string()))?;
            let meta = std::fs::symlink_metadata(path).map_err(|e| refuse(e.to_string()))?;
            let mode = meta.mode() & 0o7777;
            if mode & 0o077 != 0 {
                // Refuse a leaf whose verified mode still grants group or other access.
                return Err(refuse(format!(
                    "created but mode {mode:o} still exposes group/other bits"
                )));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path).map_err(|e| refuse(e.to_string()))?;
            if meta.file_type().is_symlink() {
                return Err(refuse(
                    "exists but is a symlink, not a directory".to_owned(),
                ));
            }
            if !meta.file_type().is_dir() {
                return Err(refuse("exists but is not a directory".to_owned()));
            }
            if let Some(uid) = crate::runtime_dir::current_uid()
                && meta.uid() != uid
            {
                return Err(refuse(format!(
                    "exists but is owned by uid {} not our uid {uid}",
                    meta.uid()
                )));
            }
            let mode = meta.mode() & 0o7777;
            if mode & 0o077 != 0 {
                if tighten {
                    return tighten_leaf(path, &meta).map_err(refuse);
                }
                return Err(refuse(format!(
                    "exists with mode {mode:o} which exposes group/other bits"
                )));
            }
            Ok(())
        }
        Err(e) => Err(refuse(e.to_string())),
    }
}

// Makes the inspected directory `0o700` through a handle that cannot follow a
// symlink, after checking the handle is the same directory, then verifies.
#[cfg(unix)]
fn tighten_leaf(path: &Path, inspected: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("could not open it to make it private: {e}"))?;
    let held = handle.metadata().map_err(|e| e.to_string())?;
    if !held.is_dir() || held.dev() != inspected.dev() || held.ino() != inspected.ino() {
        return Err("changed while it was being made private".to_owned());
    }
    handle
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("could not make it private: {e}"))?;
    let mode = handle.metadata().map_err(|e| e.to_string())?.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(format!("mode {mode:o} still exposes group/other bits"));
    }
    Ok(())
}

/// A validated existing application data directory.
///
/// Child resolution remains fallible to prevent escape from this directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDataDir {
    path: PathBuf,
}

impl AppDataDir {
    /// The directory itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolves a confined relative object path inside this directory.
    ///
    /// # Errors
    ///
    /// Returns [`XdgPathError`] for an empty, absolute, dotted, NUL-containing, or otherwise non-normal relative path.
    pub fn join(&self, name: impl AsRef<Path>) -> Result<PathBuf, XdgPathError> {
        let name = XdgRelativePath::new(name)?;
        Ok(self.path.join(name.as_path()))
    }
}
