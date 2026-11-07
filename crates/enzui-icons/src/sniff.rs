//! Content sniffing with an explicit worker boundary, bounded reads,
//! regular-file-only access, single flight, and bounded generation-keyed
//! positive and negative caches.
//!
//! [`crate::mime`] resolves a MIME type from a filename with no filesystem
//! access; this module adds the content half and the concurrency discipline
//! that makes it safe under a UI.
//!
//! # Why an explicit worker, not `Task::perform`
//!
//! A future whose body calls [`std::fs::read`] blocks the thread that polls it,
//! which on the iced runtime is a dropped frame for every icon resolved on a
//! slow or network-mounted path. No `async` wrapper makes synchronous
//! filesystem I/O non-blocking, so the offload must be a real thread that owns
//! the blocking call: [`SniffWorker`](worker::SniffWorker).
//!
//! # Policy separated from execution
//!
//! What to cache, when to invalidate and how to coalesce duplicate in-flight
//! requests live in [`SniffCache`], a pure state machine over an injected clock
//! of generations that never spawns, reads or blocks. The blocking read sits
//! behind the [`Sniffer`] trait, so tests drive the whole policy with a counting
//! fake and no thread or disk. [`SniffWorker`](worker::SniffWorker) is the thin
//! seam that binds a real [`FsSniffer`] to a background thread.
//!
//! # What the real sniffer does
//!
//! [`FsSniffer`] reads at most [`SNIFF_READ_CAP`] bytes from the head of a file
//! (magic numbers live in the first bytes, and an unbounded read on a
//! multi-gigabyte file is a denial of service against the UI), and it sniffs
//! regular files only: a FIFO opened for reading blocks forever and a symlink is
//! a redirection primitive. It never spawns a shell helper and needs no broad
//! filesystem sandbox grant.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::handle_cache::Fingerprint;

/// Maximum number of bytes [`FsSniffer`] reads from the head of a file.
///
/// Magic numbers live in the first bytes; nothing needs the tail of a file to
/// classify it. The freedesktop magic database's longest rule is well under a
/// kilobyte, so 4 KiB is generous headroom while still bounding a hostile or
/// network-mounted multi-gigabyte file to a single small read.
pub const SNIFF_READ_CAP: usize = 4096;

/// The kind of a filesystem entry, as classified **without following
/// symlinks**. Only [`FileKind::Regular`] is ever sniffed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    /// A regular file — the only kind [`FsSniffer`] will read.
    Regular,
    /// A directory. Refused: there is nothing to sniff.
    Directory,
    /// A symbolic link. Refused: following it is a redirection primitive, and
    /// classifying the link itself is meaningless.
    Symlink,
    /// A FIFO / named pipe. Refused: opening it for reading blocks forever.
    Fifo,
    /// Any other special file (socket, block/char device). Refused.
    Other,
}

impl FileKind {
    /// Whether a file of this kind may be sniffed. Only regular files.
    pub fn is_sniffable(self) -> bool {
        matches!(self, FileKind::Regular)
    }
}

/// The reason a sniff produced no MIME determination.
///
/// Distinct from a regular file whose content matched no magic rule:
/// [`SniffOutcome::Unknown`] is a successful sniff that found nothing, while
/// this means the sniff was refused or failed before content was examined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SniffError {
    /// The path is not a regular file (symlink, directory, FIFO, device, …).
    /// The sniff was refused, not attempted — see [`SniffError::kind`].
    NotRegularFile(FileKind),
    /// The path could not be examined at all (does not exist, permission
    /// denied, or an I/O error while reading the head bytes).
    Io,
}

impl SniffError {
    /// If this error is a refusal due to file kind, the offending kind.
    pub fn kind(self) -> Option<FileKind> {
        match self {
            SniffError::NotRegularFile(k) => Some(k),
            SniffError::Io => None,
        }
    }
}

/// The result of sniffing one path's content.
///
/// A sniff yields a MIME string with [`crate::mime::Certainty::Content`]
/// certainty, resolves to nothing ([`SniffOutcome::Unknown`], a regular file
/// whose head matched no magic rule), or is refused or failed
/// ([`SniffOutcome::Failed`]). All three are cacheable: re-sniffing a file that
/// resolved to nothing on every frame is the waste the negative cache prevents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SniffOutcome {
    /// Content matched a magic rule; here is the MIME type.
    Mime(String),
    /// The file was a regular file and was read, but its head matched no magic
    /// rule: a successful negative result, cached to avoid re-reading.
    Unknown,
    /// The sniff was refused (not a regular file) or failed (I/O). Also cached
    /// negatively.
    Failed(SniffError),
}

impl SniffOutcome {
    /// The MIME type, if content matched a rule.
    pub fn mime(&self) -> Option<&str> {
        match self {
            SniffOutcome::Mime(m) => Some(m.as_str()),
            _ => None,
        }
    }

    /// Whether this outcome is a positive (content-matched) result. `false` for
    /// both `Unknown` and `Failed`, which are the negative-cache cases.
    pub fn is_positive(&self) -> bool {
        matches!(self, SniffOutcome::Mime(_))
    }
}

/// The blocking-read seam. The real implementation ([`FsSniffer`]) touches the
/// filesystem; tests inject a fake that counts invocations and returns canned
/// answers, so the entire cache/single-flight policy is exercised with no
/// thread and no disk.
///
/// A `Sniffer` must itself enforce the regular-file-only and bounded-read
/// rules: those are properties of *reading a file*, so they belong with the
/// implementation that reads, not with the cache. The cache treats a `Sniffer`
/// as an opaque `Path -> SniffOutcome` function.
pub trait Sniffer {
    /// Classify the content at `path`. Must not follow symlinks, must read at
    /// most [`SNIFF_READ_CAP`] bytes, and must return
    /// [`SniffOutcome::Failed`] for any non-regular file.
    fn sniff(&self, path: &Path) -> SniffOutcome;
}

/// The real content sniffer: bounded, regular-file-only reads of the host
/// filesystem. It classifies the entry with [`std::fs::symlink_metadata`], so a
/// symlink is seen as a symlink and never followed, refuses everything that is
/// not a regular file, then reads at most [`SNIFF_READ_CAP`] bytes and matches
/// them against `magic`.
///
/// `magic` is the injected rule set, a list of [`MagicRule`]s, which keeps this
/// type testable and leaves the on-disk database to a separate pure layer.
#[must_use]
pub struct FsSniffer {
    magic: Vec<MagicRule>,
    read_cap: usize,
}

impl FsSniffer {
    /// A sniffer with the given magic rules and the default read cap
    /// ([`SNIFF_READ_CAP`]).
    pub fn new(magic: Vec<MagicRule>) -> Self {
        Self {
            magic,
            read_cap: SNIFF_READ_CAP,
        }
    }

    /// Override the read cap, clamped to `1..=SNIFF_READ_CAP`.
    ///
    /// The lower clamp stops a mis-configured `0` turning every sniff into a
    /// zero-byte read that matches nothing; the upper clamp keeps
    /// [`SNIFF_READ_CAP`] the hard ceiling, so this may only lower it.
    /// Primarily for tests that assert the cap with a small fixture.
    pub fn with_read_cap(mut self, cap: usize) -> Self {
        self.read_cap = cap.clamp(1, SNIFF_READ_CAP);
        self
    }

    /// Classify an *open file handle*'s `FileKind` from the handle's own
    /// metadata. Because this interrogates the already-open descriptor — not
    /// a path that could be re-resolved — the classification and the read
    /// that follows apply to the same inode, closing the TOCTOU window.
    fn classify_open(file: &std::fs::File) -> Result<FileKind, SniffError> {
        let meta = file.metadata().map_err(|_| SniffError::Io)?;
        let ft = meta.file_type();
        // A handle opened with O_NOFOLLOW cannot itself be a symlink (the open
        // fails with ELOOP), so this branch covers the non-unix path where
        // O_NOFOLLOW is unavailable.
        if ft.is_symlink() {
            Ok(FileKind::Symlink)
        } else if ft.is_dir() {
            Ok(FileKind::Directory)
        } else if ft.is_file() {
            Ok(FileKind::Regular)
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileTypeExt;
                if ft.is_fifo() {
                    return Ok(FileKind::Fifo);
                }
            }
            Ok(FileKind::Other)
        }
    }

    /// Open `path` without following a final symlink and without blocking on a
    /// FIFO.
    ///
    /// On unix the open carries `O_NOFOLLOW` and `O_NONBLOCK`, and it is the
    /// single syscall from which both the kind check and the read derive, so
    /// nothing can swap the path between check and use. A symlink at the final
    /// component makes the open fail with `ELOOP`, which is surfaced as
    /// `NotRegularFile(Symlink)` rather than a generic I/O error.
    #[cfg(unix)]
    fn open_no_follow(path: &Path) -> Result<std::fs::File, SniffError> {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = libc_o_nofollow() | libc_o_nonblock();
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(path)
            .map_err(|e| {
                // ELOOP means O_NOFOLLOW refused a symlink at the final
                // component; surface it as a refusal, not a generic I/O error.
                if e.raw_os_error() == Some(ELOOP) {
                    SniffError::NotRegularFile(FileKind::Symlink)
                } else {
                    SniffError::Io
                }
            })
    }

    /// Non-unix fallback: a plain read-only open. `O_NOFOLLOW` has no portable
    /// equivalent, so the pre-open `symlink_metadata` check is the only guard
    /// there; deriving the read from a single open still narrows the window.
    #[cfg(not(unix))]
    fn open_no_follow(path: &Path) -> Result<std::fs::File, SniffError> {
        std::fs::File::open(path).map_err(|_| SniffError::Io)
    }

    /// Read at most `read_cap` bytes from the head of an already-open regular
    /// file handle. Bounded so a huge file never reads past the cap.
    fn read_head_from(&self, file: std::fs::File) -> Result<Vec<u8>, SniffError> {
        use std::io::Read;
        // `take` bounds the reader to `read_cap` bytes regardless of file size.
        let cap = self.read_cap.min(SNIFF_READ_CAP);
        let mut buf = Vec::with_capacity(cap);
        file.take(cap as u64)
            .read_to_end(&mut buf)
            .map_err(|_| SniffError::Io)?;
        Ok(buf)
    }
}

impl Sniffer for FsSniffer {
    fn sniff(&self, path: &Path) -> SniffOutcome {
        // Open the file exactly once with no-follow, no-block semantics, then
        // classify and read that same handle. A separate `symlink_metadata` and
        // `File::open` would let the checks apply to one file and the read to
        // another.
        let file = match Self::open_no_follow(path) {
            Ok(f) => f,
            Err(e) => return SniffOutcome::Failed(e),
        };
        let kind = match Self::classify_open(&file) {
            Ok(k) => k,
            Err(e) => return SniffOutcome::Failed(e),
        };
        if !kind.is_sniffable() {
            // Refused, not a successful-but-empty Unknown: the caller can tell
            // the two apart.
            return SniffOutcome::Failed(SniffError::NotRegularFile(kind));
        }
        let head = match self.read_head_from(file) {
            Ok(h) => h,
            Err(e) => return SniffOutcome::Failed(e),
        };
        match match_magic(&self.magic, &head) {
            Some(mime) => SniffOutcome::Mime(mime),
            None => SniffOutcome::Unknown,
        }
    }
}

/// `O_NOFOLLOW` flag value (refuse a symlink as the final path component).
#[cfg(unix)]
fn libc_o_nofollow() -> i32 {
    // Defined identically across Linux, the BSDs and macOS in <fcntl.h>; used
    // directly to avoid a `libc` dependency.
    #[cfg(target_os = "linux")]
    {
        0o400000
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        0x0100
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        // FreeBSD/OpenBSD/NetBSD value.
        0x0100
    }
}

/// `O_NONBLOCK` flag value (do not block opening a FIFO with no writer).
#[cfg(unix)]
fn libc_o_nonblock() -> i32 {
    #[cfg(target_os = "linux")]
    {
        0o4000
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        0x0004
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        0x0004
    }
}

/// `ELOOP` errno (too many symbolic links / symlink where not permitted).
#[cfg(unix)]
const ELOOP: i32 = {
    #[cfg(target_os = "linux")]
    {
        40
    }
    #[cfg(not(target_os = "linux"))]
    {
        62
    }
};

/// One content-magic rule: a byte pattern that must appear at, or within a
/// small range around, a fixed offset in the head of a file for it to be
/// classified as `mime`. This is the subset of freedesktop's `magic` format
/// that [`magic_parse`] extracts: fixed offset, exact bytes, an optional byte
/// mask, an optional range-length, and a priority from the section header.
///
/// The word-size (`~N`) qualifier and nested continuation rules are not
/// modelled; the parser flattens the format to depth 0 and drops indented
/// rules.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct MagicRule {
    /// The MIME type this rule assigns when it matches.
    pub mime: String,
    /// Byte offset at which `bytes` must appear (or the start of a range if
    /// [`MagicRule::range`] is non-zero).
    pub offset: usize,
    /// The exact bytes that must be present at `offset` (optionally under a
    /// [`MagicRule::mask`]).
    pub bytes: Vec<u8>,
    /// Optional byte mask and-ed with the file bytes and the value bytes
    /// before comparison. When present, its length must equal
    /// `bytes.len()`; a length mismatch is treated as "does not match" (the
    /// parser rejects such rules up front).
    pub mask: Option<Vec<u8>>,
    /// The range-length: the number of consecutive candidate start positions to
    /// check, matching freedesktop's `+N` suffix, whose default when absent is
    /// `1`.
    ///
    /// `range == 1`, the value [`MagicRule::new`] uses, means the pattern must
    /// appear at exactly `offset`. `range == N` accepts any start in the
    /// half-open window `offset .. offset+N`. A `range` of `0` is coerced to `1`
    /// at match time, so it can never mean "check nothing".
    pub range: usize,
    /// Priority carried over from the section header `[priority:mimetype]`.
    /// Higher priority wins in `match_magic`. Defaults to 50 (the fdo
    /// default) so a hand-built rule slots in the middle of a parsed database
    /// without special casing.
    pub priority: u16,
    /// The `~word-size` qualifier from the freedesktop format (default 1).
    ///
    /// The spec uses it to byte-swap the value and mask on little-endian
    /// machines for `host16`/`host32` matches. The parsed value is preserved but
    /// no swap is performed: the database is byte-swap-neutral for the `string`,
    /// `byte` and `big*`/`little*` rule types, which are the overwhelming
    /// majority.
    pub word_size: u16,
}

impl MagicRule {
    /// A rule matching `bytes` at `offset`, with no mask, a single candidate
    /// position, and the default priority (50).
    pub fn new(mime: impl Into<String>, offset: usize, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            mime: mime.into(),
            offset,
            bytes: bytes.into(),
            mask: None,
            // The spec's range-length default: a single candidate position at
            // `offset`.
            range: 1,
            priority: 50,
            word_size: 1,
        }
    }

    /// Set the `~word-size` qualifier. Stored for later host-match support;
    /// does not currently affect matching (see [`MagicRule::word_size`]).
    pub fn with_word_size(mut self, word_size: u16) -> Self {
        self.word_size = word_size;
        self
    }

    /// Set a byte mask. The mask must have the same length as `bytes` for the
    /// rule to ever match (a mismatch is treated as "does not match").
    pub fn with_mask(mut self, mask: Vec<u8>) -> Self {
        self.mask = Some(mask);
        self
    }

    /// Set the range-length (number of candidate positions to check). See
    /// [`MagicRule::range`] for the semantics: `range` positions starting at
    /// `offset`, half-open. `0` is coerced to `1` at match time.
    pub fn with_range(mut self, range: usize) -> Self {
        self.range = range;
        self
    }

    /// Set the priority (0..=100 in the freedesktop format).
    pub fn with_priority(mut self, priority: u16) -> Self {
        self.priority = priority;
        self
    }

    /// Whether this rule matches the given file head.
    ///
    /// Public so a test can drive the matcher directly; `match_magic` is the
    /// production entry point [`FsSniffer`] uses.
    pub fn matches_head(&self, head: &[u8]) -> bool {
        self.matches_head_counted(head, &mut 0)
    }

    /// Like [`MagicRule::matches_head`] but records how many candidate start
    /// positions were examined into `iters`, so a test can assert the work is
    /// bounded by the head length rather than by the rule's `range`. Both entry
    /// points share this body.
    pub fn matches_head_counted(&self, head: &[u8], iters: &mut u64) -> bool {
        // A masked rule requires mask length == value length; a mismatch, which
        // only direct construction can produce, means "does not match".
        if let Some(mask) = &self.mask
            && mask.len() != self.bytes.len()
        {
            return false;
        }

        // range-length is the NUMBER of candidate positions (spec default 1);
        // a stored 0 is coerced to 1 so it can never mean "check nothing".
        let count = self.range.max(1);

        // `range` is attacker-controlled data from an on-disk database; the head
        // length is not. A candidate start can only match if the whole pattern
        // fits, so the last examined start is clamped to what the buffer can
        // hold, with saturating arithmetic. Iterating to `offset + range`
        // instead would let a hostile rule spin billions of times on the shared
        // worker while never being able to match.
        let Some(max_fit_start) = head.len().checked_sub(self.bytes.len()) else {
            return false;
        };
        // The requested window is `offset .. offset+count` (half-open).
        // Its last start is `offset + count - 1`, saturating on overflow so a
        // hostile `offset`/`range` near `usize::MAX` cannot wrap. `count >= 1`
        // (it is `range.max(1)`), so `count - 1` never underflows.
        let requested_last = self.offset.saturating_add(count - 1);
        // Effective last start = min(requested_last, max_fit_start). If
        // `offset` itself is already past `max_fit_start`, nothing fits.
        let last_start = requested_last.min(max_fit_start);
        if self.offset > last_start {
            return false;
        }

        let mut start = self.offset;
        loop {
            *iters += 1;
            // `end` cannot overflow here: start <= max_fit_start means
            // start + bytes.len() <= head.len(), which is a valid usize.
            let end = start + self.bytes.len();
            if self.matches_at(&head[start..end]) {
                return true;
            }
            if start == last_start {
                return false;
            }
            start += 1;
        }
    }

    /// Compare `slice` (exactly `bytes.len()` long) against `bytes` under the
    /// optional mask.
    fn matches_at(&self, slice: &[u8]) -> bool {
        debug_assert_eq!(
            slice.len(),
            self.bytes.len(),
            "the window was cut to the pattern length"
        );
        match &self.mask {
            None => slice == self.bytes.as_slice(),
            Some(mask) => slice
                .iter()
                .zip(self.bytes.iter())
                .zip(mask.iter())
                .all(|((h, v), m)| (h & m) == (v & m)),
        }
    }
}

/// Match a file head against magic rules, returning the first matching MIME.
///
/// Rules are tried in slice order and the first match wins. [`magic_parse`]
/// sorts its output by descending [`MagicRule::priority`], so a parsed set is
/// already priority-ordered while a hand-built set keeps the order its author
/// encoded; nothing is re-sorted here.
fn match_magic(rules: &[MagicRule], head: &[u8]) -> Option<String> {
    rules
        .iter()
        .find(|r| r.matches_head(head))
        .map(|r| r.mime.clone())
}

/// A monotonic cache generation for global resolution inputs such as the
/// active theme or display scale. A file's on-disk fingerprint is keyed
/// independently, so replacing one path never invalidates unrelated entries.
///
/// It is a single opaque counter because the cache only asks whether an entry
/// belongs to the current global resolution world; [`Generation::bump`] is how
/// the caller signals that world changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[must_use]
pub struct Generation(u64);

impl Generation {
    /// The initial generation.
    pub const ZERO: Generation = Generation(0);

    /// The next generation. A theme or scale change calls this, invalidating
    /// every entry stamped with the old value.
    pub fn bump(self) -> Generation {
        Generation(self.0 + 1)
    }

    /// The raw counter value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl Default for Generation {
    fn default() -> Self {
        Generation::ZERO
    }
}

/// The identity of one content-sniff attempt.
///
/// An attempt binds work to the exact path, filesystem fingerprint and cache
/// generation observed by [`FileIconResolver::resolve`](crate::resolver::FileIconResolver::resolve).
/// Its monotonic nonce distinguishes retries even when all three inputs match.
/// Fields are private so dispatchers and workers can only carry the identity
/// intact, never accidentally reconstruct part of it.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SniffAttempt {
    path: PathBuf,
    fingerprint: Fingerprint,
    generation: Generation,
    nonce: u64,
}

impl SniffAttempt {
    pub(crate) fn new(
        path: PathBuf,
        fingerprint: Fingerprint,
        generation: Generation,
        nonce: u64,
    ) -> Self {
        Self {
            path,
            fingerprint,
            generation,
            nonce,
        }
    }

    /// The path whose bytes this attempt classifies.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The exact filesystem fingerprint observed before dispatch.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// The cache generation observed before dispatch.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The cache-local monotonic attempt number.
    pub fn nonce(&self) -> u64 {
        self.nonce
    }
}

/// The outcome of trying to admit a path as in-flight (see
/// [`SniffCache::mark_in_flight`]).
///
/// Admission is bounded: the in-flight table has a hard cap so a stalled or
/// backed-up worker cannot make the cache retain an arbitrary number of path
/// copies. A refused admission is a typed answer the caller must handle by
/// degrading, never by inserting the path anyway — a rejected path leaves no
/// trace in the table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    /// The exact attempt was newly recorded as in-flight; the caller must carry
    /// this identity unchanged through dispatch and completion.
    Accepted(SniffAttempt),
    /// A sniff for this path is already in flight for the current
    /// generation (single flight). Nothing was inserted or changed; the caller
    /// must not dispatch a duplicate.
    Busy,
    /// The in-flight table is at capacity. The path was not inserted — the
    /// caller must degrade rather than dispatch, and no path copy is retained.
    Full,
}

impl Admission {
    /// Whether the path was newly admitted (the only case that should dispatch).
    pub fn is_accepted(&self) -> bool {
        matches!(self, Admission::Accepted(_))
    }
}

/// Whether a lookup found a live cached answer, needs a sniff dispatched, or is
/// already being sniffed (single flight).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Lookup {
    /// A live cached outcome (positive or negative) for the current generation.
    Hit(SniffOutcome),
    /// No live entry and nothing in flight: the caller should dispatch a sniff.
    /// The path is returned so the caller can hand it to the worker.
    Miss,
    /// No live entry, but a sniff for this path is already in flight for the
    /// current generation. The caller must not dispatch a second sniff —
    /// this is the single-flight guarantee. The result will land via
    /// [`SniffCache::complete`].
    InFlight,
}

/// A bounded, fingerprint-and-generation-keyed cache with single-flight
/// coalescing of exact attempts.
///
/// The whole policy layer, and a pure state machine: it never spawns a thread,
/// opens a file or blocks. [`SniffCache::lookup`] decides hit, miss or already
/// in flight, and [`SniffCache::complete`] records a finished sniff and releases
/// the coalesced waiters.
///
/// Positive and negative entries share one capacity-bounded map with
/// least-recently-used eviction, because an unbounded path-keyed cache is a
/// memory leak driven by user input: a directory of a million uniquely named
/// files would grow it without limit. Negative entries (`Unknown` and `Failed`)
/// are cached with the same discipline, so a file that failed to resolve is not
/// re-sniffed every frame.
#[must_use]
pub struct SniffCache {
    /// The current world generation. Entries stamped older are dead.
    generation: Generation,
    /// Path -> (exact fingerprint, generation, outcome). One map for both
    /// positive and negative outcomes.
    entries: HashMap<PathBuf, (Fingerprint, Generation, SniffOutcome)>,
    /// LRU order of keys in `entries`, front = least recently used.
    lru: VecDeque<PathBuf>,
    /// One currently active attempt per path. A changed fingerprint supersedes
    /// the old path attempt without consuming another bounded slot.
    in_flight: HashMap<PathBuf, SniffAttempt>,
    /// Source of cache-local monotonic attempt nonces.
    next_nonce: u64,
    /// Maximum number of cached entries (positive + negative combined).
    capacity: usize,
    /// Maximum number of simultaneously in-flight paths. A stalled or backed-up
    /// worker must not let this map grow without bound (each entry retains a
    /// path copy), so admission is refused once the cap is reached.
    in_flight_cap: usize,
}

impl SniffCache {
    /// A cache bounded to `capacity` entries, starting at [`Generation::ZERO`].
    /// `capacity` is clamped to a minimum of 1 so an unusable zero-capacity
    /// cache — which would make every lookup a permanent miss — cannot be
    /// constructed by accident.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            generation: Generation::ZERO,
            entries: HashMap::new(),
            lru: VecDeque::new(),
            in_flight: HashMap::new(),
            next_nonce: 0,
            capacity,
            // Default the in-flight cap to the entry capacity: a sane bound for
            // concurrent unresolved paths, overridden by `with_in_flight_cap`.
            in_flight_cap: capacity,
        }
    }

    /// Override the maximum number of simultaneously in-flight paths. Clamped to
    /// a minimum of 1 so a mis-configured zero cannot make every dispatch
    /// refuse. This is the bound that makes teardown and memory use independent
    /// of a stalled worker's backlog.
    pub fn with_in_flight_cap(mut self, cap: usize) -> Self {
        self.in_flight_cap = cap.max(1);
        self
    }

    /// The maximum number of simultaneously in-flight paths.
    pub fn in_flight_cap(&self) -> usize {
        self.in_flight_cap
    }

    /// The number of paths currently marked in-flight.
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    /// The current generation.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The cache's capacity (positive + negative entries combined).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The number of currently live cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advance the generation, invalidating every existing entry and
    /// cancelling the acceptance of any in-flight sniff dispatched under the
    /// old generation.
    ///
    /// This does not eagerly clear the maps — a stale entry is simply never
    /// served, because [`SniffCache::lookup`] checks the stamp against the
    /// current generation. Clearing lazily keeps `bump` O(1); stale entries are
    /// reclaimed by ordinary LRU eviction as new ones arrive. In-flight
    /// completions for the old generation are dropped by [`SniffCache::complete`].
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.bump();
    }

    /// Look up the exact (`path`, `fingerprint`, current generation) key.
    ///
    /// - A live entry (stamped with the current generation) is a
    ///   [`Lookup::Hit`], and touching it makes it most-recently-used.
    /// - No live entry and nothing in flight is a [`Lookup::Miss`]: the caller
    ///   should dispatch a sniff and then call [`SniffCache::mark_in_flight`].
    /// - No live entry but a sniff for the same fingerprint is already in
    ///   flight for the current generation is [`Lookup::InFlight`]. A different
    ///   fingerprint is a miss and must supersede that path's old attempt.
    ///
    /// An entry stamped with an older generation is treated as absent — it
    /// is never served — which is the generation-invalidation guarantee.
    /// Likewise an in-flight marker from an older generation does not coalesce a
    /// current-generation request.
    pub fn lookup(&mut self, path: &Path, fingerprint: &Fingerprint) -> Lookup {
        // An entry stamped with an older generation is treated as absent and
        // falls through to Miss or InFlight.
        if let Some((entry_fingerprint, stamp, outcome)) = self.entries.get(path)
            && entry_fingerprint == fingerprint
            && *stamp == self.generation
        {
            let outcome = outcome.clone();
            self.touch(path);
            return Lookup::Hit(outcome);
        }
        if let Some(attempt) = self.in_flight.get(path)
            && attempt.fingerprint() == fingerprint
            && attempt.generation() == self.generation
        {
            return Lookup::InFlight;
        }
        Lookup::Miss
    }

    /// Record that a sniff for the exact path and fingerprint has been
    /// dispatched under the current generation, so matching lookups return
    /// [`Lookup::InFlight`]. A different fingerprint supersedes the old path
    /// attempt and receives a fresh nonce. This is the
    /// single-flight bookkeeping; call it exactly when a [`Lookup::Miss`] is
    /// turned into a real dispatch.
    ///
    /// The return is a typed [`Admission`]:
    ///
    /// - [`Admission::Accepted`] — newly recorded, with the identity to dispatch.
    /// - [`Admission::Busy`] — the exact path/fingerprint/generation is already
    ///   in flight; do not dispatch a duplicate.
    /// - [`Admission::Full`] — the in-flight table is at capacity; the path was
    ///   not inserted, so nothing is retained and the caller must degrade.
    ///
    /// Before deciding, entries stamped with an older generation are
    /// evicted: a generation bump makes their pending answers stale, so keeping
    /// them would let generation churn wedge the cap with dead paths. This is
    /// what keeps in-flight bounded across churn, not just at a single instant.
    #[must_use = "a Full/Busy admission must be handled; ignoring it dispatches unbounded or duplicate work"]
    pub fn mark_in_flight(&mut self, path: &Path, fingerprint: Fingerprint) -> Admission {
        self.evict_stale_in_flight();
        if let Some(active) = self.in_flight.get(path)
            && active.fingerprint() == &fingerprint
            && active.generation() == self.generation
        {
            return Admission::Busy;
        }
        // Replacing an attempt for this same path keeps the table length fixed,
        // so it is allowed at capacity. Only an unrelated path needs a slot.
        if !self.in_flight.contains_key(path) && self.in_flight.len() >= self.in_flight_cap {
            return Admission::Full;
        }
        let nonce = self.next_nonce;
        // A `u64` attempt counter never wraps in practice.
        self.next_nonce = self.next_nonce.wrapping_add(1);
        let attempt = SniffAttempt::new(path.to_path_buf(), fingerprint, self.generation, nonce);
        self.in_flight.insert(path.to_path_buf(), attempt.clone());
        Admission::Accepted(attempt)
    }

    /// Roll back an admitted attempt when dispatch fails. Only the exact active
    /// identity is removed, so a late abort cannot clobber a superseding retry.
    pub fn abort_in_flight(&mut self, attempt: &SniffAttempt) -> bool {
        if attempt.generation() == self.generation
            && self.in_flight.get(attempt.path()) == Some(attempt)
        {
            self.in_flight.remove(attempt.path());
            true
        } else {
            false
        }
    }

    /// Drop every in-flight marker stamped with a generation other than the
    /// current one. A bumped generation makes those pending answers stale (a
    /// completion for them is dropped by [`SniffCache::complete`] anyway), so
    /// they must not occupy in-flight capacity.
    fn evict_stale_in_flight(&mut self) {
        let current = self.generation;
        self.in_flight
            .retain(|_, attempt| attempt.generation() == current);
    }

    /// Record the outcome only if `attempt` is still the exact active identity.
    ///
    /// Returns `false` without mutating state for stale-generation,
    /// fingerprint-superseded, or duplicate terminal completions. Stale markers
    /// are reclaimed on the next admission by `evict_stale_in_flight`.
    ///
    /// Both positive and negative outcomes are cached identically; eviction is
    /// LRU and bounded by `capacity`.
    pub fn complete(&mut self, attempt: &SniffAttempt, outcome: SniffOutcome) -> bool {
        // Acceptance requires the exact identity to remain active. This rejects
        // superseded, stale-generation, fabricated, and duplicate terminal
        // completions before they can overwrite a newer path result.
        if self.in_flight.get(attempt.path()) != Some(attempt)
            || attempt.generation() != self.generation
        {
            return false;
        }
        self.in_flight.remove(attempt.path());
        self.insert(
            attempt.path().to_path_buf(),
            attempt.fingerprint().clone(),
            outcome,
        );
        true
    }

    /// Insert or overwrite an entry at the current generation, maintaining LRU
    /// order and evicting the least-recently-used entry when over capacity.
    fn insert(&mut self, path: PathBuf, fingerprint: Fingerprint, outcome: SniffOutcome) {
        if self.entries.contains_key(&path) {
            self.entries
                .insert(path.clone(), (fingerprint, self.generation, outcome));
            self.touch(&path);
            return;
        }
        self.entries
            .insert(path.clone(), (fingerprint, self.generation, outcome));
        self.lru.push_back(path);
        self.evict_if_needed();
    }

    /// Move `path` to the most-recently-used end of the LRU queue.
    fn touch(&mut self, path: &Path) {
        if let Some(pos) = self.lru.iter().position(|p| p.as_path() == path) {
            let p = self.lru.remove(pos).expect("position just found");
            self.lru.push_back(p);
        }
    }

    /// Evict least-recently-used entries until within capacity.
    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.capacity {
            match self.lru.pop_front() {
                Some(victim) => {
                    self.entries.remove(&victim);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests;

/// Freedesktop `magic` database parser. See the module docs for the exact
/// subset supported and the malformed-input policy.
pub mod magic_parse;

/// The explicit off-UI-thread worker: the one seam that spawns and blocks. It
/// owns a boxed [`Sniffer`] and processes sniff requests on a background thread,
/// so the UI thread never calls [`std::fs::read`].
///
/// It is thin by design: caching, single flight and invalidation live in
/// [`SniffCache`], which is tested without threads.
pub mod worker;
