//! Routed-page build counting and opt-in headless frame measurement.
//!
//! The `perf-counter` feature exposes the lightweight build counter used at a
//! production view seam. The `test-frame` feature adds the headless layout and
//! raster harness backed by `iced_test`.

use core::cell::Cell;

// Counts are thread-local so parallel application tests measure only their own
// view builds; `try_with` also keeps calls safe during thread-local teardown.
thread_local! {
    static PAGE_BUILDS: Cell<usize> = const { Cell::new(0) };
    static TOTAL_PAGE_BUILDS: Cell<u64> = const { Cell::new(0) };
}

/// Records one top-level routed-page build for the current thread.
pub fn record_page_build() {
    let _ = PAGE_BUILDS.try_with(|c| c.set(c.get() + 1));
    let _ = TOTAL_PAGE_BUILDS.try_with(|c| c.set(c.get().wrapping_add(1)));
}

/// Returns total routed-page builds on this thread without resetting between frames.
#[must_use]
pub fn total_page_builds() -> u64 {
    TOTAL_PAGE_BUILDS.try_with(Cell::get).unwrap_or(0)
}

/// The number of [`record_page_build`] calls since the last [`reset_page_builds`].
#[must_use]
pub fn page_builds() -> usize {
    PAGE_BUILDS.try_with(Cell::get).unwrap_or(0)
}

/// Resets the routed-page rebuild counter to zero.
pub fn reset_page_builds() {
    let _ = PAGE_BUILDS.try_with(|c| c.set(0));
}

#[cfg(feature = "test-frame")]
#[path = "perf_frame/headless.rs"]
mod headless;
#[cfg(feature = "test-frame")]
pub use headless::*;
