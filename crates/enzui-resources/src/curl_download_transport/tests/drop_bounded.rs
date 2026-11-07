//! Bounded-join teardown for the curl download transport.

use super::*;

// Absorbs host scheduling noise while staying an order of magnitude under the
// injected stall, so only a detach satisfies it.
const DROP_SLACK: Duration = Duration::from_millis(500);

// Announces a `Content-Length` it never satisfies, so curl ends its perform
// loop promptly and only `worker_exit_delay` keeps the thread alive.
fn header_only_server() -> (String, std::thread::JoinHandle<()>) {
    one_shot_server(|mut stream| {
        let _ = drain_request(&mut stream);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n");
        let _ = stream.flush();
    })
}

#[test]
fn bounded_drop_curl_detaches_a_worker_still_alive_past_the_budget() {
    let (url, server) = header_only_server();
    let stall = Duration::from_secs(2);
    let worker_exited = Arc::new(AtomicBool::new(false));
    let transport = CurlDownloadTransport::with_lifecycle_test_hooks(
        lifecycle_stall_deadlines(),
        None,
        Some(Arc::clone(&worker_exited)),
        stall,
    );
    assert_eq!(
        transport
            .start(&url, None, &CancelSignal::new())
            .unwrap()
            .status,
        200,
        "the fixture must reach headers, or the stall under test never starts"
    );

    let started = Instant::now();
    drop(transport);
    let elapsed = started.elapsed();

    assert!(
        elapsed < DROP_JOIN_BUDGET + DROP_SLACK,
        "Drop must not wait out the {stall:?} stall; took {elapsed:?} against \
         a {DROP_JOIN_BUDGET:?} budget"
    );
    assert!(
        !worker_exited.load(Ordering::Acquire),
        "the worker exited before Drop returned, so the stall was joined \
         rather than detached at the budget"
    );

    let settle = Instant::now() + stall + Duration::from_secs(1);
    while !worker_exited.load(Ordering::Acquire) && Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        worker_exited.load(Ordering::Acquire),
        "the detached worker never exited on its own"
    );

    server.join().unwrap();
}
