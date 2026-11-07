use super::*;

// Strict directory-entry collection: discovery failures must fail closed.

/// Strict-collect full `DirEntry` values for callers that need the path.
fn strict_dir_entries(
    dir: &std::path::Path,
    iter: impl Iterator<Item = std::io::Result<std::fs::DirEntry>>,
) -> Vec<std::fs::DirEntry> {
    iter.map(|r| r.unwrap_or_else(|e| panic!("entry read failed in {}: {e}", dir.display())))
        .collect()
}

fn strict_verdict_entries(dir: &std::path::Path) -> Vec<std::fs::DirEntry> {
    let rd = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("open dir {}: {e}", dir.display()));
    strict_dir_entries(dir, rd)
}

fn strict_read(path: &std::path::Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("read file {}: {e}", path.display()))
}

// ==== Age / Date / Expires freshness (RFC 9111) =============================

// ==== Vary partitioning ======================================================

// ==== Last-Modified round trip ==============================================

// ==== Secrets never reach disk =============================================

// ==== Bounded reads ========================================================

/// Substring search over raw bytes, for the secret-persistence check.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ==== CacheKey never renders its secret identity ===========================

// ==== v2 -> v3 migration is fail-safe ======================================

/// The pre-fix on-disk encoder (`FORMAT_VERSION == 2`), which embedded the raw
/// URL — secrets and all — where v3 now stores an opaque identity digest.
fn encode_v2_entry(raw_url: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&unix_seconds(SystemTime::now()).to_le_bytes());
    out.extend_from_slice(&3600u64.to_le_bytes()); // lifetime
    out.extend_from_slice(&0u64.to_le_bytes()); // initial_age
    out.push(0); // no_cache
    write_str(&mut out, raw_url); // v2 persisted the RAW URL here
    write_str(&mut out, ""); // etag
    write_str(&mut out, ""); // last_modified
    out.extend_from_slice(&0u32.to_le_bytes()); // vary_len
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&checksum(body).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// An old version-2 entry carrying a raw signed URL is rejected as a miss and
/// deleted, and its secret is never re-persisted.
#[test]
fn an_old_v2_entry_with_a_raw_url_is_rejected_deleted_and_never_served() {
    const TOKEN: &str = "V2LEFTOVERSIGNINGTOKEN0123456789";
    let raw = format!("https://example.test/old.png?sig={TOKEN}");

    let root = TempRoot::new("v2migrate");
    let cache = cache_at(&root);

    // Positive control: a valid v3 entry under a different URL is a hit, so the
    // misses below are meaningful rather than a dead cache.
    const LIVE: &str = "https://example.test/live.png";
    assert!(cache.store(LIVE, BODY, &fresh_for(3600), NO_REQ));
    assert_eq!(
        cache.get(LIVE, NO_REQ).expect("v3 control hit").body,
        BODY,
        "calibration: a current v3 entry must still be served"
    );

    let mut servable = encode_entry(&StoredEntry {
        identity: identity_digest(&raw),
        display_url: "https://example.test/old.png?<redacted>".to_owned(),
        body: BODY.to_vec(),
        etag: None,
        last_modified: None,
        lifetime: Some(3600),
        initial_age: 0,
        no_cache: false,
        vary_names: Vec::new(),
        stored_at: SystemTime::now(),
    });
    assert!(
        decode_entry(&servable).is_some(),
        "control: the entry decodes while its version is current"
    );
    // Patch only the 4-byte version word (right after the 8-byte magic) to 2.
    servable[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&2u32.to_le_bytes());
    assert!(
        decode_entry(&servable).is_none(),
        "an entry whose only defect is version 2 must be rejected at the version word"
    );

    // A genuine v2-layout file (raw URL with the secret) is written to disk at
    // its cache path, then must be a miss and must be removed.
    let v2 = encode_v2_entry(&raw, BODY);
    assert!(
        contains_subslice(&v2, TOKEN.as_bytes()),
        "the fixture must actually contain the secret for this test to mean anything"
    );
    assert!(
        decode_entry(&v2).is_none(),
        "a v2-layout entry must not decode under the v3 reader"
    );
    let path = root.path().join(format!("{:032x}.enzhc", digest(&raw)));
    fs::write(&path, &v2).unwrap();
    assert!(
        cache.get(raw.as_str(), NO_REQ).is_none(),
        "an old v2 entry must be a MISS, never served"
    );
    assert!(
        !path.exists(),
        "and the old v2 file must be removed, not left to be re-read forever"
    );

    // The secret must not have been copied into any surviving cache file.
    for item in strict_verdict_entries(root.path()) {
        let path = item.path();
        let bytes = strict_read(&path);
        assert!(
            !contains_subslice(&bytes, TOKEN.as_bytes()),
            "the old secret must not be re-persisted anywhere after migration"
        );
    }
}

// ==== store never writes an immediately-unreadable entry ===================

// ==== clear reports failures and accounting ================================

// ==== decode_entry bounds an inflated vary count before allocating =========

// strict_entry_names: fail-closed seam and positive-control tests.
