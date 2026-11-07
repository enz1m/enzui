//! Bounded resolver work and redirect/resume policy tests.

use super::*;

/// A resolver that never answers, plus the rendezvous that proves the waiter
/// was inside it.
struct StalledResolver {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::SyncSender<()>,
    lookup: Arc<DownloadLookup>,
}

fn stalled_resolver() -> StalledResolver {
    let (entered_tx, entered) = std::sync::mpsc::sync_channel::<()>(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let entered_tx = std::sync::Mutex::new(entered_tx);
    let release_rx = std::sync::Mutex::new(release_rx);
    StalledResolver {
        entered,
        release,
        lookup: Arc::new(move |_: &str, _: u16| {
            let _ = entered_tx.lock().unwrap().send(());
            // Held until the test releases it; a resolver the OS is not going
            // to answer for behaves exactly like this.
            let _ = release_rx.lock().unwrap().recv();
            Ok(vec!["203.0.113.7".parse().unwrap()])
        }),
    }
}

/// Start the transfer on its own thread so a *missing* bound is a failed
/// assertion rather than a hung test suite.
fn start_off_thread(
    transport: CurlDownloadTransport,
    url: String,
    cancel: CancelSignal,
) -> (
    std::sync::mpsc::Receiver<Result<TransportResponseMeta, TransportError>>,
    std::thread::JoinHandle<()>,
) {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let handle = std::thread::spawn(move || {
        let outcome = transport.start(&url, None, &cancel);
        let _ = tx.send(outcome);
    });
    (rx, handle)
}

/// Cancellation reaches a transfer parked in name resolution, and `start`
/// returns without waiting for the resolver to be released.
#[test]
fn cancellation_returns_from_a_stalled_resolver_without_waiting_for_it() {
    let resolver = stalled_resolver();
    let transport = CurlDownloadTransport::with_policy_and_deadlines(
        CurlDownloadTransport::fixture_policy(),
        lifecycle_stall_deadlines(),
    )
    .with_shared_lookup(Arc::clone(&resolver.lookup));
    let cancel = CancelSignal::new();
    let (outcome, handle) = start_off_thread(
        transport,
        "http://stalled.invalid/file".to_owned(),
        cancel.clone(),
    );

    resolver
        .entered
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the resolver must have been entered, or this proves nothing");
    cancel.cancel();
    let result = outcome
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("a cancelled start must return while the resolver is still stalled");
    assert_eq!(
        result.err(),
        Some(TransportError::Cancelled),
        "the typed cancellation must survive the resolver seam"
    );

    // Only now is the resolver allowed to finish, so nothing above could have
    // been satisfied by its answer.
    let _ = resolver.release.send(());
    handle.join().expect("the start thread must not panic");
}
