/// A private cache directory per test, so concurrent tests cannot see each
/// other's entries and a previous run cannot pre-warm this one.
struct CacheDir {
    path: PathBuf,
}

impl CacheDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "enzui-svc-cache-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("test cache dir");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for CacheDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn cache_config_at(label: &str) -> (CacheDir, HttpCacheConfig) {
    (
        CacheDir::new(label),
        HttpCacheConfig {
            max_bytes: 1 << 20,
            max_entry_bytes: 1 << 19,
        },
    )
}

/// Cancelling a running flight must dispatch a queued one in its place.
#[test]
fn cancelling_a_running_flight_dispatches_a_queued_one_in_its_place() {
    // One worker, so exactly one flight runs and the rest queue.
    let (mut service, gate) = blocked_service(16, 1 << 20);
    let g = Generation::FIRST.next();

    let running = service
        .request("https://example.test/occupies.png", g)
        .expect("valid url")
        .subscriber()
        .expect("admitted");
    wait_until(|| service.running_len() == 1);

    service
        .request("https://example.test/waiting.png", g)
        .expect("valid url");
    assert_eq!(
        service.queued_len(),
        1,
        "calibration: the second flight must be QUEUED behind the single worker"
    );

    // Cancel the running flight. Its worker is parked in the transport and will
    // observe the cancel, but the queued job must not have to wait for that.
    assert!(service.cancel(running, CancelCause::Cancelled));

    wait_until(|| {
        let _ = service.poll();
        service.queued_len() == 0
    });
    assert_eq!(
        service.queued_len(),
        0,
        "cancelling the running flight must let the queued one be dispatched; \
         otherwise the pool sits idle with work waiting and every queued \
         subscriber stalls forever"
    );

    release(&gate);
}
