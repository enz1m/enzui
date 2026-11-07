use super::*;

#[test]
fn reap_exact_child_returns_no_child_on_permanent_echild() {
    static REGISTRY: ReaperRegistry = ReaperRegistry::new();
    let fixture = ReaperFixture(&REGISTRY);
    let mut owner = reaper_sleep_owner(&REGISTRY, &CleanupSeam::default());
    owner.child_mut().kill().expect("terminate fixture");
    reaper_wait_until("fixture child must be reaped", || {
        owner.child_mut().try_wait().unwrap().is_some()
    });
    assert_eq!(
        reap_exact_child(owner.child_mut(), |_| Err(io::Error::from_raw_os_error(
            libc::ECHILD
        ))),
        Some(ReapOutcome::NoChild)
    );
    for errno in [libc::EINTR, libc::EIO, libc::EINVAL, libc::EAGAIN] {
        assert_eq!(
            reap_exact_child(owner.child_mut(), |_| Err(io::Error::from_raw_os_error(
                errno
            ))),
            None
        );
    }
    owner.child.take();
    drop(owner);
    fixture.shutdown_and_join();
}
