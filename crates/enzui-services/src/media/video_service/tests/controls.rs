use super::*;

#[test]
fn queued_controls_do_not_block_and_drain_before_shutdown() {
    let (commands, receiver) = std::sync::mpsc::sync_channel(1);
    let service = VideoService {
        commands,
        shared: Arc::new(Shared::default()),
        generation: Generation::FIRST,
    };
    let (sent, done) = std::sync::mpsc::channel();
    let sender = std::thread::spawn(move || {
        service.pause();
        service.play();
        service.select_audio(1);
        sent.send(service).unwrap();
    });
    let service = done
        .recv_timeout(Duration::from_secs(5))
        .expect("a full command queue blocked the caller");
    sender.join().unwrap();
    let worker = super::super::worker::Worker::new(
        VideoServiceConfig::default(),
        Arc::clone(&service.shared),
        service.commands.clone(),
    );
    let running = std::thread::spawn(move || worker.run(&receiver));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !service.shared.lock().controls.is_empty() {
        assert!(Instant::now() < deadline, "waiting controls never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(service.shutdown(Duration::from_secs(5)));
    running.join().unwrap();
}
