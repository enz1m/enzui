//! Persisted download state and startup reconciliation.

use super::*;
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

/// The on-disk format id and version for download state.
///
/// Bumped when a change would confuse an older reader. The reader refuses a
/// mismatch outright rather than guessing — see [`crate::atomic_state`].
pub const STATE_FORMAT_ID: &str = "downloads";
/// The on-disk format version. See [`STATE_FORMAT_ID`].
pub const STATE_VERSION: u32 = 1;

/// Persisted state for one download.
///
/// Snapshots omit process-local notifications and store active transfers as paused; this module does not open `partial`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRecord {
    /// Stable application-owned identifier preserved across restarts.
    pub id: DownloadId,
    /// Display name, so a restored row can be drawn before any transport runs.
    pub name: String,
    /// The attempt in force when the record was written.
    pub attempt: Attempt,
    /// Phase recorded for restoration; [`snapshot`] converts active transfers to [`Phase::Paused`].
    pub phase: Phase,
    /// Bytes the persisted record claims were received.
    pub received: u64,
    /// Announced total, if the transport had one.
    pub total: Option<u64>,
    /// Path to the partial file, when one was recorded.
    pub partial: Option<PathBuf>,
    /// Whether a terminal failure was blocked by policy.
    ///
    /// `false` is represented by an absent field and decodes as retryable.
    pub blocked: bool,
}

/// Describes a recorded partial file at startup.
///
/// The caller supplies this state; reconciliation performs no partial-file I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialFile {
    /// No partial file was recorded, or the record named none.
    None,
    /// The file is absent from disk although the record named one.
    Missing,
    /// The file exists and holds exactly this many bytes.
    Present {
        /// Size on disk, in bytes.
        len: u64,
    },
}

/// The result of reconciling one persisted record with its partial file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciled {
    /// Restore the download and resume from `resume_at` bytes.
    Resumable {
        /// The restored model state.
        download: Download,
        /// Byte offset a transport should resume from. Always equal to the
        /// restored `received`.
        resume_at: u64,
        /// The display name, carried through from the record.
        name: String,
    },
    /// Restores from byte zero with an advanced attempt when the persisted byte state is inconsistent.
    RestartFromZero {
        /// The restored model state, at a fresh attempt with `received == 0`.
        download: Download,
        /// The display name.
        name: String,
        /// Why the partial was rejected.
        reason: RestartReason,
    },
    /// Restore as an already-terminal download. Nothing to transfer.
    Terminal {
        /// The restored model state.
        download: Download,
        /// The display name.
        name: String,
    },
    /// Drop the record entirely; it describes nothing usable.
    Discard {
        /// Why.
        reason: DiscardReason,
    },
}

/// Why a partial file was rejected and the body restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartReason {
    /// The record named a partial file that is not on disk.
    PartialMissing,
    /// The partial is shorter than the recorded byte count.
    PartialTruncated,
    /// The partial is longer than the recorded byte count.
    PartialLonger,
    /// The recorded byte count exceeds the recorded total.
    ReceivedExceedsTotal,
}

/// Why a record was dropped outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    /// The record's id collides with one already restored.
    DuplicateId,
    /// The record has an empty display name.
    Malformed,
}

/// Reconciles a persisted record with its partial file.
///
/// Records with empty names are discarded, and other terminal records remain terminal. A nonterminal record with an impossible total starts a new attempt; otherwise an unrecorded partial resumes at zero, a matching file resumes at `received`, and a missing or mismatched file starts a new attempt.
#[must_use]
pub fn reconcile(record: &DownloadRecord, partial: PartialFile) -> Reconciled {
    if record.name.is_empty() {
        return Reconciled::Discard {
            reason: DiscardReason::Malformed,
        };
    }

    let restored = |received: u64, attempt: Attempt, phase: Phase| {
        let mut d = Download::queued(record.id);
        d.attempt = attempt;
        d.phase = phase;
        d.received = received;
        d.total = record.total;
        // Retain `blocked` only for terminal failures; resumable states must offer a new attempt.
        d.blocked = record.blocked && phase == Phase::Failed;
        d
    };

    if record.phase.is_terminal() {
        return Reconciled::Terminal {
            download: restored(record.received, record.attempt, record.phase),
            name: record.name.clone(),
        };
    }

    let restart = |reason: RestartReason| Reconciled::RestartFromZero {
        // Advance the attempt so responses from the pre-restart generation are rejected.
        download: {
            let mut d = restored(0, record.attempt.next(), Phase::Queued);
            // Clear the stored total because the new attempt must negotiate its own length.
            d.total = None;
            d
        },
        name: record.name.clone(),
        reason,
    };

    if record.total.is_some_and(|t| record.received > t) {
        return restart(RestartReason::ReceivedExceedsTotal);
    }

    match partial {
        // With no recorded partial, restore this attempt at byte zero rather than advancing it.
        PartialFile::None => Reconciled::Resumable {
            download: restored(0, record.attempt, Phase::Paused),
            resume_at: 0,
            name: record.name.clone(),
        },
        PartialFile::Missing => restart(RestartReason::PartialMissing),
        PartialFile::Present { len } if len == record.received => Reconciled::Resumable {
            download: restored(record.received, record.attempt, Phase::Paused),
            resume_at: record.received,
            name: record.name.clone(),
        },
        PartialFile::Present { len } if len < record.received => {
            restart(RestartReason::PartialTruncated)
        }
        PartialFile::Present { .. } => restart(RestartReason::PartialLonger),
    }
}

// Encodes records in input order, with byte-faithful percent escaping for partial paths.
pub(super) fn encode(records: &[DownloadRecord]) -> String {
    let mut out = String::new();
    for r in records {
        let _ = write!(
            out,
            "id={}\tname={}\tattempt={}\tphase={}\treceived={}",
            r.id.0,
            escape(&r.name),
            r.attempt.0,
            phase_token(r.phase),
            r.received,
        );
        if let Some(t) = r.total {
            let _ = write!(out, "\ttotal={t}");
        }
        if let Some(p) = &r.partial {
            let _ = write!(out, "\tpartial={}", encode_partial(p));
        }
        // Encode `blocked` only when true; absent and non-`1` values decode as retryable, and unknown keys are ignored.
        if r.blocked {
            out.push_str("\tblocked=1");
        }
        out.push('\n');
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            other => out.push(other),
        }
    }
    out
}

// Invalid escapes reject only the containing record.
fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let a = it.next()?;
        let b = it.next()?;
        let hex = format!("{a}{b}");
        let byte = u8::from_str_radix(&hex, 16).ok()?;
        out.push(byte as char);
    }
    Some(out)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn escape_path_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' || b < 0x20 || b == 0x7f {
            let _ = write!(out, "%{b:02X}");
            i += 1;
        } else if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else {
            let rest = &bytes[i..];
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    let ch = s.chars().next().expect("non-empty slice has a char");
                    out.push(ch);
                    i += ch.len_utf8();
                }
                Err(e) if e.valid_up_to() > 0 => {
                    let v = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..v]).expect("valid_up_to is valid"));
                    i += v;
                }
                Err(_) => {
                    let _ = write!(out, "%{b:02X}");
                    i += 1;
                }
            }
        }
    }
    out
}

fn unescape_to_bytes(s: &str) -> Option<Vec<u8>> {
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hi = hex_value(*raw.get(i + 1)?)?;
            let lo = hex_value(*raw.get(i + 2)?)?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    Some(out)
}

// On Unix, persist raw path bytes so decoding preserves the path's cleanup identity.
#[cfg(unix)]
fn encode_partial(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    escape_path_bytes(p.as_os_str().as_bytes())
}

// This fallback lossily encodes path components that have no Unicode representation.
#[cfg(not(unix))]
fn encode_partial(p: &Path) -> String {
    escape_path_bytes(p.to_string_lossy().as_bytes())
}

#[cfg(unix)]
fn decode_partial(v: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let bytes = unescape_to_bytes(v)?;
    // Reject NUL because no Unix syscall can open the resulting path.
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

// This decoder rejects non-UTF-8 byte sequences instead of changing the path lossily.
#[cfg(not(unix))]
fn decode_partial(v: &str) -> Option<PathBuf> {
    let bytes = unescape_to_bytes(v)?;
    // NUL cannot appear in a real path on any platform, and `from_utf8` would
    // otherwise accept it as valid UTF-8; reject it fail-closed.
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

// Persistence tokens are independent of user-facing phase labels.
fn phase_token(p: Phase) -> &'static str {
    match p {
        Phase::Queued => "queued",
        Phase::Active => "active",
        Phase::Paused => "paused",
        Phase::Complete => "complete",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
    }
}

fn phase_from_token(t: &str) -> Option<Phase> {
    Some(match t {
        "queued" => Phase::Queued,
        "active" => Phase::Active,
        "paused" => Phase::Paused,
        "complete" => Phase::Complete,
        "failed" => Phase::Failed,
        "cancelled" => Phase::Cancelled,
        _ => return None,
    })
}

// A malformed line is dropped without discarding other valid records.
fn decode_line(line: &str) -> Option<DownloadRecord> {
    #[cfg(test)]
    PERSISTED_LINE_DECODES.with(|count| count.set(count.get() + 1));

    let mut id = None;
    let mut name = None;
    let mut attempt = None;
    let mut phase = None;
    let mut received = None;
    let mut total = None;
    let mut partial = None;
    let mut blocked = false;
    for part in line.split('\t') {
        let (k, v) = part.split_once('=')?;
        match k {
            "id" => id = v.parse::<u64>().ok().map(DownloadId),
            "name" => name = unescape(v),
            "attempt" => attempt = v.parse::<u32>().ok().map(Attempt),
            "phase" => phase = phase_from_token(v),
            "received" => received = v.parse::<u64>().ok(),
            "total" => total = Some(v.parse::<u64>().ok()?),
            "partial" => partial = Some(decode_partial(v)?),
            "blocked" => blocked = v == "1",
            _ => {}
        }
    }
    Some(DownloadRecord {
        id: id?,
        name: name?,
        attempt: attempt?,
        phase: phase?,
        received: received?,
        total,
        partial,
        blocked,
    })
}

/// Builds a persistable queue snapshot.
///
/// Active transfers become paused; nonterminal records retain admission order and terminal records retain completion order.
#[must_use]
pub fn snapshot(
    queue: &DownloadQueue,
    names: &dyn Fn(DownloadId) -> String,
    partials: &dyn Fn(DownloadId) -> Option<PathBuf>,
) -> Vec<DownloadRecord> {
    let live = queue
        .downloads
        .iter()
        .filter(|download| !download.phase.is_terminal());
    let terminal = queue
        .completion_order
        .iter()
        .filter_map(|id| queue.get(*id))
        .filter(|download| download.phase.is_terminal());
    let missing_terminal = queue.downloads.iter().filter(|download| {
        download.phase.is_terminal() && !queue.completion_order.contains(&download.id)
    });

    live.chain(terminal)
        .chain(missing_terminal)
        .map(|d| DownloadRecord {
            id: d.id,
            name: names(d.id),
            attempt: d.attempt,
            phase: if d.phase == Phase::Active {
                Phase::Paused
            } else {
                d.phase
            },
            received: d.received,
            total: d.total,
            partial: partials(d.id),
            // Persist blocked status so restored terminal failures remain nonretryable.
            blocked: d.is_blocked(),
        })
        .collect()
}

/// Writes download state atomically under the versioned header.
///
/// # Errors
///
/// Returns an error when the state cannot be durably written.
pub fn save_state(path: &Path, records: &[DownloadRecord]) -> std::io::Result<()> {
    crate::atomic_state::save_atomic(path, STATE_FORMAT_ID, STATE_VERSION, &encode(records))
}

/// The result of loading persisted download state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRestore {
    /// Records that parsed. May be empty.
    pub records: Vec<DownloadRecord>,
    /// Diagnostic note when defaults were used, or a line was dropped. `None`
    /// means a clean load.
    pub note: Option<String>,
}

/// Loads persisted download state without returning errors.
///
/// File-level refusal yields an empty restore with a diagnostic note; records that fail field decoding are dropped while valid records are retained.
#[must_use]
pub fn load_state(path: &Path) -> StateRestore {
    load_state_with_limits(path, crate::atomic_state::LineBodyLimits::PRODUCTION)
}

pub(super) fn load_state_with_limits(
    path: &Path,
    limits: crate::atomic_state::LineBodyLimits,
) -> StateRestore {
    let loaded = crate::atomic_state::load(path, STATE_FORMAT_ID, STATE_VERSION);
    let body = match &loaded {
        crate::atomic_state::StateLoad::Loaded(b) => b,
        crate::atomic_state::StateLoad::UsedDefaults { note } => {
            return StateRestore {
                records: Vec::new(),
                note: Some(note.clone()),
            };
        }
    };

    match decode_body_with_limits_and_hasher(
        body,
        limits,
        std::collections::hash_map::RandomState::new(),
    ) {
        Ok(restored) => restored,
        Err(error) => StateRestore {
            records: Vec::new(),
            note: Some(error.to_string()),
        },
    }
}

pub(super) fn decode_body_with_limits_and_hasher<S: BuildHasher>(
    body: &str,
    limits: crate::atomic_state::LineBodyLimits,
    hash_builder: S,
) -> Result<StateRestore, crate::atomic_state::LineBodyError<String>> {
    let mut records = Vec::new();
    let mut dropped = 0usize;
    let mut seen = HashSet::with_hasher(hash_builder);
    crate::atomic_state::visit_line_body(body, limits, |line| {
        match decode_line(line) {
            Some(r) if seen.insert(r.id) => records.push(r),
            _ => dropped += 1,
        }
        Ok::<(), String>(())
    })?;

    Ok(StateRestore {
        records,
        note: (dropped > 0).then(|| format!("dropped {dropped} unparseable record(s)")),
    })
}

#[cfg(test)]
thread_local! {
    static PERSISTED_LINE_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
