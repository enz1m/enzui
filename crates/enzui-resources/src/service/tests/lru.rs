/// Decoded assets are 4 bytes each (1x1 RGBA), so this budget holds exactly two.
fn lru_service(cache_bytes: u64) -> ImageService {
    pool(ImageService::new(
        ImmediateTransport {
            calls: Arc::new(AtomicUsize::new(0)),
        },
        ServiceConfig {
            worker_threads: 1,
            decoded_cache_bytes: cache_bytes,
            ..config()
        },
    ))
}

/// Load `url` to completion so it lands in the decoded cache.
fn load(service: &mut ImageService, url: &str, g: Generation) {
    service.request(url, g).expect("valid url");
    let _ = wait_terminal(service, 1);
}

/// Re-inserting an existing key makes it most-recent and keeps byte accounting
/// exact, rather than double-counting or leaving a stale index slot.
#[test]
fn overwriting_a_key_makes_it_most_recent_and_keeps_bytes_exact() {
    const A: &str = "https://example.test/ov-a.png";
    const B: &str = "https://example.test/ov-b.png";
    const C: &str = "https://example.test/ov-c.png";
    let mut service = lru_service(8);
    let g = Generation::FIRST.next();

    load(&mut service, A, g);
    assert_eq!(
        service.decoded_cache_bytes(),
        4,
        "calibration: one 1x1 RGBA asset must account for exactly 4 bytes"
    );
    load(&mut service, B, g);
    assert_eq!(
        service.decoded_cache_bytes(),
        8,
        "two entries must account for exactly 8 bytes, not double-counted"
    );

    // Re-decode A by evicting it first is not possible here, so overwrite it
    // directly: a second completion for the same key replaces the entry.
    service.insert_cache_for_test(A, 4);
    assert_eq!(
        service.decoded_cache_bytes(),
        8,
        "overwriting a key must not grow the byte total: {} bytes",
        service.decoded_cache_bytes()
    );
    assert_eq!(service.decoded_cache_len(), 2, "and must not add an entry");

    // A is now most-recent, so inserting C must evict B.
    load(&mut service, C, g);
    assert!(
        matches!(
            service.request(A, g).expect("valid url"),
            RequestDisposition::CacheHit(_)
        ),
        "the overwritten key must be most-recent and survive"
    );
    let _ = service.poll();
    assert!(
        matches!(
            service.request(B, g).expect("valid url"),
            RequestDisposition::Started(_)
        ),
        "B must be the victim"
    );
}

/// Warm requests answer at once, so without polls they could queue events
/// without limit: past `max_pending_results` they are refused as busy, and a
/// poll makes room again.
#[test]
fn unpolled_cache_hits_stop_at_the_result_budget() {
    const A: &str = "https://example.test/warm.png";
    let mut service = pool(ImageService::new(
        ImmediateTransport {
            calls: Arc::new(AtomicUsize::new(0)),
        },
        ServiceConfig {
            worker_threads: 1,
            max_pending_results: 3,
            ..config()
        },
    ));
    let g = Generation::FIRST.next();
    load(&mut service, A, g);
    for _ in 0..3 {
        assert!(matches!(
            service.request(A, g).expect("valid url"),
            RequestDisposition::CacheHit(_)
        ));
    }
    assert_eq!(
        service.request(A, g).expect("valid url"),
        RequestDisposition::Busy(BusyReason::ResultBudget)
    );
    assert_eq!(service.poll().len(), 3);
    assert!(matches!(
        service.request(A, g).expect("valid url"),
        RequestDisposition::CacheHit(_)
    ));
}
