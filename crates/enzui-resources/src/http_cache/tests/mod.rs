//! Encoded HTTP cache: freshness, revalidation, bounding, and poisoning.

pub(super) use super::*;
pub(super) use std::sync::atomic::Ordering;

/// This cache sends no request headers, so the varying set is empty.
pub(super) const NO_REQ: &[(String, String)] = &[];

/// A unique cache root per test, removed again when the guard drops.
pub(super) struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    pub(super) fn new(label: &str) -> Self {
        let unique = format!(
            "enzui-hc-test-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let path = std::env::temp_dir().join(unique);
        let _ = fs::remove_dir_all(&path);
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// Strict directory-entry collection: discovery failures must fail closed.

fn strict_read(path: &std::path::Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("read file {}: {e}", path.display()))
}

pub(super) fn cache_at(root: &TempRoot) -> HttpCache {
    HttpCache::open_at(root.path(), HttpCacheConfig::DEFAULT)
}

/// An open measurement window over the process-global [`DISK_WRITES`] counter.
#[must_use = "the window closes as soon as the guard is dropped"]
pub(super) struct DiskWriteWindow {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl DiskWriteWindow {
    /// The physical-write count at this instant, inside the window.
    pub(super) fn writes(&self) -> u64 {
        DISK_WRITES.load(Ordering::SeqCst)
    }
}

impl Drop for DiskWriteWindow {
    fn drop(&mut self) {
        IN_DISK_WRITE_WINDOW.with(|owner| owner.set(false));
    }
}

/// Open a [`DiskWriteWindow`], blocking until any other open window closes.
fn lock_disk_write_window<'mutex>(
    mutex: &'mutex std::sync::Mutex<()>,
    owner: &std::cell::Cell<bool>,
) -> std::sync::MutexGuard<'mutex, ()> {
    assert!(
        !owner.get(),
        "this thread already owns the disk-write window; nesting it would \
         measure a delta the inner window cannot exclude writers from"
    );
    let guard = mutex.lock().expect(
        "the disk-write window mutex is poisoned: a counter-based test panicked \
         while holding the window, so no physical-write delta measured in this \
         process is trustworthy",
    );
    // Set ownership only after locking succeeds. In particular, a poisoned
    // mutex panics above without leaving this thread marked as an owner.
    owner.set(true);
    guard
}

pub(super) fn disk_write_window() -> DiskWriteWindow {
    let guard =
        IN_DISK_WRITE_WINDOW.with(|owner| lock_disk_write_window(&DISK_WRITE_WINDOW, owner));
    DiskWriteWindow { _guard: guard }
}

pub(super) fn fresh_for(seconds: u64) -> Freshness {
    Freshness {
        max_age: Some(seconds),
        ..Freshness::default()
    }
}

pub(super) const URL: &str = "https://example.test/pic.png";
pub(super) const BODY: &[u8] = b"\x89PNG\r\n\x1a\nfake-but-consistent-body";

#[cfg(unix)]
mod secure_root {
    use super::*;

    /// Positive private-root control through the real `open()` (the XDG path),
    /// not `open_at`: an ordinary user cache directory is provisioned, caching is
    /// enabled, and nothing is reported as unavailable.
    #[test]
    fn open_adopts_a_normal_private_xdg_root() {
        let _base =
            xdg_cache_root().expect("test host must resolve an XDG cache root for this control");
        let cache = HttpCache::open(HttpCacheConfig::DEFAULT);
        assert_eq!(
            cache.unavailable(),
            None,
            "a normal private XDG cache root must be adopted through the secure \
             boundary"
        );
        assert!(cache.is_enabled());
    }
}

// strict_entry_names: fail-closed seam and positive-control tests.

mod freshness;
mod partition;
mod race;
