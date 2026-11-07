use super::*;

const WAIT: Duration = Duration::from_secs(2);

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.poll(&mut context)
}

fn await_with_timeout<F: Future>(future: F) -> F::Output {
    let deadline = Instant::now() + WAIT;
    let mut future = Box::pin(future);
    loop {
        if let Poll::Ready(output) = poll_once(future.as_mut()) {
            return output;
        }
        assert!(Instant::now() < deadline, "future did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn dispatcher_rejects_work_after_its_owner_is_dropped() {
    let owner = BlockingTaskOwner::new(
        "blocking-owner-drop",
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    )
    .unwrap();
    let dispatcher = owner.dispatcher();
    drop(owner);

    let refused = await_with_timeout(dispatcher.submit(64, |_| 1));
    assert_eq!(refused.generation(), 64);
    assert_eq!(refused.into_result(), Err(BlockingTaskError::Shutdown));
    let snapshot = dispatcher.snapshot();
    assert!(snapshot.shutdown);
    assert_eq!(snapshot.accepted, 0);
    assert_eq!(snapshot.detached_workers, 0);
    assert_eq!(snapshot.panicked_workers, 0);
}
