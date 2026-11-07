use super::*;
use std::time::Instant;

const WAIT: Duration = Duration::from_secs(3);

fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "task condition did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct FutureDrop(Arc<AtomicUsize>);
impl Drop for FutureDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
fn task_host_capacity_counts_running_streams_and_shutdown_drops_their_futures() {
    assert_eq!(TASK_CAPACITY, 64);
    let host = TaskHost::<u32>::spawn().unwrap();
    let started = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    for _ in 0..TASK_CAPACITY {
        let began = Arc::clone(&started);
        let guard = FutureDrop(Arc::clone(&dropped));
        host.submit(Task::perform(
            async move {
                let _guard = guard;
                began.fetch_add(1, Ordering::AcqRel);
                std::future::pending::<u32>().await
            },
            |value| value,
        ))
        .unwrap();
    }
    until(|| started.load(Ordering::Acquire) == TASK_CAPACITY);
    assert_eq!(
        host.commands.capacity(),
        TASK_CAPACITY,
        "command queue must be empty; running streams fill the limit"
    );
    assert_eq!(host.submit(Task::done(1)), Err(TaskHostError::Full));
    assert!(host.submit(Task::none()).is_ok());
    let task_count = Arc::clone(&host.tasks);
    let began = Instant::now();
    drop(host);
    assert!(
        began.elapsed() < Duration::from_secs(1),
        "cooperative shutdown did not join promptly"
    );
    assert_eq!(dropped.load(Ordering::Acquire), TASK_CAPACITY);
    assert_eq!(task_count.load(Ordering::Acquire), 0);
}
