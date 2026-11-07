//! Bounded random-access paging for large UTF-8 plain-text documents.
//!
//! Execute generation-tagged [`PagedTextOperation`] values off the UI thread, apply them to [`PagedTextState`], and materialize only the viewport plus bounded overscan.

#![allow(
    clippy::string_slice,
    reason = "page offsets are moved onto char boundaries before any slice"
)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Maximum retained out-of-order newline summaries; contiguous summaries are folded into the cumulative prefix.
pub const MAX_SPARSE_NEWLINE_SUMMARIES: usize = 256;

/// Maximum retained cumulative newline checkpoints.
///
/// Evicted prefixes are reconstructed from the nearest preceding checkpoint through bounded page operations.
pub const MAX_NEWLINE_CHECKPOINTS: usize = 256;

const MAX_PAGE_READ_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODED_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Configures eager reads, page size, row segmentation, decoded-cache size, and overscan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagedTextPolicy {
    /// Sources no larger than this are read in one operation.
    pub eager_cutoff: usize,
    /// Nominal random-access page size for larger sources.
    pub read_chunk_size: usize,
    /// Maximum UTF-8 bytes handed to one display-row text shaper.
    pub max_display_row_bytes: usize,
    /// Maximum decoded text retained (compact newline summaries are separate).
    pub cache_budget: usize,
    /// Rows realised above and below the visible range.
    pub overscan_rows: usize,
}

impl Default for PagedTextPolicy {
    fn default() -> Self {
        Self {
            eager_cutoff: 256 * 1024,
            read_chunk_size: 64 * 1024,
            max_display_row_bytes: 4 * 1024,
            cache_budget: 512 * 1024,
            overscan_rows: 3,
        }
    }
}

impl PagedTextPolicy {
    fn checked(self) -> Self {
        assert!(
            self.read_chunk_size > 0,
            "paged-text chunk size must be non-zero"
        );
        assert!(
            self.max_display_row_bytes >= 4,
            "paged-text row segment must fit one complete UTF-8 scalar"
        );
        assert!(
            self.max_display_row_bytes <= self.read_chunk_size,
            "paged-text row segment must fit in one chunk"
        );
        assert!(
            self.eager_cutoff <= MAX_PAGE_READ_BYTES && self.read_chunk_size <= MAX_PAGE_READ_BYTES,
            "paged-text reads must fit the per-operation allocation bound"
        );
        assert!(
            self.cache_budget
                >= self
                    .eager_cutoff
                    .max(self.read_chunk_size.saturating_add(4)),
            "paged-text cache must hold one eager page or one chunk plus boundary bytes"
        );
        assert!(
            self.cache_budget <= MAX_DECODED_CACHE_BYTES,
            "paged-text decoded cache exceeds the resident-memory bound"
        );
        self
    }
}

/// Identity of one open-document lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PagedTextIdentity(u64);

/// Monotonic token carried by every operation and result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoadGeneration(u64);

impl LoadGeneration {
    /// Numeric token, useful in logs and task keys.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Classifies source failures so callers can select bounded user-facing text without parsing backend diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PagedTextSourceErrorKind {
    /// The OS refused to read the source.
    Permission,
    /// The source no longer exists.
    NotFound,
    /// Anything else: a device error, a truncated read, a backend fault.
    Unreadable,
}

impl PagedTextSourceErrorKind {
    /// Bounded, single-line, user-safe wording for this category.
    ///
    /// A `&'static str` by construction: no errno, no path, no backend
    /// payload and no control character can reach a surface through it.
    #[must_use]
    pub fn safe_label(self) -> &'static str {
        match self {
            Self::Permission => "Permission denied",
            Self::NotFound => "The file is no longer available",
            Self::Unreadable => "The file could not be read",
        }
    }
}

/// Stores a classified source failure and its unmodified diagnostic.
///
/// [`user_summary`](Self::user_summary) is bounded UI text; [`diagnostic`](Self::diagnostic) and `Display` may contain paths, error numbers, newlines, or control characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagedTextSourceError {
    kind: PagedTextSourceErrorKind,
    /// Raw backend diagnostic, which may contain sensitive paths or control characters and is not UI-safe.
    pub message: String,
}

impl PagedTextSourceError {
    /// Build a classified failure with its own retained diagnostic.
    #[must_use]
    pub fn new(kind: PagedTextSourceErrorKind, diagnostic: impl Into<String>) -> Self {
        Self {
            kind,
            message: diagnostic.into(),
        }
    }

    /// Creates an unclassified backend failure with a retained diagnostic.
    #[must_use]
    pub fn unreadable(diagnostic: impl Into<String>) -> Self {
        Self::new(PagedTextSourceErrorKind::Unreadable, diagnostic)
    }

    /// The failure category.
    #[must_use]
    pub fn kind(&self) -> PagedTextSourceErrorKind {
        self.kind
    }

    /// The unmodified backend payload, for logs — never for a label.
    #[must_use]
    pub fn diagnostic(&self) -> &str {
        &self.message
    }

    /// Localized summary without backend diagnostics, capped at 256 non-control characters.
    #[must_use]
    pub fn user_summary(&self) -> String {
        let key = match self.kind {
            PagedTextSourceErrorKind::Permission => "enzui-source-permission-denied",
            PagedTextSourceErrorKind::NotFound => "enzui-source-not-found",
            PagedTextSourceErrorKind::Unreadable => "enzui-source-unreadable",
        };
        crate::localization::safe_text(key)
    }
}

impl fmt::Display for PagedTextSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for PagedTextSourceError {}
impl From<std::io::Error> for PagedTextSourceError {
    // Classify the I/O error while retaining its original text as the diagnostic.
    fn from(value: std::io::Error) -> Self {
        let kind = match crate::io_class::IoFailureCategory::classify(&value) {
            crate::io_class::IoFailureCategory::Permission => PagedTextSourceErrorKind::Permission,
            crate::io_class::IoFailureCategory::NotFound
            | crate::io_class::IoFailureCategory::NotADirectory => {
                PagedTextSourceErrorKind::NotFound
            }
            crate::io_class::IoFailureCategory::StorageExhausted
            | crate::io_class::IoFailureCategory::Unknown => PagedTextSourceErrorKind::Unreadable,
        };
        Self::new(kind, value.to_string())
    }
}

/// Random-access byte source. Implementations must return at most `length`
/// bytes beginning exactly at `offset`; EOF may return fewer bytes.
pub trait PagedTextSource: Send + Sync {
    /// Current source length.
    ///
    /// # Errors
    ///
    /// A [`PagedTextSourceError`] when the length cannot be read.
    fn len(&self) -> Result<u64, PagedTextSourceError>;
    /// Whether the source is empty.
    ///
    /// # Errors
    ///
    /// A [`PagedTextSourceError`] when the length cannot be read.
    fn is_empty(&self) -> Result<bool, PagedTextSourceError> {
        Ok(self.len()? == 0)
    }
    /// Read a byte range without changing a caller-visible cursor.
    ///
    /// # Errors
    ///
    /// A [`PagedTextSourceError`] when the range cannot be read.
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, PagedTextSourceError>;
    /// Exact newline count strictly before `offset`, if known without I/O or scanning.
    /// Defaults to unknown; fixed-record formatted sources can avoid historical recounts.
    fn line_prefix_hint(&self, _offset: u64) -> Option<u64> {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileSnapshot {
    device: u64,
    inode: u64,
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: u32,
    ctime_seconds: i64,
    ctime_nanoseconds: u32,
}

impl FileSnapshot {
    fn from_metadata(metadata: &Metadata) -> Self {
        let normalize_nanoseconds = |nanoseconds: i64| {
            if (0..1_000_000_000).contains(&nanoseconds) {
                nanoseconds as u32
            } else {
                0
            }
        };
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: normalize_nanoseconds(metadata.mtime_nsec()),
            ctime_seconds: metadata.ctime(),
            ctime_nanoseconds: normalize_nanoseconds(metadata.ctime_nsec()),
        }
    }
}

/// Real filesystem random-access source bound to the opened file and its initial contents.
pub struct FilePagedTextSource {
    file: Mutex<File>,
    snapshot: FileSnapshot,
}
impl FilePagedTextSource {
    /// Opens `path`; later mutation of that file is reported as an unreadable source.
    ///
    /// # Errors
    ///
    /// A [`PagedTextSourceError`] when the file cannot be opened or inspected.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PagedTextSourceError> {
        let file = File::open(path)?;
        let snapshot = FileSnapshot::from_metadata(&file.metadata()?);
        Ok(Self {
            file: Mutex::new(file),
            snapshot,
        })
    }

    // Recovering is sound because each operation seeks absolutely and validates the file snapshot.
    fn file(&self) -> std::sync::MutexGuard<'_, File> {
        self.file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn validate(&self, file: &File) -> Result<(), PagedTextSourceError> {
        if FileSnapshot::from_metadata(&file.metadata()?) == self.snapshot {
            Ok(())
        } else {
            Err(PagedTextSourceError::unreadable(
                "paged-text file changed after it was opened",
            ))
        }
    }
}
impl PagedTextSource for FilePagedTextSource {
    fn len(&self) -> Result<u64, PagedTextSourceError> {
        let file = self.file();
        self.validate(&file)?;
        Ok(self.snapshot.len)
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, PagedTextSourceError> {
        let mut file = self.file();
        self.validate(&file)?;
        let remaining = self.snapshot.len.saturating_sub(offset);
        let length = length.min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if length > MAX_PAGE_READ_BYTES.saturating_add(7) {
            return Err(PagedTextSourceError::unreadable(
                "paged-text read exceeds the allocation bound",
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut out = vec![0; length];
        let mut used = 0;
        while used < length {
            let n = file.read(&mut out[used..])?;
            if n == 0 {
                break;
            }
            used += n;
        }
        self.validate(&file)?;
        out.truncate(used);
        Ok(out)
    }
}

/// In-memory source with deterministic read counters.
#[derive(Clone)]
pub struct MemoryPagedTextSource {
    bytes: Arc<[u8]>,
    reads: Arc<AtomicU64>,
    bytes_read: Arc<AtomicU64>,
}
impl MemoryPagedTextSource {
    /// Constructs a source from bytes without validating UTF-8.
    #[must_use]
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
            reads: Arc::new(AtomicU64::new(0)),
            bytes_read: Arc::new(AtomicU64::new(0)),
        }
    }
    /// Number of `read_at` calls.
    #[must_use]
    pub fn read_count(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }
    /// Bytes actually returned.
    #[must_use]
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }
}
impl PagedTextSource for MemoryPagedTextSource {
    fn len(&self) -> Result<u64, PagedTextSourceError> {
        Ok(self.bytes.len() as u64)
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, PagedTextSourceError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.bytes.len());
        let end = start.saturating_add(length).min(self.bytes.len());
        let out = self.bytes[start..end].to_vec();
        self.bytes_read
            .fetch_add(out.len() as u64, Ordering::Relaxed);
        Ok(out)
    }
}

struct Barrier {
    released: Mutex<bool>,
    entered: Condvar,
    release: Condvar,
    pending: AtomicBool,
}
/// In-memory source whose reads wait at a controllable barrier.
#[derive(Clone)]
pub struct BarrierPagedTextSource {
    inner: MemoryPagedTextSource,
    barrier: Arc<Barrier>,
}
impl BarrierPagedTextSource {
    /// Construct a closed barrier.
    #[must_use]
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            inner: MemoryPagedTextSource::new(bytes),
            barrier: Arc::new(Barrier {
                released: Mutex::new(false),
                entered: Condvar::new(),
                release: Condvar::new(),
                pending: AtomicBool::new(false),
            }),
        }
    }
    /// Wait until a worker is blocked in `read_at`.
    pub fn wait_until_pending(&self) {
        let mut released = self
            .barrier
            .released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !self.barrier.pending.load(Ordering::Acquire) {
            released = self
                .barrier
                .entered
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    /// Release all pending and future reads.
    pub fn release(&self) {
        *self
            .barrier
            .released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.barrier.release.notify_all();
    }
}
impl PagedTextSource for BarrierPagedTextSource {
    fn len(&self) -> Result<u64, PagedTextSourceError> {
        self.inner.len()
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, PagedTextSourceError> {
        let mut released = self
            .barrier
            .released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.barrier.pending.store(true, Ordering::Release);
        self.barrier.entered.notify_all();
        while !*released {
            released = self
                .barrier
                .release
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(released);
        self.inner.read_at(offset, length)
    }
}

/// Typed open/load failure. Invalid UTF-8 is never replaced or made lossy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PagedTextOpenFailure {
    /// Source metadata or byte read failed.
    Source(PagedTextSourceError),
    /// The loaded source range contains malformed UTF-8.
    InvalidUtf8 {
        /// Absolute byte offset of the malformed sequence.
        offset: u64,
    },
}

/// A decoded page covering one owned byte range.
#[derive(Debug, Clone)]
pub struct PagedTextPage {
    /// Absolute first owned byte.
    pub byte_start: u64,
    /// Absolute exclusive end byte.
    pub byte_end: u64,
    text: Arc<str>,
    newline_count: u32,
    starts_in_line: bool,
    chunk_index: u64,
}
impl PagedTextPage {
    /// Decoded bytes owned by this page.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Number of logical newline terminators in the page.
    #[must_use]
    pub fn newline_count(&self) -> usize {
        self.newline_count as usize
    }
    fn retained_bytes(&self) -> usize {
        self.text.len()
    }
}

/// Successful or failed operation payload, always generation tagged.
#[derive(Debug, Clone)]
pub struct PagedTextResult {
    /// Document lifetime this belongs to.
    pub identity: PagedTextIdentity,
    /// Request generation this belongs to.
    pub generation: LoadGeneration,
    /// Page result.
    pub outcome: Result<PagedTextPage, PagedTextOpenFailure>,
    /// Bytes returned by the source for this operation.
    pub bytes_read: usize,
}

/// Sendable random-access operation. Execute this off the UI thread.
#[derive(Clone)]
pub struct PagedTextOperation {
    identity: PagedTextIdentity,
    generation: LoadGeneration,
    source: Arc<dyn PagedTextSource>,
    source_len: u64,
    policy: PagedTextPolicy,
    anchor: u64,
}
impl PagedTextOperation {
    /// Open-document lifetime this operation targets.
    #[must_use]
    pub fn identity(&self) -> PagedTextIdentity {
        self.identity
    }
    /// Operation generation.
    #[must_use]
    pub fn generation(&self) -> LoadGeneration {
        self.generation
    }
    /// Perform the bounded read and decode.
    #[must_use]
    pub fn execute(self) -> PagedTextResult {
        let eager = usize::try_from(self.source_len)
            .ok()
            .is_some_and(|len| len <= self.policy.eager_cutoff);
        let chunk = if eager {
            usize::try_from(self.source_len.max(1)).unwrap_or(self.policy.read_chunk_size)
        } else {
            self.policy.read_chunk_size
        };
        let chunk_index = self.anchor.min(self.source_len.saturating_sub(1)) / chunk as u64;
        let core_start = chunk_index.saturating_mul(chunk as u64);
        let core_end = core_start.saturating_add(chunk as u64).min(self.source_len);
        let read_start = core_start.saturating_sub(3);
        let read_end = core_end.saturating_add(4).min(self.source_len);
        let requested = usize::try_from(read_end - read_start).unwrap_or(usize::MAX);
        let raw = match self.source.read_at(read_start, requested) {
            Ok(raw) => raw,
            Err(e) => {
                return PagedTextResult {
                    identity: self.identity,
                    generation: self.generation,
                    outcome: Err(PagedTextOpenFailure::Source(e)),
                    bytes_read: 0,
                };
            }
        };
        let bytes_read = raw.len();
        // A read that stops short of what the source reported is its end too.
        let at_end = read_end >= self.source_len || raw.len() < requested;
        let outcome = decode_page(&raw, read_start, core_start, core_end, chunk_index, at_end);
        PagedTextResult {
            identity: self.identity,
            generation: self.generation,
            outcome,
            bytes_read,
        }
    }
}

/// Decodes the page `core_start..core_end` from `raw`, read from `read_start`
/// with up to three bytes before and four after. A character cut off at the
/// end of `raw` is tolerated only when it lies wholly in the lookahead and
/// the source goes on (`!at_end`); cut off in the page itself, or at the
/// source's real end, it is invalid UTF-8 at its offset.
fn decode_page(
    raw: &[u8],
    read_start: u64,
    core_start: u64,
    core_end: u64,
    chunk_index: u64,
    at_end: bool,
) -> Result<PagedTextPage, PagedTextOpenFailure> {
    let mut lead = 0;
    if read_start > 0 {
        while lead < raw.len() && lead < 3 && raw[lead] & 0b1100_0000 == 0b1000_0000 {
            lead += 1;
        }
    }
    let candidate = &raw[lead..];
    let valid_len = match std::str::from_utf8(candidate) {
        Ok(_) => candidate.len(),
        Err(e)
            if e.error_len().is_none()
                && !at_end
                && read_start + (lead + e.valid_up_to()) as u64 >= core_end =>
        {
            e.valid_up_to()
        }
        Err(e) => {
            return Err(PagedTextOpenFailure::InvalidUtf8 {
                offset: read_start + lead as u64 + e.valid_up_to() as u64,
            });
        }
    };
    let valid = std::str::from_utf8(&candidate[..valid_len]).map_err(|e| {
        PagedTextOpenFailure::InvalidUtf8 {
            offset: read_start + lead as u64 + e.valid_up_to() as u64,
        }
    })?;
    let absolute_valid_start = read_start + lead as u64;
    let mut start = usize::try_from(core_start.saturating_sub(absolute_valid_start))
        .unwrap_or(usize::MAX)
        .min(valid.len());
    while start < valid.len() && !valid.is_char_boundary(start) {
        start += 1;
    }
    let target_end = usize::try_from(core_end.saturating_sub(absolute_valid_start))
        .unwrap_or(usize::MAX)
        .min(valid.len());
    let mut end = target_end;
    while end < valid.len() && !valid.is_char_boundary(end) {
        end += 1;
    }
    if start < valid.len()
        && valid.as_bytes()[start] == b'\n'
        && start > 0
        && valid.as_bytes()[start - 1] == b'\r'
    {
        start += 1;
    }
    if end > start
        && end < valid.len()
        && valid.as_bytes()[end - 1] == b'\r'
        && valid.as_bytes()[end] == b'\n'
    {
        end += 1;
    }
    let owned = &valid[start..end];
    let mut newline_count = 0u32;
    let bytes = owned.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            newline_count += 1;
            i += 1;
        } else if bytes[i] == b'\r' {
            newline_count += 1;
            i += if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                2
            } else {
                1
            };
        } else {
            i += 1;
        }
    }
    let starts_in_line = absolute_valid_start + start as u64 > 0 && {
        let preceding = if start > 0 {
            Some(valid.as_bytes()[start - 1])
        } else if lead > 0 {
            Some(raw[lead - 1])
        } else {
            None
        };
        !matches!(preceding, Some(b'\n' | b'\r'))
    };
    Ok(PagedTextPage {
        byte_start: absolute_valid_start + start as u64,
        byte_end: absolute_valid_start + end as u64,
        text: Arc::from(owned),
        newline_count,
        starts_in_line,
        chunk_index,
    })
}

#[derive(Debug, Clone)]
struct CacheEntry {
    page: PagedTextPage,
    tick: u64,
    line_prefix: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct SparseNewlineSummary {
    chunk: u64,
    newline_count: u32,
    tick: u64,
}

#[derive(Debug, Clone, Copy)]
struct NewlineCheckpoint {
    // Chunk whose first line follows `newline_prefix` newlines.
    chunk: u64,
    newline_prefix: u64,
    tick: u64,
}

#[derive(Debug, Clone, Copy)]
struct HistoricalRecount {
    target_chunk: u64,
    next_chunk: u64,
    newline_prefix: u64,
}

/// Reports cumulative accepted I/O and decoded-cache accounting; frame row counts live on [`PagedTextFrame`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PagedTextCounters {
    /// Source bytes from accepted successful page results since open or refresh.
    pub bytes_read: u64,
    /// Decoded text currently cached.
    pub decoded_cache_bytes: usize,
}

/// Contains materialized rows, the visible subrange, and per-frame row counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagedTextFrame {
    /// Visible plus overscan rows, and no other document text.
    pub rows: Vec<PagedTextRow>,
    /// Range within [`rows`](Self::rows) that is actually visible, excluding overscan.
    ///
    /// Accessibility and scrollbar ranges must derive from this slice.
    pub visible: std::ops::Range<usize>,
    /// Number of display rows materialized in this frame.
    pub materialized_rows: usize,
    /// Number of display rows offered to the text shaper.
    pub shaped_rows: usize,
}

impl PagedTextFrame {
    /// The on-screen rows only, with overscan excluded.
    #[must_use]
    pub fn visible_rows(&self) -> &[PagedTextRow] {
        self.rows.get(self.visible.clone()).unwrap_or(&[])
    }
}

/// Outcome of a viewport [`resolve`](PagedTextViewport::resolve) call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigateOutcome {
    /// No fetch is needed because the target is cached or no loadable document is available.
    Ready,
    /// The target page is uncached; dispatch [`PagedTextState::request`] for this byte anchor.
    NeedsFetch(u64),
}

/// Stateful document/cache endpoint living on the UI thread.
pub struct PagedTextState {
    policy: PagedTextPolicy,
    identity: Option<PagedTextIdentity>,
    generation: LoadGeneration,
    next_identity: u64,
    source: Option<Arc<dyn PagedTextSource>>,
    source_len: u64,
    cache: BTreeMap<u64, CacheEntry>,
    indexed_prefix_chunks: u64,
    indexed_prefix_newlines: u64,
    sparse_summaries: Vec<SparseNewlineSummary>,
    newline_checkpoints: Vec<NewlineCheckpoint>,
    historical_recount: Option<HistoricalRecount>,
    tick: u64,
    counters: PagedTextCounters,
    failure: Option<PagedTextOpenFailure>,
}
impl PagedTextState {
    /// Creates an empty state with the supplied policy.
    ///
    /// # Panics
    /// Panics if a read exceeds 16 MiB, the decoded cache exceeds 64 MiB, the chunk size is zero, the row-byte maximum is outside `4..=read_chunk_size`, or the cache cannot hold the larger of the eager cutoff and one chunk plus boundary bytes.
    #[must_use]
    pub fn new(policy: PagedTextPolicy) -> Self {
        Self {
            policy: policy.checked(),
            identity: None,
            generation: LoadGeneration(0),
            next_identity: 1,
            source: None,
            source_len: 0,
            cache: BTreeMap::new(),
            indexed_prefix_chunks: 0,
            indexed_prefix_newlines: 0,
            sparse_summaries: Vec::new(),
            newline_checkpoints: Vec::new(),
            historical_recount: None,
            tick: 0,
            counters: PagedTextCounters::default(),
            failure: None,
        }
    }
    fn advance(&mut self) -> LoadGeneration {
        self.generation.0 = self
            .generation
            .0
            .checked_add(1)
            .expect("paged-text generation overflow");
        self.generation
    }
    /// Open a new document and return its first off-thread operation.
    ///
    /// # Errors
    ///
    /// A [`PagedTextOpenFailure`] when the source length cannot be read.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: an open source always yields its first \
                  operation"
    )]
    pub fn open(
        &mut self,
        source: Arc<dyn PagedTextSource>,
    ) -> Result<PagedTextOperation, PagedTextOpenFailure> {
        let len = source.len().map_err(PagedTextOpenFailure::Source)?;
        self.identity = Some(PagedTextIdentity(self.next_identity));
        self.next_identity += 1;
        self.source = Some(source);
        self.source_len = len;
        self.cache.clear();
        self.reset_line_index();
        self.failure = None;
        self.counters = PagedTextCounters::default();
        Ok(self.operation_for(0).expect("just opened"))
    }
    /// Refresh the same source, invalidating every earlier result.
    ///
    /// # Errors
    ///
    /// A [`PagedTextOpenFailure`] when there is no source or its length cannot be read.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: an open source always yields its first \
                  operation"
    )]
    pub fn refresh(&mut self) -> Result<PagedTextOperation, PagedTextOpenFailure> {
        let source = self.source.clone().ok_or_else(|| {
            PagedTextOpenFailure::Source(PagedTextSourceError::unreadable(
                "no open paged-text source",
            ))
        })?;
        self.source_len = source.len().map_err(PagedTextOpenFailure::Source)?;
        self.cache.clear();
        self.reset_line_index();
        self.failure = None;
        self.counters = PagedTextCounters::default();
        Ok(self.operation_for(0).expect("source remains open"))
    }
    /// Close and invalidate pending operations.
    pub fn close(&mut self) {
        self.advance();
        self.identity = None;
        self.source = None;
        self.source_len = 0;
        self.cache.clear();
        self.reset_line_index();
        self.failure = None;
        self.counters = PagedTextCounters::default();
    }
    fn reset_line_index(&mut self) {
        self.indexed_prefix_chunks = 0;
        self.indexed_prefix_newlines = 0;
        self.tick = 0;
        // Replace index storage so document-lifetime changes release its allocations.
        self.sparse_summaries = Vec::new();
        self.newline_checkpoints = Vec::new();
        self.historical_recount = None;
    }
    /// Creates a superseding operation around `anchor`.
    ///
    /// Returns `None` while closed or after a terminal failure until open or refresh. Evicted historical prefixes are rebuilt one bounded chunk at a time.
    #[must_use]
    pub fn request(&mut self, anchor: u64) -> Option<PagedTextOperation> {
        if self.failure.is_some() {
            return None;
        }
        let target_chunk =
            anchor.min(self.source_len.saturating_sub(1)) / self.effective_chunk_size() as u64;
        let anchor = self
            .recount_chunk_for(target_chunk)
            .saturating_mul(self.effective_chunk_size() as u64);
        self.operation_for(anchor)
    }

    fn recount_chunk_for(&mut self, target_chunk: u64) -> u64 {
        if self.source_line_prefix(target_chunk).is_some() {
            self.historical_recount = None;
            return target_chunk;
        }
        let has_exact_cached_prefix = self
            .cache
            .get(&target_chunk)
            .is_some_and(|entry| entry.line_prefix.is_some());
        if target_chunk >= self.indexed_prefix_chunks || has_exact_cached_prefix {
            self.historical_recount = None;
            return target_chunk;
        }

        if self
            .historical_recount
            .is_none_or(|recount| recount.target_chunk != target_chunk)
        {
            let checkpoint = self
                .newline_checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.chunk <= target_chunk)
                .max_by_key(|checkpoint| checkpoint.chunk);
            self.historical_recount = Some(HistoricalRecount {
                target_chunk,
                next_chunk: checkpoint.map_or(0, |checkpoint| checkpoint.chunk),
                newline_prefix: checkpoint.map_or(0, |checkpoint| checkpoint.newline_prefix),
            });
        }

        self.historical_recount
            .map_or(target_chunk, |recount| recount.next_chunk)
    }
    fn operation_for(&mut self, anchor: u64) -> Option<PagedTextOperation> {
        let source = self.source.clone()?;
        let identity = self.identity?;
        let generation = self.advance();
        Some(PagedTextOperation {
            identity,
            generation,
            source,
            source_len: self.source_len,
            policy: self.policy,
            anchor,
        })
    }
    /// Apply only the current document's latest generation. Returns `false`
    /// for close-, refresh-, or overlap-stale results without mutating cache.
    pub fn apply(&mut self, result: PagedTextResult) -> bool {
        if self.identity != Some(result.identity) || self.generation != result.generation {
            return false;
        }
        match result.outcome {
            Err(failure) => {
                self.cache.clear();
                self.reset_line_index();
                self.counters.decoded_cache_bytes = 0;
                self.failure = Some(failure);
            }
            Ok(page) => {
                self.tick = self.tick.saturating_add(1);
                self.counters.bytes_read = self
                    .counters
                    .bytes_read
                    .saturating_add(result.bytes_read as u64);
                let chunk = page.chunk_index;
                let prior_prefix = self.cache.get(&chunk).and_then(|entry| entry.line_prefix);
                let recounting = self
                    .historical_recount
                    .is_some_and(|recount| recount.next_chunk == chunk);
                let line_prefix = if recounting {
                    self.record_historical_recount(chunk, page.newline_count)
                } else {
                    self.record_newline_summary(chunk, page.newline_count)
                }
                .or(prior_prefix);
                let line_prefix = self.source_line_prefix(chunk).or(line_prefix);
                self.cache.insert(
                    chunk,
                    CacheEntry {
                        page,
                        tick: self.tick,
                        line_prefix,
                    },
                );
                self.evict();
            }
        }
        true
    }
    fn record_historical_recount(&mut self, chunk: u64, newline_count: u32) -> Option<u64> {
        let recount = self.historical_recount.as_mut()?;
        if recount.next_chunk != chunk {
            return None;
        }
        let line_prefix = recount.newline_prefix.saturating_add(1);
        recount.newline_prefix = recount
            .newline_prefix
            .saturating_add(u64::from(newline_count));
        recount.next_chunk = recount.next_chunk.saturating_add(1);
        let checkpoint = (recount.next_chunk, recount.newline_prefix);
        let complete = recount.next_chunk > recount.target_chunk;
        self.retain_newline_checkpoint(checkpoint.0, checkpoint.1);
        if complete {
            self.historical_recount = None;
        }
        Some(line_prefix)
    }

    fn record_newline_summary(&mut self, chunk: u64, newline_count: u32) -> Option<u64> {
        if chunk < self.indexed_prefix_chunks {
            // This immutable chunk was already folded into the prefix. Its
            // cached line prefix, when present, is preserved by `apply`.
            return None;
        }

        if chunk > self.indexed_prefix_chunks {
            if let Some(summary) = self
                .sparse_summaries
                .iter_mut()
                .find(|summary| summary.chunk == chunk)
            {
                summary.newline_count = newline_count;
                summary.tick = self.tick;
                return None;
            }

            if self.sparse_summaries.is_empty() {
                self.sparse_summaries = Vec::with_capacity(MAX_SPARSE_NEWLINE_SUMMARIES);
            } else if self.sparse_summaries.len() == MAX_SPARSE_NEWLINE_SUMMARIES {
                let victim = self
                    .sparse_summaries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, summary)| (summary.tick, summary.chunk))
                    .map(|(index, _)| index)?;
                self.sparse_summaries.swap_remove(victim);
            }
            self.sparse_summaries.push(SparseNewlineSummary {
                chunk,
                newline_count,
                tick: self.tick,
            });
            return None;
        }

        let line_prefix = self.indexed_prefix_newlines.saturating_add(1);
        self.indexed_prefix_newlines = self
            .indexed_prefix_newlines
            .saturating_add(u64::from(newline_count));
        self.indexed_prefix_chunks = self.indexed_prefix_chunks.saturating_add(1);
        self.retain_newline_checkpoint(self.indexed_prefix_chunks, self.indexed_prefix_newlines);

        // Consume at most the sparse-summary allowance and publish exact prefixes to cached pages while extending the cumulative prefix.
        while let Some(index) = self
            .sparse_summaries
            .iter()
            .position(|summary| summary.chunk == self.indexed_prefix_chunks)
        {
            let summary = self.sparse_summaries.swap_remove(index);
            if let Some(entry) = self.cache.get_mut(&summary.chunk) {
                entry.line_prefix = Some(self.indexed_prefix_newlines.saturating_add(1));
            }
            self.indexed_prefix_newlines = self
                .indexed_prefix_newlines
                .saturating_add(u64::from(summary.newline_count));
            self.indexed_prefix_chunks = self.indexed_prefix_chunks.saturating_add(1);
            self.retain_newline_checkpoint(
                self.indexed_prefix_chunks,
                self.indexed_prefix_newlines,
            );
        }
        Some(line_prefix)
    }

    fn retain_newline_checkpoint(&mut self, chunk: u64, newline_prefix: u64) {
        if let Some(checkpoint) = self
            .newline_checkpoints
            .iter_mut()
            .find(|checkpoint| checkpoint.chunk == chunk)
        {
            checkpoint.newline_prefix = newline_prefix;
            checkpoint.tick = self.tick;
            return;
        }

        if self.newline_checkpoints.is_empty() {
            self.newline_checkpoints = Vec::with_capacity(MAX_NEWLINE_CHECKPOINTS);
        } else if self.newline_checkpoints.len() == MAX_NEWLINE_CHECKPOINTS {
            let victim = self
                .newline_checkpoints
                .iter()
                .enumerate()
                .min_by_key(|(_, checkpoint)| (checkpoint.tick, checkpoint.chunk))
                .map(|(index, _)| index)
                .expect("bounded newline checkpoints are non-empty");
            self.newline_checkpoints.swap_remove(victim);
        }
        self.newline_checkpoints.push(NewlineCheckpoint {
            chunk,
            newline_prefix,
            tick: self.tick,
        });
    }

    fn evict(&mut self) {
        while self
            .cache
            .values()
            .map(|e| e.page.retained_bytes())
            .sum::<usize>()
            > self.policy.cache_budget
            && self.cache.len() > 1
        {
            let victim = self
                .cache
                .iter()
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| *k)
                .expect("non-empty");
            self.cache.remove(&victim);
        }
        self.counters.decoded_cache_bytes =
            self.cache.values().map(|e| e.page.retained_bytes()).sum();
    }
    /// Current typed failure, if any.
    #[must_use]
    pub fn failure(&self) -> Option<&PagedTextOpenFailure> {
        self.failure.as_ref()
    }
    /// Returns the current open-document lifetime, or `None` while closed.
    #[must_use]
    pub fn identity(&self) -> Option<PagedTextIdentity> {
        self.identity
    }
    /// Latest issued generation, including close invalidations.
    #[must_use]
    pub fn generation(&self) -> LoadGeneration {
        self.generation
    }
    /// Source length without reading content.
    #[must_use]
    pub fn source_len(&self) -> u64 {
        self.source_len
    }
    /// Active paging policy (including the navigation chunk size).
    #[must_use]
    pub fn policy(&self) -> PagedTextPolicy {
        self.policy
    }
    /// Current mechanism counters.
    #[must_use]
    pub fn counters(&self) -> PagedTextCounters {
        self.counters
    }
    /// Returns the fraction of effective chunks whose newline summaries are known.
    ///
    /// The value is monotonic within a document lifetime and measures index coverage, not loaded content or generic progress.
    #[must_use]
    pub fn index_progress(&self) -> f32 {
        if self.source_len == 0 {
            return 1.0;
        }
        let chunks = self.line_index_page_count();
        let accounted = self
            .indexed_prefix_chunks
            .saturating_add(self.sparse_summaries.len() as u64);
        (accounted as f32 / chunks as f32).min(1.0)
    }

    /// Returns whether every source chunk contributes to the contiguous newline index.
    ///
    /// Completion is independent of whether the current page is cached; evicted prefixes remain recoverable through bounded operations.
    #[must_use]
    pub fn line_index_complete(&self) -> bool {
        self.source_len == 0 || self.indexed_prefix_chunks == self.line_index_page_count()
    }

    fn line_index_page_count(&self) -> u64 {
        self.source_len.div_ceil(self.effective_chunk_size() as u64)
    }

    fn effective_chunk_size(&self) -> usize {
        let eager = usize::try_from(self.source_len)
            .ok()
            .is_some_and(|len| len <= self.policy.eager_cutoff);
        if eager {
            usize::try_from(self.source_len.max(1)).unwrap_or(self.policy.read_chunk_size)
        } else {
            self.policy.read_chunk_size
        }
    }

    /// Whether a page covering the given byte is currently cached.
    #[must_use]
    pub fn has_page(&self, byte: u64) -> bool {
        self.page_for(byte).is_some()
    }
    fn page_for(&self, byte: u64) -> Option<&PagedTextPage> {
        let chunk = self.effective_chunk_size();
        self.cache
            .get(&(byte.min(self.source_len.saturating_sub(1)) / chunk as u64))
            .map(|e| &e.page)
    }
    fn source_line_prefix(&self, chunk: u64) -> Option<u64> {
        let byte = chunk.checked_mul(self.effective_chunk_size() as u64)?;
        self.source.as_ref()?.line_prefix_hint(byte)
    }
    fn line_prefix(&self, chunk: u64) -> Option<u64> {
        self.source_line_prefix(chunk)
            .or_else(|| self.cache.get(&chunk).and_then(|entry| entry.line_prefix))
    }
}

/// One bounded, owned row ready for a viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagedTextRow {
    /// Absolute byte range represented by this display segment.
    pub byte_range: std::ops::Range<u64>,
    /// Exact logical line number when preceding chunks have been indexed.
    pub line_number: Option<u64>,
    /// Whether this is a continuation segment of the same logical line.
    pub continuation: bool,
    /// UTF-8 display text, never longer than the policy segment maximum.
    pub text: String,
}

/// Controlled viewport for a page-backed read-only document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagedTextViewport {
    /// Absolute byte anchor selecting the current page.
    pub anchor_byte: u64,
    /// First visible display row within that page.
    pub first_row: usize,
    /// Update-time estimate of visible rows, excluding overscan.
    ///
    /// Painting and accessibility use the measured count passed to [`materialize_rows`](Self::materialize_rows), not this field.
    pub visible_rows: usize,
}
const SEEK_ROW: usize = usize::MAX - 1;
impl PagedTextViewport {
    /// Construct a viewport at the document start.
    #[must_use]
    pub fn new(visible_rows: usize) -> Self {
        Self {
            anchor_byte: 0,
            first_row: 0,
            visible_rows,
        }
    }
    /// Applies keyboard or wheel movement; upward underflow moves to the previous page anchor, while [`resolve`](Self::resolve) handles downward overflow and uncached targets.
    pub fn navigate(&mut self, movement: PagedTextNavigation, source_len: u64, chunk_size: usize) {
        let page = chunk_size as u64;
        match movement {
            PagedTextNavigation::LineUp => {
                if self.first_row > 0 {
                    self.first_row -= 1;
                } else if self.anchor_byte > 0 {
                    self.anchor_byte = self.anchor_byte.saturating_sub(page);
                    self.first_row = usize::MAX;
                }
            }
            PagedTextNavigation::LineDown => self.first_row = self.first_row.saturating_add(1),
            PagedTextNavigation::PageUp => {
                if self.first_row >= self.visible_rows {
                    self.first_row -= self.visible_rows;
                } else if self.anchor_byte > 0 {
                    self.anchor_byte = self.anchor_byte.saturating_sub(page);
                    self.first_row = usize::MAX;
                } else {
                    self.first_row = 0;
                }
            }
            PagedTextNavigation::PageDown => {
                let next = self
                    .anchor_byte
                    .saturating_add(page)
                    .min(source_len.saturating_sub(1));
                if next > self.anchor_byte {
                    self.anchor_byte = next;
                    self.first_row = 0;
                } else if self.first_row != usize::MAX {
                    // Keep advancing inside the final page instead of jumping backwards.
                    self.first_row = self.first_row.saturating_add(self.visible_rows.max(1));
                }
            }
            PagedTextNavigation::Home => {
                self.anchor_byte = 0;
                self.first_row = 0;
            }
            PagedTextNavigation::End => {
                self.anchor_byte = source_len.saturating_sub(1);
                self.first_row = usize::MAX;
            }
            PagedTextNavigation::SeekByte(byte) => {
                self.anchor_byte = byte.min(source_len.saturating_sub(1));
                self.first_row = if byte >= source_len.saturating_sub(1) {
                    usize::MAX
                } else {
                    SEEK_ROW
                };
            }
            PagedTextNavigation::Wheel(lines) => {
                if lines < 0 {
                    let up = lines.unsigned_abs() as usize;
                    if self.first_row >= up {
                        self.first_row -= up;
                    } else if self.anchor_byte > 0 {
                        self.anchor_byte = self.anchor_byte.saturating_sub(page);
                        self.first_row = usize::MAX;
                    } else {
                        self.first_row = 0;
                    }
                } else {
                    self.first_row = self.first_row.saturating_add(lines as usize);
                }
            }
        }
    }
    /// Normalizes the viewport after navigation or cache changes.
    ///
    /// Returns [`NavigateOutcome::NeedsFetch`] when the target page is not cached.
    pub fn resolve(&mut self, state: &PagedTextState) -> NavigateOutcome {
        let chunk = state.effective_chunk_size();
        let Some(page) = state.page_for(self.anchor_byte) else {
            // An accepted terminal failure resolves as ready until open or refresh clears it.
            return if state.identity().is_some() && state.failure().is_none() {
                NavigateOutcome::NeedsFetch(self.anchor_byte)
            } else {
                NavigateOutcome::Ready
            };
        };
        let all_len = row_spans(page, state.policy.max_display_row_bytes).len();
        if self.first_row == SEEK_ROW {
            self.first_row = self.seek_row(page, state.policy.max_display_row_bytes);
        }
        // Stop the final page with its last row at the viewport bottom.
        let last_page = page.byte_end >= state.source_len;
        if last_page && all_len == 0 && page.chunk_index > 0 {
            // A final scalar or CRLF can be wholly owned by the preceding page.
            self.anchor_byte = (page.chunk_index - 1).saturating_mul(chunk as u64);
            self.first_row = usize::MAX;
            return self.resolve(state);
        }
        let max_first = if last_page {
            all_len.saturating_sub(self.visible_rows.max(1))
        } else {
            all_len
        };
        if self.first_row != usize::MAX && all_len > 0 && self.first_row >= all_len {
            let overflow = self.first_row - all_len;
            let next_anchor = self.anchor_byte.saturating_add(chunk as u64);
            if next_anchor < state.source_len {
                self.anchor_byte = next_anchor;
                self.first_row = overflow;
                if !state.has_page(self.anchor_byte) {
                    return NavigateOutcome::NeedsFetch(self.anchor_byte);
                }
                if let Some(np) = state.page_for(self.anchor_byte) {
                    let new_len = row_spans(np, state.policy.max_display_row_bytes).len();
                    self.first_row = self.first_row.min(new_len);
                }
            } else {
                self.first_row = max_first;
            }
            return NavigateOutcome::Ready;
        }
        if self.first_row == usize::MAX {
            self.first_row = all_len.saturating_sub(self.visible_rows);
        } else {
            self.first_row = self.first_row.min(max_first);
        }
        NavigateOutcome::Ready
    }
    /// Returns whether signed row movement can change the viewport, allowing boundary events to bubble to an ancestor.
    #[must_use]
    pub fn can_move_lines(&self, state: &PagedTextState, visible_rows: usize, lines: i32) -> bool {
        if lines == 0 {
            return false;
        }
        let Some(page) = state.page_for(self.anchor_byte) else {
            return false;
        };
        let all_len = row_spans(page, state.policy.max_display_row_bytes).len();
        let first = if self.first_row == SEEK_ROW {
            self.seek_row(page, state.policy.max_display_row_bytes)
        } else if self.first_row == usize::MAX {
            all_len.saturating_sub(visible_rows.max(1))
        } else {
            self.first_row.min(all_len)
        };
        if lines < 0 {
            first > 0 || page.byte_start > 0
        } else {
            first.saturating_add(visible_rows.max(1)) < all_len || page.byte_end < state.source_len
        }
    }

    /// Materializes visible rows plus policy-bounded overscan from immutable state.
    #[must_use]
    pub fn materialize(&self, state: &PagedTextState) -> PagedTextFrame {
        self.materialize_rows(state, self.visible_rows)
    }
    /// Materializes up to the requested on-screen row count plus bounded overscan, treating zero as one and ignoring the stored estimate.
    ///
    /// [`PagedTextFrame::visible`] identifies the on-screen subrange.
    #[must_use]
    pub fn materialize_rows(&self, state: &PagedTextState, visible_rows: usize) -> PagedTextFrame {
        let visible_rows = visible_rows.max(1);
        let Some(page) = state.page_for(self.anchor_byte) else {
            return PagedTextFrame {
                rows: Vec::new(),
                visible: 0..0,
                materialized_rows: 0,
                shaped_rows: 0,
            };
        };
        let all = row_spans(page, state.policy.max_display_row_bytes);
        let first = if self.first_row == SEEK_ROW {
            self.seek_row(page, state.policy.max_display_row_bytes)
        } else if self.first_row == usize::MAX {
            all.len().saturating_sub(visible_rows)
        } else {
            self.first_row.min(all.len())
        };
        let start = first.saturating_sub(state.policy.overscan_rows);
        let end = first
            .saturating_add(visible_rows)
            .saturating_add(state.policy.overscan_rows)
            .min(all.len());
        let prefix = state.line_prefix(page.chunk_index);
        let rows: Vec<PagedTextRow> = all[start..end]
            .iter()
            .map(|span| PagedTextRow {
                byte_range: (page.byte_start + span.start as u64)
                    ..(page.byte_start + span.end as u64),
                line_number: prefix.map(|p| p + span.line),
                continuation: span.continuation,
                text: page.text[span.start..span.end].to_owned(),
            })
            .collect();
        let count = rows.len();
        let visible_start = (first - start).min(count);
        let visible_end = visible_start.saturating_add(visible_rows).min(count);
        PagedTextFrame {
            rows,
            visible: visible_start..visible_end,
            materialized_rows: count,
            shaped_rows: count,
        }
    }
    /// Returns the uncached anchor that should be passed to [`PagedTextState::request`], if any.
    #[must_use]
    pub fn needs_fetch(&self, state: &PagedTextState) -> Option<u64> {
        if state.identity().is_none()
            || state.has_page(self.anchor_byte)
            || state.failure().is_some()
        {
            None
        } else {
            Some(self.anchor_byte)
        }
    }

    fn seek_row(&self, page: &PagedTextPage, max_row_bytes: usize) -> usize {
        let relative = self.anchor_byte.saturating_sub(page.byte_start) as usize;
        let rows = row_spans(page, max_row_bytes);
        rows.partition_point(|row| row.start <= relative)
            .saturating_sub(1)
    }
}

/// Navigation vocabulary mapped by the shared widget from arrows,
/// PageUp/PageDown, Home/End and wheel deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagedTextNavigation {
    /// One display row up.
    LineUp,
    /// One display row down.
    LineDown,
    /// One viewport up.
    PageUp,
    /// One viewport down.
    PageDown,
    /// Document start.
    Home,
    /// Document tail.
    End,
    /// Seek within the page covering a source byte, clamped to the source.
    /// Selects the containing display row when available. A nonterminal seek
    /// inside a UTF-8 character owned by the preceding page starts at the
    /// next complete character.
    /// An uncached target resolves as `NeedsFetch`; executing the returned read
    /// operation remains the caller's off-thread responsibility. Seeking to or
    /// beyond the final byte anchors the document tail like `End`.
    SeekByte(u64),
    /// Signed wheel rows (negative is up).
    Wheel(i32),
}

#[derive(Clone, Copy)]
struct RowSpan {
    start: usize,
    end: usize,
    line: u64,
    continuation: bool,
}
fn row_spans(page: &PagedTextPage, max: usize) -> Vec<RowSpan> {
    let mut out = Vec::new();
    let bytes = page.text.as_bytes();
    let mut line_start = 0;
    let mut line = 0u64;
    let mut i = 0;
    while i <= bytes.len() {
        let newline = if i < bytes.len() && bytes[i] == b'\n' {
            Some(1)
        } else if i < bytes.len() && bytes[i] == b'\r' {
            Some(if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                2
            } else {
                1
            })
        } else {
            None
        };
        if newline.is_some() || i == bytes.len() {
            let at_end = i == bytes.len();
            let mut segment = line_start;
            let mut continuation = line == 0 && page.starts_in_line;
            if segment == i && !at_end {
                out.push(RowSpan {
                    start: segment,
                    end: i,
                    line,
                    continuation,
                });
            }
            while segment < i {
                let mut end = (segment + max).min(i);
                while end > segment && !page.text.is_char_boundary(end) {
                    end -= 1;
                }
                if end == segment {
                    end = page.text[segment..]
                        .char_indices()
                        .nth(1)
                        .map_or(i, |(n, _)| segment + n)
                        .min(i);
                }
                out.push(RowSpan {
                    start: segment,
                    end,
                    line,
                    continuation,
                });
                continuation = true;
                segment = end;
            }
            line += 1;
            if at_end {
                break;
            }
            i += newline.unwrap_or(0);
            line_start = i;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests;
