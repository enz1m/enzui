/// Number of live OS threads belonging to this pool, read straight from the
/// kernel via each task's `comm` name.
fn live_pool_threads(prefix: &str) -> usize {
    let mut count = 0;
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        panic!("/proc/self/task must be readable on Linux");
    };
    for task in tasks.flatten() {
        if let Ok(comm) = std::fs::read_to_string(task.path().join("comm"))
            && comm.trim_end().starts_with(prefix)
        {
            count += 1;
        }
    }
    count
}

/// [`settle_pool_threads`] with an explicit bound.
fn settle_pool_threads_within(prefix: &str, bound: Duration) -> usize {
    let deadline = Instant::now() + bound;
    loop {
        let now = live_pool_threads(prefix);
        if now == 0 || Instant::now() >= deadline {
            return now;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait until `probe` returns `expected`, or the deadline expires; return the
/// final observation.
fn settle(mut probe: impl FnMut() -> usize, expected: usize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = probe();
        if now == expected || Instant::now() >= deadline {
            return now;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Absorbs host scheduling noise while staying far below the stall a missing
// deadline or detach would produce.
const SERVICE_DROP_SLACK: Duration = Duration::from_millis(500);

#[test]
fn bounded_drop_service_joins_idle_cooperative_workers() {
    const PREFIX: &str = "enzui-coop-";
    const WORKERS: usize = 3;

    let calls = Arc::new(AtomicUsize::new(0));
    let cfg = ServiceConfig {
        worker_threads: WORKERS,
        ..config()
    };
    let service = ImageService::new_with_spawn_gate_named(
        ImmediateTransport {
            calls: Arc::clone(&calls),
        },
        cfg,
        PREFIX,
        None,
        |_| Ok(()),
    )
    .expect("worker pool must start in the test environment");
    assert_eq!(
        settle(|| live_pool_threads(PREFIX), WORKERS),
        WORKERS,
        "calibration: cooperative pool must run its workers"
    );

    let started = Instant::now();
    drop(service);
    let elapsed = started.elapsed();
    // A joined worker's `/proc/self/task` entry briefly outlives `join`, so the
    // window must exceed that yet stay under a detached worker's stall.
    let cleared_within = settle_pool_threads_within(PREFIX, Duration::from_millis(250));

    assert!(
        elapsed < DROP_JOIN_BUDGET + SERVICE_DROP_SLACK,
        "Drop exceeded its own budget on the cooperative path: {elapsed:?}"
    );
    assert_eq!(
        cleared_within, 0,
        "cooperative workers must be joined by Drop, {cleared_within} still live"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "calibration: this pool must be idle, so no worker entered fetch()"
    );
}
