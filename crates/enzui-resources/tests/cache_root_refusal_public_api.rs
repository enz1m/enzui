//! The cache-root refusal reason is reachable from *outside* this crate.
//!
//! # Why an integration target
//!
//! `http_cache` is a private module; only the names in `lib.rs`'s
//! `pub use http_cache::{...}` list exist for an application. This is the exact
//! trap `tests/cache_key_reachable_downstream.rs` was written for: an in-crate
//! test can see a type an application cannot, so "the refusal is observable" has
//! to be asserted by a compilation unit that sees the crate as an app does.
//!
//! A typed reason nobody can name is the same defect as no typed reason at all:
//! callers need a typed unavailable outcome, not merely an internal enum.
//!
//! SABOTAGE: remove `CacheUnavailable` from the `pub use http_cache::{...}` list
//! in `crates/enzui-resources/src/lib.rs`. This target then fails to
//! compile ("no `CacheUnavailable` in the root"), which is the red state.

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
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use enzui_resources::{CacheUnavailable, HttpCache, HttpCacheConfig};
use enzui_security::secure_dir::SecureDirRefusal;

/// Scratch on real disk under `target/`, never the RAM-backed system temp directory.
fn scratch(tag: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "cache-root-refusal-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch base");
    path
}

/// An application can name the reason *and* match its payload: a world-writable
/// cache root reports `Root(GroupOrWorldWritable { .. })`, while a healthy root
/// reports nothing. Both halves matter — the second is what stops "always
/// unavailable" from passing.
#[test]
fn an_application_can_observe_and_match_the_root_refusal() {
    let base = scratch("observe");

    let hostile = base.join("hostile");
    std::fs::create_dir_all(&hostile).expect("create hostile root");
    std::fs::set_permissions(&hostile, std::fs::Permissions::from_mode(0o777))
        .expect("chmod hostile root");
    let refused = HttpCache::open_at(&hostile, HttpCacheConfig::DEFAULT);
    match refused.unavailable() {
        Some(CacheUnavailable::Root(SecureDirRefusal::GroupOrWorldWritable { mode })) => {
            assert_eq!(*mode & 0o022, 0o022, "the offending bits are reported");
        }
        other => panic!("expected a typed group/world-writable root refusal, got {other:?}"),
    }
    assert!(!refused.is_enabled());

    // Positive control: a root this crate provisions itself is enabled and has
    // no reason attached.
    let healthy = HttpCache::open_at(base.join("healthy"), HttpCacheConfig::DEFAULT);
    assert_eq!(healthy.unavailable(), None);
    assert!(healthy.is_enabled());

    // And the benign arm is nameable too, so an app can log a real finding
    // differently from "the user turned caching off".
    let off = HttpCache::open_at(
        base.join("off"),
        HttpCacheConfig {
            max_bytes: 0,
            ..HttpCacheConfig::DEFAULT
        },
    );
    assert_eq!(off.unavailable(), Some(&CacheUnavailable::Disabled));
}
