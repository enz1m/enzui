#[cfg(target_os = "linux")]
mod linux_backend {

    use super::super::linux::LinuxInotifyBackend;

    use std::sync::mpsc;
    use std::time::Duration;

    const DEADLOCK_BOUND: Duration = Duration::from_secs(1);
    const WAKE_BOUND: Duration = Duration::from_millis(150);

    #[test]
    fn drop_wakes_idle_reader_and_joins_before_return() {
        let (backend, control) = LinuxInotifyBackend::new_observed().expect("backend");
        let probe = backend.shared_probe();
        control
            .ready
            .recv_timeout(DEADLOCK_BOUND)
            .expect("idle reader did not enter poll");

        let (started_tx, started_rx) = mpsc::channel();
        let (returned_tx, returned_rx) = mpsc::channel();
        let dropper = std::thread::spawn(move || {
            let _ = started_tx.send(());
            drop(backend);
            let _ = returned_tx.send(());
        });
        started_rx
            .recv_timeout(DEADLOCK_BOUND)
            .expect("dropper did not start");
        control
            .wake_seen
            .recv_timeout(WAKE_BOUND)
            .expect("idle reader was not woken before the poll timeout");
        assert!(
            matches!(returned_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "Drop returned before the reader was released"
        );
        control
            .release_exit
            .send(())
            .expect("release the reader from its exit hold");
        returned_rx
            .recv_timeout(DEADLOCK_BOUND)
            .expect("Drop did not return after the reader was released");
        dropper
            .join()
            .unwrap_or_else(|e| panic!("dropper panicked: {e:?}"));
        assert!(probe.is_released(), "shared state retained after Drop");
        assert!(
            control.joined_reader(),
            "an idle reader was detached instead of joined"
        );
    }
}
