//! Keyed document-tab state, navigation, and viewport filtering.
//!
//! Keys keep selection and focus stable across reorder and removal. Pinned and session tabs
//! share one reconciliation domain, and unknown keys are rejected. [`VisibleTabs`] limits
//! session-tab work to viewport ranges while retaining every pinned tab.

pub use crate::semantics::Orientation;

mod item;
mod metrics;
mod model;
mod outcome;

pub use item::*;
pub use metrics::*;
pub use model::*;
pub(crate) use outcome::RepairOutcome;
pub use outcome::*;

/// Test instrumentation for counting key-candidate probes during reconciliation.
#[doc(hidden)]
pub mod reconcile_probes {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static ARMED: AtomicBool = AtomicBool::new(false);
    static COUNT: AtomicUsize = AtomicUsize::new(0);

    #[inline]
    pub(super) fn bump() {
        if ARMED.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Runs `f` while counting probes. Counting scopes must not overlap.
    #[doc(hidden)]
    pub fn counting<T>(f: impl FnOnce() -> T) -> (T, usize) {
        ARMED.store(true, Ordering::Relaxed);
        COUNT.store(0, Ordering::Relaxed);
        let out = f();
        let n = COUNT.load(Ordering::Relaxed);
        ARMED.store(false, Ordering::Relaxed);
        (out, n)
    }
}
