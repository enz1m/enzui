use super::*;

fn id(value: &str) -> NotificationId {
    NotificationId::new(value).unwrap()
}

fn client() -> DesktopNotificationClient<MockNotificationBackend> {
    DesktopNotificationClient::new(
        AppId::new("org.enzui.Test").unwrap(),
        MockNotificationBackend::new(),
    )
}

#[test]
fn invalid_payload_never_reaches_the_backend() {
    let mut client = client();
    let invalid = DesktopNotification::new(id("bad"), "").display_hints([
        NotificationDisplayHint::Transient,
        NotificationDisplayHint::Tray,
    ]);
    assert_eq!(
        client.send(invalid),
        Err(NotificationError::InvalidPayload(
            NotificationValidationError::InvalidTitle
        ))
    );
    assert!(client.backend().submitted().is_empty());

    let conflicting = DesktopNotification::new(id("bad"), "Title").display_hints([
        NotificationDisplayHint::Transient,
        NotificationDisplayHint::Tray,
    ]);
    assert_eq!(
        client.send(conflicting),
        Err(NotificationError::InvalidPayload(
            NotificationValidationError::ConflictingDisplayHints
        ))
    );
    assert!(client.backend().submitted().is_empty());
}

/// Sends `notice` and completes it, so it is the current, shown generation.
fn shown(client: &mut DesktopNotificationClient<MockNotificationBackend>, notice: &str) {
    let _ = client
        .send(DesktopNotification::new(id(notice), "Title"))
        .unwrap();
    let operation = client.backend().submitted().back().unwrap().0;
    client
        .backend_mut()
        .complete(operation, NotificationBackendOutcome::Sent);
    assert!(matches!(
        client.poll_event(),
        Some(NotificationEvent::Operation { .. })
    ));
}

#[test]
fn a_closure_releases_its_registration_unless_stale_or_racing_a_replacement() {
    let mut client = client();
    shown(&mut client, "a");
    shown(&mut client, "b");

    // An older owner's closure says nothing about what shows now.
    client
        .backend_mut()
        .close(&id("a"), 0, NotificationCloseReason::Dismissed);
    assert_eq!(client.poll_event(), None);
    assert!(client.current_generation(&id("a")).is_some());

    // A closure racing a replacement does not retire the newer generation.
    let _ = client
        .send(DesktopNotification::new(id("b"), "Newer"))
        .unwrap();
    client
        .backend_mut()
        .close(&id("b"), 1, NotificationCloseReason::Expired);
    assert_eq!(client.poll_event(), None);
    assert!(client.current_generation(&id("b")).is_some());

    // A real closure releases the slot and wakes the reducer.
    let generation = client.current_generation(&id("a")).unwrap();
    client
        .backend_mut()
        .close(&id("a"), 1, NotificationCloseReason::Dismissed);
    assert_eq!(
        client.poll_event(),
        Some(NotificationEvent::Closed {
            notification_id: id("a"),
            generation,
            reason: NotificationCloseReason::Dismissed,
        })
    );
    assert_eq!(client.current_generation(&id("a")), None);
}
