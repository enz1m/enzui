//! Shared primitives for an app that hosts third-party extensions.
//!
//! An app keeps its own contribution schema (actions, panels, file
//! handlers) and decides where contributions appear; this module owns the
//! parts every extension host repeats and gets wrong:
//!
//! * [`Manifest`]: a versioned envelope (identity, semantic version, API,
//!   worker, permissions, typed settings) around the app's
//!   [`Contributions`], validated against the app's [`Host`], with
//!   API/platform negotiation that answers [`Incompatible`] instead of a
//!   string.
//! * Package admission ([`package::read`]): a strict classic-ZIP reader that
//!   refuses duplicate or colliding names, ZIP64 and multi-disk archives,
//!   a directory count that disagrees with its entries, encryption, links,
//!   overlapping data, CRC mismatches and anything past [`PackageLimits`].
//! * The [`Store`]: one private folder per extension under an instance
//!   lock, a SHA-256 record of every file checked on load and again before
//!   each run, durable enablement and settings, installs and updates
//!   written in private staging and swapped in with one atomic rename
//!   (an update must be a newer version), and removal tombstones for
//!   extensions bundled with the app.
//! * Workers ([`invoke`], [`serve`], [`serve_builtin`]): one bounded JSON
//!   request and response over standard streams through the process
//!   transport, every ending a [`WorkerFailure`], run under the user's
//!   permissions or inside a bubblewrap [`Sandbox`], and an
//!   [`ExtensionRunner`] keyed by activity row whose cancellation stops the
//!   process and drops any late success.
//! * Settings ([`SettingsDraft`]): typed fields that keep invalid input
//!   visible with its [`SettingProblem`] and apply all or none.
//!
//! A native worker runs as the user unless sandboxed; say so on the review
//! screen before installing. Effects a response asks for (clipboard,
//! navigation) are the app's to apply, and only for permissions the
//! manifest declared.

mod manifest;
pub mod package;
mod settings;
mod store;
mod worker;

pub use manifest::{
    Contributions, Host, Incompatible, MANIFEST_LIMIT, MANIFEST_NAME, Manifest, STATE_NAME,
    WorkerSpec, display_text, host_target, identifier, plain_relative,
};
pub use package::PackageLimits;
pub use settings::{
    DraftValue, SETTING_LIMIT, SettingField, SettingKind, SettingProblem, SettingSpec,
    SettingsDraft, TEXT_LIMIT, check_values, resolve,
};
pub use store::{Catalog, Installed, Prepared, State, Store, load, verify};
pub use worker::{
    BUILTIN_FLAG, ExtensionRunner, Isolation, RunOutcome, Sandbox, WorkerFailure, WorkerLimits,
    invoke, serve, serve_builtin,
};

use std::path::Path;
use std::sync::Arc;

/// Why a store operation failed.
#[derive(Clone, Debug)]
pub enum StoreError {
    /// Another process holds the store's lock; try again.
    Busy,
    /// The extension is not in the store.
    NotInstalled,
    /// The package or request is invalid: why.
    Invalid(String),
    /// Installed files are missing, changed, or not private: why.
    Damaged(String),
    /// It cannot run on this host.
    Incompatible(Incompatible),
    /// An update must be newer than the installed version.
    NotNewer {
        /// The installed version.
        installed: String,
        /// The package's version.
        offered: String,
    },
    /// The store holds as many extensions as it allows.
    Limit,
    /// The filesystem refused.
    Io(Arc<std::io::Error>),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Another window is changing extensions; try again"),
            Self::NotInstalled => f.write_str("The extension is no longer installed"),
            Self::Invalid(why) | Self::Damaged(why) => f.write_str(why),
            Self::Incompatible(why) => why.fmt(f),
            Self::NotNewer { installed, offered } => write!(
                f,
                "Version {offered} is not newer than the installed {installed}; remove it first to downgrade"
            ),
            Self::Limit => f.write_str("Remove an extension before installing another"),
            Self::Io(error) => write!(f, "The extension store could not be changed: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}

fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Reads a regular file, not following a final symlink and not blocking on
/// a FIFO, refusing it past `limit` bytes.
fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Extension data must be regular files".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Extension data exceeds its size limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
