//! Shared response-header accumulation limits for all transports.
//!
//! A hostile origin can send an arbitrary number of response headers, each up
//! to libcurl's per-line maximum (~100 KiB). Without a bound this allocates
//! hundreds of MiB before any body handling runs. Both
//! [`crate::curl_transport`] and [`crate::curl_download_transport`] share
//! these exact ceilings so a header flood is detected identically regardless
//! of the transfer mode.

/// Maximum number of response headers retained per hop.
///
/// Normal HTTP responses carry 10–30 headers; 128 is generous headroom
/// for CDN chains, Set-Cookie, and diagnostic headers.
pub const MAX_HEADER_COUNT: usize = 128;

/// Maximum total bytes of response-header name+value storage per hop.
///
/// 64 KiB is far above any legitimate response-header set. Each entry's byte
/// cost includes [`ENTRY_OVERHEAD`] to account for the two heap `String`
/// allocations and the `Vec` slot.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Per-entry overhead added to `name.len() + value.len()` for deterministic
/// byte accounting.
///
/// Each `(String, String)` occupies at minimum two `String` headers (3×usize
/// each on the heap side, though only a flat overhead is charged for the
/// allocator metadata) plus one `Vec` slot (2×usize for the fat pair). The charge is
/// a flat 64 bytes: this is deliberately rounded and generous so tests
/// can assert exact arithmetic without chasing platform-specific allocator
/// padding.
pub const ENTRY_OVERHEAD: usize = 64;

/// Which ceiling was hit first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderLimitKind {
    /// More than [`MAX_HEADER_COUNT`] headers.
    Count {
        /// The limit that was exceeded.
        limit: usize,
    },
    /// Accumulated bytes (name + value + overhead) exceeded [`MAX_HEADER_BYTES`].
    Bytes {
        /// Accumulated bytes at the point of refusal.
        accumulated: usize,
        /// The limit that was exceeded.
        limit: usize,
    },
    /// A singleton response header appeared more than once with a different
    /// value, making consumer agreement impossible.
    ///
    /// The one exception is `Content-Length` with an identical numeric value
    /// across occurrences (RFC 9110 §8.6).
    DuplicateSingleton {
        /// The lowercased header name.
        name: &'static str,
    },
    /// Both `Transfer-Encoding` and `Content-Length` are present.
    ///
    /// RFC 9112 §6.1: the combination "might indicate an attempt to perform
    /// request smuggling or response splitting and ought to be handled as an
    /// error."
    ConflictingFraming,
}

/// Singleton response header names where a second distinct value creates
/// ambiguity that different consumers resolve differently (first-wins vs
/// last-wins), enabling cache poisoning or request smuggling.
///
/// Indices correspond to bit positions in
/// [`HeaderAccumulator::singleton_seen`].
const SINGLETON_NAMES: [&str; 11] = [
    "content-length",    // 0
    "content-type",      // 1
    "content-encoding",  // 2
    "transfer-encoding", // 3
    "location",          // 4
    "etag",              // 5
    "last-modified",     // 6
    "age",               // 7
    "date",              // 8
    "expires",           // 9
    "content-range",     // 10
];

/// Bit index for a singleton header name, or `None` for non-singletons.
fn singleton_bit_index(name: &str) -> Option<u8> {
    SINGLETON_NAMES
        .iter()
        .position(|&s| s == name)
        .and_then(|i| u8::try_from(i).ok())
}

/// Tracks response-header accumulation against shared ceilings.
///
/// Constructed once per hop (or per status line in the download transport) and
/// queried from each header callback invocation. If [`Self::check`] or
/// [`Self::check_header`] returns `Err`, the callback must immediately return
/// `false` to abort the transfer.
pub struct HeaderAccumulator {
    count: usize,
    bytes: usize,
    /// Bitmask: bit *i* is set when `SINGLETON_NAMES[i]` has been seen.
    singleton_seen: u16,
    /// The numeric value of the first `Content-Length` occurrence. A second
    /// occurrence is accepted only when its value matches (RFC 9110 §8.6).
    content_length_value: Option<u64>,
    /// Set when `Transfer-Encoding` has been seen; a subsequent
    /// `Content-Length` (or vice versa) is a framing conflict.
    has_transfer_encoding: bool,
    /// Set when `Content-Length` has been seen; a subsequent
    /// `Transfer-Encoding` is a framing conflict.
    has_content_length: bool,
}

impl Default for HeaderAccumulator {
    // A default accumulator is exactly `Self::new()`: this shim preserves the
    // const constructor as the canonical spelling (used by call sites in
    // `header_function` callbacks that need a const initializer), while
    // satisfying `clippy::new_without_default` without adding a lint allow.
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderAccumulator {
    /// A fresh accumulator with no headers counted.
    pub const fn new() -> Self {
        Self {
            count: 0,
            bytes: 0,
            singleton_seen: 0,
            content_length_value: None,
            has_transfer_encoding: false,
            has_content_length: false,
        }
    }

    /// Reset the accumulator (e.g. on a new status line during redirects).
    pub fn reset(&mut self) {
        self.count = 0;
        self.bytes = 0;
        self.singleton_seen = 0;
        self.content_length_value = None;
        self.has_transfer_encoding = false;
        self.has_content_length = false;
    }

    /// Account for one header and check count and byte ceilings.
    ///
    /// `name_len` and `value_len` are the byte lengths of the trimmed,
    /// lowercased name and the trimmed value that would be pushed into the
    /// header `Vec`. [`ENTRY_OVERHEAD`] is added automatically.
    ///
    /// Returns `Ok(())` when the entry fits, or `Err(HeaderLimitKind)` with
    /// the specific ceiling that was exceeded.
    ///
    /// Prefer [`Self::check_header`] in production header callbacks: it adds
    /// singleton and framing checks that this method does not perform.
    ///
    /// # Errors
    ///
    /// A [`HeaderLimitKind`] when the header count or byte total is over its ceiling.
    pub fn check(&mut self, name_len: usize, value_len: usize) -> Result<(), HeaderLimitKind> {
        let next_count = self.count + 1;
        if next_count > MAX_HEADER_COUNT {
            return Err(HeaderLimitKind::Count {
                limit: MAX_HEADER_COUNT,
            });
        }
        let entry_bytes = name_len
            .saturating_add(value_len)
            .saturating_add(ENTRY_OVERHEAD);
        let next_bytes = self.bytes.saturating_add(entry_bytes);
        if next_bytes > MAX_HEADER_BYTES {
            return Err(HeaderLimitKind::Bytes {
                accumulated: next_bytes,
                limit: MAX_HEADER_BYTES,
            });
        }
        self.count = next_count;
        self.bytes = next_bytes;
        Ok(())
    }

    /// Account for one response header, checking size ceilings and rejecting
    /// singleton/framing ambiguity.
    ///
    /// `name` must be the already-trimmed, lowercased header name.
    /// `value` must be the already-trimmed value.
    ///
    /// Rejects:
    /// - Duplicate singleton headers (Content-Length, Content-Type, Location,
    ///   ETag, …) except Content-Length with an identical numeric value
    ///   across occurrences (RFC 9110 §8.6).
    /// - Coexistence of `Transfer-Encoding` and `Content-Length`
    ///   (RFC 9112 §6.1 — request-smuggling indicator).
    /// - Count and byte ceilings from [`Self::check`].
    ///
    /// # Errors
    ///
    /// As [`Self::check`], and for a duplicated singleton or ambiguous framing header.
    pub fn check_header(&mut self, name: &str, value: &str) -> Result<(), HeaderLimitKind> {
        self.check_ambiguity(name, value)?;
        self.check(name.len(), value.len())
    }

    /// Singleton and framing checks only, without counting the header toward
    /// the byte/count budget. Called by [`Self::check_header`].
    fn check_ambiguity(&mut self, name: &str, value: &str) -> Result<(), HeaderLimitKind> {
        let Some(idx) = singleton_bit_index(name) else {
            return Ok(());
        };
        let mask = 1u16 << idx;

        if self.singleton_seen & mask != 0 {
            // Content-Length: RFC 9110 §8.6 permits identical duplicates.
            if name == "content-length"
                && let Some(prev) = self.content_length_value
                && value.parse::<u64>().ok() == Some(prev)
            {
                return Ok(());
            }
            return Err(HeaderLimitKind::DuplicateSingleton {
                name: SINGLETON_NAMES[idx as usize],
            });
        }
        self.singleton_seen |= mask;

        // Track framing headers for conflict detection.
        if name == "content-length" {
            self.has_content_length = true;
            self.content_length_value = value.parse::<u64>().ok();
            if self.has_transfer_encoding {
                return Err(HeaderLimitKind::ConflictingFraming);
            }
        }
        if name == "transfer-encoding" {
            self.has_transfer_encoding = true;
            if self.has_content_length {
                return Err(HeaderLimitKind::ConflictingFraming);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_over_count() {
        let mut acc = HeaderAccumulator::new();
        for _ in 0..MAX_HEADER_COUNT {
            assert!(acc.check(1, 1).is_ok());
        }
        let err = acc.check(1, 1).unwrap_err();
        assert!(
            matches!(err, HeaderLimitKind::Count { limit } if limit == MAX_HEADER_COUNT),
            "expected Count limit, got {err:?}"
        );
    }

    // ── Singleton / framing ambiguity ────────────────────────────────
}
