//! A real [`DownloadTransport`]
//! backed by system libcurl for the effectful download worker.
//!
//! # Why a second curl transport
//!
//! [`crate::curl_transport::CurlTransport`] fetches a whole image body into a
//! `Vec` and returns it — right for an image (bounded, decoded once), wrong for
//! a download (a body may be gigabytes and must stream to disk). This transport
//! implements the streaming `DownloadTransport` seam instead: `start`
//! returns the response headers, then `read_chunk` pulls the body one piece at
//! a time so the worker can write to disk without buffering the whole file.
//!
//! # Push-to-pull bridge, and why it cannot hang
//!
//! libcurl is *push* (its `perform` drives write callbacks); `DownloadTransport`
//! is *pull* (`read_chunk` asks for the next piece). The bridge is a background
//! thread running `perform`, feeding a bounded channel that `read_chunk`
//! drains. Two properties make this deadlock-free, which matters because a
//! network test that hangs holds the host-wide cargo lock:
//!
//! - The producer is a thread this transport owns, not an accept loop
//!   waiting for a client that may never connect. When the transfer ends (or
//!   errors, or is cancelled) the thread drops its sender, so `read_chunk`'s
//!   receive returns `Disconnected` and the pull side ends deterministically.
//! - The channel is bounded: if the consumer stops draining, the producer
//!   blocks on `send` rather than buffering an unbounded body. If the consumer
//!   is dropped, the producer's `send` fails and the write callback returns 0,
//!   which makes libcurl abort (`CURLE_ABORTED_BY_CALLBACK`).
//!
//! # Deadlines and cancellation
//!
//! [`DownloadDeadlines`] bounds connection establishment, completion of the
//! response headers, gaps in body progress, and the whole request. Connect and
//! total limits are installed in libcurl; its progress callback enforces the
//! phase deadlines while the header/write callbacks are idle.
//!
//! # URL policy across the whole redirect chain
//!
//! The URL handed to [`DownloadTransport::start`] is caller-supplied and
//! proves nothing, and a redirect target is chosen by the origin. This
//! transport therefore owns an explicit [`UrlPolicy`] —
//! [`UrlPolicy::DEFAULT`] unless a caller chooses otherwise, so the safe
//! behavior is what default construction gives — and applies the same
//! layered screening [`crate::curl_transport`] does:
//!
//! - the URL string is judged before any DNS or socket work, so a
//!   `file:`, credentialed, or cleartext destination costs nothing;
//! - every DNS answer is screened, not just the one that would be used:
//!   a mixed public/internal answer set fails closed rather than concealing
//!   a rebinding attempt;
//! - the chosen address is pinned with `CURLOPT_RESOLVE`, proxies are
//!   disabled (an ambient proxy would make the observed peer the proxy), and
//!   `CURLINFO_PRIMARY_IP` re-verifies the peer the connection landed on
//!   before response metadata or body bytes can cross the worker boundary;
//! - libcurl's own redirect following stays off; each hop is a separate,
//!   individually screened request whose `Location` is resolved against the
//!   hop that produced it and re-judged by
//!   [`UrlPolicy::check_redirect`], with a finite hop ceiling. A followed
//!   hop's body is discarded under a ceiling instead of streamed.
//!
//! All of it shares one total budget: the deadline clock starts once, so
//! a redirect chain cannot extend a transfer beyond what the caller allowed.
//! Resume headers (`Range` + `If-Range`) cross an allowed hop as one unit,
//! never one without the other.
//!
//! A `206` is admitted only when that hop sent `Range`, so stripping resume
//! headers at an origin change cannot splice the new origin into a partial.
//!
//! The core [`CancelSignal`] is shared into the curl thread. libcurl's write and
//! progress callbacks poll it and return the abort sentinel the moment it
//! fires, so a cancelled transfer stops reading from the socket promptly — it
//! does not merely stop reporting. The progress route is essential before
//! headers and between body bytes, when no write callback is available.

use crate::bounded_drop::{DROP_JOIN_BUDGET, bounded_drop_join};
use crate::header_limits::{HeaderAccumulator, HeaderLimitKind};
use crate::resolver_gate::{ResolverGate, shared_resolver_gate};
use curl::easy::{Easy2, Handler, List, WriteError};
use curl::multi::{Easy2Handle, Multi};
use enzui_core::download::{
    BodyChunk, CancelSignal, DownloadTransport, DownloadTransportFactory, RangeRequestPlan,
    TransportError, TransportResponseMeta, TransportTimeout, Validator,
};
use enzui_core::url_policy::{
    AddressVerdict, SafeUrl, Scheme, UrlPolicy, UrlVerdict, resolve_reference,
};
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How many body chunks may sit in the bridge channel before the curl thread
/// blocks. Small: back-pressure to the socket is the point, and a large buffer
/// would defeat the streaming guarantee this transport exists for.
const CHANNEL_DEPTH: usize = 4;

/// How often a blocked pull-side receive re-checks cancellation and its local
/// deadline. libcurl's progress callback is the primary abort mechanism; this
/// bounded receive is a second line of defence and keeps the caller responsive.
const RECEIVE_SLICE: Duration = Duration::from_millis(25);

/// Ceiling on the body of a hop that is being followed as a redirect. The
/// bytes are discarded (the `Location` header is all a redirect contributes),
/// but they must still be bounded: an origin that answers `302` and then
/// streams gigabytes would otherwise be read for the whole total budget.
const REDIRECT_BODY_CAP: u64 = 64 * 1024;

/// Name resolution seam. Production is [`system_lookup`]; a test scripts the
/// answer set so all-answer screening, mixed answers, and rebinding are
/// reachable without external DNS.
type DownloadLookup = dyn Fn(&str, u16) -> Result<Vec<IpAddr>, ()> + Send + Sync + 'static;

/// The real blocking resolver, run on the transport's own worker thread.
fn system_lookup(host: &str, port: u16) -> Result<Vec<IpAddr>, ()> {
    (host, port)
        .to_socket_addrs()
        .map(|addrs| addrs.map(|socket| socket.ip()).collect::<Vec<_>>())
        .map_err(|_| ())
}

/// A policy refusal, tagged rather than stringified at the throw site.
fn blocked(reason: impl Into<String>) -> TransportError {
    TransportError::Blocked {
        reason: reason.into(),
    }
}

/// Map a URL/redirect verdict to a typed refusal.
fn url_blocked(verdict: UrlVerdict) -> TransportError {
    blocked(verdict.reason())
}

/// Deadlines applied to one streaming download.
///
/// The connect and total values are also installed in libcurl. Header and body
/// progress are enforced by libcurl's progress callback, so they continue to
/// run while no header or body callback is firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadDeadlines {
    /// Maximum time allowed to establish a connection.
    pub connect: Duration,
    /// Maximum time from starting the request to complete response headers.
    pub response_headers: Duration,
    /// Maximum interval without a response-body byte after headers complete.
    pub body_progress: Duration,
    /// Maximum duration of the whole transfer, including a progressing body.
    pub total: Duration,
}

impl Default for DownloadDeadlines {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            response_headers: Duration::from_secs(30),
            body_progress: Duration::from_secs(30),
            total: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// One item flowing over the body bridge.
///
/// Completion is an explicit message rather than an inference from channel
/// disconnection. A sender also disappears when its worker panics, so treating
/// `Disconnected` as EOF can accept a truncated download as complete.
#[derive(Debug)]
enum BodyItem {
    Bytes(Vec<u8>),
    Error(TransportError),
    Done,
}

/// The one-shot lifecycle records for an admitted transfer.
///
/// The receiver, worker handle, and abort signal are the same transfer's
/// records; keeping them together in a single field is what makes them
/// impossible to cross-pair. Under the previous design each lived in its
/// own mutex, so a second successful [`start`](DownloadTransport::start)
/// could replace the trio one field at a time and leave the admitted worker
/// reachable only through a mismatched receiver / abort pair.
#[derive(Debug)]
struct ActiveTransfer {
    /// The receiver end of the body-chunk bridge.
    rx: Receiver<BodyItem>,
    /// The curl worker thread, joined when the body is drained (or the
    /// transfer is torn down).
    worker: JoinHandle<()>,
    /// Out-of-band stop the curl progress and back-pressure loops observe
    /// even when no wire callback fires.
    abort: Arc<AtomicBool>,
}

/// The transport's atomic lifecycle. Every transition happens under the
/// state mutex, so no [`start`](DownloadTransport::start) can observe a
/// half-transitioned trio of records.
#[derive(Debug)]
enum LifecycleState {
    /// Never started. The only state from which [`start`] admits a transfer.
    Idle,
    /// A [`start`] call has claimed the transport and is spawning its
    /// worker or waiting for the response metadata. A concurrent [`start`]
    /// observing this state must refuse with
    /// [`TransportError::AlreadyActive`] before touching the network.
    Starting,
    /// The admitted transfer's records live here as one atom.
    Active(ActiveTransfer),
    /// The transfer has completed, errored, cancelled, or the transport was
    /// dropped: the type is one-shot and refuses any further
    /// [`start`](DownloadTransport::start).
    Terminal,
}

/// A streaming HTTP(S) download transport over system libcurl.
///
/// One instance drives one transfer: [`start`](DownloadTransport::start)
/// atomically claims an idle transport, spawns the curl thread, and
/// [`read_chunk`](DownloadTransport::read_chunk) drains it. A second or
/// concurrent [`start`](DownloadTransport::start) is refused with
/// [`TransportError::AlreadyActive`] rather than replacing the admitted
/// worker's lifecycle records. The type is one-shot: after the transfer
/// completes, errors, is cancelled, or fails to spawn, further
/// [`start`](DownloadTransport::start) calls also observe
/// [`TransportError::AlreadyActive`]. Construct a fresh instance per
/// download through [`CurlDownloadTransportFactory`].
pub struct CurlDownloadTransport {
    /// The receiver, worker handle, and abort signal for the admitted
    /// transfer, kept together so a claim, a teardown, or a `Drop` cannot
    /// see a partial trio.
    state: Mutex<LifecycleState>,
    deadlines: DownloadDeadlines,
    /// The URL/redirect policy applied to the initial URL, to every resolved
    /// address, and to every redirect hop. Never optional: the default is
    /// [`UrlPolicy::DEFAULT`], so a caller who says nothing still gets the
    /// SSRF-safe behavior.
    policy: UrlPolicy,
    /// Name resolution. Production is [`system_lookup`]; only this module's
    /// tests replace it, and a replacement never decides a verdict — it only
    /// supplies the answers that [`screen_destination`] judges.
    lookup: Arc<DownloadLookup>,
    /// Test-only override of the peer libcurl reports for a hop, so the
    /// rebinding branch (connected address ≠ screened address) is reachable
    /// without a hostile resolver on the host.
    #[cfg(test)]
    observed_peer_override: Option<IpAddr>,
    /// Deterministic fault at the production worker boundary. It is compiled
    /// only for this module's tests and never changes the public transport API.
    #[cfg(test)]
    panic_before_body_done: bool,
    /// Deterministic OS-thread-spawn failure at the transport's spawn
    /// seam. Compiled only for this module's tests; makes acceptance 5
    /// reachable without probing the host's thread limits.
    #[cfg(test)]
    force_spawn_failure: bool,
    /// Test-only rendezvous proving cancellation was first observed inside
    /// the bounded receive loop rather than by `read_chunk`'s entry check.
    #[cfg(test)]
    read_waiting: Option<Arc<std::sync::Barrier>>,
    /// Test-only witness set at the end of the worker frame. Unlike a peer
    /// disconnect, this proves that joining waited for the Rust worker itself.
    #[cfg(test)]
    worker_exit_witness: Option<Arc<AtomicBool>>,
    /// Makes a missing join deterministic: the worker remains alive for this
    /// long after curl returns, before setting `worker_exit_witness`.
    #[cfg(test)]
    worker_exit_delay: Duration,
}

impl std::fmt::Debug for CurlDownloadTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurlDownloadTransport")
            .field("state", &self.state)
            .field("deadlines", &self.deadlines)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl Default for CurlDownloadTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl CurlDownloadTransport {
    /// A fresh, unstarted transport with conservative finite deadlines and
    /// the safe [`UrlPolicy::DEFAULT`] (HTTPS only, public addresses only,
    /// at most five redirect hops).
    #[must_use]
    pub fn new() -> Self {
        Self::with_deadlines(DownloadDeadlines::default())
    }

    /// A transport with caller-selected policy and the default deadlines.
    #[must_use]
    pub fn with_policy(policy: UrlPolicy) -> Self {
        Self::with_policy_and_deadlines(policy, DownloadDeadlines::default())
    }

    /// A transport with caller-selected policy and deadlines.
    #[must_use]
    pub fn with_policy_and_deadlines(policy: UrlPolicy, deadlines: DownloadDeadlines) -> Self {
        let mut transport = Self::with_deadlines(deadlines);
        transport.policy = policy;
        transport
    }

    /// A fresh transport with caller-selected finite deadlines.
    ///
    /// A zero duration is an immediate typed timeout, never libcurl's special
    /// spelling for "timeout disabled".
    #[must_use]
    pub fn with_deadlines(deadlines: DownloadDeadlines) -> Self {
        Self {
            state: Mutex::new(LifecycleState::Idle),
            deadlines,
            policy: UrlPolicy::DEFAULT,
            lookup: Arc::new(system_lookup),
            #[cfg(test)]
            observed_peer_override: None,
            #[cfg(test)]
            panic_before_body_done: false,
            #[cfg(test)]
            force_spawn_failure: false,
            #[cfg(test)]
            read_waiting: None,
            #[cfg(test)]
            worker_exit_witness: None,
            #[cfg(test)]
            worker_exit_delay: Duration::ZERO,
        }
    }

    /// The policy the localhost fixtures in this module's tests need: they
    /// serve cleartext HTTP on loopback, which the production default
    /// refuses. Test-only, so no product path can reach it.
    #[cfg(test)]
    pub(crate) fn fixture_policy() -> UrlPolicy {
        UrlPolicy {
            allow_http: true,
            allow_private_network: true,
            ..UrlPolicy::DEFAULT
        }
    }

    /// The lifecycle state, recovered after poisoning.
    ///
    /// A panic while some other thread held this guard poisons the mutex. The
    /// lifecycle records are still exactly what that thread published (the
    /// trio is replaced as one atom, never field by field), so the transport
    /// stays callable and — crucially — droppable: propagating the poison here
    /// would make `Drop` panic during unwinding and abort the process before
    /// crash reporting runs.
    fn state(&self) -> MutexGuard<'_, LifecycleState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Test-only: install an already-shared resolver, so a test can keep a
    /// handle on the same closure it gave the transport (the rendezvous the
    /// stalled-resolver tests wait on).
    #[cfg(test)]
    fn with_shared_lookup(mut self, lookup: Arc<DownloadLookup>) -> Self {
        self.lookup = lookup;
        self
    }

    #[cfg(test)]
    fn fixture(deadlines: DownloadDeadlines) -> Self {
        Self::with_policy_and_deadlines(Self::fixture_policy(), deadlines)
    }

    #[cfg(test)]
    fn with_lifecycle_test_hooks(
        deadlines: DownloadDeadlines,
        read_waiting: Option<Arc<std::sync::Barrier>>,
        worker_exit_witness: Option<Arc<AtomicBool>>,
        worker_exit_delay: Duration,
    ) -> Self {
        let mut transport = Self::fixture(deadlines);
        transport.read_waiting = read_waiting;
        transport.worker_exit_witness = worker_exit_witness;
        transport.worker_exit_delay = worker_exit_delay;
        transport
    }
}

/// A [`DownloadTransportFactory`] that hands out a fresh
/// [`CurlDownloadTransport`] for each spawned transfer.
///
/// A [`CurlDownloadTransport`] is one-shot by design (see the type-level
/// doc); wiring the factory at the spawn seam is the structural fix that
/// makes per-transfer construction the only shape the code can take.
#[derive(Debug, Clone, Copy)]
pub struct CurlDownloadTransportFactory {
    deadlines: DownloadDeadlines,
    /// Carried so every transport the factory hands out screens under the
    /// same explicit policy. Defaults to [`UrlPolicy::DEFAULT`].
    policy: UrlPolicy,
}

impl CurlDownloadTransportFactory {
    /// A factory that produces transports with the default finite deadlines
    /// and the safe default [`UrlPolicy`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            deadlines: DownloadDeadlines::default(),
            policy: UrlPolicy::DEFAULT,
        }
    }

    /// A factory that produces transports with caller-selected deadlines and
    /// the safe default [`UrlPolicy`].
    #[must_use]
    pub fn with_deadlines(deadlines: DownloadDeadlines) -> Self {
        Self {
            deadlines,
            policy: UrlPolicy::DEFAULT,
        }
    }

    /// A factory that produces transports with caller-selected policy and
    /// deadlines.
    #[must_use]
    pub fn with_policy_and_deadlines(policy: UrlPolicy, deadlines: DownloadDeadlines) -> Self {
        Self { deadlines, policy }
    }

    /// The policy every transport from this factory will apply.
    #[must_use]
    pub fn policy(&self) -> UrlPolicy {
        self.policy
    }
}

impl Default for CurlDownloadTransportFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadTransportFactory for CurlDownloadTransportFactory {
    fn create(&self) -> Box<dyn DownloadTransport> {
        Box::new(CurlDownloadTransport::with_policy_and_deadlines(
            self.policy,
            self.deadlines,
        ))
    }
}

/// The message the curl thread sends first: the parsed response metadata, or
/// the error that stopped the request before any body.
type MetaResult = Result<TransportResponseMeta, TransportError>;

impl DownloadTransport for CurlDownloadTransport {
    fn start(
        &self,
        url: &str,
        plan: Option<&RangeRequestPlan>,
        cancel: &CancelSignal,
    ) -> Result<TransportResponseMeta, TransportError> {
        // Atomically claim the idle transport. A concurrent or serial
        // second start observes `Starting`, `Active`, or `Terminal` and
        // refuses without touching the network — so it cannot detach the
        // admitted worker by replacing the lifecycle records field by
        // field. The claim happens before the pre-flight checks so a
        // zero-deadline / cancelled start still burns the one-shot.
        {
            let mut state = self.state();
            match *state {
                LifecycleState::Idle => *state = LifecycleState::Starting,
                _ => return Err(TransportError::AlreadyActive),
            }
        }

        // Pre-flight, cheap synchronous refusals. On any of these the claim
        // burns the one-shot to `Terminal`: an idle transport that has been
        // told the deadlines are unusable must not become usable by
        // retrying `start` on the same instance.
        if cancel.is_cancelled() {
            *self.state() = LifecycleState::Terminal;
            return Err(TransportError::Cancelled);
        }
        if self.deadlines.connect.is_zero() {
            *self.state() = LifecycleState::Terminal;
            return Err(TransportError::Timeout(TransportTimeout::Connect));
        }
        if self.deadlines.response_headers.is_zero() {
            *self.state() = LifecycleState::Terminal;
            return Err(TransportError::Timeout(TransportTimeout::ResponseHeaders));
        }
        if self.deadlines.total.is_zero() {
            *self.state() = LifecycleState::Terminal;
            return Err(TransportError::Timeout(TransportTimeout::Total));
        }

        // Stage 1, before any DNS or socket work: judge the URL string
        // under this transport's policy. A refusal here never reaches the
        // resolver, so a `file:`, credentialed, or cleartext URL costs
        // nothing and leaks nothing. Removing this check is the sabotage the
        // initial-policy SSRF tests predict red.
        let safe = match self.policy.check_url(url) {
            Ok(safe) => safe,
            Err(verdict) => {
                *self.state() = LifecycleState::Terminal;
                return Err(url_blocked(verdict));
            }
        };

        let (body_tx, body_rx) = std::sync::mpsc::sync_channel::<BodyItem>(CHANNEL_DEPTH);
        let (meta_tx, meta_rx) = std::sync::mpsc::channel::<MetaResult>();
        let internal_abort = Arc::new(AtomicBool::new(false));

        let url = url.to_owned();
        // The whole resume capability, taken once as one value: there is no
        // point at which a `Range` exists without its `If-Range`.
        let resume = ResumeHeaders::from_plan(plan);
        let worker_cancel = cancel.clone();
        let worker_abort = Arc::clone(&internal_abort);
        let deadlines = self.deadlines;
        let screening = Screening {
            policy: self.policy,
            lookup: Arc::clone(&self.lookup),
            #[cfg(test)]
            observed_peer_override: self.observed_peer_override,
        };
        #[cfg(test)]
        let panic_before_body_done = self.panic_before_body_done;
        #[cfg(test)]
        let worker_exit_witness = self.worker_exit_witness.clone();
        #[cfg(test)]
        let worker_exit_delay = self.worker_exit_delay;

        // Deterministic OS-thread-spawn failure. Simulating the OS refusal
        // this way keeps acceptance 5 exercisable without exhausting the
        // host's thread limits, and the surrounding transition still runs.
        #[cfg(test)]
        let spawn_result = if self.force_spawn_failure {
            Err(std::io::Error::other("injected spawn failure"))
        } else {
            std::thread::Builder::new()
                .name("enzui-download".to_owned())
                .spawn(move || {
                    run_worker(
                        Destination { url, safe },
                        resume,
                        &screening,
                        &worker_cancel,
                        WorkerChannels {
                            internal_abort: &worker_abort,
                            meta_tx: &meta_tx,
                            body_tx: &body_tx,
                        },
                        deadlines,
                        panic_before_body_done,
                    );
                    if !worker_exit_delay.is_zero() {
                        std::thread::sleep(worker_exit_delay);
                    }
                    if let Some(witness) = worker_exit_witness {
                        witness.store(true, Ordering::Release);
                    }
                })
        };
        #[cfg(not(test))]
        let spawn_result = std::thread::Builder::new()
            .name("enzui-download".to_owned())
            .spawn(move || {
                run_worker(
                    Destination { url, safe },
                    resume,
                    &screening,
                    &worker_cancel,
                    WorkerChannels {
                        internal_abort: &worker_abort,
                        meta_tx: &meta_tx,
                        body_tx: &body_tx,
                    },
                    deadlines,
                );
            });

        let worker = match spawn_result {
            Ok(handle) => handle,
            Err(e) => {
                // The channels and the abort flag are dropped here (no
                // worker owns them), so the transport holds nothing that
                // could survive the failed start; then `Terminal` refuses
                // any further `start` on this instance.
                drop(meta_rx);
                drop(body_rx);
                *self.state() = LifecycleState::Terminal;
                return Err(TransportError::Io(format!("spawn failed: {e}")));
            }
        };

        // This is a hard request-wide backstop, not the header-progress
        // mechanism. The curl progress callback owns the shorter typed phase
        // deadline and must close the socket; keeping the two independent
        // makes a missing callback observable instead of silently green.
        let meta = recv_bounded(
            &meta_rx,
            cancel,
            self.deadlines.total,
            TransportTimeout::Total,
        )
        .and_then(|result| result);
        if matches!(
            meta,
            Err(TransportError::Cancelled | TransportError::Timeout(_))
        ) {
            internal_abort.store(true, Ordering::Release);
        }

        match meta {
            Ok(meta) => {
                // Publish the trio as one atom. A concurrent second start
                // that reached its refusal path observes `Starting`
                // through here — never a half-populated `Active`.
                *self.state() = LifecycleState::Active(ActiveTransfer {
                    rx: body_rx,
                    worker,
                    abort: internal_abort,
                });
                Ok(meta)
            }
            Err(e) => {
                drop(meta_rx);
                drop(body_rx);
                let _ = worker.join();
                *self.state() = LifecycleState::Terminal;
                Err(e)
            }
        }
    }

    fn read_chunk(&self, cancel: &CancelSignal) -> Result<BodyChunk, TransportError> {
        // `start` exclusively owns `Starting`. In particular, a concurrent
        // cancelled read must not turn it into `Terminal` and let `start`
        // resurrect that terminal lifecycle by publishing `Active` later.
        let mut guard = self.state();
        match &*guard {
            LifecycleState::Active(_) => {}
            LifecycleState::Starting => return Err(TransportError::AlreadyActive),
            LifecycleState::Idle | LifecycleState::Terminal => {
                return Err(TransportError::Io("read_chunk before start".into()));
            }
        }

        if cancel.is_cancelled() {
            let taken = std::mem::replace(&mut *guard, LifecycleState::Terminal);
            drop(guard);
            let _ = finalize_active(taken, true);
            return Err(TransportError::Cancelled);
        }
        if self.deadlines.body_progress.is_zero() {
            let taken = std::mem::replace(&mut *guard, LifecycleState::Terminal);
            drop(guard);
            let _ = finalize_active(taken, true);
            return Err(TransportError::Timeout(TransportTimeout::BodyProgress));
        }

        #[cfg(test)]
        if let Some(read_waiting) = &self.read_waiting {
            read_waiting.wait();
        }

        let LifecycleState::Active(active) = &*guard else {
            unreachable!("active lifecycle was checked while holding the state lock");
        };
        let item = recv_bounded(
            &active.rx,
            cancel,
            self.deadlines.total,
            TransportTimeout::Total,
        );
        match item {
            Ok(BodyItem::Bytes(bytes)) => Ok(BodyChunk::Bytes(bytes)),
            Ok(BodyItem::Error(e)) => {
                let taken = std::mem::replace(&mut *guard, LifecycleState::Terminal);
                drop(guard);
                let _ = finalize_active(taken, false);
                Err(e)
            }
            Ok(BodyItem::Done) => {
                let taken = std::mem::replace(&mut *guard, LifecycleState::Terminal);
                drop(guard);
                finalize_active(taken, false)?;
                Ok(BodyChunk::Done)
            }
            Err(TransportError::Io(_)) => {
                let taken = std::mem::replace(&mut *guard, LifecycleState::Terminal);
                drop(guard);
                match finalize_active(taken, false) {
                    Err(error) => Err(error),
                    Ok(()) => Err(TransportError::Io(
                        "curl body channel disconnected before completion".into(),
                    )),
                }
            }
            Err(e) => {
                // `recv_bounded` can observe a cancellation signal different
                // from the one captured by `start`. Always wake the transport
                // worker through its own abort before joining it.
                let taken = std::mem::replace(&mut *guard, LifecycleState::Terminal);
                drop(guard);
                let _ = finalize_active(taken, true);
                Err(e)
            }
        }
    }
}

/// Receive without parking forever. A disconnected producer is represented by
/// an internal `Io` sentinel; only an explicit [`BodyItem::Done`] is EOF.
fn recv_bounded<T>(
    rx: &Receiver<T>,
    cancel: &CancelSignal,
    timeout: Duration,
    phase: TransportTimeout,
) -> Result<T, TransportError> {
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(TransportError::Timeout(phase));
        }
        match rx.recv_timeout(left.min(RECEIVE_SLICE)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(TransportError::Io("curl worker exited early".into()));
            }
        }
    }
}

/// Signal, drop, and join the exact admitted transfer's records as one
/// atom. The panic payload is deliberately discarded: callers only learn
/// that the worker failed, they never see its private payload.
fn finalize_active(state: LifecycleState, signal_abort: bool) -> Result<(), TransportError> {
    match state {
        LifecycleState::Active(active) => {
            let ActiveTransfer { rx, worker, abort } = active;
            if signal_abort {
                abort.store(true, Ordering::Release);
            }
            drop(rx);
            worker.join().map_err(|_| {
                TransportError::Io("curl worker panicked before body completion".into())
            })
        }
        _ => Ok(()),
    }
}

impl Drop for CurlDownloadTransport {
    fn drop(&mut self) {
        // `get_mut` on a poisoned mutex must not panic: `Drop` can run while
        // an earlier panic unwinds, and a second panic there aborts.
        let state = std::mem::replace(
            self.state.get_mut().unwrap_or_else(PoisonError::into_inner),
            LifecycleState::Terminal,
        );
        finalize_active_bounded(state);
    }
}

// `finalize_active` for the `Drop` path, where a typed error has no caller
// and an unbounded join would wedge teardown: the same abort-then-close
// wake-up, followed by a join that detaches at the budget.
fn finalize_active_bounded(state: LifecycleState) {
    let LifecycleState::Active(active) = state else {
        return;
    };
    let ActiveTransfer { rx, worker, abort } = active;
    abort.store(true, Ordering::Release);
    drop(rx);
    bounded_drop_join(worker, Instant::now().checked_add(DROP_JOIN_BUDGET));
}

/// The worker-owned communication endpoints for one download.
///
/// Grouped rather than passed as three positional parameters: `run_worker`
/// and `run_curl` both need every field for the entire transfer, and the
/// bundle keeps a call site from reordering `body_tx` past `meta_tx` or
/// forgetting to poll `internal_abort`. It also keeps the worker and hop
/// drivers below `clippy::too_many_arguments`.
struct WorkerChannels<'a> {
    internal_abort: &'a Arc<AtomicBool>,
    meta_tx: &'a std::sync::mpsc::Sender<MetaResult>,
    body_tx: &'a SyncSender<BodyItem>,
}

/// A destination that policy already accepted: the exact string handed to
/// libcurl plus the parsed [`SafeUrl`] the screening stages judge.
///
/// The pair travels together so a hop can never screen one URL and request
/// another — the confusion that makes a redirect loop exploitable.
#[derive(Debug, Clone)]
struct Destination {
    url: String,
    safe: SafeUrl,
}

/// One resume capability: a `Range` and, when the caller had a strong
/// validator, the `If-Range` that makes it conditional.
///
/// The `If-Range` lives inside the range, so it is unrepresentable
/// without it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResumeCapability {
    range: String,
    if_range: Option<String>,
}

/// The resume headers for a range request, carried as one indivisible unit.
///
/// `Range` and `If-Range` are never separated. `If-Range` is what turns a
/// range request into a conditional one: with a matching validator the origin
/// answers `206` for the same bytes, without it a `Range` alone invites a
/// `206` from a *different* resource, which the worker would then append to
/// the partial file. Dropping only the validator across a redirect would
/// therefore be a silent corruption, and dropping only the range would
/// silently restart a resume; both go, or neither does.
///
/// That rule is a type, not a convention: one `Option` covers the whole
/// capability, so no call site can construct or strip half of it. Stripping is
/// [`ResumeHeaders::strip`], which empties the single option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResumeHeaders {
    resume: Option<ResumeCapability>,
}

impl ResumeHeaders {
    /// The resume capability the caller's plan asks for, as one unit.
    fn from_plan(plan: Option<&RangeRequestPlan>) -> Self {
        Self {
            resume: plan.map(|plan| {
                let (range, if_range) = plan.headers();
                ResumeCapability { range, if_range }
            }),
        }
    }

    /// Drop the whole resume capability.
    ///
    /// Used when a redirect leaves the origin the caller's offset and
    /// validator describe: byte 4 of *that* resource says nothing about the
    /// new one, and a validator is a token minted by the old origin. Both go
    /// at once because there is only one thing to drop.
    fn strip(&mut self) {
        self.resume = None;
    }

    fn sends_range(&self) -> bool {
        self.resume.is_some()
    }

    /// The header lines to install, all-or-nothing.
    fn lines(&self) -> Vec<String> {
        match &self.resume {
            None => Vec::new(),
            Some(resume) => {
                let mut lines = vec![format!("Range: {}", resume.range)];
                if let Some(if_range) = &resume.if_range {
                    lines.push(format!("If-Range: {if_range}"));
                }
                lines
            }
        }
    }
}

/// The policy and the effects it screens: one URL policy for the whole
/// transfer, plus the (injectable) resolver and the peer libcurl reports.
struct Screening {
    policy: UrlPolicy,
    lookup: Arc<DownloadLookup>,
    #[cfg(test)]
    observed_peer_override: Option<IpAddr>,
}

/// The port a destination connects on.
fn destination_port(safe: &SafeUrl) -> Result<u16, TransportError> {
    Ok(safe.port().unwrap_or(match safe.scheme() {
        Scheme::Https => 443,
        Scheme::Http => 80,
        Scheme::Other => return Err(blocked("scheme-refused")),
    }))
}

/// The one total budget, as the resolver sees it: when the transfer started
/// and how long it is allowed to last in total.
///
/// Name resolution is inside that single budget, not beside it. A resolver is
/// the one blocking call in a hop that no libcurl deadline covers.
#[derive(Debug, Clone, Copy)]
struct ResolveBudget {
    started: Instant,
    total: Duration,
}

impl ResolveBudget {
    /// What is left of the total budget, or `None` once it is spent.
    fn remaining(&self) -> Option<Duration> {
        self.total.checked_sub(self.started.elapsed())
    }
}

/// Run the (blocking) name lookup so that cancellation and the total budget
/// can end the *wait* without waiting for the resolver — and so that giving up
/// cannot create unbounded resolver work.
///
/// `getaddrinfo` is uninterruptible: a stalled or hostile resolver holds the
/// calling thread for as long as it likes. Calling it on the worker thread
/// therefore put DNS outside every deadline the transport advertises — and
/// because `start` joins that worker on its error path, an expired total
/// budget or a cancellation could still block the *caller* until the resolver
/// returned.
///
/// So the lookup runs on its own thread and this function only waits on a
/// channel, in [`RECEIVE_SLICE`] steps, re-checking cancellation and the
/// remaining budget.
///
/// # Why the abandoned thread must still hold a permit
///
/// Returning promptly is only half the requirement. The abandoned helper is
/// still parked inside libc, so a redirect chain, a cancelled-and-retried
/// download, or a hostile origin that redirects to N stalling hosts would
/// otherwise accumulate one detached stack per hop with nothing bounding the
/// total. The lookup therefore runs under the process-wide
/// [`ResolverGate`](crate::resolver_gate::ResolverGate) that
/// [`crate::curl_transport`] already uses: the permit is moved into the helper
/// and dropped after the real lookup returns, so a caller that stops
/// waiting releases no capacity and admits no replacement. Capacity is
/// reclaimed by the work finishing, never by the waiter leaving.
fn resolve_under_budget(
    host: &str,
    port: u16,
    screening: &Screening,
    cancel: &CancelSignal,
    budget: ResolveBudget,
) -> Result<Vec<IpAddr>, TransportError> {
    resolve_under_budget_with_gate(
        host,
        port,
        screening,
        cancel,
        budget,
        shared_resolver_gate().clone(),
    )
}

/// [`resolve_under_budget`] against an explicit gate, so a test can build a
/// small one and saturate it deterministically.
fn resolve_under_budget_with_gate(
    host: &str,
    port: u16,
    screening: &Screening,
    cancel: &CancelSignal,
    budget: ResolveBudget,
    gate: ResolverGate,
) -> Result<Vec<IpAddr>, TransportError> {
    resolve_under_budget_with_gate_after_receive(host, port, screening, cancel, budget, gate, || {})
}

/// Resolver implementation with a post-receive observation seam.
///
/// Production supplies a no-op. A deterministic test can hold execution after
/// the resolver result becomes observable, trigger cancellation or total
/// expiry, and prove that the typed cause wins before the result is consumed.
fn resolve_under_budget_with_gate_after_receive(
    host: &str,
    port: u16,
    screening: &Screening,
    cancel: &CancelSignal,
    budget: ResolveBudget,
    gate: ResolverGate,
    mut after_receive: impl FnMut(),
) -> Result<Vec<IpAddr>, TransportError> {
    let remaining = || {
        budget
            .remaining()
            .filter(|remaining| !remaining.is_zero())
            .ok_or(TransportError::Timeout(TransportTimeout::Total))
    };

    // Waiting for capacity is inside the same cancellation and total budget as
    // everything else: a saturated gate must not become an unbounded wait.
    let permit = gate.acquire(|| {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        Ok(RECEIVE_SLICE.min(remaining()?))
    })?;
    // Cancellation or expiry may have won in the gap after admission. Refuse
    // before starting irrevocable libc work; the local permit returns the slot
    // because no lookup body ever began.
    if cancel.is_cancelled() {
        return Err(TransportError::Cancelled);
    }
    remaining()?;

    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<Vec<IpAddr>, ()>>(1);
    let lookup = Arc::clone(&screening.lookup);
    let host_owned = host.to_owned();
    if std::thread::Builder::new()
        .name("enzui-download-dns".to_owned())
        .spawn(move || {
            let resolved = lookup(&host_owned, port);
            // Released only after the real blocking call exits, and before
            // publishing the answer so a received result also means its slot
            // is back. Moving this above `lookup` — or dropping it on the
            // caller's cancellation path — is the ownership regression the
            // saturation gate catches.
            drop(permit);
            // `send` fails once the waiter has given up; that is the normal
            // abandoned-resolver path, not an error.
            let _ = tx.send(resolved);
        })
        .is_err()
    {
        return Err(TransportError::Io("dns resolver thread unavailable".into()));
    }

    loop {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        let wait = RECEIVE_SLICE.min(remaining()?);
        match rx.recv_timeout(wait) {
            Ok(resolved) => {
                after_receive();
                // Receiving a value and observing cancellation/expiry can race.
                // The caller's terminal cause wins even for a queued success or
                // DNS error, so no result can start a socket after cancellation
                // or launder an expired total deadline into `Io`.
                if cancel.is_cancelled() {
                    return Err(TransportError::Cancelled);
                }
                remaining()?;
                return resolved.map_err(|()| TransportError::Io("dns lookup failed".into()));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if cancel.is_cancelled() {
                    return Err(TransportError::Cancelled);
                }
                remaining()?;
                return Err(TransportError::Io(
                    "dns resolver ended without an answer".into(),
                ));
            }
        }
    }
}

/// Resolve a destination's host and screen every answer, returning the
/// address the hop may connect to.
///
/// The whole answer set is judged, not just the address that will be used: a
/// resolver that mixes one public and one internal address is a rebinding
/// attempt, and quietly picking the public one would conceal it. A literal IP
/// host needs no resolver but is screened identically.
///
/// The lookup itself runs under `budget` and `cancel` (see
/// [`resolve_under_budget`]), so a stalled resolver cannot outlive the one
/// total deadline shared by the whole redirect chain.
fn screen_destination(
    safe: &SafeUrl,
    screening: &Screening,
    cancel: &CancelSignal,
    budget: ResolveBudget,
) -> Result<(IpAddr, u16), TransportError> {
    let port = destination_port(safe)?;
    if cancel.is_cancelled() {
        return Err(TransportError::Cancelled);
    }
    let host = safe.host();
    let addresses: Vec<IpAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![ip]
    } else {
        resolve_under_budget(host, port, screening, cancel, budget)?
    };
    if addresses.is_empty() {
        return Err(TransportError::Io("dns lookup returned no address".into()));
    }
    let mut selected = None;
    for address in addresses {
        match screening.policy.check_address(address) {
            AddressVerdict::Allow { class } => {
                selected.get_or_insert((address, class));
            }
            AddressVerdict::Blocked { class } => {
                return Err(blocked(format!("address-blocked: {class:?}")));
            }
        }
    }
    let (address, _class) = selected.ok_or_else(|| blocked("address-blocked: no answer"))?;
    Ok((address, port))
}

/// Judge the address libcurl actually connected to.
///
/// Two independent refusals: the peer must be the exact address screening
/// selected (a mismatch means the `CURLOPT_RESOLVE` pin did not hold, i.e. a
/// rebind), and it must still pass the address policy in its own right.
fn verify_peer(
    policy: UrlPolicy,
    screened: IpAddr,
    observed: IpAddr,
) -> Result<(), TransportError> {
    if observed != screened {
        return Err(blocked(format!(
            "peer-mismatch: {observed} is not the screened {screened}"
        )));
    }
    match policy.check_address(observed) {
        AddressVerdict::Allow { .. } => Ok(()),
        AddressVerdict::Blocked { class } => Err(blocked(format!("address-blocked: {class:?}"))),
    }
}

/// Round a positive remainder up to libcurl's millisecond resolution.
///
/// The `curl` crate converts with `as_millis`, which rounds down: a positive
/// sub-millisecond remainder would become `0`, libcurl's spelling for
/// "timeout disabled" — turning an expiring budget into an unbounded one.
fn ceil_to_curl_millis(remaining: Duration) -> Duration {
    const NANOS_PER_MILLI: u32 = 1_000_000;
    let remainder = remaining.subsec_nanos() % NANOS_PER_MILLI;
    if remainder == 0 {
        remaining
    } else {
        remaining.saturating_add(Duration::from_nanos(u64::from(NANOS_PER_MILLI - remainder)))
    }
}

/// The Rust worker frame libcurl runs inside. Split out so the injected
/// panic (test-only) and the terminal `Done` publication both go through a
/// single point that owns the body-tx.
fn run_worker(
    destination: Destination,
    resume: ResumeHeaders,
    screening: &Screening,
    cancel: &CancelSignal,
    channels: WorkerChannels<'_>,
    deadlines: DownloadDeadlines,
    #[cfg(test)] panic_before_body_done: bool,
) {
    let WorkerChannels {
        internal_abort,
        body_tx,
        ..
    } = channels;
    let completed = follow_redirects(
        destination,
        resume,
        screening,
        cancel,
        WorkerChannels {
            internal_abort,
            meta_tx: channels.meta_tx,
            body_tx,
        },
        deadlines,
        #[cfg(test)]
        panic_before_body_done,
    );
    if completed {
        #[cfg(test)]
        if panic_before_body_done {
            panic!("injected private worker panic payload");
        }
        send_terminal(body_tx, BodyItem::Done, internal_abort);
    }
}

#[derive(Clone, Copy)]
enum CurlAbort {
    Cancelled,
    Internal,
    Timeout(TransportTimeout),
    HeaderLimit(HeaderLimitKind),
    /// A followed redirect's discarded body exceeded [`REDIRECT_BODY_CAP`].
    RedirectBodyCap,
    #[cfg(test)]
    InjectedWorkerPanic,
}

/// Put a terminal item behind every already-buffered body chunk. The loop is
/// interruptible by `Drop`: dropping the receiver makes `try_send` fail, while
/// the out-of-band abort also bounds the wait even if that ordering changes.
fn send_terminal(body_tx: &SyncSender<BodyItem>, item: BodyItem, internal_abort: &AtomicBool) {
    send_terminal_with_full_observer(body_tx, item, internal_abort, || {});
}

/// The terminal-send loop with a deterministic observation point for its
/// saturation test. Production supplies a no-op observer.
fn send_terminal_with_full_observer(
    body_tx: &SyncSender<BodyItem>,
    item: BodyItem,
    internal_abort: &AtomicBool,
    mut on_full: impl FnMut(),
) {
    let mut pending = item;
    loop {
        if internal_abort.load(Ordering::Acquire) {
            return;
        }
        match body_tx.try_send(pending) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(item)) => {
                pending = item;
                on_full();
                std::thread::sleep(RECEIVE_SLICE);
            }
        }
    }
}

/// What one hop concluded.
enum HopOutcome {
    /// The hop was terminal: metadata (or an error) has been published, and
    /// `completed` says whether the body finished and `Done` is owed.
    Terminal { completed: bool },
    /// The hop answered a redirect with this `Location` value. No metadata
    /// was published, and the body was discarded.
    Redirect(String),
}

/// Follow redirects manually under the transport's [`UrlPolicy`].
///
/// libcurl's own redirect following is off (`follow_location(false)`); each
/// hop is a separate, individually screened request. The loop runs
/// `max_redirects + 1` requests at most, and every hop re-runs the full
/// screening stack — URL string, all DNS answers, connected peer — so a
/// redirect into an internal address is refused at whatever hop it appears.
///
/// The total deadline is one budget for the whole chain: `started` is
/// taken once here, never per hop, so a redirect chain cannot extend the
/// transfer beyond what the caller allowed.
fn follow_redirects(
    mut destination: Destination,
    mut resume: ResumeHeaders,
    screening: &Screening,
    cancel: &CancelSignal,
    channels: WorkerChannels<'_>,
    deadlines: DownloadDeadlines,
    #[cfg(test)] panic_before_body_done: bool,
) -> bool {
    let WorkerChannels {
        internal_abort,
        meta_tx,
        body_tx,
    } = channels;
    let started = Instant::now();
    let policy = screening.policy;

    for hop in 0..=policy.max_redirects {
        if cancel.is_cancelled() {
            let _ = meta_tx.send(Err(TransportError::Cancelled));
            return false;
        }
        let (peer, port) = match screen_destination(
            &destination.safe,
            screening,
            cancel,
            ResolveBudget {
                started,
                total: deadlines.total,
            },
        ) {
            Ok(screened) => screened,
            Err(error) => {
                let _ = meta_tx.send(Err(error));
                return false;
            }
        };
        let outcome = run_hop(
            HopRequest {
                destination: &destination,
                resume: &resume,
                peer,
                port,
            },
            screening,
            cancel,
            WorkerChannels {
                internal_abort,
                meta_tx,
                body_tx,
            },
            deadlines,
            started,
            #[cfg(test)]
            panic_before_body_done,
        );
        let location = match outcome {
            HopOutcome::Terminal { completed } => return completed,
            HopOutcome::Redirect(location) => location,
        };

        // Resolve the `Location` against the hop that produced it. A
        // reference we cannot resolve is refused rather than guessed: an
        // invented URL would be screened under the wrong host.
        let Some(target) = resolve_reference(&destination.safe, &location) else {
            let _ = meta_tx.send(Err(blocked("redirect-unresolvable")));
            return false;
        };
        // Stage 3: the target is re-run through the whole stage-1 rule set
        // and the hop ceiling, from the URL that produced it.
        let decision = match policy.check_redirect(&destination.url, &target, hop + 1) {
            Ok(decision) => decision,
            Err(verdict) => {
                let _ = meta_tx.send(Err(url_blocked(verdict)));
                return false;
            }
        };
        // The cross-origin verdict is applied, not merely computed. A
        // `Range`/`If-Range` pair describes one representation on one origin:
        // carried to another host it either appends a foreign origin's bytes
        // to the partial file or hands that host a validator the caller never
        // meant it to see. Same-origin hops keep the whole capability; a
        // cross-origin hop loses all of it, which is one call because the
        // capability is one value (see [`ResumeHeaders`]). Discarding this
        // verdict is exactly the leak this line prevents.
        if !decision.carry_credentials {
            resume.strip();
        }
        destination = Destination {
            url: target,
            safe: decision.target,
        };
    }

    // Defensive: `check_redirect` refuses `hop > max_redirects` first, so a
    // chain normally ends there. Reaching here still ends the transfer with
    // the same typed refusal rather than looping.
    let _ = meta_tx.send(Err(url_blocked(UrlVerdict::TooManyRedirects {
        limit: policy.max_redirects,
    })));
    false
}

/// Everything one screened hop requests.
struct HopRequest<'a> {
    destination: &'a Destination,
    resume: &'a ResumeHeaders,
    /// The address [`screen_destination`] approved; libcurl is pinned to it
    /// and [`verify_peer`] re-checks that the connection landed there.
    peer: IpAddr,
    port: u16,
}

/// Callback-owned state for one multi-interface hop.
///
/// The easy interface drives callbacks until the entire response completes,
/// which is too late to inspect `CURLINFO_PRIMARY_IP`: metadata and body bytes
/// have already crossed their channels by then. The multi interface gives the
/// worker a control point between callbacks. The first body callback pauses,
/// the outer loop verifies the connected peer, publishes metadata, and only
/// then unpauses bounded streaming.
struct VerifiedHopHandler {
    headers: Vec<(String, String)>,
    header_acc: HeaderAccumulator,
    header_limit_hit: bool,
    metadata_sent: bool,
    status: u16,
    headers_complete: bool,
    redirect_to: Option<String>,
    discarded: u64,
    peer_accepted: bool,
    body_paused: bool,
    hop_started: Instant,
    last_body_progress: Instant,
    abort_reason: Option<CurlAbort>,
    cancel: CancelSignal,
    internal_abort: Arc<AtomicBool>,
    body_tx: SyncSender<BodyItem>,
    deadlines: DownloadDeadlines,
    chain_started: Instant,
    #[cfg(test)]
    panic_before_body_done: bool,
}

impl Handler for VerifiedHopHandler {
    fn header(&mut self, line: &[u8]) -> bool {
        let Ok(line) = std::str::from_utf8(line) else {
            return true;
        };
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix("HTTP/") {
            if let Some(code) = rest.split_whitespace().nth(1)
                && let Ok(code) = code.parse::<u16>()
            {
                self.status = code;
                self.headers.clear();
                self.header_acc.reset();
                self.headers_complete = false;
                self.redirect_to = None;
            }
            return true;
        }
        if line.is_empty() {
            if self.status < 200 {
                return true;
            }
            self.headers_complete = true;
            self.last_body_progress = Instant::now();
            if (300..400).contains(&self.status) {
                self.redirect_to = self
                    .headers
                    .iter()
                    .find(|(name, _)| name == "location")
                    .map(|(_, value)| value.clone());
            }
            return true;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if let Err(kind) = self.header_acc.check_header(&name, &value) {
                self.header_limit_hit = true;
                self.abort_reason = Some(CurlAbort::HeaderLimit(kind));
                return false;
            }
            self.headers.push((name, value));
        }
        true
    }

    fn write(&mut self, chunk: &[u8]) -> Result<usize, WriteError> {
        // No response byte may cross the transport boundary until the exact
        // connected peer has been accepted. Pausing asks libcurl to redeliver
        // this same chunk after the multi loop verifies `primary_ip`.
        if !self.peer_accepted {
            self.body_paused = true;
            return Err(WriteError::Pause);
        }

        if self.redirect_to.is_some() {
            let next = self
                .discarded
                .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            if next > REDIRECT_BODY_CAP {
                self.abort_reason = Some(CurlAbort::RedirectBodyCap);
                return Ok(0);
            }
            self.discarded = next;
            self.last_body_progress = Instant::now();
            return Ok(chunk.len());
        }

        #[cfg(test)]
        if self.panic_before_body_done {
            self.abort_reason = Some(CurlAbort::InjectedWorkerPanic);
            return Ok(0);
        }

        self.last_body_progress = Instant::now();
        let mut pending = BodyItem::Bytes(chunk.to_vec());
        loop {
            if self.cancel.is_cancelled() {
                self.abort_reason = Some(CurlAbort::Cancelled);
                return Ok(0);
            }
            if self.internal_abort.load(Ordering::Acquire) {
                self.abort_reason = Some(CurlAbort::Internal);
                return Ok(0);
            }
            match self.body_tx.try_send(pending) {
                Ok(()) => return Ok(chunk.len()),
                Err(TrySendError::Full(item)) => {
                    pending = item;
                    std::thread::sleep(RECEIVE_SLICE);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.abort_reason = Some(CurlAbort::Internal);
                    return Ok(0);
                }
            }
        }
    }

    fn progress(&mut self, _: f64, _: f64, _: f64, _: f64) -> bool {
        let reason = if self.cancel.is_cancelled() {
            Some(CurlAbort::Cancelled)
        } else if self.internal_abort.load(Ordering::Acquire) {
            Some(CurlAbort::Internal)
        } else if self.chain_started.elapsed() >= self.deadlines.total {
            Some(CurlAbort::Timeout(TransportTimeout::Total))
        } else if !self.headers_complete
            && self.hop_started.elapsed() >= self.deadlines.response_headers
        {
            Some(CurlAbort::Timeout(TransportTimeout::ResponseHeaders))
        } else if self.headers_complete
            && self.last_body_progress.elapsed() >= self.deadlines.body_progress
        {
            Some(CurlAbort::Timeout(TransportTimeout::BodyProgress))
        } else {
            None
        };
        if let Some(reason) = reason {
            self.abort_reason = Some(reason);
            false
        } else {
            true
        }
    }
}

/// Deliver a terminal error without overtaking buffered body chunks and
/// without losing it when the bounded channel is full.
fn publish_hop_error(
    metadata_sent: bool,
    error: TransportError,
    channels: WorkerChannels<'_>,
) -> HopOutcome {
    publish_hop_error_with_full_observer(metadata_sent, error, channels, || {})
}

/// [`publish_hop_error`] with a deterministic observation point for the
/// saturated-bridge test. Production supplies a no-op observer.
fn publish_hop_error_with_full_observer(
    metadata_sent: bool,
    error: TransportError,
    channels: WorkerChannels<'_>,
    on_full: impl FnMut(),
) -> HopOutcome {
    if metadata_sent {
        send_terminal_with_full_observer(
            channels.body_tx,
            BodyItem::Error(error),
            channels.internal_abort,
            on_full,
        );
    } else {
        let _ = channels.meta_tx.send(Err(error));
    }
    HopOutcome::Terminal { completed: false }
}

fn observed_multi_peer(
    handle: &Easy2Handle<VerifiedHopHandler>,
    screening: &Screening,
) -> Result<IpAddr, TransportError> {
    #[cfg(test)]
    if let Some(peer) = screening.observed_peer_override {
        return Ok(peer);
    }
    let _ = screening;
    handle
        .primary_ip()
        .map_err(be)?
        .and_then(|value| value.parse::<IpAddr>().ok())
        .ok_or_else(|| blocked("peer-unknown"))
}

/// Drive one libcurl hop while making connected-peer acceptance a publication
/// barrier. The body remains streamed through the same bounded channel; only
/// the first callback is paused long enough for the worker to inspect the live
/// easy handle's `primary_ip` through the multi interface. Redirect bodies are
/// likewise held until peer acceptance, then discarded under their fixed cap.
fn run_hop(
    request: HopRequest<'_>,
    screening: &Screening,
    cancel: &CancelSignal,
    channels: WorkerChannels<'_>,
    deadlines: DownloadDeadlines,
    chain_started: Instant,
    #[cfg(test)] panic_before_body_done: bool,
) -> HopOutcome {
    let HopRequest {
        destination,
        resume,
        peer,
        port,
    } = request;
    let range_requested = resume.sends_range();
    let handler = VerifiedHopHandler {
        headers: Vec::new(),
        header_acc: HeaderAccumulator::new(),
        header_limit_hit: false,
        metadata_sent: false,
        status: 0,
        headers_complete: false,
        redirect_to: None,
        discarded: 0,
        peer_accepted: false,
        body_paused: false,
        hop_started: Instant::now(),
        last_body_progress: Instant::now(),
        abort_reason: None,
        cancel: cancel.clone(),
        internal_abort: Arc::clone(channels.internal_abort),
        body_tx: channels.body_tx.clone(),
        deadlines,
        chain_started,
        #[cfg(test)]
        panic_before_body_done,
    };
    let mut easy = Easy2::new(handler);
    if let Err(error) = configure_verified(
        &mut easy,
        HopRequest {
            destination,
            resume,
            peer,
            port,
        },
        deadlines,
        chain_started,
    ) {
        return publish_hop_error(false, error, channels);
    }

    let multi = Multi::new();
    let mut handle = match multi.add2(easy) {
        Ok(handle) => handle,
        Err(error) => {
            return publish_hop_error(
                false,
                TransportError::Io(format!("curl multi setup: {error}")),
                channels,
            );
        }
    };
    let mut running = match multi.perform() {
        Ok(running) => running,
        Err(error) => {
            let _ = multi.remove2(handle);
            return publish_hop_error(
                false,
                TransportError::Io(format!("curl multi perform: {error}")),
                channels,
            );
        }
    };

    loop {
        // A received status line proves the connection exists. Inspect the
        // actual peer before publishing metadata or allowing the paused first
        // body chunk to cross the bounded bridge.
        if (handle.get_ref().status != 0
            || handle.get_ref().body_paused
            || handle.get_ref().headers_complete
            || handle.get_ref().header_limit_hit)
            && !handle.get_ref().peer_accepted
        {
            if cancel.is_cancelled() {
                let _ = multi.remove2(handle);
                return publish_hop_error(false, TransportError::Cancelled, channels);
            }
            if chain_started.elapsed() >= deadlines.total {
                let _ = multi.remove2(handle);
                return publish_hop_error(
                    false,
                    TransportError::Timeout(TransportTimeout::Total),
                    channels,
                );
            }
            if let Err(error) = observed_multi_peer(&handle, screening)
                .and_then(|observed| verify_peer(screening.policy, peer, observed))
            {
                let _ = multi.remove2(handle);
                return publish_hop_error(false, error, channels);
            }
            // Verification itself is small, but cancellation/expiry still
            // outranks publication if it became observable while primary_ip
            // was being read.
            if cancel.is_cancelled() {
                let _ = multi.remove2(handle);
                return publish_hop_error(false, TransportError::Cancelled, channels);
            }
            if chain_started.elapsed() >= deadlines.total {
                let _ = multi.remove2(handle);
                return publish_hop_error(
                    false,
                    TransportError::Timeout(TransportTimeout::Total),
                    channels,
                );
            }
            handle.get_mut().peer_accepted = true;
        }

        if handle.get_ref().peer_accepted && handle.get_ref().header_limit_hit {
            let error = match handle.get_ref().abort_reason {
                Some(CurlAbort::HeaderLimit(kind)) => {
                    TransportError::HeaderLimit(format!("{kind:?}"))
                }
                _ => TransportError::Io("header callback".into()),
            };
            let metadata_sent = handle.get_ref().metadata_sent;
            let _ = multi.remove2(handle);
            return publish_hop_error(metadata_sent, error, channels);
        }

        if handle.get_ref().peer_accepted
            && handle.get_ref().headers_complete
            && handle.get_ref().redirect_to.is_none()
            && !handle.get_ref().metadata_sent
        {
            let built = build_meta(
                handle.get_ref().status,
                &handle.get_ref().headers,
                range_requested,
            );
            let meta = match built {
                Ok(meta) => meta,
                Err(error) => {
                    // The paused first chunk cannot cross before the rejected metadata.
                    let _ = multi.remove2(handle);
                    return publish_hop_error(false, error, channels);
                }
            };
            handle.get_mut().metadata_sent = true;
            if channels.meta_tx.send(Ok(meta)).is_err() {
                channels.internal_abort.store(true, Ordering::Release);
                let _ = multi.remove2(handle);
                return HopOutcome::Terminal { completed: false };
            }
        }

        if handle.get_ref().peer_accepted && handle.get_ref().body_paused {
            handle.get_mut().body_paused = false;
            if let Err(error) = handle.unpause_write() {
                // Unpausing delivers the held chunk synchronously; a write
                // callback that aborts there surfaces as a write error, and
                // its recorded reason is the real outcome.
                if handle.get_ref().abort_reason.is_some() {
                    break;
                }
                let metadata_sent = handle.get_ref().metadata_sent;
                let _ = multi.remove2(handle);
                return publish_hop_error(metadata_sent, be(error), channels);
            }
        }

        if running == 0 {
            break;
        }
        if cancel.is_cancelled() {
            let metadata_sent = handle.get_ref().metadata_sent;
            let _ = multi.remove2(handle);
            return publish_hop_error(metadata_sent, TransportError::Cancelled, channels);
        }
        if channels.internal_abort.load(Ordering::Acquire) {
            channels.internal_abort.store(true, Ordering::Release);
            let _ = multi.remove2(handle);
            return HopOutcome::Terminal { completed: false };
        }
        if let Err(error) = multi.wait(&mut [], RECEIVE_SLICE) {
            let metadata_sent = handle.get_ref().metadata_sent;
            let _ = multi.remove2(handle);
            return publish_hop_error(
                metadata_sent,
                TransportError::Io(format!("curl multi wait: {error}")),
                channels,
            );
        }
        running = match multi.perform() {
            Ok(running) => running,
            Err(error) => {
                let metadata_sent = handle.get_ref().metadata_sent;
                let _ = multi.remove2(handle);
                return publish_hop_error(
                    metadata_sent,
                    TransportError::Io(format!("curl multi perform: {error}")),
                    channels,
                );
            }
        };
    }

    let mut perform = None;
    multi.messages(|message| {
        if let Some(result) = message.result_for2(&handle) {
            perform = Some(result);
        }
    });
    let metadata_sent = handle.get_ref().metadata_sent;
    let status = handle.get_ref().status;
    let abort_reason = handle.get_ref().abort_reason;
    let redirect_to = handle.get_ref().redirect_to.clone();
    let headers_complete = handle.get_ref().headers_complete;
    let peer_accepted = handle.get_ref().peer_accepted;
    let headers = handle.get_ref().headers.clone();
    let _ = multi.remove2(handle);

    if !peer_accepted && status != 0 {
        return publish_hop_error(false, blocked("peer-unknown"), channels);
    }
    if let Some(reason) = abort_reason {
        let error = match reason {
            CurlAbort::Cancelled => Some(TransportError::Cancelled),
            CurlAbort::Internal => None,
            CurlAbort::Timeout(phase) => Some(TransportError::Timeout(phase)),
            CurlAbort::RedirectBodyCap => Some(TransportError::Io(
                "redirect response body exceeded its ceiling".into(),
            )),
            CurlAbort::HeaderLimit(kind) => Some(TransportError::HeaderLimit(format!("{kind:?}"))),
            #[cfg(test)]
            CurlAbort::InjectedWorkerPanic => return HopOutcome::Terminal { completed: true },
        };
        return match error {
            Some(error) => publish_hop_error(metadata_sent, error, channels),
            None => HopOutcome::Terminal { completed: false },
        };
    }
    match perform {
        Some(Ok(())) => {
            if !headers_complete {
                return publish_hop_error(
                    metadata_sent,
                    TransportError::Io("response ended before headers completed".into()),
                    channels,
                );
            }
            match redirect_to {
                Some(location) => HopOutcome::Redirect(location),
                None => {
                    if !metadata_sent {
                        match build_meta(status, &headers, range_requested) {
                            Ok(meta) => {
                                if channels.meta_tx.send(Ok(meta)).is_err() {
                                    return HopOutcome::Terminal { completed: false };
                                }
                            }
                            Err(error) => return publish_hop_error(false, error, channels),
                        }
                    }
                    HopOutcome::Terminal { completed: true }
                }
            }
        }
        Some(Err(error)) => {
            let error =
                if error.is_operation_timedout() && chain_started.elapsed() >= deadlines.total {
                    TransportError::Timeout(TransportTimeout::Total)
                } else if error.is_operation_timedout() && status == 0 {
                    TransportError::Timeout(TransportTimeout::Connect)
                } else if error.is_operation_timedout() {
                    TransportError::Timeout(TransportTimeout::Total)
                } else {
                    TransportError::Io(format!("curl: {error}"))
                };
            publish_hop_error(metadata_sent, error, channels)
        }
        None => publish_hop_error(
            metadata_sent,
            TransportError::Io("curl transfer ended without a result".into()),
            channels,
        ),
    }
}

fn configure_verified(
    easy: &mut Easy2<VerifiedHopHandler>,
    request: HopRequest<'_>,
    deadlines: DownloadDeadlines,
    started: Instant,
) -> Result<(), TransportError> {
    let HopRequest {
        destination,
        resume,
        peer,
        port,
    } = request;
    easy.url(&destination.url).map_err(be)?;
    easy.follow_location(false).map_err(be)?;
    easy.unrestricted_auth(false).map_err(be)?;
    easy.proxy("").map_err(be)?;
    easy.useragent("enzUI-ICED/download").map_err(be)?;
    easy.progress(true).map_err(be)?;

    let remaining_total = deadlines.total.saturating_sub(started.elapsed());
    if remaining_total.is_zero() {
        return Err(TransportError::Timeout(TransportTimeout::Total));
    }
    easy.connect_timeout(ceil_to_curl_millis(deadlines.connect.min(remaining_total)))
        .map_err(be)?;
    easy.timeout(ceil_to_curl_millis(remaining_total))
        .map_err(be)?;

    let mut pinned = List::new();
    let address = match peer {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    pinned
        .append(&format!("{}:{port}:{address}", destination.safe.host()))
        .map_err(be)?;
    easy.resolve(pinned).map_err(be)?;

    let header_lines = resume.lines();
    if !header_lines.is_empty() {
        let mut list = List::new();
        for line in header_lines {
            list.append(&line).map_err(be)?;
        }
        easy.http_headers(list).map_err(be)?;
    }
    Ok(())
}

fn build_meta(
    status: u16,
    headers: &[(String, String)],
    range_requested: bool,
) -> Result<TransportResponseMeta, TransportError> {
    if status == 206 && !range_requested {
        // The destination is allowed; the response is malformed for this request.
        return Err(TransportError::Io(
            "partial content for a request that sent no Range".into(),
        ));
    }
    let get = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let content_range = get("content-range")
        .filter(|_| range_requested)
        .map(str::to_owned);
    let content_length = get("content-length").and_then(|v| v.parse::<u64>().ok());
    let content_encoding = get("content-encoding")
        .filter(|v| !v.eq_ignore_ascii_case("identity"))
        .map(str::to_owned);
    // Prefer a strong ETag; fall back to Last-Modified.
    let response_validator = get("etag")
        .and_then(Validator::parse_etag)
        .or_else(|| get("last-modified").and_then(Validator::last_modified));

    Ok(TransportResponseMeta {
        status,
        content_range,
        response_validator,
        content_length,
        content_encoding,
    })
}

/// Map a libcurl error to a [`TransportError`].
fn be(e: curl::Error) -> TransportError {
    TransportError::Io(format!("curl setup: {e}"))
}

#[cfg(test)]
mod tests;
