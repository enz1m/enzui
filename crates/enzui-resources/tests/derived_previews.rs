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

use enzui_resources::derived_preview::*;
use enzui_test_support::test_scratch::ScratchDir;
use iced_core::image::Handle;
use std::{sync::Arc, task::Wake};

#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
fn config(bytes: u64) -> PreviewStoreConfig {
    PreviewStoreConfig {
        disk_bytes: bytes,
        max_preview_rgba_bytes: 1024 * 1024,
    }
}
fn key(source: &[u8]) -> PreviewKey {
    PreviewKey::new(source, (16, 16), b"crop", b"decoder-v1")
}
fn raster() -> Handle {
    Handle::from_rgba(16, 16, [17, 53, 89, 117].repeat(256))
}
fn pixels(handle: &Handle) -> &[u8] {
    let Handle::Rgba { pixels, .. } = handle else {
        panic!("RGBA required");
    };
    pixels
}
fn get(store: &PreviewStore, key: &PreviewKey) -> PreviewResult {
    store
        .get_or_derive(key, &[], || true, || false, |_cancelled| Ok(raster()))
        .unwrap()
}
#[test]
fn derived_previews_corruption_heals_and_unusable_root_still_returns_pixels() {
    let scratch = ScratchDir::new("preview-corrupt");
    let path = scratch.path().join("cache");
    let store = PreviewStore::open(&path, config(100000));
    let k = key(b"heal");
    get(&store, &k);
    let entry = std::fs::read_dir(&path)
        .unwrap()
        .map(|x| x.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "png"))
        .unwrap();
    std::fs::write(&entry, b"broken").unwrap();
    assert!(!get(&store, &k).disk_hit);
    assert!(get(&store, &k).disk_hit);
    assert_eq!(store.usage().unwrap().entries, 1);
    let bad = scratch.path().join("file");
    std::fs::write(&bad, b"not a directory").unwrap();
    let result = get(&PreviewStore::open(&bad, config(100000)), &k);
    assert_eq!(pixels(&result.handle), pixels(&raster()));
    assert!(result.cache_error.is_some());
}

#[test]
fn an_over_budget_store_evicts_the_oldest_unprotected_entry() {
    let scratch = ScratchDir::new("preview-evict");
    let mut store = PreviewStore::open(scratch.path().join("cache"), config(10_000_000));
    let [a, b, c] = [key(b"a"), key(b"b"), key(b"c")];
    for k in [&a, &b, &c] {
        assert!(!get(&store, k).disk_hit);
    }
    let full = store.usage().unwrap();
    assert_eq!(full.entries, 3);

    // Room for one entry fewer: the oldest, `a`, is visible, so `b` goes.
    store
        .set_disk_budget(full.disk_bytes - 1, std::slice::from_ref(&a))
        .unwrap();
    assert_eq!(store.usage().unwrap().entries, 2);
    // Look with room to spare, so looking (which refreshes recency) evicts nothing.
    store.set_disk_budget(10_000_000, &[]).unwrap();
    assert!(get(&store, &c).disk_hit);
    assert!(get(&store, &a).disk_hit);
    assert!(
        !get(&store, &b).disk_hit,
        "the evicted entry is derived again"
    );

    // A budget that already fits evicts nothing.
    let fits = store.usage().unwrap();
    store.set_disk_budget(fits.disk_bytes + 1, &[]).unwrap();
    assert_eq!(store.usage().unwrap().entries, fits.entries);
}

/// A hit on the most recently used entry rewrites nothing (its recency cannot
/// change); a hit on an older one is moved to the newest.
#[test]
fn a_hit_on_the_newest_entry_is_not_written_again() {
    let scratch = ScratchDir::new("preview-newest");
    let path = scratch.path().join("cache");
    let store = PreviewStore::open(&path, config(10_000_000));
    let state = || {
        std::fs::read_dir(&path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().is_some_and(|name| name == "epoch"))
            .map(|path| std::fs::read(path).unwrap())
            .unwrap()
    };
    let (a, b) = (key(b"a"), key(b"b"));
    get(&store, &a);
    get(&store, &b);
    let before = state();
    assert!(get(&store, &b).disk_hit);
    assert_eq!(state(), before, "the newest entry was written again");
    assert!(get(&store, &a).disk_hit);
    assert_ne!(state(), before, "an older entry moves to the newest");
}
