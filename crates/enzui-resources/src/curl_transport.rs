//! Real HTTP(S) transport backed by system libcurl.
//!
//! Security is layered rather than delegated to libcurl defaults:
//! - redirects are disabled in libcurl and followed manually, re-running
//!   [`UrlPolicy`] and connected-address screening for every hop;
//! - DNS runs behind a process-wide concurrency gate, every candidate is
//!   screened, and the chosen address is pinned with `CURLOPT_RESOLVE` so DNS
//!   rebinding cannot swap it;
//! - `CURLINFO_PRIMARY_IP` verifies the actual peer after the transfer;
//! - proxies are disabled (an ambient proxy would make the observed peer the
//!   proxy, invalidating destination policy);
//! - the progress callback returns `false` on cancellation, making libcurl abort
//!   the transfer (`CURLE_ABORTED_BY_CALLBACK`) instead of merely abandoning a
//!   future that continues network I/O.
//!
//! ## `CURLOPT_PROTOCOLS_STR` (deliberately not set)
//!
//! The residual risk `CURLOPT_PROTOCOLS_STR` would close is a scheme **parse
//! differential**: the policy's URL parser classifying a raw string as `https` while
//! libcurl parses a different, dangerous scheme from the *same* string.
//! It is not set, for two reasons:
//!
//! 1. **The safe `curl` 0.4.50 API does not expose it.** There is no
//!    `Easy::protocols`/`redirect_protocols` setter; reaching
//!    `CURLOPT_PROTOCOLS_STR` needs a raw `curl_easy_setopt` FFI call, which is
//!    `unsafe` — and this workspace denies `unsafe_code`. Adding it would mean a
//!    lint exception for one belt-and-braces option, which is a worse trade than
//!    the structural mitigations already in place.
//! 2. The differential is already closed structurally. A `RemoteRequest`
//!    only exists after [`UrlPolicy::check_url`] accepted the scheme
//!    (`Scheme::Http`/`Scheme::Https` only; everything else is refused
//!    *before* `easy.url()`), redirects are followed manually with the same
//!    check per hop, DNS is pinned via `CURLOPT_RESOLVE` so a differential host
//!    cannot be swapped in, and `CURLINFO_PRIMARY_IP` re-verifies the connected
//!    peer against policy after the transfer. A scheme libcurl would honour but
//!    the policy would not (e.g. `file`, `gopher`, `dict`) can never appear in the URL
//!    handed to `easy.url()` because it never survives `check_url`.
//!
//! If a future `curl` release exposes a safe protocols setter, pinning it to
//! `http,https` is cheap defence-in-depth and should be added then.

use crate::header_limits::{HeaderAccumulator, HeaderLimitKind};
use crate::resolver_gate::{ResolverGate, shared_resolver_gate};
use crate::service::{
    CancelSignal, RemoteRequest, TransferProgress, Transport, TransportError, TransportResponse,
    Validators,
};
use curl::easy::{Easy, List};
use enzui_core::cancellation::CancelCause;
use enzui_core::url_policy::{
    AddressClass, AddressVerdict, NO_LIMIT, Scheme, TransportLimits, UrlPolicy, resolve_reference,
};
// Re-exported for this module's own tests, which keep addressing
// `super::has_scheme`: the function moved to core (one definition for this
// transport and the media destination gate), the coverage did not. `cfg(test)`
// because the production path reaches it through `resolve_reference`.
use std::cell::{Cell, RefCell};
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// System-libcurl HTTP(S) transport.
#[derive(Debug, Default, Clone, Copy)]
pub struct CurlTransport;

impl CurlTransport {
    /// Construct the transport. libcurl handles are per request; connection
    /// pooling is intentionally deferred to the HTTP-cache slice so policy
    /// changes cannot accidentally reuse a connection screened under old rules.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Fetch one policy-checked request, polling an external cancellation
    /// predicate in addition to the shared [`CancelSignal`].
    ///
    /// This is the reusable seam a caller reaches for when its own cancellation
    /// lives *outside* the [`CancelSignal`] — for example a blocking task
    /// context whose `is_cancelled` predicate the caller wraps in a
    /// [`CancelProbe`]. The probe is polled from libcurl's progress callback
    /// itself and before every DNS and redirect hop, so a transfer that is only
    /// idling in DNS, connect, the header exchange or a bodyless response is
    /// aborted promptly rather than waiting for the total timeout. There is
    /// no bridge thread: the predicate runs inline on the worker performing the
    /// transfer.
    ///
    /// When the external predicate first reports cancellation, the probe records
    /// its caller-selected [`CancelCause`] on the shared signal
    /// (first-writer-wins; see [`CancelProbe`]), so the abort carries the same
    /// typed cause the ordinary path would surface.
    ///
    /// [`Transport::fetch`] delegates here with a
    /// [signal-only](CancelProbe::signal_only) probe, so the plain and probed
    /// paths run one implementation and cannot drift apart.
    ///
    /// # Errors
    ///
    /// A [`TransportError`] for a policy refusal, network failure, a limit breach, or cancellation.
    pub fn fetch_with_cancel_probe(
        &self,
        request: &RemoteRequest,
        policy: &UrlPolicy,
        limits: &TransportLimits,
        probe: &CancelProbe<'_>,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<TransportResponse, TransportError> {
        fetch_with_headers(
            request,
            HopContext {
                policy,
                limits,
                extra_headers: &[],
            },
            probe,
            progress,
            real_effects(),
        )
    }
}

/// An external cancellation predicate mapped onto a shared [`CancelSignal`].
///
/// The libcurl progress callback and every DNS/redirect hop poll
/// [`CancelProbe::is_cancelled`], so a caller whose cancellation lives outside
/// the [`CancelSignal`] (a blocking task context, say) can abort a transfer
/// that is only idling in DNS, connect, the header exchange or a bodyless
/// response — the phases in which no body bytes flow and a body-only probe
/// would wait for the total timeout.
///
/// The first time the external predicate reports cancellation, the chosen
/// [`CancelCause`] is recorded on the shared signal with
/// [`CancelSignal::cancel`], which is first-writer-wins: if any other cause has
/// already won, that earlier cause is preserved and the mapped cause is
/// absorbed. No bridge thread is involved — the predicate is polled inline on
/// the worker running the transfer.
pub struct CancelProbe<'a> {
    signal: &'a CancelSignal,
    external: Option<(&'a dyn Fn() -> bool, CancelCause)>,
}

impl<'a> CancelProbe<'a> {
    /// A probe that observes only the shared [`CancelSignal`], with no external
    /// predicate. This is what [`Transport::fetch`] delegates with, so the
    /// ordinary and probed paths share one implementation.
    #[must_use]
    pub fn signal_only(signal: &'a CancelSignal) -> Self {
        Self {
            signal,
            external: None,
        }
    }

    /// A probe that also polls `predicate`. The first time `predicate` returns
    /// `true`, `cause` is recorded on `signal` (first-writer-wins) and the
    /// transfer aborts at the next poll.
    #[must_use]
    pub fn new(
        signal: &'a CancelSignal,
        predicate: &'a dyn Fn() -> bool,
        cause: CancelCause,
    ) -> Self {
        Self {
            signal,
            external: Some((predicate, cause)),
        }
    }

    /// The winning cancellation cause, or `None` while still live.
    ///
    /// A cause already recorded on the signal short-circuits, so the external
    /// predicate can never override a first writer. Otherwise the predicate is
    /// polled and, on its first `true`, mapped onto the signal; the returned
    /// cause is then whatever actually won the signal's race.
    #[must_use]
    pub fn cause(&self) -> Option<CancelCause> {
        if let Some(cause) = self.signal.cause() {
            return Some(cause);
        }
        if let Some((predicate, cause)) = self.external
            && predicate()
        {
            self.signal.cancel(cause);
            return self.signal.cause();
        }
        None
    }

    /// Whether cancellation has been observed, from either source.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cause().is_some()
    }
}

/// The real effects: system DNS plus a libcurl transfer.
fn real_effects() -> HopEffects<'static> {
    HopEffects {
        lookup: Arc::new(blocking_lookup),
        wire: &CurlWire,
    }
}

impl Transport for CurlTransport {
    fn fetch(
        &self,
        request: &RemoteRequest,
        policy: &UrlPolicy,
        limits: &TransportLimits,
        cancel: &CancelSignal,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<TransportResponse, TransportError> {
        self.fetch_with_cancel_probe(
            request,
            policy,
            limits,
            &CancelProbe::signal_only(cancel),
            progress,
        )
    }

    /// Send `If-None-Match` so the origin may answer `304 Not Modified`.
    ///
    /// The `304` is returned to the caller as a response, not an error. The
    /// unconditional path treats any non-2xx/3xx as [`TransportError::HttpStatus`],
    /// which is right when no validator was sent — a bare `304` would be a
    /// protocol violation — but here a `304` is the successful outcome the
    /// request asked for, and the cache layer needs to see it to serve the bytes
    /// it already holds.
    ///
    /// With no validator this is exactly [`Transport::fetch`]: there is nothing
    /// to revalidate against, so no header is sent and a `304` would again be a
    /// protocol error.
    fn fetch_conditional(
        &self,
        request: &RemoteRequest,
        policy: &UrlPolicy,
        limits: &TransportLimits,
        cancel: &CancelSignal,
        progress: &mut dyn FnMut(TransferProgress),
        validators: &Validators<'_>,
    ) -> Result<TransportResponse, TransportError> {
        let headers = conditional_headers(validators);
        if headers.is_empty() {
            return self.fetch(request, policy, limits, cancel, progress);
        }
        fetch_with_headers(
            request,
            HopContext {
                policy,
                limits,
                extra_headers: &headers,
            },
            &CancelProbe::signal_only(cancel),
            progress,
            real_effects(),
        )
    }
}

/// What one wire transfer reports back.
///
/// Deliberately not carrying a peer *class*: classification is policy, and
/// policy is applied by [`perform_hop`] above every wire. A wire reports the
/// address it observed (`CURLINFO_PRIMARY_IP` for the real one) and nothing more,
/// so no wire — real or injected — can decide whether its own peer was allowed.
struct WireOutcome {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    /// The address the connection actually landed on.
    primary: IpAddr,
}

/// One HTTP request/response exchange against an already-screened peer.
///
/// # Why this seam exists
///
/// Before it, the whole redirect loop was only reachable through libcurl and a
/// real socket, and hop 1 was unreachable from any test: the only
/// redirect a `TcpListener` fixture could express under a single
/// `allow_private_network` flag was one the URL-string stage refused first, so
/// `check_redirect` returned `CredentialsInUrl` at hop 0 and the loop never
/// iterated. Per-hop connected-address screening, [`resolve_location`] and the
/// redirect bound therefore had no coverage at all while looking exercised.
///
/// The seam is placed below screening on purpose. Injecting at the
/// [`Transport`] level would replace the redirect loop rather than drive it, and
/// injecting above `check_address` would let the fake decide the verdict under
/// test. A `Wire` receives a peer that policy already accepted and cannot reach
/// any of the code the tests are about.
trait Wire {
    fn transfer(
        &self,
        exchange: &WireExchange<'_>,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<WireOutcome, TransportError>;
}

/// Everything one wire transfer needs, other than the progress sink.
///
/// Grouped rather than passed as seven positional parameters: `peer` and `port`
/// are only meaningful together, and `peer` is the *screened* destination, so
/// grouping them keeps a call site from reordering them past `limits`.
struct WireExchange<'a> {
    request: &'a RemoteRequest,
    /// The peer [`resolve_and_screen`] approved. A wire must connect to this,
    /// and [`perform_hop`] re-checks that it did.
    peer: IpAddr,
    port: u16,
    limits: &'a TransportLimits,
    /// Remaining request-wide connect and total budgets after DNS and all
    /// preceding redirect hops, rounded up to libcurl's millisecond precision.
    timeouts: TransferTimeouts,
    probe: &'a CancelProbe<'a>,
    extra_headers: &'a [String],
}

/// The injectable effects of a hop: name resolution and the transfer itself.
///
/// Everything else a hop does — address screening, the post-transfer peer
/// re-verification, `Location` resolution, `check_redirect`, the hop bound — is
/// shared production code that runs identically for both.
struct HopEffects<'a> {
    lookup: Arc<Lookup>,
    wire: &'a dyn Wire,
}

trait MonotonicClock: Send + Sync {
    fn now(&self) -> Duration;
}

#[derive(Debug)]
struct SystemClock {
    started: Instant,
}

impl SystemClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl MonotonicClock for SystemClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
}

/// An absolute point on a monotonic clock. `Unlimited` is produced only by the
/// explicit [`NO_LIMIT`] sentinel; finite arithmetic saturates at
/// [`Duration::MAX`] instead of accidentally becoming unlimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deadline {
    Unlimited,
    Finite(Duration),
}

impl Deadline {
    fn from_timeout(now: Duration, timeout_ms: u64) -> Self {
        if timeout_ms == NO_LIMIT {
            Self::Unlimited
        } else {
            Self::Finite(now.saturating_add(Duration::from_millis(timeout_ms)))
        }
    }

    fn earliest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unlimited, deadline) | (deadline, Self::Unlimited) => deadline,
            (Self::Finite(left), Self::Finite(right)) => Self::Finite(left.min(right)),
        }
    }

    fn remaining(self, now: Duration) -> Result<Option<Duration>, TransportError> {
        match self {
            Self::Unlimited => Ok(None),
            Self::Finite(deadline) if now < deadline => Ok(Some(deadline - now)),
            Self::Finite(_) => Err(TransportError::Timeout),
        }
    }

    fn wait_slice(self, now: Duration, maximum: Duration) -> Result<Duration, TransportError> {
        Ok(self
            .remaining(now)?
            .map_or(maximum, |left| left.min(maximum)))
    }
}

#[derive(Clone, Copy, Debug)]
struct RequestBudget {
    connect: Deadline,
    total: Deadline,
}

impl RequestBudget {
    fn new(limits: &TransportLimits, now: Duration) -> Self {
        Self {
            connect: Deadline::from_timeout(now, limits.connect_timeout_ms),
            total: Deadline::from_timeout(now, limits.total_timeout_ms),
        }
    }

    fn resolver_deadline(self) -> Deadline {
        self.connect.earliest(self.total)
    }

    fn transfer_timeouts(self, now: Duration) -> Result<TransferTimeouts, TransportError> {
        Ok(TransferTimeouts {
            connect: self.connect.remaining(now)?.map(ceil_to_curl_millis),
            total: self.total.remaining(now)?.map(ceil_to_curl_millis),
        })
    }

    fn ensure_total_remaining(self, now: Duration) -> Result<(), TransportError> {
        self.total.remaining(now).map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransferTimeouts {
    connect: Option<Duration>,
    total: Option<Duration>,
}

/// Convert a positive finite remainder to libcurl's millisecond resolution.
///
/// The `curl` crate uses `Duration::as_millis`, which rounds down. Passing a
/// positive sub-millisecond remainder through unchanged would therefore set a
/// zero timeout: libcurl's spelling for disabled/default timeout behavior. A
/// ceiling conversion preserves finite intent; saturating the final nanosecond
/// adjustment also keeps extreme finite durations finite.
fn ceil_to_curl_millis(remaining: Duration) -> Duration {
    debug_assert!(!remaining.is_zero(), "a zero wait would spin");
    const NANOS_PER_MILLI: u32 = 1_000_000;
    let remainder = remaining.subsec_nanos() % NANOS_PER_MILLI;
    if remainder == 0 {
        remaining
    } else {
        remaining.saturating_add(Duration::from_nanos(u64::from(NANOS_PER_MILLI - remainder)))
    }
}

/// The subset of a request that survives across redirect hops unchanged: the
/// URL/redirect policy, transport limits, and any conditional request headers
/// the cache asked to be sent. Grouped rather than passed as three positional
/// parameters so the redirect loop can hand them to `perform_hop` without
/// crossing the `clippy::too_many_arguments` threshold, and so a call site
/// cannot reorder policy and limits past `extra_headers`.
struct HopContext<'a> {
    policy: &'a UrlPolicy,
    limits: &'a TransportLimits,
    extra_headers: &'a [String],
}

/// Shared redirect-following fetch for both the plain and conditional paths.
fn fetch_with_headers(
    request: &RemoteRequest,
    ctx: HopContext<'_>,
    probe: &CancelProbe<'_>,
    progress: &mut dyn FnMut(TransferProgress),
    effects: HopEffects<'_>,
) -> Result<TransportResponse, TransportError> {
    fetch_with_headers_clock(
        request,
        ctx,
        probe,
        progress,
        effects,
        Arc::new(SystemClock::new()),
    )
}

fn fetch_with_headers_clock(
    request: &RemoteRequest,
    ctx: HopContext<'_>,
    probe: &CancelProbe<'_>,
    progress: &mut dyn FnMut(TransferProgress),
    effects: HopEffects<'_>,
    clock: Arc<dyn MonotonicClock>,
) -> Result<TransportResponse, TransportError> {
    let HopContext {
        policy,
        limits,
        extra_headers,
    } = ctx;
    if limits.connect_timeout_ms == 0 || limits.read_timeout_ms == 0 || limits.total_timeout_ms == 0
    {
        return Err(TransportError::Timeout);
    }

    let budget = RequestBudget::new(limits, clock.now());
    let mut validators = ConditionalValidators::from_lines(extra_headers.to_vec());
    let mut current = request.clone();
    for hop in 0..=policy.max_redirects {
        // Poll before every hop: this is the "before each DNS/redirect hop"
        // seam, so an external probe that fires between hops aborts before the
        // next resolve/transfer rather than after it.
        if probe.is_cancelled() {
            return Err(TransportError::Cancelled(
                probe.cause().unwrap_or(CancelCause::Cancelled),
            ));
        }
        let response = perform_hop(
            &current,
            HopContext {
                policy,
                limits,
                extra_headers: validators.lines(),
            },
            budget,
            &clock,
            probe,
            progress,
            &effects,
        )?;
        // A 304 answers a conditional request: no body, and the cached bytes
        // are the payload. It sits in the 3xx block but is not a redirect, so
        // it must be returned before the Location handling below (which would
        // otherwise fail with `Redirect` on a response that has no Location).
        if response.status == 304 && !validators.lines().is_empty() {
            return Ok(TransportResponse {
                status: 304,
                body: Vec::new(),
                final_url: current.safe_url().clone(),
                peer_class: response.peer_class,
                headers: response.headers,
            });
        }
        if !(300..400).contains(&response.status) {
            if !(200..300).contains(&response.status) {
                return Err(TransportError::HttpStatus(response.status));
            }
            return Ok(TransportResponse {
                status: response.status,
                body: response.body,
                final_url: current.safe_url().clone(),
                peer_class: response.peer_class,
                headers: response.headers,
            });
        }

        let location = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("location"))
            .map(|(_, value)| value.as_str())
            .ok_or(TransportError::Redirect)?;
        let target = resolve_location(&current, location)?;
        let decision = policy
            .check_redirect(current.transport_url(), &target, hop + 1)
            .map_err(TransportError::Url)?;
        // A cross-origin destination must not receive a validator minted by the
        // origin that issued the cached bytes, or it could answer 304 against
        // freshness it never issued and revalidate the original entry.
        if !decision.carry_credentials {
            validators.strip();
        }
        current = RemoteRequest::from_checked(target, decision.target);
    }
    Err(TransportError::Url(
        enzui_core::url_policy::UrlVerdict::TooManyRedirects {
            limit: policy.max_redirects,
        },
    ))
}

struct HopResponse {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    peer_class: AddressClass,
}

// One indivisible set of conditional validators. A validator is a token minted
// by the origin that issued the cached bytes (`If-None-Match` from its `ETag`,
// `If-Modified-Since` from its `Last-Modified`), so `strip` empties the whole
// set at once and no call site can leave one behind to reach another origin.
#[derive(Debug, Clone, Default)]
struct ConditionalValidators {
    lines: Vec<String>,
}

impl ConditionalValidators {
    fn from_lines(lines: Vec<String>) -> Self {
        Self { lines }
    }

    fn lines(&self) -> &[String] {
        &self.lines
    }

    fn strip(&mut self) {
        self.lines.clear();
    }
}

/// Build the conditional request headers for a revalidation.
///
/// Pure and separately tested on purpose. Whether libcurl puts these on the
/// wire cannot be verified in this environment — there is no network and no
/// local HTTPS origin — so the honest split is to make the part that *can* be
/// checked a total function over its inputs, and keep the transport's job down to
/// handing the result to `Easy::http_headers`. A test asserting "this header would
/// be sent" is true; a test pretending to complete a round trip would
/// not be.
///
/// Returns one `If-None-Match:` line per validator, or an empty vector when there
/// is nothing to revalidate against — in which case the caller must not set a
/// header list at all, since an empty list is not the same as no list.
///
/// A validator containing CR or LF is dropped, not escaped: it would
/// otherwise inject arbitrary extra headers into the request. The value comes
/// from a cache entry that a hostile origin supplied, so this is reachable input,
/// not a theoretical one.
fn conditional_headers(validators: &Validators<'_>) -> Vec<String> {
    let mut out = Vec::new();
    // `ETag` first: it is the strong validator, and RFC 9110 §13.1.3 says a
    // recipient honouring `If-None-Match` must ignore `If-Modified-Since`. Both
    // are still sent when both exist, so an origin that only implements the
    // weaker one can still answer 304.
    if let Some(etag) = sanitized_validator(validators.etag) {
        out.push(format!("If-None-Match: {etag}"));
    }
    if let Some(date) = sanitized_validator(validators.last_modified) {
        out.push(format!("If-Modified-Since: {date}"));
    }
    out
}

/// A validator that is safe to place in a header, or `None`.
///
/// A blank value is dropped: sending `If-None-Match:` with no value invites a 400
/// from strict origins.
///
/// CR/LF/NUL are dropped rather than escaped. The value comes from a cache
/// entry whose content a hostile origin supplied, so this is reachable input, and
/// splitting it would inject arbitrary extra headers into the request.
fn sanitized_validator(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        return None;
    }
    Some(value.to_owned())
}

#[derive(Debug, Clone, Copy)]
enum AbortReason {
    Cancelled(CancelCause),
    ByteLimit { received: u64, limit: u64 },
    Timeout,
    ContentEncoding,
    HeaderLimit(HeaderLimitKind),
}

/// One hop: screen the destination, run the transfer, then re-verify the peer.
///
/// This is the policy shell, and it is the same code for every wire. It is
/// where per-hop connected-address screening lives (`resolve_and_screen` before
/// the transfer, `check_address` on the observed peer after it), so a redirect to
/// an internal address is refused at whatever hop it appears — not merely at hop
/// 0. The transfer is delegated to [`HopEffects::wire`], which never sees a peer
/// policy has not already accepted.
fn perform_hop(
    request: &RemoteRequest,
    ctx: HopContext<'_>,
    budget: RequestBudget,
    clock: &Arc<dyn MonotonicClock>,
    probe: &CancelProbe<'_>,
    progress: &mut dyn FnMut(TransferProgress),
    effects: &HopEffects<'_>,
) -> Result<HopResponse, TransportError> {
    let HopContext {
        policy,
        limits,
        extra_headers,
    } = ctx;
    let safe = request.safe_url();
    let port = safe.port().unwrap_or(match safe.scheme() {
        Scheme::Https => 443,
        Scheme::Http => 80,
        Scheme::Other => return Err(TransportError::Dns),
    });
    let (peer, _class) = resolve_and_screen(
        safe.host(),
        port,
        *policy,
        probe,
        Arc::clone(&effects.lookup),
        budget.resolver_deadline(),
        Arc::clone(clock),
    )?;
    let timeouts = budget.transfer_timeouts(clock.now())?;

    let outcome = effects.wire.transfer(
        &WireExchange {
            request,
            peer,
            port,
            limits,
            timeouts,
            probe,
            extra_headers,
        },
        progress,
    )?;
    // A fake wire may not enforce its supplied total timeout. The real curl wire
    // does, but checking the shared shell also keeps redirect-budget behavior
    // deterministic and prevents policy work after an expired request.
    budget.ensure_total_remaining(clock.now())?;

    // The address actually connected to must be the one policy screened. A
    // mismatch means the pin did not hold, so it is judged (and refused) as the
    // address that was really reached.
    if outcome.primary != peer {
        return Err(TransportError::AddressBlocked(AddressClass::of(
            outcome.primary,
        )));
    }
    match policy.check_address(outcome.primary) {
        AddressVerdict::Allow { class } => Ok(HopResponse {
            status: outcome.status,
            body: outcome.body,
            headers: outcome.headers,
            peer_class: class,
        }),
        AddressVerdict::Blocked { class } => Err(TransportError::AddressBlocked(class)),
    }
}

/// The real libcurl transfer.
struct CurlWire;

impl Wire for CurlWire {
    fn transfer(
        &self,
        exchange: &WireExchange<'_>,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<WireOutcome, TransportError> {
        curl_transfer(exchange, progress)
    }
}

fn curl_transfer(
    exchange: &WireExchange<'_>,
    progress: &mut dyn FnMut(TransferProgress),
) -> Result<WireOutcome, TransportError> {
    let &WireExchange {
        request,
        peer,
        port,
        limits,
        timeouts,
        probe,
        extra_headers,
    } = exchange;
    let safe = request.safe_url();
    let mut easy = Easy::new();
    easy.url(request.transport_url())
        .map_err(|e| TransportError::Backend(e.code()))?;
    easy.follow_location(false)
        .map_err(|e| TransportError::Backend(e.code()))?;
    easy.unrestricted_auth(false)
        .map_err(|e| TransportError::Backend(e.code()))?;
    // Only set a list when there is something in it: an empty `List` is not the
    // same as no list, and handing libcurl one would replace its defaults.
    if !extra_headers.is_empty() {
        let mut list = curl::easy::List::new();
        for header in extra_headers {
            list.append(header)
                .map_err(|e| TransportError::Backend(e.code()))?;
        }
        easy.http_headers(list)
            .map_err(|e| TransportError::Backend(e.code()))?;
    }
    // Empty proxy disables environment proxies; otherwise connected-address
    // policy would inspect the proxy rather than the destination.
    easy.proxy("")
        .map_err(|e| TransportError::Backend(e.code()))?;
    easy.useragent("enzUI-ICED/async-image")
        .map_err(|e| TransportError::Backend(e.code()))?;
    easy.progress(true)
        .map_err(|e| TransportError::Backend(e.code()))?;

    if let Some(connect_timeout) = timeouts.connect {
        easy.connect_timeout(connect_timeout)
            .map_err(|e| TransportError::Backend(e.code()))?;
    }
    if let Some(total_timeout) = timeouts.total {
        easy.timeout(total_timeout)
            .map_err(|e| TransportError::Backend(e.code()))?;
    }
    if limits.max_bytes != NO_LIMIT {
        easy.max_filesize(limits.max_bytes)
            .map_err(|e| TransportError::Backend(e.code()))?;
    }

    let mut pinned = List::new();
    let address = match peer {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    pinned
        .append(&format!("{}:{port}:{address}", safe.host()))
        .map_err(|e| TransportError::Backend(e.code()))?;
    easy.resolve(pinned)
        .map_err(|e| TransportError::Backend(e.code()))?;

    let mut body = Vec::new();
    let mut headers = Vec::new();
    let header_acc = RefCell::new(HeaderAccumulator::new());
    let abort = Cell::new(None::<AbortReason>);
    let received = Cell::new(0u64);
    let declared_total = Cell::new(None::<u64>);
    let body_started = Cell::new(false);
    let last_change = Cell::new(Instant::now());
    let response_status = Cell::new(0u16);
    let progress = RefCell::new(progress);

    {
        let mut transfer = easy.transfer();
        transfer
            .header_function(|line| {
                let Ok(line) = std::str::from_utf8(line) else {
                    return true;
                };
                let line = line.trim_end_matches(['\r', '\n']);
                // A valid status line starts a fresh header/framing block.
                if let Some(rest) = line.strip_prefix("HTTP/") {
                    if let Some(code) = rest.split_whitespace().nth(1)
                        && let Ok(code) = code.parse::<u16>()
                    {
                        response_status.set(code);
                        headers.clear();
                        header_acc.borrow_mut().reset();
                        declared_total.set(None);
                    }
                    return true;
                }
                let Some((name, value)) = line.split_once(':') else {
                    return true;
                };
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim().to_owned();
                // Only a confirmed final block can contribute final framing.
                if response_status.get() >= 200 {
                    if name == "content-length"
                        && let Ok(total) = value.parse::<u64>()
                    {
                        declared_total.set(Some(total));
                        if limits.exceeds_bytes(total) {
                            abort.set(Some(AbortReason::ByteLimit {
                                received: total,
                                limit: limits.max_bytes,
                            }));
                            return false;
                        }
                    }
                    if name == "content-encoding" && !value.eq_ignore_ascii_case("identity") {
                        abort.set(Some(AbortReason::ContentEncoding));
                        return false;
                    }
                }
                if let Err(kind) = header_acc.borrow_mut().check_header(&name, &value) {
                    abort.set(Some(AbortReason::HeaderLimit(kind)));
                    return false;
                }
                headers.push((name, value));
                true
            })
            .map_err(|e| TransportError::Backend(e.code()))?;
        transfer
            .write_function(|chunk| {
                body_started.set(true);
                if probe.is_cancelled() {
                    abort.set(Some(AbortReason::Cancelled(
                        probe.cause().unwrap_or(CancelCause::Cancelled),
                    )));
                    return Ok(0);
                }
                let next = received
                    .get()
                    .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
                if limits.exceeds_bytes(next) {
                    abort.set(Some(AbortReason::ByteLimit {
                        received: next,
                        limit: limits.max_bytes,
                    }));
                    return Ok(0);
                }
                body.extend_from_slice(chunk);
                received.set(next);
                last_change.set(Instant::now());
                (progress.borrow_mut())(TransferProgress {
                    received: next,
                    total: declared_total.get(),
                });
                Ok(chunk.len())
            })
            .map_err(|e| TransportError::Backend(e.code()))?;
        transfer
            .progress_function(|total, now, _, _| {
                // libcurl invokes this even while only DNS/connect/header work
                // or a bodyless response is pending, so it is the seam that lets
                // an external probe abort an idle transfer without a bridge
                // thread and long before the total timeout.
                if probe.is_cancelled() {
                    abort.set(Some(AbortReason::Cancelled(
                        probe.cause().unwrap_or(CancelCause::Cancelled),
                    )));
                    return false;
                }
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "float-to-int `as` saturates, which is the intent"
                )]
                let now = now.max(0.0) as u64;
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "float-to-int `as` saturates, which is the intent"
                )]
                let total = if total > 0.0 {
                    Some(total as u64)
                } else {
                    declared_total.get()
                };
                if limits.exceeds_bytes(now) {
                    abort.set(Some(AbortReason::ByteLimit {
                        received: now,
                        limit: limits.max_bytes,
                    }));
                    return false;
                }
                if body_started.get()
                    && limits.read_timeout_ms != NO_LIMIT
                    && last_change.get().elapsed() >= Duration::from_millis(limits.read_timeout_ms)
                {
                    abort.set(Some(AbortReason::Timeout));
                    return false;
                }
                (progress.borrow_mut())(TransferProgress {
                    received: now,
                    total,
                });
                true
            })
            .map_err(|e| TransportError::Backend(e.code()))?;

        if let Err(error) = transfer.perform() {
            if let Some(reason) = abort.get() {
                return Err(match reason {
                    AbortReason::Cancelled(cause) => TransportError::Cancelled(cause),
                    AbortReason::ByteLimit { received, limit } => {
                        TransportError::ByteLimit { received, limit }
                    }
                    AbortReason::Timeout => TransportError::Timeout,
                    AbortReason::ContentEncoding => TransportError::ContentEncoding,
                    AbortReason::HeaderLimit(kind) => TransportError::HeaderLimit(kind),
                });
            }
            if error.is_operation_timedout() {
                return Err(TransportError::Timeout);
            }
            return Err(TransportError::Backend(error.code()));
        }
    }

    let status = easy
        .response_code()
        .map_err(|e| TransportError::Backend(e.code()))?;
    // An out-of-range status is a malformed reply (libcurl's
    // CURLE_WEIRD_SERVER_REPLY, code 8).
    let status = u16::try_from(status).map_err(|_| TransportError::Backend(8))?;
    let primary = easy
        .primary_ip()
        .map_err(|e| TransportError::Backend(e.code()))?
        .and_then(|value| value.parse::<IpAddr>().ok())
        .ok_or(TransportError::Dns)?;
    Ok(WireOutcome {
        status,
        body,
        headers,
        primary,
    })
}

/// Name of a bounded DNS helper thread (see [`resolve_with`]). Linux truncates
/// `comm` to 15 bytes, so this 15-byte name is reported verbatim.
const RESOLVER_THREAD_NAME: &str = "enzui-resolver";

/// How often capacity/result waits re-check cancellation and deadline state.
const RESOLVE_POLL_INTERVAL: Duration = Duration::from_millis(5);

type Lookup = dyn Fn(&str, u16) -> Result<Vec<IpAddr>, ()> + Send + Sync + 'static;

fn resolve_and_screen(
    host: &str,
    port: u16,
    policy: UrlPolicy,
    probe: &CancelProbe<'_>,
    lookup: Arc<Lookup>,
    deadline: Deadline,
    clock: Arc<dyn MonotonicClock>,
) -> Result<(IpAddr, AddressClass), TransportError> {
    if probe.is_cancelled() {
        return Err(TransportError::Cancelled(
            probe.cause().unwrap_or(CancelCause::Cancelled),
        ));
    }
    let addresses: Vec<IpAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        // A literal address needs no resolver, so there is nothing to block on.
        vec![ip]
    } else {
        resolve_with(host, port, probe, lookup, deadline, clock)?
    };
    if addresses.is_empty() {
        return Err(TransportError::Dns);
    }

    let mut selected = None;
    for address in addresses {
        match policy.check_address(address) {
            AddressVerdict::Allow { class } => selected.get_or_insert((address, class)),
            AddressVerdict::Blocked { class } => {
                // Fail the whole resolution set if it contains a private peer;
                // silently dropping the private candidate would make DNS answers
                // policy-dependent and conceal a rebinding attempt.
                return Err(TransportError::AddressBlocked(class));
            }
        };
    }
    selected.ok_or(TransportError::Dns)
}

/// The real blocking lookup. It runs only after obtaining a process-wide
/// [`ResolverGate`] permit.
fn blocking_lookup(host: &str, port: u16) -> Result<Vec<IpAddr>, ()> {
    (host, port)
        .to_socket_addrs()
        .map(|addrs| addrs.map(|socket| socket.ip()).collect::<Vec<_>>())
        .map_err(|_| ())
}

/// Resolve without trapping the calling service worker in libc DNS.
///
/// The helper cannot be interrupted safely, so cancellation or timeout releases
/// the caller but deliberately does not release resolver capacity. The
/// helper owns its permit until `lookup` really returns. Calls beyond the fixed
/// process-wide bound wait for capacity while still observing cancellation and
/// deadline state; therefore repeated request/cancel cannot create another
/// detached stack for every request.
fn resolve_with(
    host: &str,
    port: u16,
    probe: &CancelProbe<'_>,
    lookup: Arc<Lookup>,
    deadline: Deadline,
    clock: Arc<dyn MonotonicClock>,
) -> Result<Vec<IpAddr>, TransportError> {
    resolve_with_gate(
        host,
        port,
        probe,
        lookup,
        deadline,
        clock,
        shared_resolver_gate().clone(),
    )
}

fn resolve_with_gate(
    host: &str,
    port: u16,
    probe: &CancelProbe<'_>,
    lookup: Arc<Lookup>,
    deadline: Deadline,
    clock: Arc<dyn MonotonicClock>,
    gate: ResolverGate,
) -> Result<Vec<IpAddr>, TransportError> {
    // The gate counts; this closure is the caller's vocabulary for "stop
    // waiting". Refusing here has taken no slot and started no lookup. Polling
    // the probe here also lets an external predicate abort a resolve that is
    // still queued behind the process-wide resolver bound.
    let permit = gate.acquire(|| {
        if let Some(cause) = probe.cause() {
            return Err(TransportError::Cancelled(cause));
        }
        deadline.wait_slice(clock.now(), RESOLVE_POLL_INTERVAL)
    })?;
    // Cancellation/deadline may have won in the small interval after the gate
    // admitted us. Refuse before starting irrevocable libc work and let the
    // local permit return the slot.
    if let Some(cause) = probe.cause() {
        return Err(TransportError::Cancelled(cause));
    }
    deadline.remaining(clock.now())?;
    let (tx, rx) = mpsc::channel::<Result<Vec<IpAddr>, ()>>();
    let owned_host = host.to_owned();

    if std::thread::Builder::new()
        .name(RESOLVER_THREAD_NAME.to_owned())
        .spawn(move || {
            let resolved = lookup(owned_host.as_str(), port);
            // Release only after the real blocking call exits, and before
            // publishing completion so a received result also means its slot
            // has been reclaimed. Moving this above `lookup` is the ownership
            // regression guarded by the concurrency test.
            drop(permit);
            // Cancellation/timeout may already have dropped the receiver.
            let _ = tx.send(resolved);
        })
        .is_err()
    {
        return Err(TransportError::Dns);
    }

    loop {
        if let Some(cause) = probe.cause() {
            return Err(TransportError::Cancelled(cause));
        }
        let wait = deadline.wait_slice(clock.now(), RESOLVE_POLL_INTERVAL)?;
        match rx.recv_timeout(wait) {
            Ok(resolved) => {
                // A resolver result and the deadline can become observable in
                // either order. Recheck for both success and failure so a DNS
                // error completed after expiry cannot hide the typed timeout.
                deadline.remaining(clock.now())?;
                return resolved.map_err(|()| TransportError::Dns);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // The helper vanished without sending (it panicked); its permit was
            // still dropped by unwinding, so capacity is reclaimed.
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(TransportError::Dns),
        }
    }
}

/// Resolve a `Location` header against the hop that produced it.
///
/// The result is always an absolute URL string, handed straight to
/// [`UrlPolicy::check_redirect`]. This function therefore decides *what URL the
/// policy gets to judge*, which is why it must never invent one: a `Location`
/// this code misreads becomes a request the origin never asked for, screened
/// under the wrong host.
///
/// # Any `scheme:` prefix is passed through, not just `http`/`https`
///
/// Testing only `starts_with("http://")` would let a `Location: ftp://evil/x`
/// fall through to the *relative* branch and be pasted onto the current host
/// as `https://current.host/ftp://evil/x` — a fabricated URL, and one that
/// would make [`UrlVerdict::SchemeRefused`] unreachable from the redirect
/// path entirely. Any absolute reference is returned verbatim so `check_redirect` refuses the scheme *by name*. Refusing
/// is the policy layer's job; this function's job is to report the target
/// faithfully.
///
/// # `//host/path` is a network-path reference, not an absolute path
///
/// RFC 3986 §4.2: a reference beginning `//` replaces the whole authority,
/// inheriting only the scheme. Treating it as a path produced
/// `https://current.host//other.host/path` — again a fabricated URL. It now
/// resolves to `<base scheme>://other.host/path`, and the new host is screened
/// like any other hop.
fn resolve_location(base: &RemoteRequest, location: &str) -> Result<String, TransportError> {
    // The resolution itself lives in `enzui_core::url_policy` beside the
    // checks that judge its result, so this transport and the media
    // destination gate cannot drift apart on what a `Location` means.
    resolve_reference(base.safe_url(), location).ok_or(TransportError::Redirect)
}

// A Transport that drives the real redirect loop over a scripted wire, shared
// by the transport tests and the service/cache tests. A cross-origin hop
// between two distinct public origins cannot be expressed with a loopback
// TcpListener fixture, so each hop's response is scripted while the redirect
// loop, header building and cross-origin strip stay production code. The wire
// records the conditional headers each hop received.
#[cfg(test)]
pub(crate) mod scripted {
    use super::{
        Arc, CancelProbe, CancelSignal, HopContext, HopEffects, IpAddr, RemoteRequest,
        TransferProgress, Transport, TransportError, TransportLimits, TransportResponse, UrlPolicy,
        Validators, Wire, WireExchange, WireOutcome, conditional_headers, fetch_with_headers,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Clone)]
    pub(crate) struct Canned {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Canned {
        pub(crate) fn redirect(location: &str) -> Self {
            Self {
                status: 302,
                headers: vec![("location".to_owned(), location.to_owned())],
                body: Vec::new(),
            }
        }

        pub(crate) fn ok(body: Vec<u8>, headers: Vec<(String, String)>) -> Self {
            Self {
                status: 200,
                headers,
                body,
            }
        }

        pub(crate) fn not_modified(headers: Vec<(String, String)>) -> Self {
            Self {
                status: 304,
                headers,
                body: Vec::new(),
            }
        }
    }

    // Each host resolves to a distinct public address, so a hop's origin is a
    // property of its host rather than of the policy flags.
    fn scripted_lookup(host: &str, _port: u16) -> Result<Vec<IpAddr>, ()> {
        let addr = match host {
            "origin.test" => [93, 184, 216, 34],
            "cdn.test" => [93, 184, 216, 35],
            _ => return Err(()),
        };
        Ok(vec![IpAddr::from(addr)])
    }

    // Each hop's fetched URL paired with the conditional headers its wire
    // received.
    type WireHop = (String, Vec<String>);
    type WireLog = Arc<Mutex<Vec<WireHop>>>;

    struct ScriptWire {
        routes: HashMap<String, Canned>,
        seen: WireLog,
    }

    impl Wire for ScriptWire {
        fn transfer(
            &self,
            exchange: &WireExchange<'_>,
            _progress: &mut dyn FnMut(TransferProgress),
        ) -> Result<WireOutcome, TransportError> {
            let url = exchange.request.transport_url().to_owned();
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((url.clone(), exchange.extra_headers.to_vec()));
            let canned = self
                .routes
                .get(&url)
                .cloned()
                .ok_or(TransportError::HttpStatus(404))?;
            Ok(WireOutcome {
                status: canned.status,
                body: canned.body,
                headers: canned.headers,
                primary: exchange.peer,
            })
        }
    }

    pub(crate) struct ScriptedTransport {
        routes: HashMap<String, Canned>,
        seen: WireLog,
    }

    impl ScriptedTransport {
        pub(crate) fn new(routes: Vec<(&str, Canned)>) -> Self {
            Self {
                routes: routes
                    .into_iter()
                    .map(|(url, canned)| (url.to_owned(), canned))
                    .collect(),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn run(
            &self,
            request: &RemoteRequest,
            policy: UrlPolicy,
            limits: &TransportLimits,
            cancel: &CancelSignal,
            progress: &mut dyn FnMut(TransferProgress),
            extra_headers: &[String],
        ) -> Result<TransportResponse, TransportError> {
            let wire = ScriptWire {
                routes: self.routes.clone(),
                seen: Arc::clone(&self.seen),
            };
            fetch_with_headers(
                request,
                HopContext {
                    policy: &policy,
                    limits,
                    extra_headers,
                },
                &CancelProbe::signal_only(cancel),
                progress,
                HopEffects {
                    lookup: Arc::new(scripted_lookup),
                    wire: &wire,
                },
            )
        }
    }

    impl Transport for ScriptedTransport {
        fn fetch(
            &self,
            request: &RemoteRequest,
            policy: &UrlPolicy,
            limits: &TransportLimits,
            cancel: &CancelSignal,
            progress: &mut dyn FnMut(TransferProgress),
        ) -> Result<TransportResponse, TransportError> {
            self.run(request, *policy, limits, cancel, progress, &[])
        }

        fn fetch_conditional(
            &self,
            request: &RemoteRequest,
            policy: &UrlPolicy,
            limits: &TransportLimits,
            cancel: &CancelSignal,
            progress: &mut dyn FnMut(TransferProgress),
            validators: &Validators<'_>,
        ) -> Result<TransportResponse, TransportError> {
            let headers = conditional_headers(validators);
            self.run(request, *policy, limits, cancel, progress, &headers)
        }
    }
}

#[cfg(test)]
mod tests;
