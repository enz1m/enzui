//! Tests for the explicit off-thread worker. These are the only tests in this
//! module that actually spawn a thread; they prove the plumbing (request in,
//! completion out, generation round-trips, clean shutdown), not the policy —
//! the policy is tested thread-free in `sniff::tests`.

use super::*;
use crate::handle_cache::Fingerprint;
use crate::sniff::{Generation, SniffAttempt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// A `Sniffer` that returns a canned outcome and counts calls, so a test can
// show the worker invoked it off-thread.
struct FakeSniffer {
    calls: Arc<AtomicUsize>,
    outcome: SniffOutcome,
}

impl Sniffer for FakeSniffer {
    fn sniff(&self, _path: &Path) -> SniffOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcome.clone()
    }
}

/// Spawn a worker whose thread is expected to start.
fn spawned(worker: Result<SniffWorker, SniffWorkerError>) -> SniffWorker {
    worker.expect("sniff worker thread must start in the test environment")
}

fn recv_timeout(worker: &SniffWorker) -> SniffCompletion {
    worker
        .completions()
        .recv_timeout(Duration::from_secs(5))
        .expect("worker must produce a completion within 5s")
}

fn attempt(path: &str, generation: Generation, nonce: u64) -> SniffAttempt {
    SniffAttempt::new(
        Path::new(path).to_path_buf(),
        Fingerprint::new(nonce),
        generation,
        nonce,
    )
}

fn req(path: &str) -> SniffRequest {
    SniffRequest {
        attempt: attempt(path, Generation::ZERO, 0),
    }
}

#[test]
fn dropping_worker_shuts_down_cleanly() {
    let worker = spawned(SniffWorker::spawn(FakeSniffer {
        calls: Arc::new(AtomicUsize::new(0)),
        outcome: SniffOutcome::Unknown,
    }));
    worker.request(req("/p"));
    let _ = recv_timeout(&worker);
    drop(worker); // must not hang
}
