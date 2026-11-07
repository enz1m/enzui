use super::*;

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<DbusSessionBackend>();
};

fn new_test_backend() -> Result<DbusSessionBackend, RuntimeInitError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    DbusSessionBackend::with_address(Some(format!(
        "unix:path=/nonexistent/enzui-session-unit-{}-{n}",
        std::process::id()
    )))
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    fn assert_clone<T: Clone>() {}
    assert_send::<RemoteOpenCancel>();
    assert_sync::<RemoteOpenCancel>();
    assert_clone::<RemoteOpenCancel>();
};

#[test]
fn a_double_close_sends_the_wire_close_at_most_once() {
    let mut b = new_test_backend().unwrap();
    {
        let mut s = lock_session_shared(&b.shared);
        s.available = true;
        s.unique_name = Some(":1.999".into());
        s.portal_present = Some(true);
        s.portal_owner = Some(":1.42".to_owned());
    }
    b.probed = true;
    // Install the observed queue before creation so its reservation owns the close permit.
    let (tx, mut rx) = mpsc::channel::<SessionCommand>(SESSION_COMMAND_QUEUE_CAPACITY);
    b.cmd_tx = tx;
    let id = b.create_session().unwrap();
    let _ = rx.try_recv().expect("drain atomic Create");

    for _ in 0..5 {
        b.close_session(id, CancelReason::User);
    }

    let mut close_count = 0usize;
    while let Ok(cmd) = rx.try_recv() {
        if let SessionCommand::Close { id: got } = cmd
            && got == id
        {
            close_count += 1;
        }
    }
    assert_eq!(
        close_count, 1,
        "five close_session calls must yield exactly one wire Close"
    );
}
