use super::*;

fn queue_service(capacity: usize) -> (VideoService, std::sync::mpsc::Receiver<Command>) {
    let (commands, receiver) = std::sync::mpsc::sync_channel(capacity);
    (
        VideoService {
            commands,
            shared: Arc::new(Shared::default()),
            generation: Generation::FIRST,
        },
        receiver,
    )
}

#[test]
fn admission_backpressure_is_nonblocking_and_does_not_leak_receipts() {
    let (service, receiver) = queue_service(1);
    let first = service
        .request_seek(Duration::ZERO, SeekMode::Fast)
        .unwrap();
    for _ in 0..VIDEO_SEEK_CAPACITY + 1 {
        assert_eq!(
            service.request_seek(Duration::ZERO, SeekMode::Fast),
            Err(SeekAdmissionError::Full)
        );
    }
    assert_eq!(service.shared.lock().seeks.take().len(), 0);
    assert!(
        matches!(receiver.try_recv().unwrap(), Command::TrackedSeek(request) if request == first)
    );
    service.shared.seek_outcome(SeekOutcome {
        request: first,
        target: Duration::ZERO,
        actual: Some(Duration::ZERO),
        result: Ok(()),
    });
    assert_eq!(service.take_seek_outcomes().len(), 1);
    for _ in 0..VIDEO_SEEK_CAPACITY {
        let request = service
            .request_seek(Duration::ZERO, SeekMode::Fast)
            .unwrap();
        receiver.try_recv().unwrap();
        service.shared.seek_outcome(SeekOutcome {
            request,
            target: Duration::ZERO,
            actual: None,
            result: Err(SeekRejection::NotReady),
        });
    }
    assert_eq!(service.take_seek_outcomes().len(), VIDEO_SEEK_CAPACITY);
}

/// Settings never block on a full queue: the newest of each kind waits, and
/// while one waits a later one of its kind replaces it rather than slipping
/// into the queue ahead of it.
#[test]
fn settings_on_a_full_queue_keep_the_newest_without_blocking() {
    let (service, receiver) = queue_service(1);
    service.set_volume(0.1);
    service.set_volume(0.2);
    service.set_mute(true);
    service.set_volume(0.3);
    assert!(matches!(receiver.try_recv().unwrap(), Command::SetVolume(v) if v == 0.1));
    // The queue has room again, but a volume is waiting: the next one waits too.
    service.set_volume(0.4);
    assert!(
        receiver.try_recv().is_err(),
        "nothing overtook the waiting volume"
    );
    let waiting: Vec<String> = service
        .shared
        .lock()
        .settings
        .iter()
        .map(|command| format!("{command:?}"))
        .collect();
    assert_eq!(waiting, ["SetVolume(0.4)", "SetMute(true)"]);
}

/// The worker applies a waiting setting once it has drained the queue, so the
/// newest value is the one that lands.
#[test]
fn the_worker_applies_the_newest_waiting_setting() {
    let (service, receiver) = queue_service(1);
    service.set_volume(0.1);
    service.set_volume(0.5);
    let worker = super::super::worker::Worker::new(
        VideoServiceConfig::default(),
        Arc::clone(&service.shared),
        service.commands.clone(),
    );
    let running = std::thread::spawn(move || worker.run(&receiver));
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.shared.lock().status.volume != 0.5 {
        assert!(Instant::now() < deadline, "the waiting volume never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(service.shared.lock().settings.is_empty());
    service.commands.send(Command::Shutdown).unwrap();
    running.join().unwrap();
}
