//! Downstream reachability contract for the `HttpCache` key type.
//!
//! `HttpCache::{get, store, mark_revalidated, remove}` are all typed
//! `impl Into<CacheKey<'_>>`. Two conversions exist: the credential-safe
//! [`CacheKey::new`] split (secret-bearing `identity` digested for keying, a
//! separately redacted `display` persisted to disk) and `From<&str>`, whose own
//! documentation restricts it to URLs *with no credentials* because it writes
//! the identity to disk verbatim.
//!
//! The split constructor was reachable only from inside this crate: `CacheKey`
//! sits in the private `http_cache` module and was omitted from the crate-root
//! re-export list that publishes `HttpCache`, `HttpCacheConfig`, `Freshness`,
//! `CachedResponse` and friends. Every existing cache test lives in
//! `src/http_cache/tests.rs` — *inside* the crate — where the type is nameable,
//! so a green suite proved nothing about what an application can call. An
//! integration test is the first compilation unit that links this crate the way
//! An app does, which is why this gate lives here and not in `src/`.
//!
//! This file is therefore a compile contract first: the `use` below fails with
//! `E0432` if the re-export is dropped again, and the assertions then prove the
//! reachable API still does the thing that makes it worth reaching — keeping a
//! query token off disk while remaining a usable cache key.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_resources::{CacheKey, Freshness, HttpCache, HttpCacheConfig};
use std::path::{Path, PathBuf};

/// A cache directory under the target dir, removed on drop, so a failing
/// assertion leaves no litter and nothing touches the real `~/.cache`.
struct RunDir(PathBuf);

impl RunDir {
    fn new(label: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "enzui-cachekey-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create cache root");
        Self(path)
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fresh() -> Freshness {
    Freshness {
        max_age: Some(3_600),
        ..Freshness::default()
    }
}

fn open_cache(root: &RunDir) -> HttpCache {
    HttpCache::open_at(
        &root.0,
        HttpCacheConfig {
            max_bytes: 1 << 20,
            max_entry_bytes: 1 << 18,
        },
    )
}

/// Every byte written under `root`, concatenated, plus the number of files read.
/// Cache entries are a binary framing, so the search is over raw bytes rather
/// than a UTF-8 rendering.
fn stored_bytes(root: &Path) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut files = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read cache dir") {
            let entry = entry.expect("cache dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.extend_from_slice(&std::fs::read(&path).expect("read cache file"));
                out.extend_from_slice(path.to_string_lossy().as_bytes());
                files += 1;
            }
        }
    }
    (out, files)
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

const SECRET: &str = "sig=SUPERSECRET0123456789";

#[test]
fn a_downstream_crate_can_build_a_split_key_that_keeps_the_token_off_disk() {
    let root = RunDir::new("split");
    let cache = open_cache(&root);
    let identity = format!("https://fixture.invalid/hero.png?{SECRET}");
    let display = "https://fixture.invalid/hero.png";
    let body = vec![7_u8; 512];

    // The whole point of the type: an application names it and splits the key.
    let key = CacheKey::new(&identity, display);
    assert!(
        cache.store(key, &body, &fresh(), &[]),
        "a split-key store must be accepted"
    );
    assert_eq!(
        cache.get(key, &[]).map(|hit| hit.body.len()),
        Some(body.len()),
        "the entry must be retrievable with the same split key"
    );

    let (bytes, files) = stored_bytes(&root.0);
    assert!(files > 0, "calibration: the store wrote no file to read");
    assert!(
        contains(&bytes, display),
        "the redacted display URL must be what is persisted"
    );
    assert!(
        !contains(&bytes, SECRET),
        "the query token reached disk through a split key"
    );

    // Positive control for the absence check above: the `&str` conversion —
    // the only key a downstream crate could name before this API was reachable
    // — persists the identity verbatim, and this scan sees it. Without this,
    // "no token on disk" could be a blind scan.
    let leaky = RunDir::new("control");
    let leaky_cache = open_cache(&leaky);
    assert!(
        leaky_cache.store(identity.as_str(), &body, &fresh(), &[]),
        "control store must be accepted"
    );
    let (control_bytes, control_files) = stored_bytes(&leaky.0);
    assert!(control_files > 0, "calibration: control wrote no file");
    assert!(
        contains(&control_bytes, SECRET),
        "control: the scan cannot see a token on disk, so the absence check above proves nothing"
    );
}
