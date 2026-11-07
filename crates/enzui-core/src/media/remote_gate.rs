//! Destination and redirect screening for remote media.
//!
//! [`MediaGate`] resolves and screens every address and redirect before producing an [`ApprovedStream`]. The media connection is pinned to the approved peer, automatic redirects remain disabled, and resolver concurrency stays bounded after cancellation.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crate::media::worker::StopSignal;
use crate::media::{MediaError, MediaErrorKind, MediaSource};
use crate::url_policy::{
    AddressClass, AddressVerdict, NO_LIMIT, RemoteSource, Scheme, TransportLimits, UrlPolicy,
    UrlVerdict, resolve_reference,
};

/// Memory limits for a remote stream.
///
/// Limits bound retained rather than transferred bytes. Zero in any field makes the budget unstreamable, and [`NO_LIMIT`] is valid only where the backend can represent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamBudget {
    /// `uridecodebin`'s `buffer-size`: bytes of demuxed data held in memory.
    pub buffer_bytes: u64,
    /// Maximum GStreamer ring-buffer bytes; zero and values above `u32::MAX` are invalid.
    pub ring_buffer_bytes: u64,
    /// Bytes a redirect probe may read from one hop before aborting. A hop that
    /// answers with a body instead of a `Location` must not become a download.
    pub probe_bytes: u64,
}

impl StreamBudget {
    /// 8 MiB held, a 64 MiB ring buffer, 64 KiB per redirect probe.
    pub const DEFAULT: StreamBudget = StreamBudget {
        buffer_bytes: 8 * 1024 * 1024,
        ring_buffer_bytes: 64 * 1024 * 1024,
        probe_bytes: 64 * 1024,
    };

    /// Returns whether the budget can support a stream and be represented by GStreamer.
    #[must_use]
    pub const fn is_streamable(&self) -> bool {
        self.buffer_bytes != 0
            && self.ring_buffer_bytes != 0
            && self.ring_buffer_bytes <= u32::MAX as u64
            && self.probe_bytes != 0
    }

    /// Returns the configured GStreamer `ring-buffer-max-size` value without validating the budget.
    #[must_use]
    pub const fn ring_buffer_property(&self) -> u64 {
        self.ring_buffer_bytes
    }
}

impl Default for StreamBudget {
    fn default() -> Self {
        StreamBudget::DEFAULT
    }
}

/// A policy rule that refused a remote source.
///
/// This enum is non-exhaustive so additional screening stages may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GateRefusal {
    /// A URL-string rule refused the original URL or a redirect target.
    Url(UrlVerdict),
    /// The connected destination is not a public address.
    AddressBlocked(AddressClass),
    /// The host could not be resolved, or resolved to nothing.
    Dns,
    /// A deadline from [`TransportLimits`] elapsed.
    Deadline,
    /// The caller cancelled before the source was approved.
    Cancelled,
    /// The probe itself failed (connection refused, malformed response).
    ProbeFailed,
    /// The [`StreamBudget`] cannot support a stream or cannot be represented by
    /// the live source's bounded properties.
    BudgetRefused,
}

impl GateRefusal {
    /// A short, stable, allocation-free tag for a log line or metric label.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            GateRefusal::Url(verdict) => verdict.reason(),
            GateRefusal::AddressBlocked(_) => "address-blocked",
            GateRefusal::Dns => "dns",
            GateRefusal::Deadline => "deadline",
            GateRefusal::Cancelled => "cancelled",
            GateRefusal::ProbeFailed => "probe-failed",
            GateRefusal::BudgetRefused => "budget-refused",
        }
    }
}

impl fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateRefusal::Url(verdict) => write!(f, "the URL was refused ({})", verdict.reason()),
            GateRefusal::AddressBlocked(class) => {
                write!(
                    f,
                    "the destination is a {class:?} address, not a public one"
                )
            }
            GateRefusal::Dns => f.write_str("the host did not resolve"),
            GateRefusal::Deadline => f.write_str("the destination check ran out of time"),
            GateRefusal::Cancelled => f.write_str("the request was cancelled"),
            GateRefusal::ProbeFailed => f.write_str("the destination probe failed"),
            GateRefusal::BudgetRefused => {
                f.write_str("the stream budget is empty or not representable")
            }
        }
    }
}

impl From<UrlVerdict> for GateRefusal {
    fn from(verdict: UrlVerdict) -> Self {
        GateRefusal::Url(verdict)
    }
}

impl From<GateRefusal> for MediaError {
    /// Converts a refusal to a redacted [`MediaErrorKind::Refused`] error.
    fn from(refusal: GateRefusal) -> MediaError {
        MediaError {
            kind: MediaErrorKind::Refused,
            message: format!("the remote source was refused: {refusal}"),
        }
    }
}

/// The result of probing one redirect hop without following it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HopReport {
    /// The origin answered with a non-redirect status: this URL is the stream.
    Final,
    /// The origin answered `3xx` with this `Location` value, exactly as sent
    /// (absolute or relative — resolving it is policy, not probing).
    Redirect(String),
}

/// A resolver or probe failure that carries no policy verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EffectFailed {
    /// The effect itself failed (lookup error, malformed response, transport error).
    #[default]
    Unavailable,
    /// The request-wide deadline expired while the effect was in flight.
    Deadline,
    /// Cancellation won while the effect was in flight.
    Cancelled,
}

impl fmt::Display for EffectFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffectFailed::Unavailable => f.write_str("the injected effect failed"),
            EffectFailed::Deadline => f.write_str("the injected effect exceeded its deadline"),
            EffectFailed::Cancelled => f.write_str("the injected effect was cancelled"),
        }
    }
}

impl std::error::Error for EffectFailed {}

/// Resolves host names for destination screening.
pub trait HostResolver {
    /// All addresses `host` resolves to, or a typed effect failure.
    ///
    /// Every returned address is screened; returning only the "good" ones would
    /// hide a rebinding answer.
    ///
    /// # Errors
    ///
    /// An [`EffectFailed`] when resolution fails, times out or is cancelled.
    fn resolve(
        &self,
        host: &str,
        port: u16,
        limits: &TransportLimits,
        clock: &dyn Clock,
        cancel: &dyn CancelView,
    ) -> Result<Vec<IpAddr>, EffectFailed>;
}

/// Probes one URL through an already-screened peer without following redirects.
pub trait RedirectProbe {
    /// Asks `url` at the screened endpoint whether it redirects, reading at
    /// most `budget.probe_bytes` and obeying `limits`.
    ///
    /// # Errors
    ///
    /// An [`EffectFailed`] when the probe fails, exceeds its limits or is cancelled.
    fn probe(
        &self,
        url: &str,
        endpoint: SocketAddr,
        limits: &TransportLimits,
        budget: &StreamBudget,
        clock: &dyn Clock,
        cancel: &dyn CancelView,
    ) -> Result<HopReport, EffectFailed>;
}

/// Supplies elapsed time for approval deadline checks.
pub trait Clock {
    /// Milliseconds since the operation started.
    fn elapsed_ms(&self) -> u64;
}

/// A real monotonic clock started at construction.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    /// Start the clock now.
    #[must_use]
    pub fn start() -> Self {
        MonotonicClock {
            start: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        MonotonicClock::start()
    }
}

impl Clock for MonotonicClock {
    fn elapsed_ms(&self) -> u64 {
        crate::monotonic_ms::MonotonicMs::from_duration_saturating(self.start.elapsed())
    }
}

/// Whether the caller has given up. Implemented for the media worker's own
/// [`StopSignal`], so a shutdown observed mid-approval is not waited out.
pub trait CancelView {
    /// Whether the operation should stop now.
    fn is_cancelled(&self) -> bool;
}

impl CancelView for StopSignal {
    fn is_cancelled(&self) -> bool {
        self.should_stop()
    }
}

/// A cancellation view that never cancels, for callers with no signal.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancelled;

impl CancelView for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// The system resolver: `ToSocketAddrs` over the platform's stub resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    fn resolve(
        &self,
        host: &str,
        port: u16,
        limits: &TransportLimits,
        clock: &dyn Clock,
        cancel: &dyn CancelView,
    ) -> Result<Vec<IpAddr>, EffectFailed> {
        bounded_system_lookup(host, port, limits, clock, cancel)
    }
}

const RESOLVER_LIMIT: usize = 4;
const EFFECT_POLL: Duration = Duration::from_millis(5);

#[derive(Debug)]
struct ResolverState {
    active: usize,
}

#[derive(Clone, Debug)]
struct ResolverGate {
    shared: Arc<(Mutex<ResolverState>, Condvar)>,
    limit: usize,
}

impl ResolverGate {
    fn new(limit: usize) -> Self {
        assert!(limit > 0, "a zero limit admits nothing");
        Self {
            shared: Arc::new((Mutex::new(ResolverState { active: 0 }), Condvar::new())),
            limit,
        }
    }

    fn acquire(
        &self,
        limits: &TransportLimits,
        clock: &dyn Clock,
        cancel: &dyn CancelView,
    ) -> Result<ResolverPermit, EffectFailed> {
        let (lock, wake) = &*self.shared;
        loop {
            if cancel.is_cancelled() {
                return Err(EffectFailed::Cancelled);
            }
            let now_ms = clock.elapsed_ms();
            if limits.connect_expired(now_ms) || limits.total_expired(now_ms) {
                return Err(EffectFailed::Deadline);
            }

            let mut state = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.active < self.limit {
                state.active += 1;
                return Ok(ResolverPermit { gate: self.clone() });
            }
            let (state, _) = wake
                .wait_timeout(state, EFFECT_POLL)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(state);
        }
    }
}

fn resolver_gate() -> &'static ResolverGate {
    static GATE: OnceLock<ResolverGate> = OnceLock::new();
    GATE.get_or_init(|| ResolverGate::new(RESOLVER_LIMIT))
}

#[derive(Debug)]
struct ResolverPermit {
    gate: ResolverGate,
}

impl Drop for ResolverPermit {
    fn drop(&mut self) {
        let (lock, wake) = &*self.gate.shared;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = state.active.saturating_sub(1);
        wake.notify_one();
    }
}

fn bounded_system_lookup(
    host: &str,
    port: u16,
    limits: &TransportLimits,
    clock: &dyn Clock,
    cancel: &dyn CancelView,
) -> Result<Vec<IpAddr>, EffectFailed> {
    use std::net::ToSocketAddrs;

    let lookup = Arc::new(|host: &str, port: u16| {
        (host, port)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|socket| socket.ip()).collect::<Vec<_>>())
            .map_err(|_| EffectFailed::Unavailable)
    });
    bounded_lookup_with(
        host,
        port,
        limits,
        clock,
        cancel,
        lookup,
        resolver_gate().clone(),
    )
}

type Lookup = dyn Fn(&str, u16) -> Result<Vec<IpAddr>, EffectFailed> + Send + Sync + 'static;

fn bounded_lookup_with(
    host: &str,
    port: u16,
    limits: &TransportLimits,
    clock: &dyn Clock,
    cancel: &dyn CancelView,
    lookup: Arc<Lookup>,
    gate: ResolverGate,
) -> Result<Vec<IpAddr>, EffectFailed> {
    let permit = gate.acquire(limits, clock, cancel)?;
    let host = host.to_owned();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("enzui-media-dns".to_owned())
        .spawn(move || {
            let answer = lookup(host.as_str(), port);
            drop(permit);
            let _ = tx.send(answer);
        })
        .map_err(|_| EffectFailed::Unavailable)?;

    loop {
        if cancel.is_cancelled() {
            return Err(EffectFailed::Cancelled);
        }
        let now_ms = clock.elapsed_ms();
        if limits.connect_expired(now_ms) || limits.total_expired(now_ms) {
            return Err(EffectFailed::Deadline);
        }
        match rx.recv_timeout(EFFECT_POLL) {
            Ok(answer) => return answer,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(EffectFailed::Unavailable);
            }
        }
    }
}

/// Injected resolution, redirect, clock, and cancellation effects for approval.
pub struct GateEffects<'a> {
    /// Name resolution.
    pub resolver: &'a dyn HostResolver,
    /// Optional redirect probe; without one, approval cannot follow redirects.
    pub probe: Option<&'a dyn RedirectProbe>,
    /// Elapsed time.
    pub clock: &'a dyn Clock,
    /// Cancellation.
    pub cancel: &'a dyn CancelView,
}

/// A remote stream approved by [`MediaGate::approve`]; its fields cannot be constructed directly.
#[derive(Debug, Clone)]
pub struct ApprovedStream {
    source: RemoteSource,
    peer: IpAddr,
    class: AddressClass,
    hops: u32,
    approved_at_ms: u64,
    budget: StreamBudget,
    limits: TransportLimits,
    redirects_followed: bool,
}

impl ApprovedStream {
    /// Returns the full transport URI, including its query; this value is not safe for logging. Use [`ApprovedStream::redacted`] for display.
    #[must_use]
    pub fn transport_url(&self) -> &str {
        self.source.transport_url()
    }

    /// The approved host, for pinning the media backend's connection.
    #[must_use]
    pub fn host(&self) -> &str {
        self.source.safe_url().host()
    }

    /// The approved effective port.
    ///
    /// # Panics
    ///
    /// Never for an approved HTTP(S) source, which always has one.
    #[must_use]
    pub fn port(&self) -> u16 {
        effective_port(&self.source).expect("an approved HTTP(S) source has an effective port")
    }

    /// The approved scheme.
    #[must_use]
    pub fn scheme(&self) -> Scheme {
        self.source.safe_url().scheme()
    }

    /// The gate clock's reading when this stream was approved, in milliseconds.
    #[must_use]
    pub fn approved_at_ms(&self) -> u64 {
        self.approved_at_ms
    }

    /// The redacted form: no userinfo, no query.
    #[must_use]
    pub fn redacted(&self) -> String {
        self.source.redacted()
    }

    /// The screened address the stream is expected to connect to.
    #[must_use]
    pub fn peer(&self) -> IpAddr {
        self.peer
    }

    /// Returns the peer's approved address class.
    #[must_use]
    pub fn peer_class(&self) -> AddressClass {
        self.class
    }

    /// How many redirects were followed and re-validated to reach this URL.
    #[must_use]
    pub fn hops(&self) -> u32 {
        self.hops
    }

    /// Returns whether approval used a redirect-validation probe; the final transport must still disable automatic redirects.
    #[must_use]
    pub fn redirects_followed(&self) -> bool {
        self.redirects_followed
    }

    /// The budget the transport must apply.
    #[must_use]
    pub fn budget(&self) -> StreamBudget {
        self.budget
    }

    /// The deadlines the transport must apply.
    #[must_use]
    pub fn limits(&self) -> TransportLimits {
        self.limits
    }

    /// Returns whether this approval is too old to use.
    ///
    /// Clock rollback, an unrepresentable deadline, and age equal to the TTL all expire the approval.
    #[must_use]
    pub fn is_expired(&self, now_ms: u64, ttl_ms: u64) -> bool {
        if now_ms < self.approved_at_ms {
            return true;
        }
        if ttl_ms == NO_LIMIT {
            return false;
        }
        use crate::monotonic_ms::{Boundary, MonotonicMs, Policy};
        let deadline = MonotonicMs(self.approved_at_ms).deadline_from(ttl_ms, Policy::Expire);
        MonotonicMs(now_ms).is_expired(deadline, Boundary::Inclusive)
    }
}

impl fmt::Display for ApprovedStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

/// URL, destination, deadline, and memory policy for remote media approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaGate {
    /// URL and redirect policy applied during remote-media approval.
    pub policy: UrlPolicy,
    /// Connect/read/total deadlines applied to the approval and handed to the
    /// transport.
    pub limits: TransportLimits,
    /// What the stream may hold in memory.
    pub budget: StreamBudget,
    /// How long an approval stays usable, in milliseconds.
    pub approval_ttl_ms: u64,
}

impl MediaGate {
    /// HTTPS-only public destinations with five redirects, standard limits and budget, and a 30-second approval TTL.
    pub const DEFAULT: MediaGate = MediaGate {
        policy: UrlPolicy::DEFAULT,
        limits: TransportLimits::DEFAULT,
        budget: StreamBudget::DEFAULT,
        approval_ttl_ms: 30_000,
    };

    /// Screens a source and produces an [`ApprovedStream`] only after budget, cancellation, deadline, address, and redirect checks pass.
    ///
    /// # Errors
    ///
    /// Returns the [`GateRefusal`] for the first failed stage.
    pub fn approve(
        &self,
        source: &RemoteSource,
        effects: &GateEffects<'_>,
    ) -> Result<ApprovedStream, GateRefusal> {
        if !self.budget.is_streamable() {
            return Err(GateRefusal::BudgetRefused);
        }
        // A zero deadline is already expired, matching `CurlTransport` before it opens a handle.
        if self.limits.connect_timeout_ms == 0
            || self.limits.read_timeout_ms == 0
            || self.limits.total_timeout_ms == 0
        {
            return Err(GateRefusal::Deadline);
        }

        let mut current = source.clone();
        for hop in 0..=self.policy.max_redirects {
            self.check_live(effects)?;

            let port = effective_port(&current)?;
            let (peer, class) =
                self.resolve_and_screen(current.safe_url().host(), port, effects)?;
            // Recheck cancellation and deadlines after resolution, using one clock sample for both deadline predicates.
            self.check_live(effects)?;

            let Some(probe) = effects.probe else {
                // Without a probe, approve only the current URL and require the transport to keep redirects disabled.
                return Ok(self.approve_at(current, peer, class, hop, false, effects));
            };

            let report = probe
                .probe(
                    current.transport_url(),
                    SocketAddr::new(peer, port),
                    &self.limits,
                    &self.budget,
                    effects.clock,
                    effects.cancel,
                )
                .map_err(map_effect_failure)?;
            // A terminal probe is still an effect. Do not mint a capability if
            // cancellation/deadline won during it.
            self.check_live(effects)?;
            match report {
                HopReport::Final => {
                    return Ok(self.approve_at(current, peer, class, hop, true, effects));
                }
                HopReport::Redirect(location) => {
                    let target = resolve_reference(current.safe_url(), &location)
                        .ok_or(GateRefusal::Url(UrlVerdict::Malformed))?;
                    let decision = self.policy.check_redirect(
                        current.transport_url(),
                        &target,
                        hop.saturating_add(1),
                    )?;
                    // Evaluate cross-origin credential policy even when the current request carries no authorization header.
                    let _carry_credentials = decision.carry_credentials;
                    current = RemoteSource::checked(&target, &self.policy)?;
                }
            }
        }
        Err(GateRefusal::Url(UrlVerdict::TooManyRedirects {
            limit: self.policy.max_redirects,
        }))
    }

    /// Approves a remote [`MediaSource`] and passes a local source through unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`GateRefusal`] when a remote source fails screening.
    pub fn approve_source(
        &self,
        source: &MediaSource,
        effects: &GateEffects<'_>,
    ) -> Result<Option<ApprovedStream>, GateRefusal> {
        match source {
            MediaSource::Local(_) => Ok(None),
            MediaSource::Remote(remote) => self.approve(remote, effects).map(Some),
        }
    }

    fn check_live(&self, effects: &GateEffects<'_>) -> Result<(), GateRefusal> {
        if effects.cancel.is_cancelled() {
            return Err(GateRefusal::Cancelled);
        }
        let now_ms = effects.clock.elapsed_ms();
        if self.limits.connect_expired(now_ms) || self.limits.total_expired(now_ms) {
            return Err(GateRefusal::Deadline);
        }
        Ok(())
    }

    fn approve_at(
        &self,
        source: RemoteSource,
        peer: IpAddr,
        class: AddressClass,
        hops: u32,
        redirects_followed: bool,
        effects: &GateEffects<'_>,
    ) -> ApprovedStream {
        ApprovedStream {
            source,
            peer,
            class,
            hops,
            approved_at_ms: effects.clock.elapsed_ms(),
            budget: self.budget,
            limits: self.limits,
            redirects_followed,
        }
    }

    // Reject the entire DNS answer if any resolved address fails screening.
    fn resolve_and_screen(
        &self,
        host: &str,
        port: u16,
        effects: &GateEffects<'_>,
    ) -> Result<(IpAddr, AddressClass), GateRefusal> {
        let addresses: Vec<IpAddr> = if let Ok(literal) = host.parse::<IpAddr>() {
            // A literal address needs no resolver — and is exactly the case the
            // string stage cannot judge.
            vec![literal]
        } else {
            effects
                .resolver
                .resolve(host, port, &self.limits, effects.clock, effects.cancel)
                .map_err(map_resolver_failure)?
        };
        if addresses.is_empty() {
            return Err(GateRefusal::Dns);
        }
        let mut selected = None;
        for address in addresses {
            match self.policy.check_address(address) {
                AddressVerdict::Allow { class } => {
                    selected.get_or_insert((address, class));
                }
                AddressVerdict::Blocked { class } => {
                    return Err(GateRefusal::AddressBlocked(class));
                }
            }
        }
        selected.ok_or(GateRefusal::Dns)
    }
}

fn map_resolver_failure(failure: EffectFailed) -> GateRefusal {
    match failure {
        EffectFailed::Unavailable => GateRefusal::Dns,
        EffectFailed::Deadline => GateRefusal::Deadline,
        EffectFailed::Cancelled => GateRefusal::Cancelled,
    }
}

fn map_effect_failure(failure: EffectFailed) -> GateRefusal {
    match failure {
        EffectFailed::Unavailable => GateRefusal::ProbeFailed,
        EffectFailed::Deadline => GateRefusal::Deadline,
        EffectFailed::Cancelled => GateRefusal::Cancelled,
    }
}

impl Default for MediaGate {
    fn default() -> Self {
        MediaGate::DEFAULT
    }
}

fn effective_port(source: &RemoteSource) -> Result<u16, GateRefusal> {
    let safe = source.safe_url();
    match safe.port() {
        Some(port) => Ok(port),
        None => match safe.scheme() {
            Scheme::Https => Ok(443),
            Scheme::Http => Ok(80),
            // Unreachable through `check_url`, which refuses other schemes;
            // fail closed rather than inventing a port.
            Scheme::Other => Err(GateRefusal::Url(UrlVerdict::SchemeRefused {
                scheme: Scheme::Other,
            })),
        },
    }
}

/// Fake resolver, probe, clock and cancellation effects for exercising the
/// gate without a network. Built for this crate's tests and, behind the
/// `media-test-support` feature, for the media services' tests.
#[cfg(any(test, feature = "media-test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
