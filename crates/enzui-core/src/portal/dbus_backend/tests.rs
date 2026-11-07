use super::*;

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<DbusBackend>();
};

fn all_processed_closes(log: &DrainLog) -> Vec<RequestId> {
    let mut all = log.wire_attempts.clone();
    all.extend_from_slice(&log.receipts_only);
    all
}

// Use a nonexistent private bus address so the fixture cannot contact the ambient portal.
fn drain_test_backend() -> DbusBackend {
    static DRAIN_CTR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = DRAIN_CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let addr = format!(
        "unix:path=/nonexistent/enzui-portal-drain-{}-{n}",
        std::process::id()
    );
    let mut b = DbusBackend::with_address(Some(addr)).expect("backend with private address");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while b.probe_availability() == Availability::Initialising {
        assert!(
            std::time::Instant::now() < deadline,
            "the nonexistent-address probe must settle fast"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    {
        let mut s = lock_shared(&b.shared);
        s.available = true;
        s.unique_name = Some(":1.999".into());
        s.portal_present = Some(true);
        s.portal_owner = Some(":1.1".to_owned());
        s.unavailable_reason = None;
    }
    b.probed = true;
    b
}

pub(super) fn wait_drain_log<F: Fn(&DrainLog) -> bool>(
    drain_log: &std::sync::Arc<(std::sync::Mutex<DrainLog>, std::sync::Condvar)>,
    predicate: F,
    timeout: std::time::Duration,
) -> DrainLog {
    let (lock, cvar) = &**drain_log;
    let deadline = std::time::Instant::now() + timeout;
    let mut log = lock.lock().unwrap();
    while !predicate(&log) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let (guard, _) = cvar.wait_timeout(log, remaining).unwrap();
        log = guard;
    }
    DrainLog {
        wire_attempts: log.wire_attempts.clone(),
        receipts_only: log.receipts_only.clone(),
        worker_exited: log.worker_exited,
        response_dropped: log.response_dropped.clone(),
        delayed_inserts: log.delayed_inserts.clone(),
    }
}

#[test]
fn no_close_after_consumed_reply() {
    let mut b = drain_test_backend();
    let drain_log = std::sync::Arc::clone(&lock_shared(&b.shared).drain_log);

    let id = b.start(PortalOperation::OpenFile).unwrap();
    {
        let mut s = lock_shared(&b.shared);
        s.replies.insert(
            id,
            BackendReply {
                response: PortalResponse::Success,
                uris: vec![],
            },
        );
    }
    assert!(b.poll_reply(id).is_some(), "reply must be delivered");
    assert!(
        !b.in_flight.contains(&id),
        "consuming the terminal reply must retire the id",
    );

    b.close(id, CancelReason::Application);

    let shutdown_owner = b.shutdown_owner.take();
    drop(b);

    let log = wait_drain_log(
        &drain_log,
        |l| l.worker_exited,
        std::time::Duration::from_secs(10),
    );
    assert!(log.worker_exited);
    assert!(
        !all_processed_closes(&log).contains(&id),
        "a close() after a consumed terminal reply must emit no Close during \
         drain; drain log: {log_closes:?}",
        log_closes = all_processed_closes(&log),
    );
    if let Some(h) = shutdown_owner {
        h.join().expect("owner thread must not panic");
    }
}

#[test]
fn exported_actions_admission_does_not_require_callback_stream() {
    use crate::portal::notification::{
        NotificationAction, NotificationActionId, NotificationButton, NotificationExportedAction,
        NotificationId, NotificationInlineReply,
    };

    let mut backend = drain_test_backend();
    lock_shared(&backend.shared).notification_action_stream_ready = Some(false);
    let base = || DesktopNotification::new(NotificationId::new("admission").unwrap(), "Admission");
    for notification in [
        base().default_action(NotificationExportedAction::new("open").unwrap()),
        base().button(NotificationButton::new(
            "Open",
            NotificationExportedAction::new("open").unwrap(),
        )),
        base().inline_reply(NotificationInlineReply::new(
            NotificationExportedAction::new("reply").unwrap(),
            "Write",
            "Send",
        )),
    ] {
        assert!(
            backend
                .submit_notification(NotificationBackendCommand::Send(Box::new(notification)))
                .is_ok()
        );
    }
    let callback = || NotificationAction::new(NotificationActionId::new("open").unwrap());
    for notification in [
        base().default_action(callback()),
        base().button(NotificationButton::new("Open", callback())),
        base().inline_reply(NotificationInlineReply::new(callback(), "Write", "Send")),
    ] {
        assert_eq!(
            backend.submit_notification(NotificationBackendCommand::Send(Box::new(notification))),
            Err(NotificationBackendError::Unsupported(
                UnsupportedNotificationFeature::ActionDelivery
            ))
        );
    }
}
