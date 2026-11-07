//! What the importers in this process hold and do, for diagnostics and tests.

use std::sync::atomic::{AtomicU64, Ordering};

static IMPORTS_MADE: AtomicU64 = AtomicU64::new(0);
static IMPORTS_RELEASED: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static BINDINGS_MADE: AtomicU64 = AtomicU64::new(0);
static UNIFORM_WRITES: AtomicU64 = AtomicU64::new(0);
static UNUSED_BUDGET: AtomicU64 = AtomicU64::new(crate::UNUSED_IMPORT_BUDGET);

/// Counters across every renderer in the process; see [`stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// DMA-BUF imports made. A producer cycling a pool of buffers sees this
    /// rise once per buffer, not once per frame.
    pub imports_made: u64,
    /// Imports released, each once the GPU had finished sampling it.
    pub imports_released: u64,
    /// Imports alive now: made and not yet released.
    pub live_imports: u64,
    /// The memory the live imports hold, as the device sized it.
    pub live_bytes: u64,
    /// Bindings made between a widget and an imported texture. A widget
    /// showing the same allocation again reuses its binding.
    pub bindings_made: u64,
    /// Writes of a widget's placement and opacity to the GPU. A widget drawn
    /// again unchanged writes nothing.
    pub uniform_writes: u64,
}

/// The counters now.
pub fn stats() -> Stats {
    let made = IMPORTS_MADE.load(Ordering::Relaxed);
    let released = IMPORTS_RELEASED.load(Ordering::Relaxed);
    Stats {
        imports_made: made,
        imports_released: released,
        live_imports: made.saturating_sub(released),
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
        bindings_made: BINDINGS_MADE.load(Ordering::Relaxed),
        uniform_writes: UNIFORM_WRITES.load(Ordering::Relaxed),
    }
}

/// Sets how many bytes of imports no widget shows each renderer keeps for
/// reuse; [`UNUSED_IMPORT_BUDGET`](crate::UNUSED_IMPORT_BUDGET) by default.
/// Past it, the least recently shown are released.
pub fn set_unused_import_budget(bytes: u64) {
    UNUSED_BUDGET.store(bytes, Ordering::Relaxed);
}

pub(crate) fn unused_import_budget() -> u64 {
    UNUSED_BUDGET.load(Ordering::Relaxed)
}

pub(crate) fn import_made(bytes: u64) {
    let _ = IMPORTS_MADE.fetch_add(1, Ordering::Relaxed);
    let _ = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

pub(crate) fn import_released(bytes: u64) {
    let _ = IMPORTS_RELEASED.fetch_add(1, Ordering::Relaxed);
    let _ = LIVE_BYTES.fetch_sub(bytes, Ordering::Relaxed);
}

pub(crate) fn binding_made() {
    let _ = BINDINGS_MADE.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn uniforms_written() {
    let _ = UNIFORM_WRITES.fetch_add(1, Ordering::Relaxed);
}
