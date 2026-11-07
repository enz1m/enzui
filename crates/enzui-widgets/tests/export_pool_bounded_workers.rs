//! Export-pool occupancy, settlement, cancellation and worker lifetime.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

#[cfg(target_os = "linux")]
mod bounded {
    use std::sync::mpsc;

    use std::time::{Duration, Instant};

    use enzui_core::blocking_task::{BlockingTaskError, BlockingTaskSettlement};

    use enzui_widgets::export_pool::ExportPoolOwner;

    const WAIT: Duration = Duration::from_secs(2);

    fn poll_settlement<T>(
        task: enzui_core::blocking_task::BlockingTask<T>,
    ) -> BlockingTaskSettlement<T> {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Waker};

        let deadline = Instant::now() + WAIT;
        let mut pinned = Box::pin(task);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(s) = Pin::as_mut(&mut pinned).poll(&mut cx) {
                return s;
            }
            assert!(Instant::now() < deadline, "settlement did not arrive");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn owner_drop_with_held_work() {
        let owner = ExportPoolOwner::start().expect("thread creation");
        let dispatcher = owner.dispatcher();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let task = owner.submit(20, move |_ctx| {
            entered_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(WAIT);
        });
        entered_rx.recv_timeout(WAIT).expect("worker must start");

        drop(owner);

        let snap = dispatcher.snapshot();
        assert!(snap.shutdown, "dispatcher must reflect shutdown");
        assert_eq!(snap.detached_workers, 1);

        let s = poll_settlement(task);
        assert_eq!(s.generation(), 20);
        assert_eq!(s.into_result(), Err(BlockingTaskError::Shutdown));
        release_tx.send(()).unwrap();
    }
}
