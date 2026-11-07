//! URL and transport safety policy for remote HTTP resources.
//!
//! Callers validate URL syntax before DNS, validate the connected address after resolution, revalidate every redirect, and enforce [`TransportLimits`] while streaming. [`UrlPolicy`] refuses embedded user information, and [`SafeUrl`] redacts query values when formatted.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

/// Sentinel for an unbounded ceiling or deadline; unlike `0`, it never causes refusal.
pub const NO_LIMIT: u64 = u64::MAX;

/// The URL scheme, as classified by [`UrlPolicy`]. Only the two HTTP schemes are
/// modelled; anything else is [`Scheme::Other`] and refused by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    /// `https` — the default-allowed transport.
    Https,
    /// `http` — cleartext; allowed only when [`UrlPolicy::allow_http`] is set.
    Http,
    /// Any non-HTTP scheme, which this policy always refuses.
    Other,
}

impl Scheme {
    fn classify(s: &str) -> Scheme {
        // Scheme comparison is ASCII-case-insensitive per RFC 3986.
        if s.eq_ignore_ascii_case("https") {
            Scheme::Https
        } else if s.eq_ignore_ascii_case("http") {
            Scheme::Http
        } else {
            Scheme::Other
        }
    }
}

/// The security class of a connected IP address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressClass {
    /// A globally-routable public address — the only class allowed by default.
    Public,
    /// Loopback IPv4, IPv6, and IPv4-embedded IPv6 destinations.
    Loopback,
    /// RFC 1918 / unique-local private ranges (`10/8`, `172.16/12`,
    /// `192.168/16`, `fc00::/7`).
    Private,
    /// Link-local (`169.254/16`, `fe80::/10`) — includes the cloud metadata
    /// endpoint `169.254.169.254`.
    LinkLocal,
    /// Unspecified, broadcast, documentation, transition, translation, benchmarking, reserved, and other non-global special-use ranges.
    Special,
}

impl AddressClass {
    /// Whether an address of this class is safe to connect to under a policy
    /// that forbids private networks. Only [`AddressClass::Public`] is.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, AddressClass::Public)
    }

    /// Classify an [`IpAddr`] into its security class.
    #[must_use]
    pub fn of(addr: IpAddr) -> AddressClass {
        match addr {
            IpAddr::V4(v4) => Self::of_v4(v4),
            IpAddr::V6(v6) => Self::of_v6(v6),
        }
    }

    fn of_v4(v4: Ipv4Addr) -> AddressClass {
        if v4.is_loopback() {
            AddressClass::Loopback
        } else if v4.is_link_local() {
            // IPv4 link-local space includes the cloud metadata endpoint 169.254.169.254.
            AddressClass::LinkLocal
        } else if v4.is_private() {
            AddressClass::Private
        } else if v4.is_unspecified()
            || v4.is_broadcast()
            || v4.is_documentation()
            || v4.is_multicast()
            || is_v4_special_use(v4)
        {
            AddressClass::Special
        } else {
            AddressClass::Public
        }
    }

    fn of_v6(v6: Ipv6Addr) -> AddressClass {
        if v6.is_loopback() {
            AddressClass::Loopback
        } else if is_v6_link_local(v6) {
            AddressClass::LinkLocal
        } else if is_v6_unique_local(v6) {
            AddressClass::Private
        } else if v6.is_unspecified() || v6.is_multicast() {
            AddressClass::Special
        } else if let Some(v4) = v6.to_ipv4_mapped() {
            // IPv4-mapped IPv6 is non-global; preserve dangerous embedded IPv4 classes without promoting public IPv4.
            match Self::of_v4(v4) {
                AddressClass::Public => AddressClass::Special,
                class => class,
            }
        } else if let Some(v4) = v6_compatible_v4(v6) {
            // Classify deprecated `::a.b.c.d` forms by their embedded IPv4 address to prevent alternate-spelling bypasses.
            match Self::of_v4(v4) {
                AddressClass::Public => AddressClass::Special,
                class => class,
            }
        } else if is_v6_special_use(v6) {
            AddressClass::Special
        } else {
            AddressClass::Public
        }
    }
}

// Covers IPv4 special-use prefixes not represented by stable standard-library predicates.
fn is_v4_special_use(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    // RFC 1122 0.0.0.0/8 can route to the local host, although std marks only 0.0.0.0 unspecified.
    let this_network = a == 0;
    // RFC 6598 CGNAT can reach private tenant infrastructure on ISP and cloud networks.
    let cgnat = a == 100 && (64..=127).contains(&b);
    // 198.18.0.0/15 — RFC 2544 benchmark/test network.
    let benchmark = a == 198 && (b == 18 || b == 19);
    // RFC 1112 reserves 240.0.0.0/4; broadcast is classified separately.
    let reserved_future = a >= 240;
    // 192.0.0.0/24 is IETF protocol space, distinct from TEST-NET-1 at 192.0.2.0/24.
    let octets = v4.octets();
    let ietf_protocol = a == 192 && b == 0 && octets[2] == 0 && !matches!(octets[3], 9 | 10); // globally reachable PCP/TURN anycast
    // 192.88.99.0/24 is the deprecated non-global 6to4 relay anycast prefix (RFC 7526).
    let sixtofour_relay = a == 192 && b == 88 && v4.octets()[2] == 99;
    this_network || cgnat || benchmark || reserved_future || ietf_protocol || sixtofour_relay
}

// Covers IPv6 non-global prefixes not available through stable standard-library predicates.
fn is_v6_special_use(v6: Ipv6Addr) -> bool {
    let segments = v6.segments();
    // Treat 2001::/23 as non-global except for its exact IANA global assignments.
    let in_ietf_protocol = segments[0] == 0x2001 && segments[1] <= 0x01ff;
    let exact_anycast = segments[1] == 1
        && segments[2..7].iter().all(|segment| *segment == 0)
        && matches!(segments[7], 1..=3);
    let amt = segments[1] == 3;
    let as112 = segments[1] == 4 && segments[2] == 0x0112;
    let orchid_v2 = (segments[1] & 0xfff0) == 0x0020;
    let drone_dets = (segments[1] & 0xfff0) == 0x0030;
    let ietf_protocol_non_global =
        in_ietf_protocol && !(exact_anycast || amt || as112 || orchid_v2 || drone_dets);
    // RFC 9637 uses 3fff:0000::/20; mask the second segment to avoid swallowing real 3ffe::/16 allocations.
    let documentation_9637 = segments[0] == 0x3fff && (segments[1] & 0xf000) == 0;
    // 5f00::/16 contains non-global SRv6 segment identifiers (RFC 9602).
    let srv6 = segments[0] == 0x5f00;
    // 2001:db8::/32 — RFC 3849 documentation. This remains separate from
    // Teredo; do not broaden the Teredo check to all of 2001::/16.
    let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
    // Block 64:ff9b::/32, including the standard and local-use NAT64 prefixes that can embed IPv4 destinations.
    let nat64 = segments[0] == 0x0064 && segments[1] == 0xff9b;
    // 2002::/16 — RFC 3056 6to4, which likewise embeds an IPv4 destination.
    let sixtofour = segments[0] == 0x2002;
    // 100::/64 — RFC 6666 discard-only, plus RFC 9780's dummy prefix
    // 100:0:0:1::/64. Both are explicitly non-global in the current registry.
    let discard = segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0;
    let dummy = segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 1;
    // fec0::/10 — deprecated site-local (RFC 3879). Still resolvable, and still
    // names an internal network.
    let site_local = (segments[0] & 0xffc0) == 0xfec0;
    ietf_protocol_non_global
        || documentation
        || documentation_9637
        || srv6
        || nat64
        || sixtofour
        || discard
        || dummy
        || site_local
}

// Extract deprecated `::a.b.c.d` embeddings, excluding `::` and `::1` already classified separately.
fn v6_compatible_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    let leading_zeros = segments[..6].iter().all(|segment| *segment == 0);
    if !leading_zeros {
        return None;
    }
    let embedded = (u32::from(segments[6]) << 16) | u32::from(segments[7]);
    // Values below 2 are `::` and `::1`, handled as unspecified/loopback.
    if embedded < 2 {
        return None;
    }
    Some(Ipv4Addr::from(embedded))
}

// Stable Rust lacks the needed IPv6 predicates, so classify these prefixes directly.
fn is_v6_link_local(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

fn is_v6_unique_local(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

/// The policy: which schemes, whether private networks and cleartext are
/// allowed, and how many redirects may be followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UrlPolicy {
    /// Whether cleartext HTTP is permitted; false by default.
    pub allow_http: bool,
    /// Whether non-public connected addresses are permitted; false by default.
    pub allow_private_network: bool,
    /// Maximum redirect hops; `0` permits no redirects.
    pub max_redirects: u32,
}

impl UrlPolicy {
    /// Defaults to HTTPS-only, public addresses, and at most five redirects.
    pub const DEFAULT: UrlPolicy = UrlPolicy {
        allow_http: false,
        allow_private_network: false,
        max_redirects: 5,
    };

    /// Validates a URL before DNS lookup, rejecting unsupported schemes, embedded credentials, and missing hosts, and returns its parsed safe form.
    ///
    /// # Errors
    ///
    /// A [`UrlVerdict`] naming the refusal: malformed, credentials in the URL, cleartext, a refused
    /// scheme, a missing host, or a refused destination.
    pub fn check_url(&self, url: &str) -> Result<SafeUrl, UrlVerdict> {
        let parsed = ParsedUrl::parse(url).ok_or(UrlVerdict::Malformed)?;

        // Reject embedded credentials before scheme checks so credential-bearing cleartext URLs retain the credential-leak verdict.
        if parsed.has_userinfo {
            return Err(UrlVerdict::CredentialsInUrl);
        }

        match parsed.scheme {
            Scheme::Https => {}
            Scheme::Http if self.allow_http => {}
            Scheme::Http => return Err(UrlVerdict::CleartextRefused),
            Scheme::Other => {
                return Err(UrlVerdict::SchemeRefused {
                    scheme: parsed.scheme,
                });
            }
        }

        if parsed.host.is_empty() {
            return Err(UrlVerdict::MissingHost);
        }

        Ok(SafeUrl { parsed })
    }

    /// Validates the connected address after DNS resolution to prevent hostname checks from being bypassed by rebinding.
    #[must_use]
    pub fn check_address(&self, addr: IpAddr) -> AddressVerdict {
        let class = AddressClass::of(addr);
        if class.is_public() || self.allow_private_network {
            AddressVerdict::Allow { class }
        } else {
            AddressVerdict::Blocked { class }
        }
    }

    /// Validates the target of a 1-based redirect hop and reports whether credentials may be forwarded. The target is rechecked as a new URL, and credentials are denied across origins.
    ///
    /// # Errors
    ///
    /// A [`UrlVerdict`] when the chain is too long or `to` is refused.
    pub fn check_redirect(
        &self,
        from: &str,
        to: &str,
        hop: u32,
    ) -> Result<RedirectDecision, UrlVerdict> {
        if hop > self.max_redirects {
            return Err(UrlVerdict::TooManyRedirects {
                limit: self.max_redirects,
            });
        }
        // Apply the same URL policy to every redirect target before it can be resolved or fetched.
        let target = self.check_url(to)?;

        // Forward credentials only across matching effective origins; malformed source URLs fail closed.
        let same_origin = ParsedUrl::parse(from).is_some_and(|f| f.same_origin(&target.parsed));

        Ok(RedirectDecision {
            target,
            carry_credentials: same_origin,
        })
    }
}

impl Default for UrlPolicy {
    fn default() -> Self {
        UrlPolicy::DEFAULT
    }
}

/// Result of URL or redirect validation, identifying the rule that refused the input.
///
/// This enum is non-exhaustive so additional policy refusals may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UrlVerdict {
    /// The input is not a parseable absolute authority URL.
    Malformed,
    /// The scheme is neither `http` nor `https`.
    SchemeRefused {
        /// The refused scheme class.
        scheme: Scheme,
    },
    /// The scheme is `http` and cleartext is not allowed by this policy.
    CleartextRefused,
    /// The URL embedded a `user:pass@` (or `user@`) credential.
    CredentialsInUrl,
    /// The URL had no host component.
    MissingHost,
    /// A redirect chain exceeded [`UrlPolicy::max_redirects`].
    TooManyRedirects {
        /// The redirect limit that was exceeded.
        limit: u32,
    },
}

impl UrlVerdict {
    /// A short, stable, allocation-free tag for a log line or metric label.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            UrlVerdict::Malformed => "malformed",
            UrlVerdict::SchemeRefused { .. } => "scheme-refused",
            UrlVerdict::CleartextRefused => "cleartext-refused",
            UrlVerdict::CredentialsInUrl => "credentials-in-url",
            UrlVerdict::MissingHost => "missing-host",
            UrlVerdict::TooManyRedirects { .. } => "too-many-redirects",
        }
    }
}

/// The outcome of judging a connected address ([`UrlPolicy::check_address`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressVerdict {
    /// The address may be connected to and carries its classification.
    Allow {
        /// The class that was allowed (usually [`AddressClass::Public`]).
        class: AddressClass,
    },
    /// The address is blocked as a non-public destination.
    Blocked {
        /// The class that was blocked.
        class: AddressClass,
    },
}

impl AddressVerdict {
    /// Whether the connection is permitted.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, AddressVerdict::Allow { .. })
    }
}

/// The result of allowing one redirect hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectDecision {
    /// The re-validated target URL.
    pub target: SafeUrl,
    /// Whether credentials or authorization headers may be forwarded; false across origins.
    pub carry_credentials: bool,
}

/// Streaming byte ceiling and connect/read/total deadlines, in bytes and
/// milliseconds. `0` is literal ("nothing" / "already expired"); [`NO_LIMIT`]
/// is the sentinel for unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransportLimits {
    /// Maximum bytes to stream before aborting. `0` accepts nothing;
    /// [`NO_LIMIT`] is unbounded.
    pub max_bytes: u64,
    /// Connect deadline in milliseconds. `0` means "already expired".
    pub connect_timeout_ms: u64,
    /// Per-read (idle) deadline in milliseconds.
    pub read_timeout_ms: u64,
    /// Total-request deadline in milliseconds.
    pub total_timeout_ms: u64,
}

impl TransportLimits {
    /// Conservative defaults: 16 MiB, 10 s connect, 30 s read, 60 s total.
    pub const DEFAULT: TransportLimits = TransportLimits {
        max_bytes: 16 * 1024 * 1024,
        connect_timeout_ms: 10_000,
        read_timeout_ms: 30_000,
        total_timeout_ms: 60_000,
    };

    /// Whether streaming has exceeded the byte ceiling. Inclusive: `streamed ==
    /// max_bytes` is still allowed, `max_bytes + 1` aborts. A [`NO_LIMIT`]
    /// ceiling never aborts.
    #[must_use]
    pub const fn exceeds_bytes(&self, streamed: u64) -> bool {
        self.max_bytes != NO_LIMIT && streamed > self.max_bytes
    }

    /// Whether an elapsed connect time (ms) has hit the connect deadline.
    /// Inclusive at the deadline; a [`NO_LIMIT`] deadline never expires. A `0`
    /// deadline is expired at any elapsed time including `0`.
    #[must_use]
    pub const fn connect_expired(&self, elapsed_ms: u64) -> bool {
        deadline_expired(self.connect_timeout_ms, elapsed_ms)
    }

    /// Whether an elapsed read-idle time (ms) has hit the read deadline.
    #[must_use]
    pub const fn read_expired(&self, elapsed_ms: u64) -> bool {
        deadline_expired(self.read_timeout_ms, elapsed_ms)
    }

    /// Whether an elapsed total time (ms) has hit the total deadline.
    #[must_use]
    pub const fn total_expired(&self, elapsed_ms: u64) -> bool {
        deadline_expired(self.total_timeout_ms, elapsed_ms)
    }

    /// Converts finite, non-negative seconds to millisecond deadlines.
    ///
    /// The three outcomes are deliberately distinct so a caller can never
    /// confuse "refused", "clamped but finite", and "unbounded":
    ///
    /// - Rejected: a negative or non-finite (`NaN`, `+∞`, `-∞`) `connect`,
    ///   `read`, or `total` returns `None`. Such input is refused outright,
    ///   never clamped into a usable deadline.
    /// - Finite saturation: an accepted value is rounded to the nearest
    ///   millisecond and, if it would reach or exceed `2^64` ms, saturated to
    ///   [`NO_LIMIT`]` - 1`. A finite request therefore stays a real, expiring
    ///   deadline (it still expires at `u64::MAX`) and can never collapse onto
    ///   the unbounded sentinel.
    /// - Explicit unbounded: [`NO_LIMIT`] (a never-expiring deadline) is
    ///   reachable *only* by constructing [`TransportLimits`] directly with that
    ///   sentinel. This constructor never yields it for any input.
    #[must_use]
    pub fn from_secs_f64(
        max_bytes: u64,
        connect: f64,
        read: f64,
        total: f64,
    ) -> Option<TransportLimits> {
        Some(TransportLimits {
            max_bytes,
            connect_timeout_ms: secs_to_ms(connect)?,
            read_timeout_ms: secs_to_ms(read)?,
            total_timeout_ms: secs_to_ms(total)?,
        })
    }
}

impl Default for TransportLimits {
    fn default() -> Self {
        TransportLimits::DEFAULT
    }
}

/// Resolves an RFC 3986 reference against `base` and returns the absolute URL for policy validation.
///
/// Query-only, fragment-only, empty, network-path, relative, and dot-segment references follow RFC 3986 sections 5.2.2–5.2.4. Absolute schemes pass through for later policy checking. Returns `None` when the base scheme cannot form such a reference.
#[must_use]
pub fn resolve_reference(base: &SafeUrl, reference: &str) -> Option<String> {
    // Preserve absolute references verbatim so URL policy evaluates the origin-provided target.
    if has_scheme(reference) {
        return Some(reference.to_owned());
    }
    let scheme = match base.scheme() {
        Scheme::Https => "https",
        Scheme::Http => "http",
        Scheme::Other => return None,
    };
    // Check network-path references before absolute paths because `//h/p` also starts with `/`.
    if let Some(authority_and_path) = reference.strip_prefix("//") {
        return Some(format!("{scheme}://{authority_and_path}"));
    }
    let port = base
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let host = base.host();
    let base_path = base.path();
    let base_query = base.parsed.query.as_deref();

    let (ref_no_frag, ref_frag) = match reference.split_once('#') {
        Some((head, frag)) => (head, Some(frag)),
        None => (reference, None),
    };
    let (ref_path, ref_query) = match ref_no_frag.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (ref_no_frag, None),
    };

    let (target_path_owned, target_query): (String, Option<&str>) = if ref_path.is_empty() {
        // Same-document: preserve base path. A defined ref_query replaces the
        // base query; otherwise the base query is preserved (§5.2.2, R.path == "").
        let q = if ref_query.is_some() {
            ref_query
        } else {
            base_query
        };
        (base_path.to_owned(), q)
    } else if ref_path.starts_with('/') {
        (remove_dot_segments(ref_path), ref_query)
    } else {
        let merged = merge_reference_path(base_path, ref_path);
        (remove_dot_segments(&merged), ref_query)
    };

    let mut out = format!("{scheme}://{host}{port}{target_path_owned}");
    if let Some(q) = target_query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(frag) = ref_frag {
        out.push('#');
        out.push_str(frag);
    }
    Some(out)
}

// Merge a relative path with the base path according to RFC 3986 section 5.2.3.
fn merge_reference_path(base_path: &str, ref_path: &str) -> String {
    if base_path.is_empty() {
        // An authority with an empty base path contributes `/` before the relative path (RFC 3986 section 5.2.3).
        let mut s = String::with_capacity(ref_path.len() + 1);
        s.push('/');
        s.push_str(ref_path);
        s
    } else {
        let directory = base_path.rsplit_once('/').map_or("", |(dir, _)| dir);
        format!("{directory}/{ref_path}")
    }
}

// Remove dot segments according to RFC 3986 section 5.2.4 so policy and transport use one normalized path.
fn remove_dot_segments(path: &str) -> String {
    let mut input = path.to_owned();
    let mut output = String::with_capacity(path.len());
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") {
            input = rest.to_owned();
        } else if let Some(rest) = input.strip_prefix("./") {
            input = rest.to_owned();
        } else if let Some(rest) = input.strip_prefix("/./") {
            input = format!("/{rest}");
        } else if input == "/." {
            "/".clone_into(&mut input);
        } else if let Some(rest) = input.strip_prefix("/../") {
            input = format!("/{rest}");
            match output.rfind('/') {
                Some(pos) => output.truncate(pos),
                None => output.clear(),
            }
        } else if input == "/.." {
            "/".clone_into(&mut input);
            match output.rfind('/') {
                Some(pos) => output.truncate(pos),
                None => output.clear(),
            }
        } else if input == "." || input == ".." {
            input.clear();
        } else {
            let split = input
                .get(1..)
                .and_then(|rest| rest.find('/'))
                .map_or(input.len(), |idx| idx + 1);
            let (segment, rest) = input.split_at(split);
            output.push_str(segment);
            let rest = rest.to_owned();
            input = rest;
        }
    }
    output
}

/// Returns whether `reference` starts with an RFC 3986 scheme before any path,
/// query, or fragment delimiter.
#[must_use]
pub fn has_scheme(reference: &str) -> bool {
    let first_segment = reference
        .find(['/', '?', '#'])
        .map_or(reference, |end| reference.split_at(end).0);
    let Some((scheme, _)) = first_segment.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

const fn deadline_expired(deadline_ms: u64, elapsed_ms: u64) -> bool {
    deadline_ms != NO_LIMIT && elapsed_ms >= deadline_ms
}

fn secs_to_ms(secs: f64) -> Option<u64> {
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    let ms = (secs * 1000.0).round();
    // A finite, non-negative request is a FINITE deadline. `ms as u64` saturates
    // to `u64::MAX` for values at or above 2^64, which is exactly `NO_LIMIT` —
    // the sentinel meaning "never expires". Clamping one below it keeps an
    // astronomically large but finite ceiling a real, expiring deadline instead
    // of silently disabling the time ceiling this type enforces.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the saturating float-to-int `as` is the point, as above"
    )]
    Some((ms as u64).min(NO_LIMIT - 1))
}

/// A URL accepted by [`UrlPolicy::check_url`] whose `Display` and `Debug` forms redact query contents.
#[derive(Clone, PartialEq, Eq)]
pub struct SafeUrl {
    parsed: ParsedUrl,
}

impl SafeUrl {
    /// The scheme.
    #[must_use]
    pub fn scheme(&self) -> Scheme {
        self.parsed.scheme
    }

    /// The host (registered name or IP literal), lowercased. Never empty (a
    /// missing host is refused at parse).
    #[must_use]
    pub fn host(&self) -> &str {
        &self.parsed.host
    }

    /// The explicit port, if the URL specified one.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.parsed.port
    }

    /// The path (may be empty).
    #[must_use]
    pub fn path(&self) -> &str {
        &self.parsed.path
    }

    /// Whether the URL carried a query string (whose contents are redacted in
    /// `Display`/`Debug`).
    #[must_use]
    pub fn has_query(&self) -> bool {
        self.parsed.query.is_some()
    }

    /// Returns the redacted display form: scheme, host, port, and path, with any query replaced by `?<redacted>`.
    #[must_use]
    pub fn redacted(&self) -> String {
        let scheme = match self.parsed.scheme {
            Scheme::Https => "https",
            Scheme::Http => "http",
            Scheme::Other => "other",
        };
        let mut s = format!("{scheme}://{}", self.parsed.host);
        if let Some(port) = self.parsed.port {
            s.push(':');
            s.push_str(&port.to_string());
        }
        s.push_str(&self.parsed.path);
        if self.parsed.query.is_some() {
            s.push_str("?<redacted>");
        }
        s
    }
}

impl fmt::Display for SafeUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

impl fmt::Debug for SafeUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug uses the same redaction boundary as Display so derived diagnostics cannot expose query data.
        write!(f, "SafeUrl({})", self.redacted())
    }
}

/// A policy-checked remote URL retaining the raw transport string while redacting formatted output.
///
/// Construction is available only through [`RemoteSource::checked`]. [`RemoteSource::transport_url`] exposes the raw query-bearing URL explicitly; `Display` and `Debug` use [`SafeUrl`] redaction.
#[derive(Clone)]
pub struct RemoteSource {
    raw: Arc<str>,
    safe: SafeUrl,
}

impl RemoteSource {
    /// Validates the raw URL with [`UrlPolicy::check_url`] and constructs a remote source.
    ///
    /// This performs no DNS or connected-address validation; callers that fetch must also apply [`UrlPolicy::check_address`] and transport limits.
    ///
    /// # Errors
    /// Returns the [`UrlVerdict`] that refused the URL.
    pub fn checked(raw: &str, policy: &UrlPolicy) -> Result<Self, UrlVerdict> {
        let safe = policy.check_url(raw)?;
        Ok(RemoteSource {
            raw: Arc::from(raw),
            safe,
        })
    }

    /// The already-validated [`SafeUrl`] for scheme/host/path inspection.
    #[must_use]
    pub fn safe_url(&self) -> &SafeUrl {
        &self.safe
    }

    /// Redacted form safe for logs: scheme, host, optional port, path, and `?<redacted>` for a query.
    #[must_use]
    pub fn redacted(&self) -> String {
        self.safe.redacted()
    }

    /// Returns the complete URL for transport, including any sensitive query string. Do not log this value; use [`RemoteSource::redacted`] for display.
    #[must_use]
    pub fn transport_url(&self) -> &str {
        &self.raw
    }
}

impl PartialEq for RemoteSource {
    fn eq(&self, other: &Self) -> bool {
        // Equality compares validated parsed components, so scheme/host case and discarded fragments do not affect identity.
        self.safe == other.safe
    }
}

impl Eq for RemoteSource {}

impl fmt::Display for RemoteSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.safe.redacted())
    }
}

impl fmt::Debug for RemoteSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RemoteSource({})", self.safe.redacted())
    }
}

// Minimal parsed URL components needed for policy decisions; this is not a general RFC 3986 parser.
#[derive(Clone, PartialEq, Eq)]
struct ParsedUrl {
    scheme: Scheme,
    has_userinfo: bool,
    host: String,
    port: Option<u16>,
    path: String,
    query: Option<String>,
}

impl ParsedUrl {
    fn parse(input: &str) -> Option<ParsedUrl> {
        let (scheme_str, rest) = input.split_once("://")?;
        if scheme_str.is_empty() {
            return None;
        }
        let scheme = Scheme::classify(scheme_str);

        // Split fragments first so fragment text cannot hide query or userinfo delimiters.
        let rest = rest.split_once('#').map_or(rest, |(a, _)| a);

        let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, after_auth) = rest.split_at(auth_end);

        // Split userinfo at the last `@` so any earlier `@` still counts as credentials.
        let (has_userinfo, hostport) = match authority.rsplit_once('@') {
            Some((_userinfo, hp)) => (true, hp),
            None => (false, authority),
        };

        let (host, port) = Self::split_host_port(hostport)?;

        let (path, query) = match after_auth.split_once('?') {
            Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
            None => (after_auth.to_owned(), None),
        };

        Some(ParsedUrl {
            scheme,
            has_userinfo,
            host: host.to_ascii_lowercase(),
            port,
            path,
            query,
        })
    }

    fn split_host_port(hostport: &str) -> Option<(String, Option<u16>)> {
        if let Some(rest) = hostport.strip_prefix('[') {
            let (inside, after) = rest.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) if !p.is_empty() => Some(p.parse::<u16>().ok()?),
                Some(_) => return None,
                None if after.is_empty() => None,
                None => return None,
            };
            Some((inside.to_owned(), port))
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) if !p.is_empty() => Some((h.to_owned(), Some(p.parse::<u16>().ok()?))),
                Some((_, _)) => None,
                None => Some((hostport.to_owned(), None)),
            }
        }
    }

    fn effective_port(&self) -> Option<u16> {
        self.port.or(match self.scheme {
            Scheme::Https => Some(443),
            Scheme::Http => Some(80),
            Scheme::Other => None,
        })
    }

    fn same_origin(&self, other: &ParsedUrl) -> bool {
        self.scheme == other.scheme
            && self.host == other.host
            && self.effective_port() == other.effective_port()
    }
}

#[cfg(test)]
mod tests;
