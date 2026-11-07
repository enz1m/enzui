//! Atomic publication and bounded loading of state files.
//!
//! Framed and raw saves use a private temp sibling, file synchronization,
//! atomic rename, and parent-directory synchronization. Loads refuse
//! non-regular or oversized input and incompatible headers; Linux opens the
//! final entry descriptor-relatively with no-follow and nonblocking flags.
//! Unix loads reverify the selected name after reading; unsupported identity
//! verification fails closed.
//!
//! Writes retain the `enzui-iced` envelope prefix. Reads also accept `enzui`
//! framing with the same format identifier and version; limits count actual file bytes.

use enzui_security::tempstage::TempStage;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Resolves `$XDG_STATE_HOME/<app>/<file>`, falling back to `$HOME/.local/state/<app>/<file>`.
///
/// Returns `Ok(None)` when there is no usable absolute root.
///
/// # Errors
///
/// Returns [`crate::persist::XdgPathError`] for an unconfined app or file path.
pub fn xdg_state_path(
    app: &str,
    file: impl AsRef<Path>,
) -> Result<Option<PathBuf>, crate::persist::XdgPathError> {
    crate::persist::XdgResolver::from_env().resolve_state(app, file)
}

/// Result of loading a framed state file.
///
/// Missing, malformed, incompatible, oversized, unreadable, replaced, and
/// unverifiable files produce [`StateLoad::UsedDefaults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateLoad {
    /// The header matched and the body is returned verbatim for the caller to
    /// parse. May be empty if the file held a header and no records.
    Loaded(String),
    /// Defaults were used; `note` is diagnostic text and must not control behavior.
    UsedDefaults {
        /// Why defaults were used, for logging.
        note: String,
    },
}

impl StateLoad {
    /// The body, or an empty string when defaults were used.
    #[must_use]
    pub fn body(&self) -> &str {
        match self {
            StateLoad::Loaded(b) => b,
            StateLoad::UsedDefaults { .. } => "",
        }
    }

    /// Whether defaults were used.
    #[must_use]
    pub fn used_defaults(&self) -> bool {
        matches!(self, StateLoad::UsedDefaults { .. })
    }
}

// Use two lines so truncation after the format id cannot be mistaken for a confirmed body.
fn header(format_id: &str, version: u32) -> String {
    // The persisted prefix is a wire-format contract independent of crate names.
    format!("enzui-iced {format_id} v{version}\nbody\n")
}

// Retain the parent descriptor across the no-follow, nonblocking leaf open so
// the state name is resolved exactly once and a FIFO cannot stall before fstat.
#[cfg(target_os = "linux")]
fn open_nofollow(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};

    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "state path has no file name")
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = rustix::fs::open(
        parent,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let fd = rustix::fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    Ok(File::from(fd))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_nofollow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(
            enzui_security::platform::libc_o_nofollow()
                | enzui_security::platform::libc_o_nonblock(),
        )
        .open(path)
}

// Non-Unix `std` lacks an atomic no-follow/nonblocking open; validate the
// resulting descriptor afterward.
#[cfg(not(unix))]
fn open_nofollow(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Maximum framed state-file size: [`MAX_RECORD_BYTES`] × [`MAX_RECORDS`] (128 MiB).
///
/// Files exactly at the limit remain eligible to load.
pub const MAX_STATE_FILE_BYTES: u64 = MAX_RECORD_BYTES * MAX_RECORDS;

/// Maximum bytes admitted for one line-oriented record: 32 KiB.
pub const MAX_RECORD_BYTES: u64 = 32 * 1024;

/// Maximum number of physical records admitted in a line-oriented body.
pub const MAX_RECORDS: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineBodyLimits {
    pub(crate) max_record_bytes: u64,
    pub(crate) max_records: u64,
}

impl LineBodyLimits {
    pub(crate) const PRODUCTION: Self = Self {
        max_record_bytes: MAX_RECORD_BYTES,
        max_records: MAX_RECORDS,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LineBodyError<E> {
    RecordTooLarge {
        record: u64,
        bytes: u64,
        max_bytes: u64,
    },
    TooManyRecords {
        records: u64,
        max_records: u64,
    },
    Consumer(E),
}

impl<E: std::fmt::Display> std::fmt::Display for LineBodyError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecordTooLarge {
                record,
                bytes,
                max_bytes,
            } => write!(
                f,
                "record {record} is too large: {bytes} bytes exceeds {max_bytes}"
            ),
            Self::TooManyRecords {
                records,
                max_records,
            } => write!(
                f,
                "too many records: record {records} exceeds limit {max_records}"
            ),
            Self::Consumer(error) => error.fmt(f),
        }
    }
}

// Validate all physical lines before invoking `visit`; count empty lines but skip them during decoding.
pub(crate) fn visit_line_body<E>(
    body: &str,
    limits: LineBodyLimits,
    mut visit: impl FnMut(&str) -> Result<(), E>,
) -> Result<(), LineBodyError<E>> {
    // Validate the complete body before invoking consumer code so a late limit refusal cannot leave partially decoded state.
    for (index, line) in body.lines().enumerate() {
        let record = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
        if record > limits.max_records {
            return Err(LineBodyError::TooManyRecords {
                records: record,
                max_records: limits.max_records,
            });
        }
        let bytes = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if bytes > limits.max_record_bytes {
            return Err(LineBodyError::RecordTooLarge {
                record,
                bytes,
                max_bytes: limits.max_record_bytes,
            });
        }
    }

    for line in body.lines().filter(|line| !line.is_empty()) {
        visit(line).map_err(LineBodyError::Consumer)?;
    }
    Ok(())
}

/// Classifies why [`load`] used defaults without exposing diagnostic text as a protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateRefusal {
    /// No file at the path.
    Absent,
    /// Opening the file failed; on Unix this includes refusal of a final-component symlink.
    OpenRefused,
    /// The path resolved to something that is not a regular file.
    NotRegularFile,
    /// Reading metadata from the opened file failed.
    StatFailed,
    /// Reading the opened descriptor failed.
    ReadFailed,
    /// The descriptor yielded a different length than its pre-read metadata.
    ChangedDuringRead,
    /// The selected pathname no longer verifiably names the held descriptor.
    PathIdentityUncertain,
    /// The file exceeds the byte ceiling according to opened-file metadata or the bounded read.
    TooLarge,
    /// The file was empty.
    Empty,
    /// The file content did not match the expected format id and version.
    UnexpectedHeader,
}

/// Loads a state file after validating its type, size, and format header.
///
/// Any refusal returns [`StateLoad::UsedDefaults`].
pub fn load(path: &Path, format_id: &str, version: u32) -> StateLoad {
    load_with_reason(path, format_id, version).0
}

/// [`load`], plus the typed [`StateRefusal`] when defaults were used.
///
/// `Loaded` carries `None`. Use this instead of inspecting the diagnostic
/// `note` when behaviour must differ by refusal class.
pub fn load_with_reason(
    path: &Path,
    format_id: &str,
    version: u32,
) -> (StateLoad, Option<StateRefusal>) {
    load_bounded(path, format_id, version, MAX_STATE_FILE_BYTES)
}

/// [`load_with_reason`] with a caller-chosen size ceiling instead of
/// `MAX_STATE_FILE_BYTES`.
pub fn load_bounded(
    path: &Path,
    format_id: &str,
    version: u32,
    max_bytes: u64,
) -> (StateLoad, Option<StateRefusal>) {
    let file = match open_nofollow(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return (
                StateLoad::UsedDefaults {
                    note: "no state file".into(),
                },
                Some(StateRefusal::Absent),
            );
        }
        Err(e) => {
            // `O_NOFOLLOW` reports a final-component symlink as an open refusal.
            return (
                StateLoad::UsedDefaults {
                    note: format!("open refused: {e}"),
                },
                Some(StateRefusal::OpenRefused),
            );
        }
    };

    // Query metadata from the opened descriptor rather than resolving the path again, which could race with replacement.
    let stat_len = match file.metadata() {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => {
            return (
                StateLoad::UsedDefaults {
                    note: "state path is not a regular file".into(),
                },
                Some(StateRefusal::NotRegularFile),
            );
        }
        Err(e) => {
            return (
                StateLoad::UsedDefaults {
                    note: format!("fstat failed: {e}"),
                },
                Some(StateRefusal::StatFailed),
            );
        }
    };

    // Reject the descriptor’s reported oversize length before allocation; the bounded read below also handles growth after `fstat`.
    if stat_len > max_bytes {
        return (
            StateLoad::UsedDefaults {
                note: format!("state file too large: {stat_len} bytes exceeds {max_bytes}"),
            },
            Some(StateRefusal::TooLarge),
        );
    }

    #[cfg(test)]
    test_post_stat_hook::run();

    // The extra byte is the growth sentinel; metadata-sized reservation keeps
    // a stable admitted file in one body allocation.
    let Some(reserve) = usize::try_from(stat_len)
        .ok()
        .and_then(|len| len.checked_add(1))
    else {
        return (
            StateLoad::UsedDefaults {
                note: format!("state file too large for this platform: {stat_len} bytes"),
            },
            Some(StateRefusal::TooLarge),
        );
    };
    let mut bytes = Vec::new();
    if let Err(error) = bytes.try_reserve_exact(reserve) {
        return (
            StateLoad::UsedDefaults {
                note: format!("state file allocation failed: {error}"),
            },
            Some(StateRefusal::ReadFailed),
        );
    }

    // A stack chunk keeps the EOF probe from growing an already exact-sized
    // file allocation as `read_to_end` may do.
    let read_limit = stat_len.saturating_add(1);
    let mut reader = &file;
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let bytes_read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if bytes_read >= read_limit {
            break;
        }
        let remaining = read_limit - bytes_read;
        let chunk_len = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(chunk.len());
        match reader.read(&mut chunk[..chunk_len]) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(error) => {
                return (
                    StateLoad::UsedDefaults {
                        note: format!("read failed: {error}"),
                    },
                    Some(StateRefusal::ReadFailed),
                );
            }
        }
    }
    let observed_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if observed_len > max_bytes {
        return (
            StateLoad::UsedDefaults {
                note: format!(
                    "state file too large: read exceeded {max_bytes} bytes (grew after stat)"
                ),
            },
            Some(StateRefusal::TooLarge),
        );
    }
    if observed_len != stat_len {
        let relation = if observed_len > stat_len {
            "grew"
        } else {
            "shrank"
        };
        return (
            StateLoad::UsedDefaults {
                note: format!(
                    "state file changed while reading: {relation} from {stat_len} bytes to {observed_len} observed bytes"
                ),
            },
            Some(StateRefusal::ChangedDuringRead),
        );
    }

    #[cfg(test)]
    test_post_read_hook::run();

    if let Err(error) = enzui_security::tempstage::verify_path_names_file(&file, path) {
        return (
            StateLoad::UsedDefaults {
                note: format!("state path identity could not be verified after read: {error}"),
            },
            Some(StateRefusal::PathIdentityUncertain),
        );
    }

    let Ok(mut text) = String::from_utf8(bytes) else {
        return (
            StateLoad::UsedDefaults {
                note: "read failed: stream did not contain valid UTF-8".into(),
            },
            Some(StateRefusal::ReadFailed),
        );
    };
    let want = header(format_id, version);
    // Both persisted envelope spellings identify the same body format.
    let renamed = format!("enzui {format_id} v{version}\nbody\n");
    if retain_body(&mut text, &want) || retain_body(&mut text, &renamed) {
        return (StateLoad::Loaded(text), None);
    }

    let first = text.lines().next().unwrap_or("");
    if text.is_empty() {
        (
            StateLoad::UsedDefaults {
                note: "empty file".into(),
            },
            Some(StateRefusal::Empty),
        )
    } else {
        let end = first
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= 256)
            .last()
            .unwrap_or(0);
        let preview = if first.len() <= 256 {
            first
        } else {
            first.get(..end).unwrap_or(first)
        };
        let suffix = if preview.len() == first.len() {
            ""
        } else {
            "…"
        };
        (
            StateLoad::UsedDefaults {
                note: format!("unexpected header: {preview:?}{suffix}"),
            },
            Some(StateRefusal::UnexpectedHeader),
        )
    }
}

fn retain_body(text: &mut String, framing: &str) -> bool {
    if !text.starts_with(framing) {
        return false;
    }
    text.drain(..framing.len());
    true
}

/// Opens and validates a regular state file.
///
/// On Linux, the final entry is opened descriptor-relatively with no-follow and
/// nonblocking flags. Other Unix platforms use the same flags with a path open;
/// non-Unix platforms perform a normal open. Every platform validates the
/// resulting descriptor's file type.
///
/// # Errors
///
/// An I/O error when the open fails, the final entry is a symlink, or it is not a regular file.
pub fn open_state_file_nofollow(path: &Path) -> io::Result<File> {
    let file = open_nofollow(path)?;
    // Validate the opened descriptor with `fstat` rather than resolving the path again.
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state path is not a regular file",
        ));
    }
    Ok(file)
}

// Publish through `TempStage`, which owns exclusive private staging, identity checks, synchronization, rename, and safe cleanup.
fn publish_atomic(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    // Use a unique sibling stage so concurrent writers cannot share a temp path.
    let mut staged = TempStage::create(path)?;
    // On failure, drop removes only a stage whose identity is still verified; otherwise it leaves the private residue.
    write(staged.file_mut())?;

    #[cfg(test)]
    test_pre_rename_hook::run();

    // Commit synchronizes the staged file, verifies its identity, renames it, and synchronizes the parent; every failure propagates.
    staged.commit()
}

pub(crate) fn framed_len(format_id: &str, version: u32, body: &str) -> Option<u64> {
    u64::try_from(header(format_id, version).len())
        .ok()?
        .checked_add(u64::try_from(body.len()).ok()?)
}

/// Atomically writes a framed state file.
///
/// Publication uses a unique private sibling, file synchronization, atomic rename, and parent-directory synchronization. Durability failures are returned.
///
/// # Errors
///
/// An I/O error from staging, writing, renaming or synchronizing.
pub fn save_atomic(path: &Path, format_id: &str, version: u32, body: &str) -> io::Result<()> {
    let framing = header(format_id, version);
    publish_atomic(path, |staged| {
        staged.write_all(framing.as_bytes())?;
        staged.write_all(body.as_bytes())
    })
}

/// Atomically publishes `bytes` without framing or transcoding.
///
/// The caller remains responsible for loading, schema versions, migration, and corrupt-file handling.
///
/// # Errors
///
/// An I/O error from staging, writing, renaming or synchronizing.
pub fn save_atomic_raw(path: &Path, bytes: &[u8]) -> io::Result<()> {
    save_atomic_raw_with(path, |staged| staged.write_all(bytes))
}

/// Atomically publishes unframed bytes written to a private staged file by `write`.
///
/// A callback error leaves the destination unchanged and is returned verbatim. A later durability error may be reported after rename made the complete destination visible.
///
/// # Errors
///
/// `write`'s own error, unchanged, or an I/O error from staging, renaming or synchronizing.
pub fn save_atomic_raw_with(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    publish_atomic(path, write)
}

/// An inter-process advisory lock for a state file.
///
/// Hold it across load, mutation, and atomic save. A sibling lock file is used because publication replaces the state-file inode, and the kernel releases the lock when this value drops.
#[derive(Debug)]
pub struct StateLock {
    _file: File,
    path: PathBuf,
}

impl StateLock {
    /// The lock-file path for a given state-file path: `<state>.lock`.
    #[must_use]
    pub fn lock_path(state_path: &Path) -> PathBuf {
        let mut name = state_path
            .file_name()
            .map(std::ffi::OsString::from)
            .unwrap_or_default();
        name.push(".lock");
        state_path.with_file_name(name)
    }

    /// Blocks until an exclusive advisory lock for `state_path` is acquired.
    ///
    /// The sibling lock file and its parent are created if needed; dropping the result releases the lock.
    ///
    /// # Errors
    ///
    /// An I/O error when the lock file cannot be created or locked.
    pub fn acquire(state_path: &Path) -> io::Result<StateLock> {
        let (file, path) = Self::open_lock_file(state_path)?;
        file.lock()?;
        Ok(StateLock { _file: file, path })
    }

    /// Attempts to acquire the lock without blocking.
    ///
    /// Returns `Ok(None)` when another holder owns it and `Err` for I/O failures.
    ///
    /// # Errors
    ///
    /// An I/O error when the lock file cannot be created or locking fails for a reason other than
    /// contention.
    pub fn try_acquire(state_path: &Path) -> io::Result<Option<StateLock>> {
        let (file, path) = Self::open_lock_file(state_path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(StateLock { _file: file, path })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// The lock file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open_lock_file(state_path: &Path) -> io::Result<(File, PathBuf)> {
        let path = Self::lock_path(state_path);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        Ok((file, path))
    }
}

#[cfg(test)]
mod test_post_stat_hook {
    use std::cell::RefCell;
    type Hook = Box<dyn FnOnce()>;
    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }
    pub(super) fn run() {
        let hook = HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

#[cfg(test)]
mod test_post_read_hook {
    use std::cell::RefCell;
    type Hook = Box<dyn FnOnce()>;
    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }
    pub(super) fn run() {
        let hook = HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

#[cfg(test)]
mod test_pre_rename_hook {
    use std::cell::RefCell;
    type Hook = Box<dyn Fn()>;
    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }
    pub(super) fn run() {
        // Take the callback before invocation so a reentrant `save_atomic` does not borrow the `RefCell` twice.
        let taken = HOOK.with(|h| h.borrow_mut().take());
        if let Some(f) = taken {
            f();
            HOOK.with(|h| *h.borrow_mut() = Some(f));
        }
    }
}

#[cfg(test)]
mod tests;
