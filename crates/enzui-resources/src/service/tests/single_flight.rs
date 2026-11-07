#[test]
fn cancellation_only_aborts_transport_after_final_subscriber_leaves() {
    let calls = Arc::new(AtomicUsize::new(0));
    let aborted = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let mut service = pool(ImageService::new(
        ControlledTransport {
            calls: Arc::clone(&calls),
            aborted: Arc::clone(&aborted),
            gate,
        },
        config(),
    ));
    let g = Generation::FIRST.next();
    let a = service
        .request("https://example.test/slow.png", g)
        .unwrap()
        .subscriber()
        .expect("an admitted request has a subscriber");
    let b = service
        .request("https://example.test/slow.png", g)
        .unwrap()
        .subscriber()
        .expect("an admitted request has a subscriber");
    wait_until(|| calls.load(Ordering::SeqCst) == 1);

    assert!(service.cancel(a, CancelCause::Cancelled));
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(aborted.load(Ordering::SeqCst), 0, "subscriber remains");

    assert!(service.cancel(b, CancelCause::Cancelled));
    wait_until(|| aborted.load(Ordering::SeqCst) == 1);
    assert_eq!(
        aborted.load(Ordering::SeqCst),
        1,
        "transport itself observed the cancellation exactly once"
    );
    let events = service.poll();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ServiceEvent::Cancelled { .. }))
            .count(),
        2
    );
}
