//! The keep-alive share that holds a borrowed foreign display open.
//!
//! [`WaylandClipboard::adopt_foreign_display`] borrows a `wl_display` the
//! toolkit owns, and dispatches on it from a worker thread that teardown is
//! allowed to detach rather than wait for. A lease is the toolkit handle whose
//! liveness keeps that display connected, held as a share so the display is
//! released only once every thread that dispatches on it has finished.
//!
//! # Hazards
//!
//! * A lease keeps the display open; it never closes one. Dropping the last
//!   share only lets the toolkit's own teardown proceed.
//! * A worker that never exits keeps its share forever, which trades a held
//!   connection for the undefined behaviour of using a closed display.
//!
//! [`WaylandClipboard::adopt_foreign_display`]: crate::WaylandClipboard::adopt_foreign_display

use std::any::Any;
use std::sync::Arc;

/// A share of the handle that keeps an adopted foreign display alive.
///
/// Cloning takes another share; the display's owner is released when the last
/// share is dropped. The payload type is erased because this crate must not
/// know which windowing library the display came from.
#[derive(Clone)]
pub struct DisplayLease {
    holder: Arc<dyn Any + Send + Sync>,
}

impl DisplayLease {
    /// Takes a lease on the display `holder` keeps alive.
    ///
    /// `holder` is never read, borrowed or downcast: a lease is a lifetime
    /// token, not a type-erased value store. No display identity is verified:
    /// the caller must supply the owner of the display being adopted.
    pub fn new<H>(holder: H) -> Self
    where
        H: Send + Sync + 'static,
    {
        Self {
            holder: Arc::new(holder),
        }
    }

    fn shares(&self) -> usize {
        Arc::strong_count(&self.holder)
    }
}

impl std::fmt::Debug for DisplayLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayLease")
            .field("shares", &self.shares())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const HOLDER_BUDGET: Duration = Duration::from_secs(5);

    fn join_within(worker: std::thread::JoinHandle<()>, budget: Duration) -> bool {
        let started = Instant::now();
        while !worker.is_finished() {
            if started.elapsed() >= budget {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        worker.join().is_ok()
    }

    struct Holder {
        released: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Drop for Holder {
        fn drop(&mut self) {
            self.released
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn holder() -> (DisplayLease, Arc<std::sync::atomic::AtomicBool>) {
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lease = DisplayLease::new(Holder {
            released: Arc::clone(&released),
        });
        (lease, released)
    }

    #[test]
    fn a_share_moved_to_another_thread_outlives_its_origin() {
        let (lease, released) = holder();
        let moved = lease.clone();
        let (parked_tx, parked) = std::sync::mpsc::channel();
        let (release_tx, release) = std::sync::mpsc::channel::<()>();

        let worker = std::thread::Builder::new()
            .name(String::from("lease-share-holder"))
            .spawn(move || {
                let _ = parked_tx.send(());
                let _ = release.recv_timeout(HOLDER_BUDGET);
                drop(moved);
            })
            .expect("the share holder thread spawns");

        parked
            .recv_timeout(HOLDER_BUDGET)
            .expect("the share holder thread parks with its share");
        drop(lease);
        assert!(
            !released.load(std::sync::atomic::Ordering::Acquire),
            "dropping the origin share must not release a display another \
             thread still holds",
        );

        let _ = release_tx.send(());
        assert!(
            join_within(worker, HOLDER_BUDGET),
            "the share holder thread did not finish inside its bounded wait",
        );
        assert!(
            released.load(std::sync::atomic::Ordering::Acquire),
            "the display must be released once the last thread drops its share",
        );
    }
}
