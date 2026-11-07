//! Bounded application-owned image service.
//!
//! The service owns a fixed worker pool (never one thread per widget/view),
//! coalesces identical in-flight URLs, fans progress/completion out to
//! generation-stamped subscribers, cancels the underlying transport when the
//! final subscriber leaves, and reuses a byte-bounded LRU of stable decoded
//! iced handles.

use crate::http_cache::{CacheKey, Freshness, HttpCache, HttpCacheConfig};
use crate::{DecodeError, DecodedAsset, decode_static};
use enzui_core::async_image::{
    CompletionOutcome, EncodedImage, EnzAsyncImage, Generation, ImageCompletion,
};
use enzui_core::cancellation::{CancelCause, CancelToken};
use enzui_core::decode_limits::DecodeLimits;
use enzui_core::url_policy::{AddressClass, SafeUrl, TransportLimits, UrlPolicy, UrlVerdict};
use iced_core::image::Handle;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::bounded_drop::{DROP_JOIN_BUDGET, bounded_drop_join};

/// One subscriber to a possibly-coalesced image request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriberId(u64);

impl SubscriberId {
    /// Raw identifier for diagnostics.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A policy-checked remote URL whose raw query is private and whose
/// `Display`/`Debug` are always redacted.
#[derive(Clone)]
pub struct RemoteRequest {
    raw: Arc<str>,
    safe: SafeUrl,
}

impl RemoteRequest {
    /// Validate a raw request URL through the shared core policy.
    ///
    /// # Errors
    ///
    /// The [`UrlVerdict`] when `policy` refuses `raw`.
    pub fn checked(raw: &str, policy: &UrlPolicy) -> Result<Self, UrlVerdict> {
        Ok(Self {
            raw: Arc::from(raw),
            safe: policy.check_url(raw)?,
        })
    }

    #[cfg(feature = "curl-transport")]
    pub(crate) fn from_checked(raw: String, safe: SafeUrl) -> Self {
        Self {
            raw: Arc::from(raw),
            safe,
        }
    }

    /// The already-screened URL components used for DNS/address policy.
    #[must_use]
    pub fn safe_url(&self) -> &SafeUrl {
        &self.safe
    }

    /// Redacted form safe for logs.
    #[must_use]
    pub fn redacted(&self) -> String {
        self.safe.redacted()
    }

    /// The full URL to hand to the transport, credentials already refused by
    /// [`RemoteRequest::checked`].
    ///
    /// Public because a [`Transport`] implementation outside this crate cannot
    /// otherwise learn *which* resource it was asked for: `safe_url()` is
    /// redacted for logging, and `raw` is private. Without this an out-of-crate
    /// transport can only ever return one canned body regardless of the request.
    ///
    /// Prefer [`RemoteRequest::redacted`] for anything that will be logged — this
    /// string may contain a query string.
    #[must_use]
    pub fn transport_url(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for RemoteRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.safe.redacted())
    }
}

impl fmt::Debug for RemoteRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RemoteRequest({})", self.safe.redacted())
    }
}

/// Thread-safe, first-writer-wins cancellation shared with the transport.
///
/// This compatibility wrapper preserves the resource transport's established
/// API while delegating to the framework's one shared [`CancelToken`] state
/// machine. It does not maintain a second atomic or encode cancellation causes
/// independently.
#[derive(Clone, Debug)]
pub struct CancelSignal {
    token: CancelToken,
}

impl CancelSignal {
    /// A live signal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancelToken::new(),
        }
    }

    /// Cancel with `cause`; `true` only for the first winning cause.
    pub fn cancel(&self, cause: CancelCause) -> bool {
        self.token.cancel(cause)
    }

    /// Winning cause, or `None` while live.
    #[must_use]
    pub fn cause(&self) -> Option<CancelCause> {
        self.token.cause()
    }

    /// Whether cancellation has reached its terminal state.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_terminal()
    }
}

impl Default for CancelSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Progress observed by a transport while streaming encoded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    /// Bytes received so far.
    pub received: u64,
    /// Declared total, when the server supplied one.
    pub total: Option<u64>,
}

/// Encoded response returned by a transport before bounded decode.
#[derive(Clone)]
pub struct TransportResponse {
    /// HTTP status code.
    pub status: u16,
    /// Encoded body, already bounded by [`TransportLimits::max_bytes`].
    pub body: Vec<u8>,
    /// Redacted final URL after manually validated redirects.
    pub final_url: SafeUrl,
    /// Connected peer class, observed after a pre-screened/pinned connection.
    pub peer_class: AddressClass,
    /// Response headers needed by the later HTTP-cache/revalidation slice.
    pub headers: Vec<(String, String)>,
}

// Response bytes and header values are remote-controlled and can contain
// credentials (notably Set-Cookie), signed redirect targets, or private
// application data. Preserve the useful routing/status metadata and bounded
// sizes without making `Debug` a second exfiltration path.
impl fmt::Debug for TransportResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportResponse")
            .field("status", &self.status)
            .field(
                "body",
                &format_args!("[redacted; {} bytes]", self.body.len()),
            )
            .field("final_url", &self.final_url)
            .field("peer_class", &self.peer_class)
            .field(
                "headers",
                &format_args!("[redacted; {} entries]", self.headers.len()),
            )
            .finish()
    }
}

/// A redacted transport failure. No variant stores a raw URL or query string.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportError {
    /// URL-stage policy refusal.
    Url(UrlVerdict),
    /// DNS produced a connected destination class forbidden by policy.
    AddressBlocked(AddressClass),
    /// Name resolution failed or produced no address.
    Dns,
    /// The real transfer was aborted through the transport cancellation hook.
    Cancelled(CancelCause),
    /// Stream exceeded the configured encoded-byte ceiling.
    ByteLimit {
        /// Bytes observed when the transfer was stopped.
        received: u64,
        /// Inclusive ceiling.
        limit: u64,
    },
    /// Connect/read/total deadline expired.
    Timeout,
    /// Redirect lacked a usable Location header.
    Redirect,
    /// HTTP status was not a success.
    HttpStatus(u16),
    /// Non-identity HTTP content encoding is excluded so the encoded ceiling is
    /// measured on the exact image bytes passed to the decoder.
    ContentEncoding,
    /// Response-header accumulation exceeded the shared ceiling.
    ///
    /// Distinct from [`TransportError::Backend`] so callers can identify a
    /// hostile header flood without inspecting libcurl error codes.
    HeaderLimit(crate::header_limits::HeaderLimitKind),
    /// libcurl failed; only its numeric code is retained to avoid embedding a
    /// URL from an error buffer.
    Backend(u32),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Url(v) => write!(f, "url refused: {}", v.reason()),
            TransportError::AddressBlocked(c) => write!(f, "connected address blocked: {c:?}"),
            TransportError::Dns => f.write_str("name resolution failed"),
            TransportError::Cancelled(c) => write!(f, "transfer cancelled: {c:?}"),
            TransportError::ByteLimit { received, limit } => {
                write!(f, "encoded stream too large: {received} > {limit}")
            }
            TransportError::Timeout => f.write_str("transport deadline expired"),
            TransportError::Redirect => f.write_str("invalid redirect"),
            TransportError::HttpStatus(code) => write!(f, "HTTP status {code}"),
            TransportError::ContentEncoding => f.write_str("HTTP content encoding unsupported"),
            TransportError::HeaderLimit(kind) => {
                write!(f, "response header limit exceeded: {kind:?}")
            }
            TransportError::Backend(code) => write!(f, "transport backend error {code}"),
        }
    }
}

/// Blocking transport seam. The service only calls it on its bounded worker
/// pool, never on the application/UI thread.
pub trait Transport: Send + Sync + 'static {
    /// Fetch one policy-checked remote request. Implementations must observe
    /// `cancel` at the actual transport, enforce `limits` while streaming, and
    /// invoke `progress` as bytes arrive.
    ///
    /// A panic here is contained: the service catches it, resolves the
    /// flight with [`ServiceError::WorkerPanic`] and keeps the worker, so an
    /// unwinding implementation costs one failed request rather than a lost
    /// pool thread. Returning a typed [`TransportError`] is still the supported
    /// way to report failure — the containment is a backstop, not a channel.
    ///
    /// # Errors
    ///
    /// A [`TransportError`] for a policy refusal, network failure, a limit breach, or cancellation.
    fn fetch(
        &self,
        request: &RemoteRequest,
        policy: &UrlPolicy,
        limits: &TransportLimits,
        cancel: &CancelSignal,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<TransportResponse, TransportError>;

    /// Conditional fetch: like [`Transport::fetch`], but sends
    /// `If-None-Match: <etag>` so the origin may answer `304 Not Modified` with
    /// no body.
    ///
    /// **Defaulted to an unconditional [`Transport::fetch`]** so existing
    /// implementations keep compiling and keep working: ignoring the validator
    /// is always *correct*, merely less efficient — the origin re-sends a body
    /// that the cache already had. A transport that cannot express conditional
    /// requests therefore degrades to a plain fetch rather than failing, and
    /// only the disk-cache fast path is lost.
    ///
    /// An implementation that overrides this must return status `304` with
    /// an empty body when the validator matches; the service treats any other
    /// status as a full response.
    ///
    /// # Errors
    ///
    /// As [`Transport::fetch`].
    fn fetch_conditional(
        &self,
        request: &RemoteRequest,
        policy: &UrlPolicy,
        limits: &TransportLimits,
        cancel: &CancelSignal,
        progress: &mut dyn FnMut(TransferProgress),
        _validators: &Validators<'_>,
    ) -> Result<TransportResponse, TransportError> {
        self.fetch(request, policy, limits, cancel, progress)
    }
}

/// Cache validators available for a conditional request.
///
/// A struct rather than more positional `Option<&str>` parameters: two adjacent
/// `Option<&str>` arguments are trivially swappable at a call site, and swapping
/// these two would send a date as an entity tag — which most origins answer with
/// a full body rather than an error, so the mistake would show up only as a cache
/// that never revalidates.
#[derive(Debug, Clone, Copy, Default)]
pub struct Validators<'a> {
    /// `ETag`, sent as `If-None-Match`.
    pub etag: Option<&'a str>,
    /// `Last-Modified`, sent as `If-Modified-Since`.
    pub last_modified: Option<&'a str>,
}

impl Validators<'_> {
    /// Whether there is anything to revalidate with.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

/// Application-owned service configuration.
#[derive(Debug, Clone, Copy)]
pub struct ServiceConfig {
    /// URL, redirect, and connected-address policy.
    pub url_policy: UrlPolicy,
    /// Streaming and timeout policy.
    pub transport_limits: TransportLimits,
    /// Encoded/dimension/pixel/RGBA decode limits.
    pub decode_limits: DecodeLimits,
    /// Fixed number of blocking worker threads. Clamped to at least one.
    pub worker_threads: usize,
    /// Maximum decoded RGBA bytes retained in the stable-handle LRU. `0` means
    /// retain nothing, not unlimited.
    pub decoded_cache_bytes: u64,
    /// Maximum number of concurrently admitted flights — queued plus running.
    ///
    /// This bounds *admission*, which the fixed worker pool does not: the pool
    /// bounds how many transfers execute at once, while every accepted request
    /// retains a `Job`, a `Flight` and a `Subscription` regardless. Without
    /// this bound a hostile or merely large backlog grows memory without limit
    /// (100,000 unique URLs take RSS from 9 MiB to about 250 MiB).
    ///
    /// **`0` means zero, literally** — no new flight may be admitted. It is
    /// expressible intent, not a synonym for unlimited; an effectively
    /// unbounded queue is a large number. This follows `chat_stream`'s reading
    /// of `byte_cap`.
    ///
    /// Joining an already in-flight key costs no *flight* admission and so is
    /// allowed even at this capacity, subject to
    /// [`ServiceConfig::max_joined_subscribers_per_flight`].
    pub max_inflight_jobs: usize,
    /// Maximum additional subscribers that may join one existing flight.
    ///
    /// A flight's starting subscriber is not counted here. Consequently one
    /// flight retains at most `1 + max_joined_subscribers_per_flight`
    /// subscriptions and can materialize at most that many terminal events.
    /// This is independent of the flight and request-byte budgets because a
    /// repeated same-URL request adds neither a job nor another URL allocation,
    /// but still retains a subscriber and eventual event.
    ///
    /// **`0` means zero, literally**: the subscriber that starts a flight is
    /// allowed, but no request may join it.
    pub max_joined_subscribers_per_flight: usize,
    /// Encoded on-disk HTTP cache policy.
    ///
    /// `max_bytes: 0` disables it entirely (see [`HttpCacheConfig`]).
    pub http_cache: HttpCacheConfig,
    /// Maximum request bytes retained across concurrently admitted flights.
    ///
    /// A count budget alone does not bound memory, because a URL has no length
    /// limit (query payloads of about 1 KiB are ordinary). This bounds
    /// the retained payload independently of the count.
    ///
    /// **`0` means zero, literally**, on the same reading as
    /// [`ServiceConfig::max_inflight_jobs`].
    pub max_inflight_request_bytes: u64,
    /// Maximum events waiting for [`ImageService::poll`].
    ///
    /// Flights bound their own terminal events, but a cache hit answers at
    /// once, so repeated warm requests without a poll would otherwise queue
    /// without limit. A warm request past this is refused as
    /// [`BusyReason::ResultBudget`]; polling frees room.
    pub max_pending_results: usize,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            url_policy: UrlPolicy::DEFAULT,
            transport_limits: TransportLimits::DEFAULT,
            decode_limits: DecodeLimits::DEFAULT,
            worker_threads: 2,
            decoded_cache_bytes: 256 * 1024 * 1024,
            // Generous enough that an ordinary gallery never sees `Busy`, small
            // enough that a runaway backlog is refused long before it costs
            // hundreds of MiB. The measured leak reached 100,000 flights.
            http_cache: HttpCacheConfig::DEFAULT,
            max_inflight_jobs: 512,
            // One thumbnail may legitimately be visible in many places, but a
            // stalled URL must not retain an unbounded fan-out.
            max_joined_subscribers_per_flight: 64,
            max_inflight_request_bytes: 4 * 1024 * 1024,
            max_pending_results: 1024,
        }
    }
}

/// Immutable identity for one admitted flight.
///
/// Queued work is keyed by this rather than by URL. A URL is not an
/// identity: cancelling a flight and immediately re-requesting the same URL
/// produces two distinct flights, and a URL-keyed queue lets the late
/// progress/completion of the cancelled one be delivered to the new one. The id
/// is allocated once at admission and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FlightId(u64);

impl FlightId {
    /// Raw counter value, for logging and tests.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Which bounded resource refused an image request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyReason {
    /// The queued-plus-running flight count reached `max_inflight_jobs`.
    FlightBudget,
    /// Retained URL bytes reached `max_inflight_request_bytes`.
    RequestByteBudget,
    /// One existing flight reached `max_joined_subscribers_per_flight` joins.
    SubscriberBudget,
    /// `max_pending_results` events wait unpolled.
    ResultBudget,
}

/// What requesting a URL did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDisposition {
    /// Started a new transport/decode flight.
    Started(SubscriberId),
    /// Joined an identical existing flight (single-flight coalescing).
    Joined(SubscriberId),
    /// Reused a stable decoded handle from the memory LRU.
    CacheHit(SubscriberId),
    /// Admission was refused by a specific bounded-resource budget.
    ///
    /// No subscriber was allocated, no flight was registered and no event will
    /// ever be emitted for this call, so there is nothing to cancel. The caller
    /// is expected to retry later or drop the request; this is deliberately an
    /// observable, typed disposition rather than a panic or a silent drop.
    Busy(BusyReason),
}

impl RequestDisposition {
    /// Subscriber ID assigned to this request, or `None` when admission was
    /// refused.
    ///
    /// Returns `Option` because [`RequestDisposition::Busy`] genuinely has no
    /// subscriber. Synthesising one would be worse than this signature change:
    /// callers would hold an id that never produces an event and can never be
    /// cancelled.
    #[must_use]
    pub const fn subscriber(self) -> Option<SubscriberId> {
        match self {
            RequestDisposition::Started(id)
            | RequestDisposition::Joined(id)
            | RequestDisposition::CacheHit(id) => Some(id),
            RequestDisposition::Busy(_) => None,
        }
    }
}

/// Generation-stamped event emitted to one subscriber.
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    /// Encoded streaming progress.
    Progress {
        /// Subscriber receiving the event.
        subscriber: SubscriberId,
        /// Core request generation.
        generation: Generation,
        /// Transfer progress.
        progress: TransferProgress,
    },
    /// Validated stable-handle completion ready for the core reducer.
    Ready {
        /// Subscriber receiving the event.
        subscriber: SubscriberId,
        /// Completion to feed into [`EnzAsyncImage::complete`].
        completion: ImageCompletion<Handle>,
        /// Whether this reused the decoded LRU.
        cache_hit: bool,
    },
    /// Redacted transport/decode failure.
    Failed {
        /// Subscriber receiving the event.
        subscriber: SubscriberId,
        /// Request generation.
        generation: Generation,
        /// Failure.
        error: ServiceError,
    },
    /// Subscriber cancellation; when this was the final subscriber, the same
    /// signal aborts the actual transport.
    Cancelled {
        /// Cancelled subscriber.
        subscriber: SubscriberId,
        /// Request generation.
        generation: Generation,
        /// Winning cause.
        cause: CancelCause,
    },
}

impl ServiceEvent {
    /// Feed a ready/failure event through the real core consumer. Progress and
    /// cancellation are not terminal reducer completions and return `false`.
    pub fn apply_to(self, state: &mut EnzAsyncImage<Handle>) -> bool {
        match self {
            ServiceEvent::Ready { completion, .. } => state.complete(completion),
            ServiceEvent::Failed {
                generation, error, ..
            } => state.complete(ImageCompletion {
                generation,
                outcome: CompletionOutcome::Failed(error.to_string()),
            }),
            ServiceEvent::Progress { .. } | ServiceEvent::Cancelled { .. } => false,
        }
    }
}

/// Redacted service failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// Transport-stage failure.
    Transport(TransportError),
    /// Decode-stage failure.
    Decode(DecodeError),
    /// A worker thread could not be created, so the bounded pool was never
    /// started and [`ImageService::new`] returned nothing.
    ///
    /// Thread creation is an OS resource request, not a programming error:
    /// it fails on fd/thread exhaustion, under a restrictive `RLIMIT_NPROC`,
    /// and in containers with low pid limits. Those are runtime conditions an
    /// application can report and degrade on, so they are returned rather than
    /// panicked. Any workers already spawned when this occurs are terminated
    /// and joined *before* the error is handed back — see [`ImageService::new`].
    WorkerSpawn {
        /// Index of the worker whose creation failed. Workers `0..index` were
        /// created and have already been shut down and joined.
        index: usize,
        /// Classifying OS error kind. The originating [`std::io::Error`] is
        /// neither `Clone` nor `Eq` while `ServiceError` is both, so the kind
        /// is retained rather than the error — the same "keep the classifier,
        /// drop the payload" choice [`TransportError::Backend`] makes.
        kind: std::io::ErrorKind,
    },
    /// A pool worker panicked while executing this flight's job.
    ///
    /// The payload is discarded because it may contain request data.
    WorkerPanic,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Transport(e) => e.fmt(f),
            ServiceError::Decode(e) => e.fmt(f),
            ServiceError::WorkerSpawn { index, kind } => {
                write!(f, "image worker {index} could not be spawned: {kind}")
            }
            ServiceError::WorkerPanic => f.write_str("image worker panicked"),
        }
    }
}

#[derive(Clone)]
struct Subscriber {
    id: SubscriberId,
    generation: Generation,
}

struct Flight {
    /// Immutable identity, allocated at admission.
    id: FlightId,
    subscribers: Vec<Subscriber>,
    cancel: CancelSignal,
    /// Request bytes charged to the admission byte budget by this flight.
    charged_bytes: u64,
    /// Whether this flight is still sitting in the pending queue (as opposed to
    /// handed to the worker pool). Only a queued flight can be withdrawn.
    queued: bool,
}

struct Subscription {
    key: Arc<str>,
    generation: Generation,
}

struct CacheEntry {
    asset: DecodedAsset,
    /// Monotone recency stamp; also this entry's key into `lru`.
    stamp: u64,
}

struct Job {
    /// Immutable flight identity. Carried through the worker so completions and
    /// progress can be matched to the exact flight that produced them, rather
    /// than to whatever flight currently happens to hold the same URL.
    flight: FlightId,
    key: Arc<str>,
    request: RemoteRequest,
    cancel: CancelSignal,
}

enum WorkerMessage {
    Complete {
        flight: FlightId,
        key: Arc<str>,
        result: Result<DecodedAsset, ServiceError>,
    },
}

/// Latest-value progress shared directly between workers and the service.
///
/// A registered `None` slot identifies a live flight that has not reported
/// progress since the last poll. Keeping registration in the same lock as the
/// value prevents a callback racing with cancellation from recreating a stale
/// slot after the flight has retired. Lock poisoning is recoverable here: the
/// map remains structurally valid, and dropping all future progress because an
/// unrelated callback panicked would turn one worker failure into a permanent
/// service failure.
#[derive(Default)]
struct ProgressMailbox {
    slots: Mutex<HashMap<FlightId, Option<TransferProgress>>>,
}

impl ProgressMailbox {
    fn with_slots<R>(
        &self,
        f: impl FnOnce(&mut HashMap<FlightId, Option<TransferProgress>>) -> R,
    ) -> R {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut slots)
    }

    fn register(&self, flight: FlightId) {
        self.with_slots(|slots| {
            slots.insert(flight, None);
        });
    }

    fn update(&self, flight: FlightId, progress: TransferProgress) {
        self.with_slots(|slots| {
            if let Some(slot) = slots.get_mut(&flight) {
                *slot = Some(progress);
            }
        });
    }

    fn take(&self, flight: FlightId) -> Option<TransferProgress> {
        self.with_slots(|slots| slots.get_mut(&flight).and_then(Option::take))
    }

    fn remove(&self, flight: FlightId) {
        self.with_slots(|slots| {
            slots.remove(&flight);
        });
    }

    #[cfg(test)]
    fn sizes(&self) -> (usize, usize) {
        self.with_slots(|slots| {
            let retained = slots.values().filter(|slot| slot.is_some()).count();
            (slots.len(), retained)
        })
    }
}

/// Bounded, application-owned off-thread image service.
pub struct ImageService {
    config: ServiceConfig,
    job_tx: Option<Sender<Job>>,
    message_rx: Receiver<WorkerMessage>,
    workers: Vec<JoinHandle<()>>,
    flights: HashMap<Arc<str>, Flight>,
    subscriptions: HashMap<SubscriberId, Subscription>,
    pending: VecDeque<ServiceEvent>,
    cache: HashMap<Arc<str>, CacheEntry>,
    // Recency index, stamp -> key, oldest first: touch and eviction are
    // O(log n) on a cache holding thousands of thumbnails; `cache` keeps O(1)
    // lookup.
    lru: BTreeMap<u64, Arc<str>>,
    /// Next recency stamp. Monotone; never reused.
    ///
    /// `u64` at one stamp per cache hit cannot wrap in any realistic process
    /// lifetime (2^64 hits), so there is deliberately no rebuild-on-wrap branch
    /// — an unreachable branch is untestable, and this crate already refuses to
    /// ship those (see the `PixelOverflow` decision).
    next_stamp: u64,
    cache_bytes: u64,
    next_subscriber: u64,
    /// Jobs admitted but not yet handed to the pool.
    ///
    /// Service-owned rather than pushed straight into the `mpsc` channel,
    /// because cancelled work must be removable. Once a `Job` is in the
    /// channel nothing can take it back out: a worker has to dequeue it and
    /// observe the cancellation, so a hostile backlog stays resident and drop
    /// has to walk it. Holding the queue here makes withdrawal O(n) but
    /// possible, and makes drop a discard rather than a drain.
    pending_jobs: VecDeque<Job>,
    /// Number of jobs handed to the pool and not yet completed.
    running: usize,
    /// Request bytes currently charged against the admission byte budget.
    inflight_request_bytes: u64,
    /// Monotonic flight-id source. Never reused, so a retired flight's late
    /// messages can always be told apart from a live flight's.
    next_flight: u64,
    /// Newest unreported progress per live flight.
    ///
    /// Workers overwrite one shared slot before touching the terminal-only
    /// channel, so callback bursts are bounded even while the UI is not polling.
    /// Keyed by [`FlightId`] rather than URL so a retired flight's stale progress
    /// can never be delivered to the same-URL retry that replaced it.
    progress_mailbox: Arc<ProgressMailbox>,
    /// Flights cancelled while running: removed from the URL index but still
    /// owned by a worker, so their byte charge is still outstanding.
    ///
    /// Needed because the charge cannot be released at cancellation time (the
    /// work is still executing) and the flight is no longer reachable by URL, so
    /// the arriving completion has nothing else to look it up by.
    retired: HashMap<FlightId, u64>,
    /// Admissions refused because the count budget was full.
    rejected_count_budget: u64,
    /// Admissions refused because the byte budget was full.
    rejected_byte_budget: u64,
    /// Same-flight joins refused because the subscriber budget was full.
    rejected_subscriber_budget: u64,
}

/// Thread-name prefix for the bounded image worker pool.
///
/// Kept short deliberately: Linux truncates a thread's `comm` to 15 bytes, and
/// `enzui-image-0` is 13, so the index stays visible in `top`, `gdb` and
/// `/proc/self/task/*/comm`.
const WORKER_THREAD_PREFIX: &str = "enzui-image-";

impl ImageService {
    /// [`ImageService::new`] with the encoded HTTP cache rooted at an explicit
    /// directory instead of the XDG cache location.
    ///
    /// Exists so tests never write to the developer's real `~/.cache`, and so an
    /// application that manages its own cache layout can place it deliberately.
    ///
    /// # Errors
    ///
    /// As [`ImageService::new`].
    pub fn new_with_cache_root<T: Transport>(
        transport: T,
        config: ServiceConfig,
        cache_root: &Path,
    ) -> Result<Self, ServiceError> {
        Self::new_with_spawn_gate_named(
            transport,
            config,
            WORKER_THREAD_PREFIX,
            Some(cache_root),
            |_| Ok(()),
        )
    }

    /// Spawn a fixed worker pool around `transport`. The service, not a widget,
    /// owns these threads and joins them on drop.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::WorkerSpawn`] when the OS refuses to create a
    /// worker thread. That is a runtime resource condition (fd/thread
    /// exhaustion, a restrictive `RLIMIT_NPROC`, a container pid limit), not a
    /// programming error, so it is reported rather than panicked.
    ///
    /// **On that path the partially created pool is fully torn down before the
    /// error is returned.** This matters more than the signature change: a
    /// panic in the middle of the spawn loop leaves workers `0..index` running
    /// and blocked on the job channel, and because `Self` is never constructed
    /// [`Drop`] never runs — so the very promise this type makes ("owns these
    /// threads and joins them on drop") is broken exactly when it is needed.
    /// The threads would leak, holding the transport `Arc` alive with them.
    pub fn new<T: Transport>(transport: T, config: ServiceConfig) -> Result<Self, ServiceError> {
        // Production spawns every worker; the gate exists so the failure path
        // below is reachable from a test. See `new_with_spawn_gate`.
        Self::new_with_spawn_gate(transport, config, |_| Ok(()))
    }

    /// [`ImageService::new`] with a gate consulted before each worker spawn.
    ///
    /// The gate is the hermetic seam for the spawn-failure path. Genuinely
    /// exhausting the OS thread table inside a unit test is neither hermetic
    /// (it perturbs every other test in the process) nor reliable, so the test
    /// injects the `io::Error` the OS would have produced. Both the gate's
    /// error and a real [`std::thread::Builder::spawn`] error converge on the
    /// same `shutdown_partial_pool` cleanup below, so a test driving the
    /// gate exercises the production cleanup rather than a parallel copy of it.
    fn new_with_spawn_gate<T: Transport>(
        transport: T,
        config: ServiceConfig,
        gate: impl FnMut(usize) -> std::io::Result<()>,
    ) -> Result<Self, ServiceError> {
        Self::new_with_spawn_gate_named(transport, config, WORKER_THREAD_PREFIX, None, gate)
    }

    /// [`ImageService::new_with_spawn_gate`] with the worker thread-name prefix
    /// supplied explicitly.
    ///
    /// Worker threads are named for debuggers and crash dumps. The prefix is a
    /// parameter purely so a leak test can give its own pool a unique name
    /// and count only its threads: `cargo test` runs tests concurrently in one
    /// process, so a shared name makes "how many of my workers are live?"
    /// unanswerable, and the assertion then fails for a sibling test's reasons.
    /// Production always passes [`WORKER_THREAD_PREFIX`].
    fn new_with_spawn_gate_named<T: Transport>(
        transport: T,
        config: ServiceConfig,
        thread_prefix: &str,
        cache_root: Option<&Path>,
        mut gate: impl FnMut(usize) -> std::io::Result<()>,
    ) -> Result<Self, ServiceError> {
        let transport = Arc::new(transport);
        // One cache shared by every worker. Opening it is infallible: an
        // unwritable directory disables caching rather than failing construction.
        let cache = {
            let cache = match cache_root {
                Some(root) => HttpCache::open_at(root, config.http_cache),
                None => HttpCache::open(config.http_cache),
            };
            cache.is_enabled().then(|| Arc::new(cache))
        };
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let (message_tx, message_rx) = mpsc::channel();
        let progress_mailbox = Arc::new(ProgressMailbox::default());
        let mut workers = Vec::new();

        for index in 0..config.worker_threads.max(1) {
            let jobs = Arc::clone(&job_rx);
            let messages = message_tx.clone();
            let worker_progress = Arc::clone(&progress_mailbox);
            let transport = Arc::clone(&transport);
            let worker_config = config;
            let worker_cache = cache.clone();

            let spawned = gate(index).and_then(|()| {
                std::thread::Builder::new()
                    .name(format!("{thread_prefix}{index}"))
                    .spawn(move || {
                        worker_loop(
                            jobs,
                            messages,
                            worker_progress,
                            transport,
                            worker_config,
                            worker_cache,
                        );
                    })
            });

            match spawned {
                Ok(handle) => workers.push(handle),
                Err(error) => {
                    // Tear the partial pool down before returning. `job_tx` is
                    // moved in so it is dropped *inside* the helper, which is
                    // what unblocks the already-running workers.
                    shutdown_partial_pool(job_tx, workers);
                    return Err(ServiceError::WorkerSpawn {
                        index,
                        kind: error.kind(),
                    });
                }
            }
        }
        drop(message_tx);

        Ok(Self {
            config,
            job_tx: Some(job_tx),
            message_rx,
            workers,
            flights: HashMap::new(),
            subscriptions: HashMap::new(),
            pending: VecDeque::new(),
            cache: HashMap::new(),
            lru: BTreeMap::new(),
            next_stamp: 1,
            cache_bytes: 0,
            next_subscriber: 1,
            pending_jobs: VecDeque::new(),
            running: 0,
            inflight_request_bytes: 0,
            next_flight: 1,
            progress_mailbox,
            retired: HashMap::new(),
            rejected_count_budget: 0,
            rejected_byte_budget: 0,
            rejected_subscriber_budget: 0,
        })
    }

    /// Request a remote image for `generation`.
    ///
    /// Identical raw URLs coalesce while in flight. A decoded cache hit queues a
    /// ready event with the same stable handle ID and performs no transport.
    ///
    /// # Errors
    ///
    /// The [`UrlVerdict`] when the configured policy refuses `raw_url`.
    pub fn request(
        &mut self,
        raw_url: &str,
        generation: Generation,
    ) -> Result<RequestDisposition, UrlVerdict> {
        let request = RemoteRequest::checked(raw_url, &self.config.url_policy)?;
        let key = Arc::clone(&request.raw);
        let id = SubscriberId(self.next_subscriber);
        self.next_subscriber = self.next_subscriber.wrapping_add(1).max(1);

        // A cache hit is already terminal: the `Ready` event is queued
        // before this function returns and there is no flight to detach from,
        // so `cancel` would have nothing to do. Registering a subscription for
        // it is therefore dead on arrival, and doing so leaked one entry per
        // hit for the life of the process (the table is keyed by a
        // monotonically increasing id, so entries never collide and never
        // expire). Return before the table is touched at all rather than
        // inserting and immediately removing — there is no window in which the
        // entry is meaningful.
        if let Some(asset) = self.cache.get(&key).map(|entry| entry.asset.clone()) {
            if self.pending.len() >= self.config.max_pending_results {
                return Ok(RequestDisposition::Busy(BusyReason::ResultBudget));
            }
            self.touch_cache(&key);
            self.pending
                .push_back(ready_event(id, generation, &asset, true));
            return Ok(RequestDisposition::CacheHit(id));
        }

        // JOIN IS CHECKED BEFORE FLIGHT ADMISSION, and the order is
        // load-bearing. Joining consumes no flight slot, but it does retain a
        // subscriber and eventually materializes a terminal event. Check that
        // independent budget before either join mutation; a Busy call must
        // leave neither `flight.subscribers` nor `self.subscriptions` behind.
        //
        // `subscribers` includes the request that started the flight, hence the
        // subtraction when comparing against the budget for additional joins.
        if let Some(flight) = self.flights.get_mut(&key) {
            let joined = flight.subscribers.len().saturating_sub(1);
            if joined >= self.config.max_joined_subscribers_per_flight {
                self.rejected_subscriber_budget = self.rejected_subscriber_budget.saturating_add(1);
                return Ok(RequestDisposition::Busy(BusyReason::SubscriberBudget));
            }
            flight.subscribers.push(Subscriber { id, generation });
            self.subscriptions.insert(
                id,
                Subscription {
                    key: Arc::clone(&key),
                    generation,
                },
            );
            return Ok(RequestDisposition::Joined(id));
        }

        // A genuinely new flight must be admitted. Both budgets are checked
        // *before* any state is mutated, so a refusal leaves nothing behind:
        // no subscriber, no flight, no queued job, nothing to cancel.
        let charge = request_bytes(&request);
        if self.admitted_len() >= self.config.max_inflight_jobs {
            self.rejected_count_budget = self.rejected_count_budget.saturating_add(1);
            return Ok(RequestDisposition::Busy(BusyReason::FlightBudget));
        }
        if self.inflight_request_bytes.saturating_add(charge)
            > self.config.max_inflight_request_bytes
        {
            self.rejected_byte_budget = self.rejected_byte_budget.saturating_add(1);
            return Ok(RequestDisposition::Busy(BusyReason::RequestByteBudget));
        }

        // Admitted. From here on the request owns resources and `cancel(id)`
        // must be able to find it.
        self.subscriptions.insert(
            id,
            Subscription {
                key: Arc::clone(&key),
                generation,
            },
        );

        let flight_id = FlightId(self.next_flight);
        self.next_flight = self.next_flight.wrapping_add(1).max(1);
        let cancel = CancelSignal::new();
        self.inflight_request_bytes = self.inflight_request_bytes.saturating_add(charge);
        self.progress_mailbox.register(flight_id);
        self.flights.insert(
            Arc::clone(&key),
            Flight {
                id: flight_id,
                subscribers: vec![Subscriber { id, generation }],
                cancel: cancel.clone(),
                charged_bytes: charge,
                queued: true,
            },
        );
        self.pending_jobs.push_back(Job {
            flight: flight_id,
            key,
            request,
            cancel,
        });
        self.dispatch_ready_jobs();
        Ok(RequestDisposition::Started(id))
    }

    /// Hand queued jobs to the worker pool, up to what the pool can execute.
    ///
    /// The dispatch limit is what makes the queue real. An unbounded `mpsc`
    /// accepts every send instantly, so dispatching on admission left
    /// `pending_jobs` permanently empty: `queued_len()` was always 0 and the
    /// withdrawal path below was unreachable code that no test could enter.
    /// (Measured while building this: 10 admitted requests reported
    /// `queued=0 running=10`.)
    ///
    /// Holding back everything beyond the pool's own width means surplus work
    /// waits *here*, where it can be withdrawn on cancellation and discarded on
    /// drop, instead of inside a channel that can only be emptied by a worker
    /// dequeuing each job and observing its cancellation one at a time.
    fn dispatch_ready_jobs(&mut self) {
        let width = self.config.worker_threads.max(1);
        while self.running < width {
            let Some(job) = self.pending_jobs.pop_front() else {
                break;
            };
            let key = Arc::clone(&job.key);
            let sent = self.job_tx.as_ref().is_some_and(|tx| tx.send(job).is_ok());
            if sent {
                self.running = self.running.saturating_add(1);
                if let Some(flight) = self.flights.get_mut(&key) {
                    flight.queued = false;
                }
                continue;
            }
            // The pool is gone (every worker died). Fail the flight's
            // subscribers rather than silently retaining it forever.
            if let Some(flight) = self.flights.remove(&key) {
                self.progress_mailbox.remove(flight.id);
                self.release_charge(&flight);
                for subscriber in flight.subscribers {
                    self.subscriptions.remove(&subscriber.id);
                    self.pending.push_back(ServiceEvent::Failed {
                        subscriber: subscriber.id,
                        generation: subscriber.generation,
                        error: ServiceError::Transport(TransportError::Backend(0)),
                    });
                }
            }
        }
    }

    /// Return a flight's byte charge to the admission budget.
    fn release_charge(&mut self, flight: &Flight) {
        self.inflight_request_bytes = self
            .inflight_request_bytes
            .saturating_sub(flight.charged_bytes);
    }

    /// Cancel one subscriber. The actual transport is cancelled only when this
    /// was the last subscriber to the single flight.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a queued subscription's flight is always \
                  present"
    )]
    pub fn cancel(&mut self, id: SubscriberId, cause: CancelCause) -> bool {
        let Some(subscription) = self.subscriptions.remove(&id) else {
            return false;
        };
        self.pending.retain(|event| event_subscriber(event) != id);

        if let Some(flight) = self.flights.get_mut(&subscription.key) {
            flight.subscribers.retain(|s| s.id != id);
            if flight.subscribers.is_empty() {
                flight.cancel.cancel(cause);
                let flight_id = flight.id;
                let was_queued = flight.queued;

                // ORDERING INVARIANT — the job is REMOVED FIRST, and only then
                // is the flight (and with it the admission slot) released.
                //
                // If the slot were released before the job was actually gone,
                // the queue would briefly hold a job that no longer counts
                // against the budget, so two fresh admissions could each see a
                // free slot and race into it — admitting `capacity + 1` jobs.
                // Withdrawal must therefore complete before the accounting
                // changes, not after.
                //
                // Only a *queued* job can be withdrawn. One already handed to
                // the pool must be dequeued by a worker, which observes the
                // cancel signal set above; its slot is released when its
                // completion arrives.
                if was_queued {
                    let before = self.pending_jobs.len();
                    self.pending_jobs.retain(|job| job.flight != flight_id);
                    debug_assert_eq!(
                        before - self.pending_jobs.len(),
                        1,
                        "a queued flight must have exactly one queued job"
                    );
                    let removed = self
                        .flights
                        .remove(&subscription.key)
                        .expect("flight was present");
                    self.release_charge(&removed);
                    // Retirement drops the coalescing slot and its live
                    // registration, so a racing callback cannot recreate it.
                    self.progress_mailbox.remove(flight_id);
                } else {
                    // A RUNNING flight cannot be withdrawn — a worker owns it and
                    // must observe the cancel signal — but it must still be
                    // retired from the URL index immediately.
                    //
                    // Leaving it there made a same-URL retry *join a dead
                    // flight*: its transport is already cancelled, so the retry
                    // could only ever resolve as `Cancelled`, and the user's
                    // reload silently could not succeed. Retiring it here means
                    // the retry starts a genuinely new flight with a new
                    // `FlightId`, and the old flight's in-flight completion is
                    // then rejected by the identity check in
                    // `handle_worker_message` rather than being applied to it.
                    //
                    // The admission slot stays charged until that completion
                    // arrives, because the worker is still executing the work.
                    let charge = flight.charged_bytes;
                    self.retired.insert(flight_id, charge);
                    self.flights.remove(&subscription.key);
                    // Retirement drops the coalescing slot and registration:
                    // this flight's later callbacks cannot reach the retry.
                    self.progress_mailbox.remove(flight_id);
                }
            }
        }

        self.pending.push_back(ServiceEvent::Cancelled {
            subscriber: id,
            generation: subscription.generation,
            cause,
        });

        // A cancellation can free pool capacity, so queued work must be given the
        // chance to take it. Without this the pool sits idle with jobs waiting:
        // `running` only falls when a completion arrives, and dispatch was only
        // driven from `request` and from the `Complete` arm, so cancelling the
        // running flights stalled the queue permanently. Measured via
        // the image-service performance gate, which reached 4 of 100 URLs before hanging.
        self.dispatch_ready_jobs();
        true
    }

    /// Drain currently available progress/completion/cancellation events.
    pub fn poll(&mut self) -> Vec<ServiceEvent> {
        while let Ok(message) = self.message_rx.try_recv() {
            self.handle_worker_message(message);
        }
        self.flush_coalesced_progress();
        self.pending.drain(..).collect()
    }

    /// Emit at most one `Progress` event per subscriber per flight, carrying the
    /// newest value seen since the last poll.
    ///
    /// Drained before `pending` is collected, so progress is ordered ahead of
    /// any terminal event queued in the same poll: a subscriber must never see
    /// `Progress` after `Ready`.
    fn flush_coalesced_progress(&mut self) {
        // One pass over the live flights. Transport callbacks contend only
        // with one constant-size `take` per flight, never with subscriber
        // fan-out or pending-event allocation on the UI thread.
        #[expect(
            clippy::iter_over_hash_type,
            reason = "progress of different flights has no relative order; each flight's own events stay ordered"
        )]
        for flight in self.flights.values() {
            if let Some(progress) = self.progress_mailbox.take(flight.id) {
                push_progress(&mut self.pending, flight, progress);
            }
        }
    }

    /// `(live flight registrations, retained latest progress values)`.
    #[cfg(test)]
    pub(crate) fn progress_mailbox_sizes_for_test(&self) -> (usize, usize) {
        self.progress_mailbox.sizes()
    }

    /// Number of distinct active transports (not subscribers).
    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.flights.len()
    }

    /// Jobs admitted but not yet handed to the worker pool.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.pending_jobs.len()
    }

    /// Jobs handed to the worker pool and not yet completed.
    #[must_use]
    pub fn running_len(&self) -> usize {
        self.running
    }

    /// Total admitted flights — queued plus running. This is the quantity
    /// [`ServiceConfig::max_inflight_jobs`] bounds.
    #[must_use]
    pub fn admitted_len(&self) -> usize {
        self.pending_jobs.len().saturating_add(self.running)
    }

    /// Request bytes currently charged against the admission byte budget.
    #[must_use]
    pub fn inflight_request_bytes(&self) -> u64 {
        self.inflight_request_bytes
    }

    /// Admissions refused because [`ServiceConfig::max_inflight_jobs`] was full.
    ///
    /// A counter, not a panic and not a silent drop: the caller must be able to
    /// observe that work was refused.
    #[must_use]
    pub fn rejected_count_budget(&self) -> u64 {
        self.rejected_count_budget
    }

    /// Admissions refused because
    /// [`ServiceConfig::max_inflight_request_bytes`] was full.
    #[must_use]
    pub fn rejected_byte_budget(&self) -> u64 {
        self.rejected_byte_budget
    }

    /// Same-flight joins refused because
    /// [`ServiceConfig::max_joined_subscribers_per_flight`] was full.
    #[must_use]
    pub fn rejected_subscriber_budget(&self) -> u64 {
        self.rejected_subscriber_budget
    }

    /// Total requests refused for any bounded-resource reason.
    #[must_use]
    pub fn rejected_total(&self) -> u64 {
        self.rejected_count_budget
            .saturating_add(self.rejected_byte_budget)
            .saturating_add(self.rejected_subscriber_budget)
    }

    /// Decoded LRU bytes currently retained.
    #[must_use]
    pub fn decoded_cache_bytes(&self) -> u64 {
        self.cache_bytes
    }

    /// Number of decoded LRU entries.
    #[must_use]
    pub fn decoded_cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Number of live subscriptions — requests that have been issued and are
    /// still cancellable.
    ///
    /// This is **bounded by the number of outstanding requests, not by the
    /// number of requests ever made**, and that is the whole point of exposing
    /// it. A subscription exists only so [`ImageService::cancel`] can find the
    /// flight to detach from; once a request reaches a terminal event there is
    /// nothing left to cancel and the entry must be gone.
    ///
    /// Exposed so a test can assert the *table size across a realistic
    /// sequence* rather than a "was removed" flag. A stored boolean nothing
    /// reads is exactly the trap this accessor exists to avoid: it would report
    /// success while the map grew without bound.
    #[must_use]
    pub fn live_subscription_len(&self) -> usize {
        self.subscriptions.len()
    }

    fn handle_worker_message(&mut self, message: WorkerMessage) {
        match message {
            WorkerMessage::Complete {
                flight: flight_id,
                key,
                result,
            } => {
                // Every completion releases the running slot it occupied, even
                // when the flight it belonged to is already gone (cancelled, or
                // replaced by a same-URL retry). Otherwise the admission budget
                // would leak a slot per cancelled-then-completed job and the
                // service would wedge at `Busy` forever.
                self.running = self.running.saturating_sub(1);

                // Match on identity: a retired flight's completion must never be
                // applied to the live flight that now holds the same URL.
                let matches_live = self
                    .flights
                    .get(&key)
                    .is_some_and(|flight| flight.id == flight_id);
                if !matches_live {
                    // A retired (cancelled-while-running) flight's completion:
                    // release the charge it was still holding.
                    self.progress_mailbox.remove(flight_id);
                    if let Some(charge) = self.retired.remove(&flight_id) {
                        self.inflight_request_bytes =
                            self.inflight_request_bytes.saturating_sub(charge);
                    }
                    self.dispatch_ready_jobs();
                    return;
                }
                // Flush this flight's coalesced progress before retiring it.
                //
                // A fast transport's callbacks and its completion arrive in the
                // same `poll` drain, so retiring first discarded every progress
                // value the transfer ever reported — measured as 0 progress
                // events across 5,000 callbacks. Progress must be observable
                // even when the transfer completes promptly, and it must still
                // precede the terminal event, which holds because `Ready` is
                // pushed to `pending` below this point.
                let flight = self.flights.remove(&key).expect("checked above");
                if let Some(progress) = self.progress_mailbox.take(flight_id) {
                    push_progress(&mut self.pending, &flight, progress);
                }
                self.release_charge(&flight);
                self.progress_mailbox.remove(flight_id);
                if flight.subscribers.is_empty() {
                    self.dispatch_ready_jobs();
                    return;
                }
                match result {
                    Ok(asset) => {
                        self.insert_cache(key, asset.clone());
                        for subscriber in flight.subscribers {
                            self.subscriptions.remove(&subscriber.id);
                            self.pending.push_back(ready_event(
                                subscriber.id,
                                subscriber.generation,
                                &asset,
                                false,
                            ));
                        }
                    }
                    Err(error) => {
                        for subscriber in flight.subscribers {
                            self.subscriptions.remove(&subscriber.id);
                            self.pending.push_back(ServiceEvent::Failed {
                                subscriber: subscriber.id,
                                generation: subscriber.generation,
                                error: error.clone(),
                            });
                        }
                    }
                }

                // Pump the queue after a *delivered* completion, too.
                //
                // `running` was decremented at the top of this arm, so a slot is
                // now free regardless of how the flight resolved. The two early
                // returns above (`!matches_live` and an empty-subscriber flight)
                // each dispatch before returning, but the normal path — a live
                // flight that resolved to `Ready`/`Failed` for real subscribers —
                // fell off the end of the `match` without dispatching. That left
                // the freed slot unused: once the first `worker_threads` flights
                // completed, no queued job was ever handed to the pool again and
                // the queue wedged with `running == 0` and jobs still queued.
                //
                // Measured by the image-service performance gate: 4 workers → 4 flights delivered
                // → 40 of the 1000 non-cancelled subscribers `Ready`, and the
                // remaining 960 never reached any terminal event because their
                // flights sat in `pending_jobs` forever. A completion delivered
                // to subscribers frees exactly as much pool capacity as a
                // cancelled or superseded one, so it must pump the queue too.
                self.dispatch_ready_jobs();
            }
        }
    }

    /// Insert a synthetic decoded entry of `rgba_bytes`, for LRU tests.
    ///
    /// A test seam: driving `insert_cache` through the transport can only
    /// produce the fixture's own asset size, so overwrite and multi-eviction
    /// cannot be reached that way. This calls the exact same `insert_cache` the
    /// completion path uses — it does not reimplement the policy.
    #[cfg(test)]
    pub(crate) fn insert_cache_for_test(&mut self, key: &str, rgba_bytes: u64) {
        let asset = DecodedAsset::synthetic_for_test(rgba_bytes);
        self.insert_cache(Arc::from(key), asset);
    }

    /// Mark `key` most-recently-used. O(log n).
    fn touch_cache(&mut self, key: &Arc<str>) {
        let Some(entry) = self.cache.get_mut(key) else {
            return;
        };
        let old_stamp = entry.stamp;
        let new_stamp = self.next_stamp;
        self.next_stamp = self.next_stamp.wrapping_add(1).max(1);
        entry.stamp = new_stamp;
        // Remove the old index slot before inserting the new one: the stamp in
        // the entry and the key in `lru` must stay mirrored, and a stale slot
        // would make this key evictable twice.
        self.lru.remove(&old_stamp);
        self.lru.insert(new_stamp, Arc::clone(key));
    }

    fn insert_cache(&mut self, key: Arc<str>, asset: DecodedAsset) {
        let capacity = self.config.decoded_cache_bytes;
        if capacity == 0 || asset.rgba_bytes > capacity {
            return;
        }
        if let Some(old) = self.cache.remove(&key) {
            self.cache_bytes = self.cache_bytes.saturating_sub(old.asset.rgba_bytes);
            self.lru.remove(&old.stamp);
        }
        let stamp = self.next_stamp;
        self.next_stamp = self.next_stamp.wrapping_add(1).max(1);
        self.cache_bytes = self.cache_bytes.saturating_add(asset.rgba_bytes);
        self.cache
            .insert(Arc::clone(&key), CacheEntry { asset, stamp });
        self.lru.insert(stamp, key);
        while self.cache_bytes > capacity {
            // Oldest stamp is the eviction victim. O(log n) instead of a scan.
            let Some((&victim_stamp, victim)) = self.lru.iter().next() else {
                break;
            };
            let victim = Arc::clone(victim);
            self.lru.remove(&victim_stamp);
            if let Some(entry) = self.cache.remove(&victim) {
                self.cache_bytes = self.cache_bytes.saturating_sub(entry.asset.rgba_bytes);
            }
        }
    }
}

impl Drop for ImageService {
    fn drop(&mut self) {
        #[allow(
            clippy::iter_over_hash_type,
            reason = "every entry gets the same treatment, so the order does not matter"
        )]
        for flight in self.flights.values() {
            flight.cancel.cancel(CancelCause::Shutdown);
        }

        // DISCARD the backlog; do not drain it.
        //
        // Queued jobs have never been seen by a worker, so nothing needs to
        // observe them to shut down: dropping the deque frees them outright.
        // This is the payoff for owning the queue here instead of pushing
        // straight into the channel — with 10,000 hostile jobs already in an
        // `mpsc`, every one of them has to be dequeued by a worker and
        // individually observed as cancelled before the pool can exit, so drop
        // would scale with the size of the attack. Here it is O(1) work per
        // queued job with no thread handoff and no transport involvement.
        self.pending_jobs.clear();

        // Closing the channel is what releases a worker parked in `recv()`,
        // so it must precede the join below.
        self.job_tx = None;

        // One deadline for the whole pool, not one per worker: the budget is
        // a teardown ceiling, so it cannot scale with worker count.
        let deadline = Instant::now().checked_add(DROP_JOIN_BUDGET);
        for worker in self.workers.drain(..) {
            bounded_drop_join(worker, deadline);
        }
    }
}

/// Terminate and join a partially created worker pool.
///
/// Used only by [`ImageService::new_with_spawn_gate`]'s failure path, where
/// `Self` is never constructed and therefore [`Drop`] can never run.
///
/// The ordering is the entire point and it is not interchangeable. Each
/// live worker is blocked in `receiver.recv()`, which returns `Err` only once
/// *every* [`Sender`] for that channel has been dropped. So:
///
/// 1. `drop(job_tx)` closes the job channel, waking every blocked worker and
///    letting `worker_loop` return;
/// 2. `join` then observes threads that are already on their way out.
///
/// Joining first would block forever on a worker that is itself blocked waiting
/// for a job that can never arrive — a deadlock inside a constructor. Taking
/// `job_tx` by value is what makes step 1 unskippable: the sender cannot
/// outlive this call, so no caller can accidentally hold the channel open.
fn shutdown_partial_pool(job_tx: Sender<Job>, workers: Vec<JoinHandle<()>>) {
    drop(job_tx);
    for worker in workers {
        // A worker that panicked is already gone; the join is what guarantees
        // this function does not return while a thread is still live.
        let _ = worker.join();
    }
}

/// Fetch `job`'s bytes, consulting and updating the encoded HTTP cache.
///
/// Tier order, cheapest first:
///
/// 1. Fresh cache hit — no network at all, bytes served from disk.
/// 2. Stale hit with a validator — a conditional request; a `304` serves the
///    cached bytes and refreshes the entry, so the body is never re-sent.
/// 3. Miss — an unconditional fetch, stored afterwards if permitted.
///
/// The decoded RGBA LRU sits *above* all of this in `ImageService::request`, so
/// a decoded hit never reaches a worker and never touches the disk.
fn fetch_through_cache<T: Transport + ?Sized>(
    transport: &T,
    cache: Option<&HttpCache>,
    job: &Job,
    config: &ServiceConfig,
    progress: &mut dyn FnMut(TransferProgress),
) -> Result<Vec<u8>, TransportError> {
    // The identity carries the full URL (query token and all) for correct key
    // separation; the display form is redacted so the token never reaches the
    // cache file or a diagnostic. `CacheKey` keeps the two apart.
    let identity = job.request.raw.as_ref();
    let display = job.request.redacted();
    let key = CacheKey::new(identity, &display);
    // This service sends no request headers of its own, so the varying set is
    // empty. It is threaded through explicitly rather than hardcoded so a future
    // Accept/Accept-Language header cannot silently bypass Vary partitioning.
    let request_headers: &[(String, String)] = &[];
    let cached = cache.and_then(|cache| cache.get(key, request_headers));

    // Tier 1: fresh, so no network.
    if let Some(entry) = &cached
        && entry.usable_without_network()
    {
        return Ok(entry.body.clone());
    }

    // Tier 2: stale but revalidatable. `ETag` takes precedence over
    // `Last-Modified` (a strong validator beats a one-second-resolution one), but
    // both are sent when both exist so an origin that only honours one still
    // answers 304.
    let etag = cached.as_ref().and_then(|entry| entry.etag.clone());
    let last_modified = cached
        .as_ref()
        .and_then(|entry| entry.last_modified.clone());
    let validators = Validators {
        etag: etag.as_deref(),
        last_modified: last_modified.as_deref(),
    };
    let response = transport.fetch_conditional(
        &job.request,
        &config.url_policy,
        &config.transport_limits,
        &job.cancel,
        progress,
        &validators,
    )?;

    let freshness = Freshness::from_headers(&response.headers);

    if response.status == 304 {
        // Not Modified: the origin sent no body, so the CACHED bytes are the
        // response. Without an entry to serve there is nothing to do but fail --
        // a 304 for something we do not hold is a protocol error by the origin.
        let Some(entry) = cached else {
            return Err(TransportError::Backend(304));
        };
        if let Some(cache) = cache {
            cache.mark_revalidated(key, &freshness, request_headers);
        }
        return Ok(entry.body);
    }

    if let Some(cache) = cache {
        cache.store(key, &response.body, &freshness, request_headers);
    }
    Ok(response.body)
}

fn worker_loop<T: Transport>(
    jobs: Arc<Mutex<Receiver<Job>>>,
    messages: Sender<WorkerMessage>,
    progress_mailbox: Arc<ProgressMailbox>,
    transport: Arc<T>,
    config: ServiceConfig,
    cache: Option<Arc<HttpCache>>,
) {
    loop {
        let job = {
            let Ok(receiver) = jobs.lock() else { return };
            match receiver.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };

        if job.cancel.is_cancelled() {
            let cause = job.cancel.cause().unwrap_or(CancelCause::Cancelled);
            let _ = messages.send(WorkerMessage::Complete {
                flight: job.flight,
                key: job.key,
                result: Err(ServiceError::Transport(TransportError::Cancelled(cause))),
            });
            continue;
        }

        let flight_for_progress = job.flight;
        let mailbox_for_progress = Arc::clone(&progress_mailbox);
        let mut progress = move |progress| {
            mailbox_for_progress.update(flight_for_progress, progress);
        };
        // Keep the worker alive so the terminal message below releases the
        // flight and its pool slot. No service lock is held across this call.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fetch_through_cache(&*transport, cache.as_deref(), &job, &config, &mut progress)
                .map_err(ServiceError::Transport)
                .and_then(|body| {
                    decode_static(&body, &config.decode_limits, || job.cancel.is_cancelled())
                        .map_err(ServiceError::Decode)
                })
        }))
        .unwrap_or_else(|payload| {
            // The payload may embed a URL or response bytes; the default hook
            // has already reported the panic location.
            drop(payload);
            Err(ServiceError::WorkerPanic)
        });
        let _ = messages.send(WorkerMessage::Complete {
            flight: job.flight,
            key: job.key,
            result,
        });
    }
}

/// Request bytes charged to the admission byte budget by one request.
///
/// The URL dominates and has no length limit in this tree, which is why a count
/// budget alone does not bound memory: the measured probe used ~1 KiB query
/// payloads per URL. `Job` and `Flight` overhead is a small constant beside it,
/// so the charge is the URL length plus a fixed allowance for the surrounding
/// bookkeeping.
fn request_bytes(request: &RemoteRequest) -> u64 {
    /// Rough constant cost of a `Job` + `Flight` + `Subscription` triple.
    const BOOKKEEPING_BYTES: u64 = 256;
    request.raw.len() as u64 + BOOKKEEPING_BYTES
}

fn ready_event(
    subscriber: SubscriberId,
    generation: Generation,
    asset: &DecodedAsset,
    cache_hit: bool,
) -> ServiceEvent {
    ServiceEvent::Ready {
        subscriber,
        completion: ImageCompletion {
            generation,
            outcome: CompletionOutcome::Decoded {
                handle: asset.handle.clone(),
                encoded: EncodedImage::new(asset.encoded_bytes, asset.width, asset.height),
            },
        },
        cache_hit,
    }
}

/// Queue one already-taken progress value for each subscriber of `flight`.
fn push_progress(
    pending: &mut VecDeque<ServiceEvent>,
    flight: &Flight,
    progress: TransferProgress,
) {
    for subscriber in &flight.subscribers {
        pending.push_back(ServiceEvent::Progress {
            subscriber: subscriber.id,
            generation: subscriber.generation,
            progress,
        });
    }
}

fn event_subscriber(event: &ServiceEvent) -> SubscriberId {
    match event {
        ServiceEvent::Progress { subscriber, .. }
        | ServiceEvent::Ready { subscriber, .. }
        | ServiceEvent::Failed { subscriber, .. }
        | ServiceEvent::Cancelled { subscriber, .. } => *subscriber,
    }
}

#[cfg(test)]
mod tests;
