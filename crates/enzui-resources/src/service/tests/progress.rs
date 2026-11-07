/// Cancellation removes each flight's registration atomically with its latest
/// value, so concurrent flights neither leak nor receive one another's bytes.
#[test]
fn cancellation_cleans_concurrent_progress_slots_without_crossing_flights() {
    struct ProgressUntilCancelled {
        entered: Arc<AtomicUsize>,
        post_cancel_callbacks: Arc<AtomicUsize>,
    }

    impl Transport for ProgressUntilCancelled {
        fn fetch(
            &self,
            request: &RemoteRequest,
            _policy: &UrlPolicy,
            _limits: &TransportLimits,
            cancel: &CancelSignal,
            progress: &mut dyn FnMut(TransferProgress),
        ) -> Result<TransportResponse, TransportError> {
            let value = if request.transport_url().ends_with("first.png") {
                101
            } else {
                202
            };
            progress(TransferProgress {
                received: value,
                total: Some(1_000),
            });
            self.entered.fetch_add(1, Ordering::Release);
            while cancel.cause().is_none() {
                std::thread::sleep(Duration::from_millis(1));
            }

            // A hostile or poorly behaved transport may report once more after
            // cancellation. The removed registration must make this a no-op.
            progress(TransferProgress {
                received: value + 10_000,
                total: Some(1_000),
            });
            self.post_cancel_callbacks.fetch_add(1, Ordering::Release);
            Err(TransportError::Cancelled(
                cancel.cause().expect("cancel observed"),
            ))
        }
    }

    let entered = Arc::new(AtomicUsize::new(0));
    let post_cancel_callbacks = Arc::new(AtomicUsize::new(0));
    let mut service = pool(ImageService::new(
        ProgressUntilCancelled {
            entered: Arc::clone(&entered),
            post_cancel_callbacks: Arc::clone(&post_cancel_callbacks),
        },
        ServiceConfig {
            worker_threads: 2,
            ..config()
        },
    ));
    let generation = Generation::FIRST.next();
    let first = service
        .request("https://example.test/first.png", generation)
        .expect("valid first url")
        .subscriber()
        .expect("first admitted");
    let second = service
        .request("https://example.test/second.png", generation)
        .expect("valid second url")
        .subscriber()
        .expect("second admitted");

    wait_until(|| entered.load(Ordering::Acquire) == 2);
    assert_eq!(service.progress_mailbox_sizes_for_test(), (2, 2));

    assert!(service.cancel(first, CancelCause::Cancelled));
    wait_until(|| post_cancel_callbacks.load(Ordering::Acquire) >= 1);
    assert_eq!(
        service.progress_mailbox_sizes_for_test(),
        (1, 1),
        "a callback after cancellation must not recreate the retired flight's slot"
    );

    let events = service.poll();
    assert!(events.iter().all(|event| {
        event_subscriber(event) != first || !matches!(event, ServiceEvent::Progress { .. })
    }));
    let second_values: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::Progress {
                subscriber,
                progress,
                ..
            } if *subscriber == second => Some(progress.received),
            _ => None,
        })
        .collect();
    assert_eq!(second_values, vec![202]);
    assert_eq!(service.progress_mailbox_sizes_for_test(), (1, 0));

    assert!(service.cancel(second, CancelCause::Cancelled));
    wait_until(|| post_cancel_callbacks.load(Ordering::Acquire) == 2);
    assert_eq!(service.progress_mailbox_sizes_for_test(), (0, 0));
    let remaining = wait_terminal(&mut service, 1);
    assert!(remaining.iter().all(|event| {
        event_subscriber(event) != second || !matches!(event, ServiceEvent::Progress { .. })
    }));
    assert_eq!(service.progress_mailbox_sizes_for_test(), (0, 0));
}
