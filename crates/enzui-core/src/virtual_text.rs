//! Bounded accessibility access to large editable documents.
//!
//! Requests join a stable document revision, answers are budgeted before materialization, and edits use document coordinates. Reads and mutations preserve the viewport unless the caller explicitly requests [`VirtualTextAction::Reveal`].

use crate::chunked_text::{EditError, TextSnapshot, TextStore};
use crate::editable_text_viewport::{
    EditableTextFrame, EditableTextViewportCache, MaterializeError,
};
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::ThreadId;

use mailbox::ReplyState;

/// Default maximum UTF-8 bytes returned by one accessibility query.
pub const DEFAULT_VIRTUAL_TEXT_BYTES: usize = 64 * 1024;
/// Default maximum Unicode scalar values returned by one query.
pub const DEFAULT_VIRTUAL_TEXT_CHARACTERS: usize = 16 * 1024;
/// Default maximum backing runs returned by one query.
pub const DEFAULT_VIRTUAL_TEXT_RUNS: usize = 64;
/// Default maximum replacement or IME preedit payload.
pub const DEFAULT_VIRTUAL_TEXT_REPLACEMENT_BYTES: usize = 64 * 1024;

/// Stable application-assigned identity of one logical document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualDocumentId(pub u128);

/// Monotonic generation of a virtual document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualRevision(pub u64);

/// Independent limits at the untrusted assistive-technology boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualTextBudget {
    /// Maximum UTF-8 bytes copied into one response.
    pub max_return_bytes: usize,
    /// Maximum Unicode scalar values described by one response.
    pub max_return_characters: usize,
    /// Maximum discontiguous backing runs in one response.
    pub max_return_runs: usize,
    /// Maximum UTF-8 bytes accepted for a replacement or preedit.
    pub max_replacement_bytes: usize,
}

impl Default for VirtualTextBudget {
    fn default() -> Self {
        Self {
            max_return_bytes: DEFAULT_VIRTUAL_TEXT_BYTES,
            max_return_characters: DEFAULT_VIRTUAL_TEXT_CHARACTERS,
            max_return_runs: DEFAULT_VIRTUAL_TEXT_RUNS,
            max_replacement_bytes: DEFAULT_VIRTUAL_TEXT_REPLACEMENT_BYTES,
        }
    }
}

/// Cheap cooperative cancellation shared with a request owner.
#[derive(Clone, Debug)]
pub struct VirtualTextCancellation {
    local: Arc<CancellationState>,
    #[cfg(all(feature = "accesskit", target_os = "linux"))]
    accesskit_unix: Option<iced_winit::accessibility::virtual_text::Cancellation>,
}

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Weak<ReplyState>>>,
}

impl Default for VirtualTextCancellation {
    fn default() -> Self {
        Self {
            local: Arc::new(CancellationState::default()),
            #[cfg(all(feature = "accesskit", target_os = "linux"))]
            accesskit_unix: None,
        }
    }
}

impl VirtualTextCancellation {
    /// Marks cancellation and wakes waiters whose requests have not been admitted by the reducer.
    pub fn cancel(&self) {
        self.local.cancelled.store(true, Ordering::Release);
        let waiters = self
            .local
            .waiters
            .lock()
            .map(|mut waiters| std::mem::take(&mut *waiters))
            .unwrap_or_default();
        for waiter in waiters.into_iter().filter_map(|waiter| waiter.upgrade()) {
            let _ = waiter.cancel_unless_admitted();
        }
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        if self.local.cancelled.load(Ordering::Acquire) {
            return true;
        }
        #[cfg(all(feature = "accesskit", target_os = "linux"))]
        if self
            .accesskit_unix
            .as_ref()
            .is_some_and(iced_winit::accessibility::virtual_text::Cancellation::is_cancelled)
        {
            return true;
        }
        false
    }

    fn register(&self, waiter: &Arc<ReplyState>) {
        if let Ok(mut waiters) = self.local.waiters.lock() {
            waiters.retain(|waiter| waiter.strong_count() > 0);
            waiters.push(Arc::downgrade(waiter));
        }
        if self.is_cancelled() {
            let _ = waiter.cancel_unless_admitted();
        }
    }

    #[cfg(all(feature = "accesskit", target_os = "linux"))]
    fn from_accesskit_unix(
        cancellation: &iced_winit::accessibility::virtual_text::Cancellation,
    ) -> Self {
        Self {
            local: Arc::new(CancellationState::default()),
            accesskit_unix: Some(cancellation.clone()),
        }
    }
}

/// Complete-document selection in Unicode-scalar coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtualSelection {
    /// Fixed endpoint, or `None` for a caret.
    pub anchor: Option<u64>,
    /// Active endpoint/caret.
    pub focus: u64,
}

impl VirtualSelection {
    /// Ordered selected range, excluding a degenerate caret.
    pub fn range(self) -> Option<Range<u64>> {
        let anchor = self.anchor?;
        (anchor != self.focus).then(|| anchor.min(self.focus)..anchor.max(self.focus))
    }
}

/// Constant-size facts published eagerly for discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualTextSummary {
    /// Stable document identity.
    pub document: VirtualDocumentId,
    /// Generation to join every later request against.
    pub revision: VirtualRevision,
    /// UTF-8 bytes in the complete document.
    pub bytes: u64,
    /// Unicode scalar values in the complete document.
    pub characters: u64,
    /// Logical lines in the complete document.
    pub lines: u64,
    /// Current complete-document selection.
    pub selection: VirtualSelection,
    /// Whether mutation requests are refused.
    pub read_only: bool,
}

/// Per-character information returned with text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualCharacterMetric {
    /// UTF-8 width of this scalar (1..=4).
    pub utf8_bytes: u8,
    /// Visual advance when the backing has measured it. Off-viewport text is
    /// allowed to report `None`; callers must not invent geometry.
    pub advance: Option<f32>,
}

/// One bounded contiguous run.
#[derive(Clone, Debug, PartialEq)]
pub struct VirtualTextRun {
    /// Complete-document scalar range represented by `value`.
    pub characters: Range<u64>,
    /// Exact UTF-8 value.
    pub value: String,
    /// One entry per Unicode scalar in `value`.
    pub metrics: Vec<VirtualCharacterMetric>,
}

/// Preflight size used to refuse before materialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualRangeMeasure {
    /// UTF-8 bytes that would be returned.
    pub bytes: usize,
    /// Unicode scalar values that would be returned.
    pub characters: usize,
    /// Backing runs that would be returned.
    pub runs: usize,
}

/// A backing for bounded lazy accessibility access.
///
/// [`Self::measure`] must avoid materializing the requested value, and
/// [`Self::read`] must return exactly the measured range. Callbacks may re-enter
/// the document; same-thread access to the active backing returns
/// [`VirtualTextError::Unavailable`] while independent threads remain serialized.
pub trait VirtualTextBacking: Send {
    /// `(bytes, characters, logical_lines)` for the complete document.
    fn summary(&self) -> (u64, u64, u64);
    /// Validate and measure a scalar range without materializing it.
    fn measure(&self, range: Range<u64>) -> Option<VirtualRangeMeasure>;
    /// Read a previously measured scalar range. Observe cancellation between
    /// runs and never return more than `measure` promised.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the read fails or is cancelled.
    fn read(
        &self,
        range: Range<u64>,
        cancellation: &VirtualTextCancellation,
    ) -> Result<Vec<VirtualTextRun>, VirtualTextError>;
    /// Replace a scalar range. The provider has already checked payload size,
    /// revision, read-only state and cancellation.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the backing cannot apply the replacement.
    fn replace(&mut self, range: Range<u64>, value: &str) -> Result<(), VirtualTextError>;
}

/// Backings that support reducer-confirmed state updates.
pub trait ConfirmableBacking: VirtualTextBacking {
    /// Replace the backing's state with a new snapshot.
    fn restore(&mut self, snapshot: TextSnapshot);
}

/// [`TextStore`] adoption of the lazy backing protocol.
#[derive(Debug)]
pub struct ChunkedTextBacking {
    store: TextStore,
}

impl ChunkedTextBacking {
    /// Adopt the authoritative chunked store without flattening it.
    pub fn new(store: TextStore) -> Self {
        Self { store }
    }

    /// Current immutable generation; O(1) and source-sharing.
    pub fn snapshot(&self) -> TextSnapshot {
        self.store.snapshot()
    }
}

impl ConfirmableBacking for ChunkedTextBacking {
    fn restore(&mut self, snapshot: TextSnapshot) {
        self.store.restore(snapshot);
    }
}

impl VirtualTextBacking for ChunkedTextBacking {
    fn summary(&self) -> (u64, u64, u64) {
        let snapshot = self.store.current();
        (
            snapshot.len() as u64,
            snapshot.character_count() as u64,
            snapshot.line_count() as u64,
        )
    }

    fn measure(&self, range: Range<u64>) -> Option<VirtualRangeMeasure> {
        let snapshot = self.store.current();
        let start = usize::try_from(range.start).ok()?;
        let end = usize::try_from(range.end).ok()?;
        if start > end || end > snapshot.character_count() {
            return None;
        }
        let start_byte = snapshot.byte_of_character(start)?;
        let end_byte = snapshot.byte_of_character(end)?;
        let mut runs = 0usize;
        snapshot.for_each_chunk_in(start_byte..end_byte, |_| runs = runs.saturating_add(1));
        Some(VirtualRangeMeasure {
            bytes: end_byte.saturating_sub(start_byte),
            characters: end.saturating_sub(start),
            runs,
        })
    }

    fn read(
        &self,
        range: Range<u64>,
        cancellation: &VirtualTextCancellation,
    ) -> Result<Vec<VirtualTextRun>, VirtualTextError> {
        if cancellation.is_cancelled() {
            return Err(VirtualTextError::Cancelled);
        }
        let snapshot = self.store.current();
        let start = usize::try_from(range.start).map_err(|_| VirtualTextError::Range)?;
        let end = usize::try_from(range.end).map_err(|_| VirtualTextError::Range)?;
        let start_byte = snapshot
            .byte_of_character(start)
            .ok_or(VirtualTextError::Range)?;
        let end_byte = snapshot
            .byte_of_character(end)
            .ok_or(VirtualTextError::Range)?;
        let mut next_character = range.start;
        let mut runs = Vec::new();
        let mut cancelled = false;
        snapshot.for_each_chunk_in(start_byte..end_byte, |chunk| {
            if cancellation.is_cancelled() {
                cancelled = true;
                return;
            }
            let metrics: Vec<_> = chunk
                .chars()
                .map(|ch| VirtualCharacterMetric {
                    utf8_bytes: ch.len_utf8() as u8,
                    advance: None,
                })
                .collect();
            let run_end = next_character.saturating_add(metrics.len() as u64);
            runs.push(VirtualTextRun {
                characters: next_character..run_end,
                value: chunk.to_owned(),
                metrics,
            });
            next_character = run_end;
        });
        if cancelled {
            return Err(VirtualTextError::Cancelled);
        }
        Ok(runs)
    }

    fn replace(&mut self, range: Range<u64>, value: &str) -> Result<(), VirtualTextError> {
        let snapshot = self.store.current();
        let start = usize::try_from(range.start).map_err(|_| VirtualTextError::Range)?;
        let end = usize::try_from(range.end).map_err(|_| VirtualTextError::Range)?;
        let bytes = snapshot
            .byte_of_character(start)
            .zip(snapshot.byte_of_character(end))
            .map(|(start, end)| start..end)
            .ok_or(VirtualTextError::Range)?;
        self.store
            .replace(bytes, value.to_owned())
            .map_err(|error| match error {
                EditError::OutOfBounds | EditError::NotCharBoundary => VirtualTextError::Range,
            })
    }
}

/// Why a lazy text request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualTextError {
    /// The stable document identity did not match.
    WrongDocument,
    /// The request joined against an older/newer generation.
    StaleRevision {
        /// Requested generation.
        requested: VirtualRevision,
        /// Current generation.
        current: VirtualRevision,
    },
    /// A reducer confirmation did not advance exactly one generation.
    RevisionSequence {
        /// Provider generation before the proposed confirmation.
        previous: VirtualRevision,
        /// Proposed provider generation.
        proposed: VirtualRevision,
    },
    /// Request was cancelled before completion.
    Cancelled,
    /// Range was reversed or outside the document.
    Range,
    /// Returned UTF-8 bytes would exceed policy.
    ByteBudget {
        /// Bytes the source would return.
        requested: usize,
        /// Configured byte limit.
        limit: usize,
    },
    /// Returned scalar count would exceed policy.
    CharacterBudget {
        /// Scalars the source would return.
        requested: usize,
        /// Configured scalar limit.
        limit: usize,
    },
    /// Returned backing-run count would exceed policy.
    RunBudget {
        /// Runs the source would return.
        requested: usize,
        /// Configured run limit.
        limit: usize,
    },
    /// Mutation payload would exceed policy.
    ReplacementBudget {
        /// UTF-8 payload bytes supplied.
        requested: usize,
        /// Configured payload limit.
        limit: usize,
    },
    /// The document is read-only (including an OOM-safe fallback).
    ReadOnly,
    /// IME token no longer owns a live composition.
    StaleComposition,
    /// This composition was already committed/cancelled.
    CompositionFinished,
    /// Provider state is unavailable; no request was performed.
    Unavailable,
    /// The reducer did not answer before the bounded request deadline.
    Timeout,
    /// The mutation mailbox is full; try again after the reducer drains.
    MailboxFull,
    /// The mutation mailbox has been closed (provider is shutting down).
    MailboxClosed,
    /// The reducer refused this mutation (e.g. paragraph limit exceeded).
    ReducerRefused,
}

mod mailbox;

/// Re-exports the bounded reducer-owned mutation queue and its protocol types.
pub use mailbox::{
    DEFAULT_MUTATION_MAILBOX_CAPACITY, DEFAULT_MUTATION_REPLY_TIMEOUT, MailboxWakeRegistration,
    MutationKind, MutationMailbox, MutationReply, MutationRequest, MutationTicket, PendingMutation,
};

/// Visual consequence of an accessibility action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportEffect {
    /// Preserve visual focus and scroll.
    Preserve,
    /// Explicit action requires revealing this range.
    Reveal(RangeU64),
}

/// Copyable counterpart of `Range<u64>` for action outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeU64 {
    /// Inclusive start.
    pub start: u64,
    /// Exclusive end.
    pub end: u64,
}

impl From<Range<u64>> for RangeU64 {
    fn from(value: Range<u64>) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

/// Mutating/action request in complete-document coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VirtualTextAction {
    /// Change the semantic selection without moving the visual viewport.
    SetSelection(VirtualSelection),
    /// Replace an arbitrary range.
    Replace {
        /// Complete-document scalar range to replace.
        range: Range<u64>,
        /// Exact replacement value.
        value: String,
    },
    /// Replace the current semantic selection.
    ReplaceSelection(String),
    /// Explicitly request that a range become visible.
    Reveal(Range<u64>),
}

/// Result of a routed action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualTextActionOutcome {
    /// Revision after the action.
    pub revision: VirtualRevision,
    /// Selection after the action.
    pub selection: VirtualSelection,
    /// Whether the visual host must move.
    pub viewport: ViewportEffect,
}

/// Opaque ownership token for one IME composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositionId(u64);

#[derive(Clone, Debug)]
struct Composition {
    id: CompositionId,
    revision: VirtualRevision,
    range: Range<u64>,
    preedit: String,
}

enum BackingState<B> {
    Ready(B),
    InUse(ThreadId),
    Unavailable(B),
}

struct State<B> {
    backing: BackingState<B>,
    revision: VirtualRevision,
    selection: VirtualSelection,
    read_only: bool,
    next_composition: u64,
    composition: Option<Composition>,
    finished_composition: Option<CompositionId>,
}

/// A lazy virtual document whose mutations are resolved by a bounded [`MutationMailbox`] while reads remain direct.
pub struct VirtualTextDocument<B = ChunkedTextBacking> {
    document: VirtualDocumentId,
    budget: VirtualTextBudget,
    state: Mutex<State<B>>,
    state_ready: Condvar,
    mailbox: Arc<MutationMailbox>,
}

struct BackingLease<'a, B> {
    document: &'a VirtualTextDocument<B>,
    backing: Option<B>,
}

impl<B> BackingLease<'_, B> {
    fn backing(&self) -> &B {
        self.backing
            .as_ref()
            .expect("an active backing lease owns the backing")
    }

    fn backing_mut(&mut self) -> &mut B {
        self.backing
            .as_mut()
            .expect("an active backing lease owns the backing")
    }

    fn finish<R>(mut self, update: impl FnOnce(&mut State<B>) -> R) -> R {
        let mut state = match self.document.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let result = update(&mut state);
        state.backing = BackingState::Ready(
            self.backing
                .take()
                .expect("an active backing lease owns the backing"),
        );
        drop(state);
        self.document.state_ready.notify_all();
        result
    }
}

impl<B> Drop for BackingLease<'_, B> {
    fn drop(&mut self) {
        let Some(backing) = self.backing.take() else {
            return;
        };
        let mut state = match self.document.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.backing = if std::thread::panicking() {
            BackingState::Unavailable(backing)
        } else {
            BackingState::Ready(backing)
        };
        drop(state);
        self.document.state_ready.notify_all();
    }
}

impl<B> VirtualTextDocument<B> {
    fn lock_state(&self) -> Result<MutexGuard<'_, State<B>>, VirtualTextError> {
        let current = std::thread::current().id();
        let mut state = self
            .state
            .lock()
            .map_err(|_| VirtualTextError::Unavailable)?;
        loop {
            match &state.backing {
                BackingState::Ready(_) => return Ok(state),
                BackingState::InUse(owner) if *owner == current => {
                    return Err(VirtualTextError::Unavailable);
                }
                BackingState::InUse(_) => {
                    state = self
                        .state_ready
                        .wait(state)
                        .map_err(|_| VirtualTextError::Unavailable)?;
                }
                BackingState::Unavailable(_) => return Err(VirtualTextError::Unavailable),
            }
        }
    }

    fn acquire_backing<T>(
        &self,
        prepare: impl FnOnce(&State<B>) -> Result<T, VirtualTextError>,
    ) -> Result<(T, BackingLease<'_, B>), VirtualTextError> {
        let mut state = self.lock_state()?;
        let prepared = prepare(&state)?;
        let owner = std::thread::current().id();
        let backing = match std::mem::replace(&mut state.backing, BackingState::InUse(owner)) {
            BackingState::Ready(backing) => backing,
            other => {
                state.backing = other;
                return Err(VirtualTextError::Unavailable);
            }
        };
        drop(state);
        Ok((
            prepared,
            BackingLease {
                document: self,
                backing: Some(backing),
            },
        ))
    }
}

struct StateDebugSnapshot {
    revision: VirtualRevision,
    selection: VirtualSelection,
    read_only: bool,
    next_composition: u64,
    composition: Option<(CompositionId, VirtualRevision, Range<u64>, usize)>,
    finished_composition: Option<CompositionId>,
}

struct StateDebug<'a, B> {
    backing: &'a B,
    snapshot: &'a StateDebugSnapshot,
}

impl<B: fmt::Debug> fmt::Debug for StateDebug<'_, B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("State")
            .field("backing", self.backing)
            .field("revision", &self.snapshot.revision)
            .field("selection", &self.snapshot.selection)
            .field("read_only", &self.snapshot.read_only)
            .field("next_composition", &self.snapshot.next_composition)
            .field("composition", &self.snapshot.composition)
            .field("finished_composition", &self.snapshot.finished_composition)
            .finish()
    }
}

impl<B: fmt::Debug> fmt::Debug for VirtualTextDocument<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let acquired = self.acquire_backing(|state| {
            Ok(StateDebugSnapshot {
                revision: state.revision,
                selection: state.selection,
                read_only: state.read_only,
                next_composition: state.next_composition,
                composition: state.composition.as_ref().map(|composition| {
                    (
                        composition.id,
                        composition.revision,
                        composition.range.clone(),
                        composition.preedit.len(),
                    )
                }),
                finished_composition: state.finished_composition,
            })
        });
        let mut document = formatter.debug_struct("VirtualTextDocument");
        document
            .field("document", &self.document)
            .field("budget", &self.budget);
        match acquired {
            Ok((snapshot, lease)) => {
                document.field(
                    "state",
                    &StateDebug {
                        backing: lease.backing(),
                        snapshot: &snapshot,
                    },
                );
            }
            Err(_) => {
                document.field("state", &"unavailable");
            }
        }
        document.field("mailbox", &self.mailbox).finish()
    }
}

impl<B> Drop for VirtualTextDocument<B> {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}

impl VirtualTextDocument<ChunkedTextBacking> {
    /// Adopt a [`TextStore`] as the authoritative document.
    pub fn from_chunked(
        document: VirtualDocumentId,
        revision: VirtualRevision,
        store: TextStore,
        budget: VirtualTextBudget,
    ) -> Self {
        Self::new(document, revision, ChunkedTextBacking::new(store), budget)
    }

    /// O(1) immutable snapshot for save/search/viewport rendering.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the backing is unavailable.
    pub fn snapshot(&self) -> Result<TextSnapshot, VirtualTextError> {
        let ((), lease) = self.acquire_backing(|_| Ok(()))?;
        Ok(lease.backing().snapshot())
    }

    /// Returns a summary and snapshot from the same document revision.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the backing is unavailable.
    pub fn summary_and_snapshot(
        &self,
    ) -> Result<(VirtualTextSummary, TextSnapshot), VirtualTextError> {
        let ((revision, selection, read_only), lease) =
            self.acquire_backing(|state| Ok((state.revision, state.selection, state.read_only)))?;
        let (bytes, characters, lines) = lease.backing().summary();
        let snapshot = lease.backing().snapshot();
        Ok((
            self.make_summary(revision, selection, read_only, bytes, characters, lines),
            snapshot,
        ))
    }

    /// Materialize only the requested visual viewport. Accessibility reads do
    /// not call this method and therefore cannot recenter the view.
    ///
    /// # Errors
    ///
    /// A [`MaterializeError`] when the backing is unavailable or the range exceeds its budget.
    pub fn materialize_viewport(
        &self,
        cache: &mut EditableTextViewportCache,
        first_visible: usize,
        visible_lines: usize,
    ) -> Result<EditableTextFrame, MaterializeError> {
        let (revision, lease) = self
            .acquire_backing(|state| Ok(state.revision))
            .map_err(|_| MaterializeError::ByteBudget {
                requested: usize::MAX,
                budget: cache.policy().max_cached_bytes,
            })?;
        cache.materialize(
            lease.backing().store.current(),
            revision.0,
            first_visible,
            visible_lines,
        )
    }
}

impl<B: VirtualTextBacking> VirtualTextDocument<B> {
    /// Construct over any bounded/generated backing.
    pub fn new(
        document: VirtualDocumentId,
        revision: VirtualRevision,
        backing: B,
        budget: VirtualTextBudget,
    ) -> Self {
        Self {
            document,
            budget,
            mailbox: Arc::new(MutationMailbox::new(DEFAULT_MUTATION_MAILBOX_CAPACITY)),
            state: Mutex::new(State {
                backing: BackingState::Ready(backing),
                revision,
                selection: VirtualSelection::default(),
                read_only: false,
                next_composition: 1,
                composition: None,
                finished_composition: None,
            }),
            state_ready: Condvar::new(),
        }
    }

    /// The mutation mailbox. The reducer installs a wake callback on this and
    /// drains it in its update loop.
    pub fn mailbox(&self) -> &Arc<MutationMailbox> {
        &self.mailbox
    }

    /// Active boundary limits.
    pub fn budget(&self) -> VirtualTextBudget {
        self.budget
    }

    /// Sets whether mutation requests are refused without changing document bytes.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the provider state is unavailable.
    pub fn set_read_only(&self, read_only: bool) -> Result<(), VirtualTextError> {
        let mut state = self.lock_state()?;
        state.read_only = read_only;
        Ok(())
    }

    /// Constant-size complete-document facts.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the backing is unavailable.
    pub fn summary(&self) -> Result<VirtualTextSummary, VirtualTextError> {
        let ((revision, selection, read_only), lease) =
            self.acquire_backing(|state| Ok((state.revision, state.selection, state.read_only)))?;
        let (bytes, characters, lines) = lease.backing().summary();
        Ok(self.make_summary(revision, selection, read_only, bytes, characters, lines))
    }

    fn make_summary(
        &self,
        revision: VirtualRevision,
        selection: VirtualSelection,
        read_only: bool,
        bytes: u64,
        characters: u64,
        lines: u64,
    ) -> VirtualTextSummary {
        VirtualTextSummary {
            document: self.document,
            revision,
            bytes,
            characters,
            lines,
            selection,
            read_only,
        }
    }

    fn check_join(
        &self,
        state: &State<B>,
        document: VirtualDocumentId,
        revision: VirtualRevision,
        cancellation: &VirtualTextCancellation,
    ) -> Result<(), VirtualTextError> {
        if cancellation.is_cancelled() {
            return Err(VirtualTextError::Cancelled);
        }
        if document != self.document {
            return Err(VirtualTextError::WrongDocument);
        }
        if revision != state.revision {
            return Err(VirtualTextError::StaleRevision {
                requested: revision,
                current: state.revision,
            });
        }
        Ok(())
    }

    /// Read a bounded arbitrary scalar range. Budgets are checked against the
    /// backing's preflight measure before any text is copied.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document or revision, cancellation, an invalid range,
    /// or a range over the character budget.
    pub fn read(
        &self,
        document: VirtualDocumentId,
        revision: VirtualRevision,
        range: Range<u64>,
        cancellation: &VirtualTextCancellation,
    ) -> Result<Vec<VirtualTextRun>, VirtualTextError> {
        let ((), lease) =
            self.acquire_backing(|state| self.check_join(state, document, revision, cancellation))?;
        let document_characters = lease.backing().summary().1;
        validate_range(&range, document_characters)?;
        let requested_characters = usize::try_from(range.end - range.start).map_err(|_| {
            VirtualTextError::CharacterBudget {
                requested: usize::MAX,
                limit: self.budget.max_return_characters,
            }
        })?;
        if requested_characters > self.budget.max_return_characters {
            return Err(VirtualTextError::CharacterBudget {
                requested: requested_characters,
                limit: self.budget.max_return_characters,
            });
        }
        let measured = lease
            .backing()
            .measure(range.clone())
            .ok_or(VirtualTextError::Range)?;
        if measured.bytes > self.budget.max_return_bytes {
            return Err(VirtualTextError::ByteBudget {
                requested: measured.bytes,
                limit: self.budget.max_return_bytes,
            });
        }
        if measured.characters != requested_characters {
            return Err(VirtualTextError::Unavailable);
        }
        if measured.runs > self.budget.max_return_runs {
            return Err(VirtualTextError::RunBudget {
                requested: measured.runs,
                limit: self.budget.max_return_runs,
            });
        }
        let expected_start = range.start;
        let expected_end = range.end;
        let runs = lease.backing().read(range, cancellation)?;
        if cancellation.is_cancelled() {
            return Err(VirtualTextError::Cancelled);
        }
        let actual_bytes = runs.iter().map(|run| run.value.len()).sum::<usize>();
        let actual_characters = runs.iter().map(|run| run.metrics.len()).sum::<usize>();
        let mut next_character = expected_start;
        let structurally_valid = runs.iter().all(|run| {
            let run_characters = run.characters.end.saturating_sub(run.characters.start);
            let valid = run.characters.start == next_character
                && run_characters == run.metrics.len() as u64
                && run.value.chars().count() == run.metrics.len()
                && run
                    .value
                    .chars()
                    .zip(&run.metrics)
                    .all(|(character, metric)| {
                        character.len_utf8() == usize::from(metric.utf8_bytes)
                    });
            next_character = run.characters.end;
            valid
        });
        if runs.len() != measured.runs
            || actual_bytes != measured.bytes
            || actual_characters != measured.characters
            || !structurally_valid
            || next_character != expected_end
        {
            return Err(VirtualTextError::Unavailable);
        }
        Ok(runs)
    }

    /// Routes an action at an exact revision.
    ///
    /// Mutations enter the bounded mailbox and block for the reducer reply; [`VirtualTextAction::Reveal`] is answered directly without changing bytes.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document or revision, cancellation, a read-only
    /// document, or a replacement over budget.
    pub fn act(
        &self,
        document: VirtualDocumentId,
        revision: VirtualRevision,
        action: VirtualTextAction,
        cancellation: &VirtualTextCancellation,
    ) -> Result<VirtualTextActionOutcome, VirtualTextError> {
        // Invalid joins and read-only replacements never consume mailbox capacity.
        {
            let state = self.lock_state()?;
            self.check_join(&state, document, revision, cancellation)?;
            if matches!(
                action,
                VirtualTextAction::Replace { .. } | VirtualTextAction::ReplaceSelection(_)
            ) && state.read_only
            {
                return Err(VirtualTextError::ReadOnly);
            }
            match &action {
                VirtualTextAction::Replace { value, .. }
                | VirtualTextAction::ReplaceSelection(value)
                    if value.len() > self.budget.max_replacement_bytes =>
                {
                    return Err(VirtualTextError::ReplacementBudget {
                        requested: value.len(),
                        limit: self.budget.max_replacement_bytes,
                    });
                }
                _ => {}
            }
        }

        match action {
            VirtualTextAction::Reveal(range) => {
                let ((current_revision, selection), lease) = self.acquire_backing(|state| {
                    self.check_join(state, document, revision, cancellation)?;
                    Ok((state.revision, state.selection))
                })?;
                let characters = lease.backing().summary().1;
                validate_range(&range, characters)?;
                Ok(VirtualTextActionOutcome {
                    revision: current_revision,
                    selection,
                    viewport: ViewportEffect::Reveal(range.into()),
                })
            }
            mutation => {
                let ticket = self.enqueue_mutation(document, revision, mutation, cancellation)?;
                self.translate_reply(ticket.wait()?)
            }
        }
    }

    /// Enqueues a mutation without waiting, for callers already running on the owning reducer thread.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document or revision, cancellation, an invalid
    /// selection, a read-only document, a replacement over budget, or a full mailbox.
    pub fn enqueue_mutation(
        &self,
        document: VirtualDocumentId,
        revision: VirtualRevision,
        action: VirtualTextAction,
        cancellation: &VirtualTextCancellation,
    ) -> Result<MutationTicket, VirtualTextError> {
        let mutation = match action {
            VirtualTextAction::SetSelection(selection) => {
                let ((), lease) = self.acquire_backing(|state| {
                    self.check_join(state, document, revision, cancellation)
                })?;
                validate_selection(selection, lease.backing().summary().1)?;
                MutationKind::SetSelection(selection)
            }
            VirtualTextAction::Replace { range, value } => {
                let ((), lease) = self.acquire_backing(|state| {
                    self.check_join(state, document, revision, cancellation)?;
                    if state.read_only {
                        return Err(VirtualTextError::ReadOnly);
                    }
                    if value.len() > self.budget.max_replacement_bytes {
                        return Err(VirtualTextError::ReplacementBudget {
                            requested: value.len(),
                            limit: self.budget.max_replacement_bytes,
                        });
                    }
                    Ok(())
                })?;
                validate_range(&range, lease.backing().summary().1)?;
                MutationKind::Replace { range, value }
            }
            VirtualTextAction::ReplaceSelection(value) => {
                let state = self.lock_state()?;
                self.check_join(&state, document, revision, cancellation)?;
                if state.read_only {
                    return Err(VirtualTextError::ReadOnly);
                }
                if value.len() > self.budget.max_replacement_bytes {
                    return Err(VirtualTextError::ReplacementBudget {
                        requested: value.len(),
                        limit: self.budget.max_replacement_bytes,
                    });
                }
                let selection = state.selection;
                let range = selection
                    .range()
                    .unwrap_or(selection.focus..selection.focus);
                MutationKind::Replace { range, value }
            }
            VirtualTextAction::Reveal(_) => return Err(VirtualTextError::Unavailable),
        };
        self.mailbox.enqueue(MutationRequest {
            document,
            revision,
            cancellation: cancellation.clone(),
            mutation,
        })
    }

    fn translate_reply(
        &self,
        reply: MutationReply,
    ) -> Result<VirtualTextActionOutcome, VirtualTextError> {
        match reply {
            MutationReply::Accepted {
                revision,
                selection,
            } => Ok(VirtualTextActionOutcome {
                revision,
                selection,
                viewport: ViewportEffect::Preserve,
            }),
            MutationReply::Stale { requested, current } => {
                Err(VirtualTextError::StaleRevision { requested, current })
            }
            MutationReply::Refused => Err(VirtualTextError::ReducerRefused),
        }
    }

    /// Installs a reducer-confirmed snapshot, revision, and selection atomically.
    ///
    /// The backing restore is O(1) for persistent chunk snapshots.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document, a revision other than `expected_revision`, or
    /// a selection that does not fit.
    pub fn confirm_mutation(
        &self,
        document: VirtualDocumentId,
        expected_revision: VirtualRevision,
        new_revision: VirtualRevision,
        new_selection: VirtualSelection,
        new_snapshot: TextSnapshot,
    ) -> Result<VirtualTextSummary, VirtualTextError>
    where
        B: ConfirmableBacking,
    {
        let (read_only, mut lease) = self.acquire_backing(|state| {
            if document != self.document {
                return Err(VirtualTextError::WrongDocument);
            }
            if state.revision != expected_revision {
                return Err(VirtualTextError::StaleRevision {
                    requested: expected_revision,
                    current: state.revision,
                });
            }
            let expected_next = expected_revision
                .0
                .checked_add(1)
                .map(VirtualRevision)
                .ok_or(VirtualTextError::Unavailable)?;
            if new_revision != expected_next {
                return Err(VirtualTextError::RevisionSequence {
                    previous: expected_revision,
                    proposed: new_revision,
                });
            }
            let characters = u64::try_from(new_snapshot.character_count())
                .map_err(|_| VirtualTextError::Unavailable)?;
            validate_selection(new_selection, characters)?;
            Ok(state.read_only)
        })?;
        lease.backing_mut().restore(new_snapshot);
        let (bytes, characters, lines) = lease.backing().summary();
        lease.finish(|state| {
            state.revision = new_revision;
            state.selection = new_selection;
        });
        Ok(self.make_summary(
            new_revision,
            new_selection,
            read_only,
            bytes,
            characters,
            lines,
        ))
    }

    /// Atomically resynchronizes to a strictly newer application revision.
    ///
    /// `from_revision` must equal the provider’s current revision, `to_revision` must be greater, and the new selection must fit the supplied snapshot.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document, a `from_revision` other than the current one,
    /// a `to_revision` that is not newer, or a selection that does not fit.
    pub fn resync_forward(
        &self,
        document: VirtualDocumentId,
        from_revision: VirtualRevision,
        to_revision: VirtualRevision,
        new_selection: VirtualSelection,
        new_snapshot: TextSnapshot,
    ) -> Result<VirtualTextSummary, VirtualTextError>
    where
        B: ConfirmableBacking,
    {
        let (read_only, mut lease) = self.acquire_backing(|state| {
            if document != self.document {
                return Err(VirtualTextError::WrongDocument);
            }
            if state.revision != from_revision {
                return Err(VirtualTextError::StaleRevision {
                    requested: from_revision,
                    current: state.revision,
                });
            }
            if to_revision.0 <= from_revision.0 {
                return Err(VirtualTextError::RevisionSequence {
                    previous: from_revision,
                    proposed: to_revision,
                });
            }
            let characters = u64::try_from(new_snapshot.character_count())
                .map_err(|_| VirtualTextError::Unavailable)?;
            validate_selection(new_selection, characters)?;
            Ok(state.read_only)
        })?;
        lease.backing_mut().restore(new_snapshot);
        let (bytes, characters, lines) = lease.backing().summary();
        lease.finish(|state| {
            state.revision = to_revision;
            state.selection = new_selection;
        });
        Ok(self.make_summary(
            to_revision,
            new_selection,
            read_only,
            bytes,
            characters,
            lines,
        ))
    }

    /// Apply a reducer-confirmed selection against one exact document
    /// generation. Selection does not advance the byte revision.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document, a stale revision, or a selection that does
    /// not fit.
    pub fn confirm_selection(
        &self,
        document: VirtualDocumentId,
        expected_revision: VirtualRevision,
        new_selection: VirtualSelection,
    ) -> Result<VirtualTextSummary, VirtualTextError> {
        let (read_only, lease) = self.acquire_backing(|state| {
            if document != self.document {
                return Err(VirtualTextError::WrongDocument);
            }
            if state.revision != expected_revision {
                return Err(VirtualTextError::StaleRevision {
                    requested: expected_revision,
                    current: state.revision,
                });
            }
            Ok(state.read_only)
        })?;
        let (bytes, characters, lines) = lease.backing().summary();
        validate_selection(new_selection, characters)?;
        lease.finish(|state| state.selection = new_selection);
        Ok(self.make_summary(
            expected_revision,
            new_selection,
            read_only,
            bytes,
            characters,
            lines,
        ))
    }

    /// Begin an IME composition in document coordinates.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for the wrong document or revision, cancellation, a read-only
    /// document, an unavailable backing, or an invalid range.
    pub fn begin_composition(
        &self,
        document: VirtualDocumentId,
        revision: VirtualRevision,
        range: Range<u64>,
        cancellation: &VirtualTextCancellation,
    ) -> Result<CompositionId, VirtualTextError> {
        let ((id, next_composition), lease) = self.acquire_backing(|state| {
            self.check_join(state, document, revision, cancellation)?;
            if state.read_only {
                return Err(VirtualTextError::ReadOnly);
            }
            let id = CompositionId(state.next_composition);
            let next_composition = state
                .next_composition
                .checked_add(1)
                .ok_or(VirtualTextError::Unavailable)?;
            Ok((id, next_composition))
        })?;
        validate_range(&range, lease.backing().summary().1)?;
        lease.finish(|state| {
            state.next_composition = next_composition;
            state.composition = Some(Composition {
                id,
                revision,
                range,
                preedit: String::new(),
            });
        });
        Ok(id)
    }

    /// Replace the transient preedit without changing document revision.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] on cancellation, a replacement over budget, or a finished or stale
    /// composition.
    pub fn update_composition(
        &self,
        id: CompositionId,
        preedit: &str,
        cancellation: &VirtualTextCancellation,
    ) -> Result<(), VirtualTextError> {
        if cancellation.is_cancelled() {
            return Err(VirtualTextError::Cancelled);
        }
        if preedit.len() > self.budget.max_replacement_bytes {
            return Err(VirtualTextError::ReplacementBudget {
                requested: preedit.len(),
                limit: self.budget.max_replacement_bytes,
            });
        }
        let mut state = self.lock_state()?;
        if state.finished_composition == Some(id) {
            return Err(VirtualTextError::CompositionFinished);
        }
        let composition = state
            .composition
            .as_mut()
            .filter(|composition| composition.id == id)
            .ok_or(VirtualTextError::StaleComposition)?;
        composition.preedit.clear();
        composition.preedit.push_str(preedit);
        Ok(())
    }

    /// Enqueues the active composition commit without blocking; pass the reducer reply to [`Self::finish_composition_commit`].
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for a finished or stale composition, the wrong revision,
    /// cancellation, or a full mailbox.
    pub fn enqueue_composition_commit(
        &self,
        id: CompositionId,
        cancellation: &VirtualTextCancellation,
    ) -> Result<MutationTicket, VirtualTextError> {
        let composition = {
            let state = self.lock_state()?;
            if state.finished_composition == Some(id) {
                return Err(VirtualTextError::CompositionFinished);
            }
            let composition = state
                .composition
                .as_ref()
                .filter(|composition| composition.id == id)
                .ok_or(VirtualTextError::StaleComposition)?;
            self.check_join(&state, self.document, composition.revision, cancellation)?;
            composition.clone()
        };
        self.mailbox.enqueue(MutationRequest {
            document: self.document,
            revision: composition.revision,
            cancellation: cancellation.clone(),
            mutation: MutationKind::CommitComposition {
                id,
                range: composition.range,
                value: composition.preedit,
            },
        })
    }

    /// Mark a reducer-accepted composition as consumed exactly once.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] when the reducer refused the commit or the composition is finished or
    /// stale.
    pub fn finish_composition_commit(
        &self,
        id: CompositionId,
        reply: MutationReply,
    ) -> Result<VirtualTextActionOutcome, VirtualTextError> {
        let outcome = self.translate_reply(reply)?;
        let mut state = self.lock_state()?;
        if state.finished_composition == Some(id) {
            return Err(VirtualTextError::CompositionFinished);
        }
        if state.composition.as_ref().map(|composition| composition.id) != Some(id) {
            return Err(VirtualTextError::StaleComposition);
        }
        state.composition = None;
        state.finished_composition = Some(id);
        Ok(outcome)
    }

    /// Commit a composition through the same bounded request/reply queue as AT
    /// replacements. No provider-owned byte path bypasses the app reducer.
    ///
    /// # Errors
    ///
    /// As [`Self::enqueue_composition_commit`], and the reducer's refusal.
    pub fn commit_composition(
        &self,
        id: CompositionId,
        cancellation: &VirtualTextCancellation,
    ) -> Result<VirtualTextActionOutcome, VirtualTextError> {
        let ticket = self.enqueue_composition_commit(id, cancellation)?;
        let reply = ticket.wait()?;
        self.finish_composition_commit(id, reply)
    }

    /// Cancel a composition without changing bytes or revision.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] for a finished or stale composition.
    pub fn cancel_composition(&self, id: CompositionId) -> Result<(), VirtualTextError> {
        let mut state = self.lock_state()?;
        if state.finished_composition == Some(id) {
            return Err(VirtualTextError::CompositionFinished);
        }
        if state.composition.as_ref().map(|value| value.id) != Some(id) {
            return Err(VirtualTextError::StaleComposition);
        }
        state.composition = None;
        state.finished_composition = Some(id);
        Ok(())
    }
}

fn validate_range(range: &Range<u64>, characters: u64) -> Result<(), VirtualTextError> {
    if range.start > range.end || range.end > characters {
        Err(VirtualTextError::Range)
    } else {
        Ok(())
    }
}

fn validate_selection(
    selection: VirtualSelection,
    characters: u64,
) -> Result<(), VirtualTextError> {
    if selection.focus > characters || selection.anchor.is_some_and(|value| value > characters) {
        Err(VirtualTextError::Range)
    } else {
        Ok(())
    }
}

#[cfg(all(feature = "accesskit", target_os = "linux"))]
mod unix;

/// Binds virtual-text providers to publishing widgets and resolves them during accessibility-tree publication.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub use unix::{
    BindingResolution, PROVIDER_TREE_BUILDER_OWNER, bind_provider_to_widget,
    collect_frame_binding_providers, install_provider_tree_builder, resolve_bindings,
    try_install_provider_tree_builder, unbind_provider_from_widget,
};

#[cfg(test)]
mod tests;
