//! Encoded HTTP response cache with freshness and revalidation.
//!
//! # What this caches, and how it relates to the decoded LRU
//!
//! This is the encoded cache: response bytes exactly as fetched, plus the
//! metadata needed to reuse or revalidate them. It is distinct from
//! [`ImageService`](crate::ImageService)'s decoded RGBA LRU, and the two form
//! two tiers consulted in this order:
//!
//! 1. Decoded LRU (memory). A hit here returns a stable `Handle` and skips
//!    both network *and* decode. Checked first because it is strictly cheaper.
//! 2. Encoded cache (this module, disk). A hit here skips the network but
//!    still decodes. It survives process restart, which the decoded LRU cannot:
//!    RGBA is far larger than the encoded form and holding decoded pixels on
//!    disk would trade a lot of disk for a little CPU.
//!
//! A fresh encoded hit therefore populates the decoded LRU on its way through,
//! so a second request for the same URL is served from tier 1.
//!
//! # Freshness
//!
//! `Cache-Control: max-age` and `no-cache`/`no-store` are honoured, plus `ETag`
//! with `If-None-Match` revalidation and `304 Not Modified`. A stale entry with
//! a validator is revalidated rather than discarded, which is the whole point of
//! `304`: the body is not re-sent and the cached bytes are served.
//!
//! `no-store` genuinely does not persist. It is checked before any file is
//! created, not written and then removed — a cache that writes then deletes has
//! already leaked the bytes to disk, where a backup or another process may
//! observe them.
//!
//! # Bounding
//!
//! An unbounded disk cache is the same defect class this crate just fixed for
//! the job queue, so the store is bounded by total bytes and evicts
//! least-recently-used entries. **`0` means no cache at all**, literally,
//! matching `max_inflight_jobs` and `chat_stream`'s `byte_cap`.
//!
//! # Failure and poisoning
//!
//! Every cache operation is best-effort: an unwritable or missing cache
//! directory (a read-only home, a sandbox, a full disk) **disables caching and
//! never fails a load**. A corrupt, truncated, or hostile entry is treated as a
//! miss and removed, never trusted — the entry carries a length and a
//! checksum over the body, and a body that disagrees is discarded before it ever
//! reaches the decoder.
//!
//! # Secrets never reach disk
//!
//! The full request identity — the method, exact URL (query token and all),
//! range, opaque caller/session partition, and every repeated `Vary` request-
//! field occurrence — is folded into a single domain-separated **SHA-256
//! digest** and never persisted verbatim. Only the `Vary` field *names* and the
//! digest reach disk, so a `Vary: Authorization` bearer token or a `Vary:
//! Cookie` session value is no more persisted than a signed URL's query token.
//! This prevents direct disclosure and practical chosen collisions;
//! it does not make a low-entropy token safe from offline guessing. Two requests
//! that differ only in their secret query still resolve to two
//! distinct entries (correct key separation), yet the token itself appears in
//! no cache file, in no `Debug` output, and in no diagnostic. What *is*
//! persisted for a human reading a cache directory is a redacted display URL
//! (`scheme://host/path?<redacted>`), supplied by the caller from its already
//! screened [`SafeUrl`](enzui_core::url_policy::SafeUrl). See [`CacheKey`].
//!
//! # Bounded reads
//!
//! An entry file is read through a strict ceiling ([`HttpCache::read_limit`]):
//! the declared length is checked *before* any allocation and the read itself is
//! capped, so a replaced file that is oversized, sparse (a huge logical length
//! backed by holes), still growing, or lying about its length can never force an
//! unbounded allocation. It is refused as a miss instead.
//!
//! # Migration
//!
//! The on-disk format is versioned. An older entry — including any pre-fix entry
//! that embedded a raw URL, or a v4 entry that could carry a plaintext `Vary`
//! request value — carries a different version and so fails to decode:
//! it is treated as a miss and removed, **without its bytes ever being parsed,
//! rendered, or logged**. Migration is therefore fail-safe: an old secret on
//! disk is discarded, never surfaced.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use enzui_security::secure_dir::{SecureDirRefusal, SecureRoot, open_file_identity};
use enzui_security::tempstage::RootTempStage;

use crate::cache_identity::digest_bytes_with_domain;

mod cache_ops;

/// On-disk entry format version. A mismatch is treated as a miss, so an old
/// cache is ignored rather than misparsed after a format change.
///
/// Bumped to `3` when the raw URL stopped being persisted, to `4` when request
/// identities moved from seeded FNV lanes to SHA-256, then to `5` when the
/// complete request partition (method, URL, range, caller partition, and every
/// repeated `Vary` request-field occurrence) became the hashed identity and the
/// raw `Vary` *values* stopped being persisted — only the field names and the
/// digest remain on disk. Older entries no longer decode and are discarded
/// unread — the fail-safe migration documented above. A v4 entry could carry a
/// plaintext `Vary: Authorization`/`Cookie` value, so v4 now fails closed.
const FORMAT_VERSION: u32 = 5;

/// Length in bytes of the persisted request-identity digest.
const IDENTITY_DIGEST_LEN: usize = 32;

/// Headroom, above the per-entry body ceiling, allowed for an entry file's
/// fixed header plus its redacted display URL and validators.
///
/// A generous bound: the display URL and validators are small, so this still
/// refuses a metadata section engineered to force a large allocation while never
/// rejecting a legitimately encoded entry.
const MAX_ENTRY_METADATA_BYTES: u64 = 64 * 1024;

/// Semantic ceiling on how many `Vary` field names a legitimate entry may carry.
///
/// A real HTTP response varies on a handful of field names at most
/// (`Accept-Encoding`, `Accept`, `User-Agent`); dozens is already unusual and
/// hundreds is a corrupt or attacker-controlled count. This bound is checked
/// before any allocation so a hostile `vary_len` cannot request tens of MiB of
/// `String` capacity, even when the remaining-bytes bound would also eventually
/// reject the file.
const MAX_VARY_NAMES: usize = 64;

/// Minimum bytes an encoded `Vary` field name can occupy: one `u32` length
/// prefix with a zero-length name. A declared name count is bounded by the
/// bytes remaining divided by this, before the name vector is allocated, so a
/// small file cannot reserve millions of entries. Only field names are
/// persisted; request-header values (which can carry a bearer token or a
/// session cookie) never reach disk.
const MIN_ENCODED_VARY_NAME_BYTES: usize = 4;

/// Magic prefix identifying an entry file.
const MAGIC: &[u8; 8] = b"ENZUIHC1";

/// Count of physical disk writes performed by the cache, for tests.
///
/// Monotonic and process-global; tests read a delta.
#[cfg(test)]
pub(crate) static DISK_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Exclusion between a [`DISK_WRITES`] before/after window and every physical
/// write performed by another thread.
///
/// The counter is process-global while `cargo test` runs test functions
/// concurrently by default, so without this a sibling test's `store` lands
/// inside another test's window and a "zero physical writes" assertion goes red
/// for a reason that has nothing to do with the code under test (observed on
/// this crate: `revalidation_refuses_65_vary_names_before_rewriting` measured
/// 109 against a before-count of 108).
///
/// A writer holds this lock only for the increment itself, so tests that never
/// inspect the counter are not serialized with one another; they wait only
/// while a window is actually open, which is the shortest interval that can
/// make a delta mean anything.
#[cfg(test)]
pub(crate) static DISK_WRITE_WINDOW: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
thread_local! {
    /// Set while this thread holds [`DISK_WRITE_WINDOW`].
    ///
    /// The window owner's own writes must increment without re-locking a
    /// non-reentrant mutex, which would deadlock the test that opened the
    /// window.
    pub(crate) static IN_DISK_WRITE_WINDOW: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Cache directory name under the XDG cache root.
const CACHE_SUBDIR: &str = "enzui/http-images";

/// Policy for the encoded HTTP cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpCacheConfig {
    /// Maximum total encoded bytes retained on disk.
    ///
    /// **`0` means no cache**, literally: nothing is read and nothing is
    /// written. It is expressible intent, not a synonym for unlimited, on the
    /// same reading as `ServiceConfig::max_inflight_jobs`.
    pub max_bytes: u64,
    /// Maximum bytes for a single entry. A response larger than this is not
    /// cached, so one huge image cannot evict the entire cache.
    pub max_entry_bytes: u64,
}

impl HttpCacheConfig {
    /// A 64 MiB cache with a 8 MiB per-entry ceiling.
    pub const DEFAULT: Self = Self {
        max_bytes: 64 * 1024 * 1024,
        max_entry_bytes: 8 * 1024 * 1024,
    };

    /// Caching fully disabled.
    pub const DISABLED: Self = Self {
        max_bytes: 0,
        max_entry_bytes: 0,
    };
}

impl Default for HttpCacheConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How a request identifies a cache entry.
///
/// It carries two strings with very different handling:
///
/// * `identity` — the full request string, query token and all. It is used
///   *only* to derive a non-reversible digest for key separation and filename
///   selection. It is never persisted, never logged, and never rendered.
/// * `display` — a redacted form safe to write to disk and print in
///   diagnostics. Callers pass their [`SafeUrl`](enzui_core::url_policy::SafeUrl)'s
///   redacted string here.
///
/// `&str` converts into a key whose identity and display are the same string via
/// [`From`], for the common case (and tests) where the URL carries no secret.
/// Use [`CacheKey::new`] whenever the identity may contain credentials.
///
/// `Debug` is manual and deliberately renders only the redacted `display`:
/// the secret-bearing `identity` never reaches a `{:?}` rendering, so a stray
/// `dbg!(key)` or a derived `Debug` on an enclosing struct cannot leak a token.
///
/// Beyond the URL, a key also carries the rest of the request partition that the
/// cache must not collapse: the method (default `GET`), an optional
/// range, and an optional opaque caller/session partition. These fold
/// into the same domain-separated identity digest as the URL and `Vary` values,
/// so two requests that differ only in method, range, or partition resolve to
/// two distinct entries instead of one serving the other's body. The default
/// constructors set method `GET` and no range/partition, which is exactly what
/// [`ImageService`](crate::ImageService) needs, so its behaviour is unchanged.
#[derive(Clone, Copy)]
pub struct CacheKey<'a> {
    identity: &'a str,
    display: &'a str,
    method: &'a str,
    range: Option<&'a str>,
    partition: Option<&'a str>,
}

impl<'a> CacheKey<'a> {
    /// A key whose secret-bearing `identity` is kept off disk while `display`
    /// (already redacted by the caller) is what gets persisted and logged.
    ///
    /// Method defaults to `GET` with no range and no caller partition; use
    /// [`with_method`](Self::with_method), [`with_range`](Self::with_range) and
    /// [`with_partition`](Self::with_partition) to distinguish requests that
    /// share a URL but must not share a cache entry.
    #[must_use]
    pub const fn new(identity: &'a str, display: &'a str) -> Self {
        Self {
            identity,
            display,
            method: "GET",
            range: None,
            partition: None,
        }
    }

    /// Partition this key by request method. Two otherwise identical requests
    /// with different methods never share an entry.
    #[must_use]
    pub fn with_method(self, method: &'a str) -> Self {
        Self { method, ..self }
    }

    /// Partition this key by a request `Range`. An absent range is encoded
    /// distinctly from any present one, so a ranged request never reads a full
    /// response's bytes or vice versa.
    #[must_use]
    pub fn with_range(self, range: &'a str) -> Self {
        Self {
            range: Some(range),
            ..self
        }
    }

    /// Partition this key by an opaque caller/session identifier. The value is
    /// hashed into the identity and never persisted verbatim, so two sessions
    /// that share a URL cannot read each other's cached bodies.
    #[must_use]
    pub fn with_partition(self, partition: &'a str) -> Self {
        Self {
            partition: Some(partition),
            ..self
        }
    }
}

impl fmt::Debug for CacheKey<'_> {
    /// Renders only the redacted `display`. The `identity` — which may carry a
    /// signing token — is intentionally withheld, mirroring the redaction that
    /// [`SafeUrl`](enzui_core::url_policy::SafeUrl)'s own `Debug` performs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheKey")
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

impl<'a> From<&'a str> for CacheKey<'a> {
    /// Treats one string as both identity and display, so the identity is *also*
    /// what gets persisted and logged. **Only safe for a URL with no
    /// credentials** (no signing token, session key, or other secret in the
    /// query): the string is written to disk verbatim as the display URL.
    /// Whenever the identity may contain a secret, use [`CacheKey::new`] with a
    /// separately redacted `display`.
    fn from(both: &'a str) -> Self {
        Self::new(both, both)
    }
}

/// Directives parsed from a response's cache headers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Freshness {
    /// `Cache-Control: max-age=N`, when exactly one occurrence is present and
    /// parseable. Duplicate occurrences are treated as ambiguous.
    pub max_age: Option<u64>,
    /// `Cache-Control: no-store` — must not be persisted at all.
    pub no_store: bool,
    /// Whether the response may be stored but must be revalidated before reuse.
    ///
    /// Set by `Cache-Control: no-cache` and by ambiguous duplicate `max-age`
    /// occurrences, even when the duplicate values agree.
    pub no_cache: bool,
    /// `ETag` validator, verbatim (including quotes and any `W/` prefix).
    pub etag: Option<String>,
    /// `Last-Modified` validator, verbatim, for `If-Modified-Since`.
    pub last_modified: Option<String>,
    /// `Age` in seconds: how long this response already sat in upstream caches.
    ///
    /// Without this, a response a proxy has held for an hour is treated as if it
    /// had just been generated — the cache serves content that is already stale
    /// and believes it is fresh.
    pub age: Option<u64>,
    /// `Date` as a Unix timestamp: when the origin generated the response.
    ///
    /// Anchors freshness to the origin's clock rather than to the local receipt time,
    /// which matters for the same proxy-delay reason as [`Freshness::age`].
    pub date: Option<u64>,
    /// `Expires` as a Unix timestamp: an absolute expiry.
    ///
    /// A fallback used only when `max-age` is absent — RFC 9111 §5.3 gives
    /// `max-age` precedence — and the only freshness signal some origins send.
    pub expires: Option<u64>,
    /// Header names this response varies on, lowercased.
    ///
    /// `Vary: *` and any malformed non-token field-name element are represented
    /// as [`VARY_ANY`], which is never cacheable. Treating malformed names as
    /// uncacheable prevents an omitted `Cookie` or `Authorization` partition.
    pub vary: Vec<String>,
}

/// Marker for a `Vary` value that cannot be safely cached.
///
/// This represents both the literal `Vary: *` and a malformed field-name list;
/// neither lets the cache prove that a later request has the same partition.
pub const VARY_ANY: &str = "*";

/// Whether `value` is an HTTP `token` (RFC 9110 §5.6.2).
///
/// `Vary` elements are field names, whose grammar is exactly `token`. This is
/// deliberately ASCII-only: whitespace, controls, non-ASCII bytes, and every
/// HTTP separator are rejected instead of being silently folded into a name the
/// request lookup will never match.
fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

impl Freshness {
    /// Parse the subset of cache headers this cache acts on.
    ///
    /// Header names are matched case-insensitively because HTTP field names are
    /// case-insensitive; `curl_transport` already lowercases them, but a
    /// different transport need not. More than one `max-age` occurrence across
    /// all `Cache-Control` fields is ambiguous and forces revalidation.
    #[must_use]
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        let mut out = Self::default();
        let mut max_age_seen = false;
        let mut duplicate_max_age = false;
        let mut parsed_max_age = None;
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("etag") {
                out.etag = Some(value.trim().to_owned());
                continue;
            }
            if name.eq_ignore_ascii_case("last-modified") {
                // Kept verbatim: `If-Modified-Since` must echo the origin's own
                // formatting back, not a re-rendered equivalent.
                out.last_modified = Some(value.trim().to_owned());
                continue;
            }
            if name.eq_ignore_ascii_case("age") {
                // A malformed Age is IGNORED, not read as 0. Reading it as 0
                // would claim the response is brand new, which is the least safe
                // possible guess.
                if let Ok(seconds) = value.trim().parse::<u64>() {
                    out.age = Some(seconds);
                }
                continue;
            }
            if name.eq_ignore_ascii_case("date") {
                out.date = parse_http_date(value);
                continue;
            }
            if name.eq_ignore_ascii_case("expires") {
                // An unparseable Expires is ignored rather than treated as
                // "already expired". RFC 9111 permits treating it as expired,
                // but this cache's settled policy is that a malformed value is
                // ignored, and an origin with a broken Expires still gets
                // revalidation (no heuristic lifetimes) rather than a guess.
                out.expires = parse_http_date(value);
                continue;
            }
            if name.eq_ignore_ascii_case("vary") {
                // Once any Vary field made this response uncacheable, a later
                // header must not clear the poison by contributing valid names.
                if out.varies_on_everything() {
                    continue;
                }
                for field in value.split(',') {
                    // Only HTTP optional whitespace (SP / HTAB) may surround a
                    // list element. `str::trim` would also erase controls and
                    // Unicode whitespace, incorrectly turning malformed input
                    // into a valid field name.
                    let field = field.trim_matches([' ', '\t']);
                    if field == VARY_ANY || !is_http_token(field) {
                        // A malformed element is as unsafe as `*`: silently
                        // skipping `Cookie Cookie2`, for example, could collapse
                        // authenticated responses into the unvaried partition.
                        out.vary = vec![VARY_ANY.to_owned()];
                        break;
                    }
                    let field = field.to_ascii_lowercase();
                    if !out.vary.iter().any(|seen| seen == &field) {
                        out.vary.push(field);
                    }
                }
                continue;
            }
            if !name.eq_ignore_ascii_case("cache-control") {
                continue;
            }
            for directive in value.split(',') {
                let directive = directive.trim();
                if directive.eq_ignore_ascii_case("no-store") {
                    out.no_store = true;
                } else if directive.eq_ignore_ascii_case("no-cache") {
                    out.no_cache = true;
                } else {
                    let (key, value) = directive
                        .split_once('=')
                        .map_or((directive, None), |(key, value)| (key, Some(value)));
                    if !key.trim().eq_ignore_ascii_case("max-age") {
                        continue;
                    }

                    if max_age_seen {
                        // Duplicate freshness directives are ambiguous even when
                        // they agree. Remember that ambiguity without allowing a
                        // later valid value to clear it.
                        duplicate_max_age = true;
                    } else {
                        max_age_seen = true;
                        // A single malformed value is ignored rather than treated
                        // as 0, preserving the cache's established behavior.
                        parsed_max_age = value.and_then(|value| value.trim().parse::<u64>().ok());
                    }
                }
            }
        }
        if duplicate_max_age {
            // Field order must never decide how long remote bytes may be reused.
            // Keep the entry and its validators, but force revalidation; an
            // existing no-cache decision can only remain true.
            out.no_cache = true;
        } else {
            out.max_age = parsed_max_age;
        }
        out
    }

    /// Whether a response carrying these directives may be written to disk.
    ///
    /// `no-cache` still stores (it only forces revalidation before reuse);
    /// `no-store` does not.
    #[must_use]
    pub fn may_store(&self) -> bool {
        !self.no_store && !self.varies_on_everything()
    }

    /// Whether this response carries `Vary: *`.
    ///
    /// Such a response varies on aspects of the request the cache cannot
    /// enumerate, so no stored entry can ever be proven to match a later
    /// request. It is therefore never stored and never served.
    #[must_use]
    pub fn varies_on_everything(&self) -> bool {
        self.vary.iter().any(|field| field == VARY_ANY)
    }

    /// Freshness lifetime in seconds, or `None` when the origin gave none.
    ///
    /// `max-age` wins over `Expires` when both are present (RFC 9111 §5.3).
    /// `Expires` is converted to a lifetime relative to `Date` when the origin
    /// supplied one, and relative to `received_at` otherwise — using the local receipt
    /// time as the anchor is the only option left, and it is conservative because
    /// receipt is never earlier than generation.
    #[must_use]
    pub fn lifetime_seconds(&self, received_at: u64) -> Option<u64> {
        if let Some(max_age) = self.max_age {
            return Some(max_age);
        }
        let expires = self.expires?;
        let anchor = self.date.unwrap_or(received_at);
        // An Expires at or before the anchor means "already expired": a lifetime
        // of zero, which is expressible intent and distinct from "no signal".
        Some(expires.saturating_sub(anchor))
    }
}

/// Parse an HTTP-date into a Unix timestamp.
///
/// Handles the IMF-fixdate form RFC 9110 §5.6.7 requires senders to produce
/// (`Sun, 06 Nov 1994 08:49:37 GMT`), which is what every current origin sends.
/// The two obsolete forms (RFC 850 and asctime) are not accepted: returning
/// `None` for them means "no freshness signal", which under this cache's
/// no-heuristic-lifetimes policy degrades to revalidation — correct, just not
/// optimal. Guessing at an ambiguous two-digit year would be worse.
///
/// Returns `None` for anything malformed, so a broken date is ignored rather than
/// being read as the epoch (which would mean "expired in 1970").
fn parse_http_date(value: &str) -> Option<u64> {
    let value = value.trim();
    // `Sun, 06 Nov 1994 08:49:37 GMT`
    let rest = value.split_once(", ")?.1;
    let mut parts = rest.split(' ');
    let day: u32 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    // A missing or non-GMT zone is malformed: every other zone would need a table.
    if parts.next()? != "GMT" {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    let mut clock = time.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // The year is bounded on both sides, and the upper bound is load-bearing.
    //
    // IMF-fixdate spells the year as exactly `4DIGIT` (RFC 9110 §5.6.7), so
    // `10000` and up is malformed input, not a date — and a response header is
    // remote input. Without this bound a header naming a year near `i64::MAX`
    // parses fine and then overflows the civil-date arithmetic below: a panic
    // where `overflow-checks` are on (dev/test) and a silently wrapped instant
    // in release, which `lifetime_seconds` would turn into an arbitrary
    // freshness lifetime and serve stale bytes on. Rejecting the value keeps
    // the module's stated policy (a malformed date is ignored, so the origin
    // gets revalidation) and makes every multiplication below unreachable for
    // an out-of-domain year rather than merely checked.
    if day == 0 || day > 31 || !(1970..=9999).contains(&year) {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    // Bounded by construction: `year <= 9999` caps `days` at ~2.93e6, so the
    // widest product here is ~2.53e11 — five orders of magnitude inside `u64`.
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days since the Unix epoch for a civil date, or `None` if the date does not
/// exist (e.g. 31 February).
///
/// Uses the standard days-from-civil algorithm rather than a date crate: the
/// whole requirement is one direction of one calendar conversion, and the leap
/// rule is three comparisons.
///
/// `year` is expected inside the IMF-fixdate domain (`1970..=9999`), which its
/// sole caller [`parse_http_date`] enforces; the arithmetic below cannot
/// overflow for any year in that range.
fn days_from_civil(year: i64, month: u32, day: u32) -> Option<u64> {
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let mut limit = DAYS_IN_MONTH[(month - 1) as usize];
    if month == 2 && leap {
        limit = 29;
    }
    if day > limit {
        return None;
    }
    // Shift so the era starts in March, making the leap day the last day.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + i64::from(doy);
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days).ok()
}

/// Outcome of a [`HttpCache::clear`] operation.
///
/// A privacy-facing "Clear cached data" action must know whether it succeeded:
/// returning `()` and discarding per-entry errors would let the UI report
/// success while response bodies and display URLs remain on disk.
///
/// Callers choose their own policy: log, retry, surface to the user, or ignore.
/// Afterward, [`HttpCache::try_stored_bytes`] returns the exact bytes still
/// present or an error if a complete retained-fd census is not possible.
/// One entry that [`HttpCache::clear`] could not remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearFailure {
    /// The path that could not be removed.
    pub path: PathBuf,
    /// The OS error kind.
    pub kind: io::ErrorKind,
    /// The OS error message.
    pub message: String,
}

/// Outcome of a [`HttpCache::clear`] operation: how many entries were removed
/// and which (if any) could not be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearOutcome {
    /// Number of entries successfully removed.
    pub removed: usize,
    /// Per-entry removal failures. Empty when every entry was deleted.
    pub failures: Vec<ClearFailure>,
}

impl ClearOutcome {
    /// Whether every entry was successfully removed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }

    /// Number of entries that could not be removed.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.failures.len()
    }
}

/// A cached response and what may be done with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedResponse {
    /// Encoded body as originally fetched.
    pub body: Vec<u8>,
    /// `ETag` for revalidation, if the origin supplied one.
    pub etag: Option<String>,
    /// `Last-Modified` for revalidation, if the origin supplied one.
    ///
    /// A weaker validator than `ETag` (one-second resolution, and it cannot
    /// distinguish two edits within the same second), so `ETag` takes precedence
    /// when both are present.
    pub last_modified: Option<String>,
    /// Whether the entry is still within its freshness lifetime.
    pub fresh: bool,
}

impl CachedResponse {
    /// Whether this entry can be served without contacting the origin.
    #[must_use]
    pub const fn usable_without_network(&self) -> bool {
        self.fresh
    }
}

/// Why the cache is not storing anything, when it is not.
///
/// The cache is best-effort by design — every operation degrades to a miss — but
/// "disabled" has three very different causes and an application that cannot
/// tell them apart cannot report the interesting one. A
/// [`CacheUnavailable::Root`] carrying a [`SecureDirRefusal`] other than
/// [`SecureDirRefusal::CreateFailed`] means a directory was already at the cache
/// path and failed a security check (wrong owner, group/world-writable, a
/// planted symlink): worth logging, unlike a read-only home. Observe it with
/// [`HttpCache::unavailable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheUnavailable {
    /// Switched off by configuration ([`HttpCacheConfig::max_bytes`] is `0`).
    Disabled,
    /// Neither `$XDG_CACHE_HOME` nor `$HOME` yielded an absolute cache base, so
    /// there is no path to provision. Only [`HttpCache::open`] can report this.
    NoCacheHome,
    /// The root was refused by the shared secure-ensure boundary
    /// ([`enzui_security::secure_dir::ensure_private_dir`]).
    Root(SecureDirRefusal),
}

impl fmt::Display for CacheUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheUnavailable::Disabled => write!(f, "caching is disabled by configuration"),
            CacheUnavailable::NoCacheHome => {
                write!(f, "no XDG cache home could be resolved")
            }
            CacheUnavailable::Root(refusal) => {
                write!(f, "the cache root was refused: {refusal}")
            }
        }
    }
}

/// Bounded, best-effort encoded response cache on disk.
///
/// # Root identity is bound to a retained descriptor
///
/// The cache root is not a `PathBuf` on this side of the constructor: it is a
/// [`SecureRoot`] capability. Every read, write, unlink, rename, LRU touch and
/// enumeration below goes through a `*at` syscall relative to the retained
/// `O_DIRECTORY | O_NOFOLLOW` fd, so a same-uid process (or a writable
/// ancestor) that renames the verified leaf after adoption and substitutes a
/// symlink or a decoy directory at the same pathname cannot redirect any of
/// them. Per-entry `O_NOFOLLOW` does not close the
/// *intermediate-root-component* window: pathname resolution
/// follows the substituted root before the entry-level no-follow flag is
/// consulted, so a stored `PathBuf` was not enough.
///
/// Each public op ([`get`](Self::get), [`store`](Self::store),
/// [`remove`](Self::remove), [`clear`](Self::clear),
/// [`mark_revalidated`](Self::mark_revalidated)) begins with a cheap
/// [`SecureRoot::revalidate`] against the identity captured at open time and
/// fails closed on a mismatch. The pathname check is TOCTOU by construction —
/// an attacker could substitute back after the `lstat` — which is precisely why
/// the op itself goes through the fd: the safety property is that no pathname
/// is followed for the actual read or write, and the pathname check is the
/// *typed signal* an application logs via [`root_status`](Self::root_status).
#[derive(Debug)]
pub struct HttpCache {
    root: Option<SecureRoot>,
    config: HttpCacheConfig,
    /// `Some` exactly when `root` is `None`: the typed reason there is no store.
    unavailable: Option<CacheUnavailable>,
}

struct CensusEntry {
    name: OsString,
    size: u64,
    /// Signed nanoseconds relative to the Unix epoch. This remains signed from
    /// the secure stat boundary so pre-epoch entries sort before newer ones.
    used_at: i128,
}

/// Convert the security boundary's public retained-fd rows into the cache's
/// private census shape. The test-only branch injects one complete-census
/// failure on the current thread; product builds call `SecureRoot::entry_names`
/// directly with no switch or process-global state.
fn retained_census(root: &SecureRoot) -> io::Result<Vec<CensusEntry>> {
    #[cfg(test)]
    if let Some(error) = cache_census_fault::take() {
        return Err(error);
    }
    root.entry_names().map(|entries| {
        entries
            .into_iter()
            .map(|entry| CensusEntry {
                name: entry.name,
                size: entry.size,
                used_at: entry.mtime_nanos,
            })
            .collect()
    })
}

#[cfg(test)]
mod cache_census_fault {
    use std::cell::RefCell;
    use std::io;

    thread_local! {
        static NEXT: RefCell<Option<(io::ErrorKind, String)>> = const { RefCell::new(None) };
    }

    pub(super) fn take() -> Option<io::Error> {
        NEXT.with(|slot| {
            slot.borrow_mut()
                .take()
                .map(|(kind, message)| io::Error::new(kind, message))
        })
    }
}

struct EntryStat {
    path: PathBuf,
    size: u64,
    /// Signed nanoseconds relative to the Unix epoch.
    used_at: i128,
}

/// Put cache entries in their canonical eviction order.
///
/// Modification time is the LRU signal. Path is deliberately only a tie-break:
/// it removes filesystem enumeration order from the decision without changing
/// the intended recency policy.
fn canonicalize_eviction_order(entries: &mut [EntryStat]) {
    entries.sort_by(|left, right| {
        left.used_at
            .cmp(&right.used_at)
            .then_with(|| left.path.cmp(&right.path))
    });
}

/// Bytes the live budget stops charging for `name` after one eviction attempt,
/// using `census_size` where the removed object could not be measured directly.
///
/// `Ok(0)` reports a spared name: it no longer denotes the inode this pass
/// measured, so a concurrent writer's newer entry keeps both its bytes and its
/// budget charge and the pass can still end honestly above budget.
fn evict_one_entry(root: &SecureRoot, name: &OsStr, census_size: u64) -> io::Result<u64> {
    let victim = match root.open_entry_read(name) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(census_size),
        // Publication renames a regular file into place, so a name that will not
        // open is not a live entry and cannot be a newer replacement. Removing
        // it by name is what keeps a planted symlink or device from blocking the
        // budget permanently.
        Err(_) => {
            return match root.unlink_entry(name) {
                Ok(()) => Ok(census_size),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(census_size),
                Err(error) => Err(error),
            };
        }
    };
    // Size, age and unlink all bind to this one descriptor, which stays open
    // across the unlink so the inode number cannot be recycled under the
    // revalidation.
    let identity = open_file_identity(&victim)?;
    let size = victim.metadata()?.len();
    #[cfg(test)]
    cleanup_race::run_eviction();
    let freed = match root.unlink_entry_if_identity(name, identity) {
        Ok(true) => size,
        Ok(false) => 0,
        Err(error) if error.kind() == io::ErrorKind::NotFound => census_size,
        Err(error) => return Err(error),
    };
    drop(victim);
    Ok(freed)
}

/// A decoded on-disk entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredEntry {
    /// Non-reversible digest of the complete request partition: method,
    /// exact URL, range, opaque caller partition, and the normalized `Vary`
    /// names paired with every matching request-field occurrence in order.
    /// Persisted in place of the raw URL and header values so neither a signed
    /// URL's query token nor a `Vary: Authorization`/`Cookie` value ever lands
    /// on disk, yet any difference in that partition produces a different digest
    /// (key separation). It is the full identity `load` validates before serving.
    identity: [u8; IDENTITY_DIGEST_LEN],
    /// Redacted display URL, safe to persist and print. Never contains the query.
    display_url: String,
    body: Vec<u8>,
    etag: Option<String>,
    last_modified: Option<String>,
    /// Freshness lifetime in seconds, already resolved from `max-age`/`Expires`
    /// at store time so a later read needs no re-derivation.
    lifetime: Option<u64>,
    /// `Age` at the moment this response was received: time it had already spent
    /// in upstream caches.
    initial_age: u64,
    no_cache: bool,
    /// Header NAMES this entry varies on, lowercased, sorted, de-duplicated.
    ///
    /// Only names are persisted; the request-header values that select among
    /// varying entries live solely inside the one-way [`identity`](Self::identity)
    /// digest, never on disk, so a `Vary: Authorization`/`Cookie` value cannot
    /// leak into a `.enzhc` file.
    vary_names: Vec<String>,
    stored_at: SystemTime,
}

impl StoredEntry {
    /// Whether this entry is within its freshness lifetime at `now`.
    ///
    /// With no `max-age` an entry is not fresh: this cache does not invent
    /// heuristic lifetimes, so an origin that says nothing gets revalidation
    /// rather than a guess.
    fn is_fresh(&self, now: SystemTime) -> bool {
        let Some(lifetime) = self.lifetime else {
            return false;
        };
        self.current_age(now) <= lifetime
    }

    /// Apparent age in seconds: time already spent upstream plus time held here.
    ///
    /// The `initial_age` term is the whole point of reading `Age`/`Date`. Without
    /// it, a response a proxy had already held for an hour looks brand new the
    /// moment it is received, and a `max-age=3600` response would be served for a
    /// further hour — twice its intended lifetime.
    fn current_age(&self, now: SystemTime) -> u64 {
        let Ok(resident) = now.duration_since(self.stored_at) else {
            // A wall clock can move backwards, and an on-disk entry can be
            // replaced with a future timestamp. Neither may grant extra fresh
            // time: fail stale until a successful revalidation resets the
            // timestamp from this process's clock.
            return u64::MAX;
        };
        self.initial_age.saturating_add(resident.as_secs())
    }
}

/// Validate and normalize a response's `Vary` names into a canonical set.
///
/// Valid names are lowercased, sorted, and de-duplicated. `None` means the set
/// cannot safely be stored: it contains `*`, a non-token field name, or more
/// than [`MAX_VARY_NAMES`] unique names. The unique-name ceiling is enforced
/// while building this vector, before callers allocate canonical partition
/// material or encode/write an entry. Duplicate names therefore remain harmless
/// and do not consume the semantic limit.
fn normalized_vary_names(fields: &[String]) -> Option<Vec<String>> {
    let mut names = Vec::with_capacity(fields.len().min(MAX_VARY_NAMES));
    for field in fields {
        if field == VARY_ANY || !is_http_token(field) {
            return None;
        }
        if names
            .iter()
            .any(|name: &String| name.eq_ignore_ascii_case(field))
        {
            continue;
        }
        if names.len() == MAX_VARY_NAMES {
            return None;
        }
        names.push(field.to_ascii_lowercase());
    }
    names.sort();
    Some(names)
}

/// The complete, domain-free encoding of a request's cache partition.
///
/// This single byte string is hashed twice — once for the filename
/// ([`entry_filename_material`]) and once for the persisted identity
/// ([`identity_digest_material`]) — so a filename and its stored identity can
/// never disagree about which request an entry belongs to; otherwise two
/// sessions sharing a base URL but differing in a repeated `Cookie` could pass
/// the stored check and read each other's body.
///
/// The encoding is length-delimited and domain-separated field by field, so no
/// concatenation of one field can be mistaken for another:
///
/// * method, exact URL;
/// * range and caller/session partition, each with an explicit present/absent
///   flag so an absent value never collides with a present empty one;
/// * the count of normalized `Vary` names, then for each name: the name, the
///   count of matching request-field occurrences (explicit absence is count
///   `0`, distinct from one present-but-empty occurrence), then every occurrence
///   value in the order the request presented them. HTTP/2 may split `Cookie`
///   across repeated fields, so taking only the first occurrence — the original
///   defect — let two distinct requests collapse to one entry.
///
/// `vary_names` must already be normalized by [`normalized_vary_names`].
fn canonical_material(
    key: &CacheKey<'_>,
    vary_names: &[String],
    request_headers: &[(String, String)],
) -> Vec<u8> {
    let mut out = Vec::new();
    put_field(&mut out, key.method.as_bytes());
    put_field(&mut out, key.identity.as_bytes());
    put_optional(&mut out, key.range.map(str::as_bytes));
    put_optional(&mut out, key.partition.map(str::as_bytes));
    out.extend_from_slice(&(vary_names.len() as u64).to_le_bytes());
    for name in vary_names {
        put_field(&mut out, name.as_bytes());
        let occurrences = request_headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim());
        // Materialize so the count precedes the values, length-delimiting the
        // per-name occurrence block. Count `0` is explicit absence.
        let values: Vec<&str> = occurrences.collect();
        out.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for value in values {
            put_field(&mut out, value.as_bytes());
        }
    }
    out
}

/// Append `bytes` with a `u64` length prefix, so no field can run into the next.
fn put_field(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Append an optional field with an explicit present/absent flag byte, so an
/// absent value is encoded distinctly from a present empty one.
fn put_optional(out: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        None => out.push(0),
        Some(bytes) => {
            out.push(1);
            put_field(out, bytes);
        }
    }
}

/// Seconds since the Unix epoch, saturating at the epoch for earlier times.
fn unix_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// Serialise an entry with a checksum over the body.
fn encode_entry(entry: &StoredEntry) -> Vec<u8> {
    let mut out = Vec::with_capacity(entry.body.len() + 128);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    let stored_at = entry
        .stored_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    out.extend_from_slice(&stored_at.to_le_bytes());
    out.extend_from_slice(&entry.lifetime.unwrap_or(u64::MAX).to_le_bytes());
    out.extend_from_slice(&entry.initial_age.to_le_bytes());
    out.push(u8::from(entry.no_cache));
    out.extend_from_slice(&entry.identity);
    write_str(&mut out, &entry.display_url);
    write_str(&mut out, entry.etag.as_deref().unwrap_or(""));
    write_str(&mut out, entry.last_modified.as_deref().unwrap_or(""));
    out.extend_from_slice(
        &u32::try_from(entry.vary_names.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for name in &entry.vary_names {
        write_str(&mut out, name);
    }
    out.extend_from_slice(&(entry.body.len() as u64).to_le_bytes());
    out.extend_from_slice(&checksum(&entry.body).to_le_bytes());
    out.extend_from_slice(&entry.body);
    out
}

/// Parse an entry, returning `None` for anything that does not validate.
///
/// Every length is checked against the remaining input before use, the epoch
/// timestamp must fit [`SystemTime`], and the body checksum must match. A
/// truncated or hostile entry is a miss, never a panic or a trusted payload.
fn decode_entry(raw: &[u8]) -> Option<StoredEntry> {
    let mut cursor = Cursor::new(raw);
    if cursor.take(MAGIC.len())? != MAGIC {
        return None;
    }
    if u32::from_le_bytes(cursor.take_array::<4>()?) != FORMAT_VERSION {
        return None;
    }
    let stored_at = u64::from_le_bytes(cursor.take_array::<8>()?);
    let lifetime = u64::from_le_bytes(cursor.take_array::<8>()?);
    let initial_age = u64::from_le_bytes(cursor.take_array::<8>()?);
    let no_cache = cursor.take(1)?[0] != 0;
    let identity = cursor.take_array::<IDENTITY_DIGEST_LEN>()?;
    let display_url = cursor.take_str()?;
    let etag = cursor.take_str()?;
    let last_modified = cursor.take_str()?;
    let vary_len = u32::from_le_bytes(cursor.take_array::<4>()?) as usize;
    // Bound the declared name count before allocating. Two independent bounds
    // apply, and both must hold:
    //
    // * `MAX_VARY_NAMES` is a semantic ceiling: no legitimate HTTP response
    //   varies on this many fields, so a count past it is corruption or an
    //   attacker-controlled inflation and the file is a miss.
    // * The remaining input, divided by the minimum encoded name width, is the
    //   most names that could physically fit. The previous check compared the
    //   count directly to remaining BYTES, so a large u32 count could pass in a
    //   modest file and reserve a large capacity before the first decode failed.
    if vary_len > MAX_VARY_NAMES {
        return None;
    }
    if vary_len > cursor.remaining() / MIN_ENCODED_VARY_NAME_BYTES {
        return None;
    }
    let mut vary_names = Vec::with_capacity(vary_len);
    for _ in 0..vary_len {
        vary_names.push(cursor.take_str()?);
    }
    debug_assert!(
        vary_names.capacity() <= MAX_VARY_NAMES,
        "Vary names exceed their bound"
    );

    let body_len = u64::from_le_bytes(cursor.take_array::<8>()?);
    let expected_sum = u64::from_le_bytes(cursor.take_array::<8>()?);
    // Reject an absurd declared length before allocating anything.
    let body_len = usize::try_from(body_len).ok()?;
    let body = cursor.take(body_len)?.to_vec();
    if cursor.remaining() != 0 {
        // Trailing bytes mean the file is not what it claims.
        return None;
    }
    if checksum(&body) != expected_sum {
        return None;
    }
    Some(StoredEntry {
        identity,
        display_url,
        body,
        etag: (!etag.is_empty()).then_some(etag),
        last_modified: (!last_modified.is_empty()).then_some(last_modified),
        lifetime: (lifetime != u64::MAX).then_some(lifetime),
        initial_age,
        vary_names,
        no_cache,
        stored_at: UNIX_EPOCH.checked_add(Duration::from_secs(stored_at))?,
    })
}

/// Bounds-checked reader over an entry file.
struct Cursor<'a> {
    raw: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(raw: &'a [u8]) -> Self {
        Self { raw, at: 0 }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let slice = self.raw.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn take_array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let slice = self.take(N)?;
        slice.try_into().ok()
    }

    fn take_str(&mut self) -> Option<String> {
        let len = u32::from_le_bytes(self.take_array::<4>()?) as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).ok()
    }

    const fn remaining(&self) -> usize {
        self.raw.len() - self.at
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "cached fields are bounded by the header limits, far below 4 GiB"
)]
fn write_str(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// FNV-1a over the body. Integrity against corruption and truncation, which is
/// what this needs; it is deliberately not a cryptographic guarantee, because a
/// cache entry is only ever compared against bytes this process wrote.
fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Cache-entry basename for a fully encoded request partition.
///
/// Every filename this cache produces is a 128-bit hex digest with a fixed
/// `.enzhc` extension — a directory-relative *name*, never a full path, so it
/// can be handed straight to [`SecureRoot`]'s `*at` methods and cannot contain
/// a directory separator or a `..` component. The `material` is the canonical
/// partition ([`canonical_material`]); the base (no-Vary) partition and the
/// varied partition therefore land on different names, exactly as `get`'s
/// two-step lookup expects.
fn entry_filename_material(material: &[u8]) -> OsString {
    OsString::from(format!("{:032x}.enzhc", path_digest_material(material)))
}

/// Collision-resistant 128-bit filename digest of a canonical request partition.
///
/// SHA-256 is domain-separated from the persisted identity digest below, then
/// truncated to 128 bits. Taking whole digest bytes introduces no modulo bias;
/// the full 256-bit identity stored inside the entry remains the collision
/// check before any response is served.
fn path_digest_material(material: &[u8]) -> u128 {
    let full = digest_bytes_with_domain(b"enzui-http-cache-path-v2", material);
    u128::from_be_bytes(full[..16].try_into().expect("SHA-256 prefix is 16 bytes"))
}

/// One-way, domain-separated SHA-256 digest of a complete request partition.
///
/// Persisted in place of the raw URL and the raw `Vary` values, so neither a
/// signed URL's query token nor a `Vary: Authorization`/`Cookie` value reaches
/// disk, while still separating two requests that differ anywhere in the
/// partition (method, URL, range, caller, or any repeated `Vary` occurrence).
/// SHA-256 supplies the collision and preimage resistance this value's
/// cache-poisoning and redaction roles require. It is not a MAC, makes no
/// authenticity claim, and does not conceal a low-entropy token from offline
/// guessing; entry integrity remains the checksum's separate job.
fn identity_digest_material(material: &[u8]) -> [u8; IDENTITY_DIGEST_LEN] {
    digest_bytes_with_domain(b"enzui-http-cache-identity-v2", material)
}

/// The canonical material for a plain `GET` of `url` with no range, no caller
/// partition and no `Vary`. This is the base partition every non-varying entry
/// (and every [`ImageService`](crate::ImageService) request) uses, and the
/// single seam white-box tests key filenames and identities through.
#[cfg(test)]
fn base_material_for(url: &str) -> Vec<u8> {
    canonical_material(&CacheKey::from(url), &[], &[])
}

/// Test-only: 128-bit filename digest for a plain non-varying `GET` of `url`.
#[cfg(test)]
fn digest(url: &str) -> u128 {
    path_digest_material(&base_material_for(url))
}

/// Test-only: persisted identity digest for a plain non-varying `GET` of `url`.
#[cfg(test)]
fn identity_digest(url: &str) -> [u8; IDENTITY_DIGEST_LEN] {
    identity_digest_material(&base_material_for(url))
}

/// Read an already-open cache entry into memory without ever allocating or
/// reading past `limit`.
///
/// The caller supplies the open descriptor, which for the cache always comes
/// from [`SecureRoot::open_entry_read`] (`openat(root_fd, name, O_RDONLY |
/// O_NOFOLLOW | O_NONBLOCK)`). The descriptor is borrowed, not consumed, so the
/// caller keeps it open across a following identity-bound unlink — the inode
/// number cannot be recycled while that fd is held, which is what makes
/// [`SecureRoot::unlink_entry_if_identity`] sound after a read.
///
/// A cache entry may have been replaced by a file that is oversized, sparse (a
/// huge logical length backed mostly by holes), still growing, or lying about
/// its length. Each is refused as a miss instead of forcing an unbounded read:
///
/// * the declared length caps the initial allocation and is rejected before any
///   allocation when it already exceeds `limit`, so a sparse file's multi-gigabyte
///   logical length never becomes a multi-gigabyte `Vec`;
/// * the read itself is capped at `limit + 1` bytes, so a file that grows past
///   the ceiling between the metadata check and the read is refused rather than
///   pulled in whole.
fn read_bounded_from(file: &fs::File, limit: u64) -> Option<Vec<u8>> {
    // Validate the OPENED descriptor via fstat, not a fresh stat of the path.
    // `O_NOFOLLOW` refused a symlink at the final component in the kernel; this
    // additionally refuses a directory, fifo, socket, or device substituted at
    // the same name. A non-regular object is a miss, never read.
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    // Reject on the declared length before allocating. A sparse file reports its
    // logical length here, so a mostly-holes file is refused just like a dense
    // one, and nothing is allocated for it.
    let declared = meta.len();
    if declared > limit {
        return None;
    }
    // The declared length is already <= limit, so the initial capacity is bounded.
    let mut buf = Vec::with_capacity(usize::try_from(declared).ok()?);
    // Cap the read at limit + 1: if the file grew past the ceiling since the
    // metadata check, the extra byte is observed and the entry refused rather
    // than the whole grown file being read.
    let read = file
        .take(limit.saturating_add(1))
        .read_to_end(&mut buf)
        .ok()?;
    if read as u64 > limit {
        return None;
    }
    Some(buf)
}

/// Write an entry through the shared descriptor-relative staging owner, so a
/// reader never observes a partial entry and a crash mid-write cannot leave a
/// corrupt file under the real name.
///
/// The staging name, the owner-only exclusive creation, the write, the
/// pre-publication `fsync`, the publication and the identity-bound cleanup —
/// which unlinks the staging entry on an ordinary failure once identity is
/// established, but fails closed and leaves an owner-only entry rather than
/// unlink a possibly-foreign name when identity is inconclusive — are all
/// [`RootTempStage`](enzui_security::tempstage::RootTempStage)'s, which is
/// the same owner `enzui_core::atomic_state` stages through, so symlink
/// resistance, collision policy and cleanup cannot drift between the two
/// writers.
///
/// The stage is created and published **descriptor-relative on the retained
/// root fd** (`openat(root_fd, tmp, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW,
/// 0o600)` then `renameat(root_fd, tmp, root_fd, name)`), so a substituted root
/// pathname after adoption cannot redirect the write, an `AlreadyExists` at the
/// staging name is refused rather than unlinked and retried, and the stage's
/// `Drop` cleanup also covers an unwind.
///
/// On success this returns the `(device, inode)` identity of the inode it
/// published under `name`. A caller that must later roll its own publication
/// back (a varying store whose pointer write then fails) unlinks `name` only
/// while it still denotes this identity, so a concurrent writer's newer
/// replacement is never deleted in place of the caller's own body. Publication
/// closes the staged descriptor, so that rollback is the strict narrowing
/// [`SecureRoot::unlink_entry_if_identity`] documents rather than an
/// fd-pinned check.
fn write_atomically(root: &SecureRoot, name: &OsStr, bytes: &[u8]) -> io::Result<(u64, u64)> {
    // Every byte this cache puts on disk passes through here, which is what
    // makes "did anything get written?" answerable. A `no-store` response that
    // is written and then deleted leaves no file behind, so counting files in
    // the directory afterwards cannot tell the two apart -- and the difference
    // is the whole point of `no-store`: once the bytes exist on disk a backup,
    // a snapshot, or another process may already have seen them.
    #[cfg(test)]
    {
        // Outside a window this is an uncontended lock around one atomic add;
        // inside one it is skipped, because the window owner already holds it.
        // A poisoned lock is reported by every counter window (which is where
        // the measurement is actually trusted); an incidental writer recovers
        // instead, so one failing counter test cannot cascade a panic into
        // every other test that merely happens to write a file.
        let _exclusion = (!IN_DISK_WRITE_WINDOW.with(std::cell::Cell::get)).then(|| {
            DISK_WRITE_WINDOW
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        DISK_WRITES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    let mut staged = RootTempStage::create(root, name)?;
    // Any early return from here drops the stage: on an ordinary failure with
    // identity established that unlinks the staging entry, and an inconclusive
    // revalidation instead leaves an owner-only entry rather than unlink a
    // possibly-foreign name. Either way a failed write leaves neither a partial
    // private file published under the real name nor a damaged destination.
    staged.write_all(bytes)?;
    // The stage's exclusively-created inode is what `commit` renames into place;
    // a `renameat` moves the name, not the inode, so this identity denotes
    // `name` after a successful publication.
    let published = staged.identity();
    staged.commit()?;
    Ok(published)
}

/// Test-only deterministic seams that fire inside the window between an
/// identity-bound cleanup's read or decision and its `unlinkat`.
///
/// Each action is thread-local and one-shot, so a test drives a concurrent
/// replacement at the hostile instant with no thread join. Arming yields a
/// [`SeamGuard`] that clears the slot on drop, so an assertion that fails
/// before the seam fires cannot leave an action armed for the next test on the
/// same harness thread; [`armed`] additionally lets a test prove its own seam
/// was consumed inside the window. Every call site is behind `#[cfg(test)]`.
#[cfg(test)]
pub(crate) mod cleanup_race {
    use std::cell::RefCell;

    thread_local! {
        static CORRUPT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static ROLLBACK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static EVICTION: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    /// Disarms its slot on drop, including while unwinding from a failed
    /// assertion, so no armed action outlives the test that armed it.
    #[must_use = "binding the guard is what keeps the seam armed for the call"]
    pub(crate) struct SeamGuard(fn());

    impl Drop for SeamGuard {
        fn drop(&mut self) {
            (self.0)();
        }
    }

    /// Arm the corrupt/identity-mismatch load-cleanup seam on this thread.
    pub(crate) fn arm_corrupt(action: Box<dyn FnOnce()>) -> SeamGuard {
        CORRUPT.with(|slot| *slot.borrow_mut() = Some(action));
        SeamGuard(|| CORRUPT.with(|slot| *slot.borrow_mut() = None))
    }

    /// Whether any seam is still armed on this thread.
    pub(crate) fn armed() -> bool {
        CORRUPT.with(|slot| slot.borrow().is_some())
            || ROLLBACK.with(|slot| slot.borrow().is_some())
            || EVICTION.with(|slot| slot.borrow().is_some())
    }

    pub(super) fn run_corrupt() {
        if let Some(action) = CORRUPT.with(|slot| slot.borrow_mut().take()) {
            action();
        }
    }

    pub(super) fn run_rollback() {
        if let Some(action) = ROLLBACK.with(|slot| slot.borrow_mut().take()) {
            action();
        }
    }

    pub(super) fn run_eviction() {
        if let Some(action) = EVICTION.with(|slot| slot.borrow_mut().take()) {
            action();
        }
    }
}

/// XDG cache root: `$XDG_CACHE_HOME`, else `$HOME/.cache`.
///
/// Resolved directly rather than via a crate: the rule is two environment
/// variables and one fallback, and a new dependency for that is not worth the
/// supply-chain surface. A relative `XDG_CACHE_HOME` is ignored, as the spec
/// requires.
fn xdg_cache_root() -> Option<PathBuf> {
    resolve_cache_root(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The resolution rule itself, with the environment passed in.
///
/// Split out so it is testable without mutating process-wide environment
/// variables: `cargo test` runs tests concurrently in one process, so a test
/// that set `HOME` would corrupt its siblings, and the workspace denies the
/// `unsafe` block `set_var` now requires.
fn resolve_cache_root(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(dir) = xdg {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    let home = PathBuf::from(home?);
    if home.as_os_str().is_empty() {
        return None;
    }
    Some(home.join(".cache"))
}

#[cfg(test)]
mod tests;
