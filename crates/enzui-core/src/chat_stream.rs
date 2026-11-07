//! Pure chat-message, stream-coalescing, follow-state, and retention policies.
//!
//! The caller supplies effects and frame timing; this module performs no I/O or clock reads.

use std::sync::Arc;

/// A stable chat-message key independent of transcript position.
///
/// The string may be used as input to [`crate::identity::derive_id`] when a row is realized.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageKey(pub String);

impl MessageKey {
    /// A key from any string-like value.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        MessageKey(key.into())
    }

    /// The underlying logical key, for id derivation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for MessageKey {
    fn from(s: &str) -> Self {
        MessageKey(s.to_owned())
    }
}

impl From<String> for MessageKey {
    fn from(s: String) -> Self {
        MessageKey(s)
    }
}

/// Identifies the authoring role without depending on the widgets crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ChatRole {
    /// The human.
    User,
    /// The assistant / model.
    #[default]
    Assistant,
    /// A system / tool message.
    System,
}

/// Streaming lifecycle of one message's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamState {
    /// Body is final; no more chunks will arrive.
    #[default]
    Complete,
    /// Body may receive more chunks.
    Streaming,
    /// The stream was cancelled mid-flight; the partial body is kept.
    Cancelled,
}

impl StreamState {
    /// Whether more chunks may still arrive.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, StreamState::Streaming)
    }
}

/// A keyed chat message whose text and optional labels are cheap to clone.
///
/// `Arc<str>` fields share their allocation across provider, widget, and accessibility consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Stable identity, independent of position in the transcript.
    pub key: MessageKey,
    /// Who authored it.
    pub role: ChatRole,
    /// Optional author display name (distinct from role).
    pub author: Option<Arc<str>>,
    /// Flat message text shared by reference-counted ownership.
    pub text: Arc<str>,
    /// Streaming lifecycle of the body.
    pub stream: StreamState,
    /// Optional metadata shown below the bubble, such as a timestamp or delivery state.
    ///
    /// It is distinct from the author lane and follows the bubble’s role-derived alignment.
    pub timestamp: Option<Arc<str>>,
}

impl ChatMessage {
    /// A complete message with the given key, role and text.
    #[must_use]
    pub fn new(key: MessageKey, role: ChatRole, text: impl Into<Arc<str>>) -> Self {
        ChatMessage {
            key,
            role,
            author: None,
            text: text.into(),
            stream: StreamState::Complete,
            timestamp: None,
        }
    }

    /// Attach an author display name.
    #[must_use]
    pub fn with_author(mut self, author: impl Into<Arc<str>>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Attaches metadata for the lane below the bubble.
    ///
    /// Widget consumers treat an empty string as unset.
    #[must_use]
    pub fn with_timestamp(mut self, stamp: impl Into<Arc<str>>) -> Self {
        self.timestamp = Some(stamp.into());
        self
    }

    /// Mark the body as still streaming.
    #[must_use]
    pub fn streaming(mut self) -> Self {
        self.stream = StreamState::Streaming;
        self
    }
}

/// Provides keyed messages on demand without requiring a preallocated transcript vector.
pub trait MessageProvider {
    /// Returns the logical message count, which may exceed the number currently realized.
    fn len(&self) -> u64;

    /// Whether the provider currently holds no messages.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fetch the message at a logical index, realising it on demand. Returns
    /// `None` past the end.
    fn message(&self, index: u64) -> Option<ChatMessage>;

    /// Locates a stable key in the current logical sequence.
    ///
    /// Implementations should use procedural decoding or an index rather than materializing preceding messages; `None` means absent, evicted, or unloaded.
    fn index_of_key(&self, key: &MessageKey) -> Option<u64>;
}

/// Coalesces appended text into at most one delta per tick and an optional final flush.
///
/// The byte cap is only a pressure signal and never truncates content. Each presentation owns only the newly appended range; [`Self::body`] exposes the cumulative text for cold rebuilds.
#[derive(Debug, Clone)]
pub struct StreamCoalescer {
    buffer: String,
    // `presented` is always a UTF-8 boundary because it advances only to `buffer.len()` after whole-string appends.
    presented: usize,
    pending_bytes: usize,
    // Set by every push, including an empty chunk, and cleared by tick, flush, or begin.
    dirty: bool,
    byte_cap: usize,
    updates: u64,
    presented_bytes: u64,
}

/// A newly appended text delta and an indication that it completed the stream.
///
/// Append `delta` to existing message text; it is not the cumulative body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    /// Text appended since the preceding presentation.
    pub delta: String,
    /// `true` if this was the final flush (stream ended / cancelled).
    pub final_flush: bool,
}

impl StreamCoalescer {
    /// Creates a coalescer with a soft pending-byte cap.
    ///
    /// A zero cap is clamped to one; use a large value for an effectively unbounded cap.
    #[must_use]
    pub fn new(byte_cap: usize) -> Self {
        StreamCoalescer {
            buffer: String::new(),
            presented: 0,
            pending_bytes: 0,
            dirty: false,
            byte_cap: byte_cap.max(1),
            updates: 0,
            presented_bytes: 0,
        }
    }

    /// The soft byte cap between frames.
    #[must_use]
    pub fn byte_cap(&self) -> usize {
        self.byte_cap
    }

    /// Total presentation updates emitted so far.
    #[must_use]
    pub fn updates(&self) -> u64 {
        self.updates
    }

    /// Bytes accumulated since the last presentation update.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// Returns whether a chunk has been pushed since the last tick, flush, or stream reset.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Appends a chunk without presenting it; presentation occurs only on [`Self::tick`] or [`Self::flush`].
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.pending_bytes += chunk.len();
        self.dirty = true;
    }

    /// Presents one pending delta at this frame boundary, or `None` when nothing is pending.
    pub fn tick(&mut self) -> Option<Presentation> {
        if !self.dirty {
            return None;
        }
        self.pending_bytes = 0;
        self.dirty = false;
        self.updates += 1;
        Some(self.take_delta(false))
    }

    fn take_delta(&mut self, final_flush: bool) -> Presentation {
        let delta = self
            .buffer
            .get(self.presented..)
            .unwrap_or_default()
            .to_owned();
        self.presented = self.buffer.len();
        self.presented_bytes += delta.len() as u64;
        Presentation { delta, final_flush }
    }

    /// Reports whether pending bytes have reached the soft cap.
    ///
    /// This does not truncate or mutate the body.
    #[must_use]
    pub fn over_cap(&self) -> bool {
        self.pending_bytes >= self.byte_cap
    }

    /// Presents the pending tail as a final delta.
    ///
    /// Returns `None` when nothing is pending, making repeated flushes idempotent.
    pub fn flush(&mut self) -> Option<Presentation> {
        if !self.dirty {
            return None;
        }
        self.pending_bytes = 0;
        self.dirty = false;
        self.updates += 1;
        Some(self.take_delta(true))
    }

    /// Returns the total bytes emitted across all presentation deltas.
    ///
    /// Since each buffered byte is presented once, this grows linearly with presented content.
    #[must_use]
    pub fn presented_bytes(&self) -> u64 {
        self.presented_bytes
    }

    /// The full body accumulated so far, presented or not.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.buffer
    }

    /// Clears stream-local text and pending state for a new stream.
    ///
    /// The byte-cap policy and lifetime update counters are preserved.
    pub fn begin(&mut self) {
        self.buffer.clear();
        self.presented = 0;
        self.pending_bytes = 0;
        self.dirty = false;
    }
}

/// Whether the view follows the newest message or remains released with an unread count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Follow {
    /// Pinned to newest; new arrivals scroll into view.
    #[default]
    Following,
    /// Detached; new arrivals increment `unread` without moving the anchor.
    Released {
        /// Messages that arrived while released and have not been seen.
        unread: u32,
    },
}

impl Follow {
    /// The starting state: pinned to the newest message.
    #[must_use]
    pub fn following() -> Self {
        Follow::Following
    }

    /// Whether currently pinned to the newest message.
    #[must_use]
    pub fn is_following(self) -> bool {
        matches!(self, Follow::Following)
    }

    /// The unread count. `0` while following.
    #[must_use]
    pub fn unread(self) -> u32 {
        match self {
            Follow::Following => 0,
            Follow::Released { unread } => unread,
        }
    }

    /// Releases following while preserving any existing unread count.
    #[must_use]
    pub fn release(self) -> Self {
        match self {
            Follow::Following => Follow::Released { unread: 0 },
            released => released,
        }
    }

    /// Increments unread count saturatingly while released; following remains unchanged.
    #[must_use]
    pub fn on_new_message(self) -> Self {
        match self {
            Follow::Following => Follow::Following,
            Follow::Released { unread } => Follow::Released {
                unread: unread.saturating_add(1),
            },
        }
    }

    /// Resumes following and clears the unread count.
    #[must_use]
    pub fn rejoin(self) -> Self {
        Follow::Following
    }
}

/// A two-watermark retention policy that drains to `low` only after count strictly exceeds `high`.
///
/// The invariant `low < high` creates a stateless dead band between evictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    high: u64,
    low: u64,
}

impl Retention {
    /// Creates a retention policy with `low < high`.
    ///
    /// # Panics
    ///
    /// Panics when `low >= high`. A zero low-water mark is allowed.
    #[must_use]
    pub fn new(low: u64, high: u64) -> Self {
        assert!(
            low < high,
            "retention needs low < high for hysteresis (got low={low}, high={high}); \
             low == high is the single-threshold thrash this type prevents"
        );
        Retention { high, low }
    }

    /// The high-water mark.
    #[must_use]
    pub fn high(&self) -> u64 {
        self.high
    }

    /// The low-water mark.
    #[must_use]
    pub fn low(&self) -> u64 {
        self.low
    }

    /// Returns the number to remove from the front, draining to `low` only when `retained > high`.
    pub fn evict_count(&self, retained: u64) -> u64 {
        if retained > self.high {
            retained - self.low
        } else {
            0
        }
    }
}
