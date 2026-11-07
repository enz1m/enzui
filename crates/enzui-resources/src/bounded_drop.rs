//! Bounded join used by this crate's `Drop` implementations.
//!
//! An unconditional join is only as bounded as the worker's slowest
//! uninterruptible call, so these helpers wait until a deadline and then
//! detach, keeping teardown bounded when a worker is parked in a syscall
//! that ignores the crate's cancellation tokens.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) const DROP_JOIN_BUDGET: Duration = Duration::from_millis(500);

pub(crate) const DROP_POLL_STEP: Duration = Duration::from_millis(1);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DropWait {
    Detach,
    Sleep(Duration),
}

pub(crate) fn drop_wait_step(
    now: Instant,
    deadline: Option<Instant>,
    poll_step: Duration,
) -> DropWait {
    match deadline {
        // A deadline that overflowed the monotonic clock bounds nothing, so
        // it detaches rather than falling through to an unbounded wait.
        None => DropWait::Detach,
        Some(d) if now >= d => DropWait::Detach,
        Some(d) => DropWait::Sleep(poll_step.min(d.duration_since(now))),
    }
}

// Detach is sound only where the worker owns all state it touches; every
// spawn site in this crate moves its `Arc` clones and channel ends in.
// Callers signal the wake-up first: this deadline bounds the waiter, never
// the worker.
pub(crate) fn bounded_drop_join(worker: JoinHandle<()>, deadline: Option<Instant>) {
    loop {
        if worker.is_finished() {
            // `is_finished` is monotonic, so this join cannot block; testing
            // it before the deadline joins a worker that finished late.
            let _ = worker.join();
            return;
        }
        match drop_wait_step(Instant::now(), deadline, DROP_POLL_STEP) {
            DropWait::Detach => {
                drop(worker);
                return;
            }
            DropWait::Sleep(dur) => std::thread::sleep(dur),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_drop_wait_step_detaches_when_the_deadline_is_past() {
        let now = Instant::now();
        let past = now - Duration::from_millis(1);
        assert_eq!(
            drop_wait_step(now, Some(past), DROP_POLL_STEP),
            DropWait::Detach,
            "a deadline in the past must not wait one more poll step",
        );
    }
}
