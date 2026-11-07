#[test]
fn cancel_signal_is_a_compatibility_view_of_the_core_token() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CancelSignal>();

    let token = CancelToken::new();
    let signal = CancelSignal {
        token: token.share(),
    };
    assert!(
        !signal.is_cancelled(),
        "positive control: both handles start live"
    );
    assert!(token.cancel(CancelCause::Disconnected));
    assert_eq!(signal.cause(), Some(CancelCause::Disconnected));
    assert!(signal.is_cancelled());
    assert!(
        !signal.cancel(CancelCause::Shutdown),
        "the compatibility handle must not own an independent state"
    );
}

/// A transport that blocks until released, so admitted work stays admitted.
fn blocked_service(capacity: usize, bytes: u64) -> (ImageService, Arc<(Mutex<bool>, Condvar)>) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let service = pool(ImageService::new(
        ControlledTransport {
            calls: Arc::new(AtomicUsize::new(0)),
            aborted: Arc::new(AtomicUsize::new(0)),
            gate: Arc::clone(&gate),
        },
        ServiceConfig {
            worker_threads: 1,
            max_inflight_jobs: capacity,
            max_inflight_request_bytes: bytes,
            ..config()
        },
    ));
    (service, gate)
}

fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cv) = &**gate;
    *lock.lock().unwrap() = true;
    cv.notify_all();
}


