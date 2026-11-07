use super::private_bus::PrivateBus;
use super::replies::{event, outcome, runtime};
use enzui_core::app_data::AppId;
use enzui_core::portal::dbus_backend::{DbusBackend, PortalIdentity};
use enzui_core::portal::notification::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::zvariant::{OwnedValue, Value};

const APP: &str = "org.enzui.ExportedPolicy";
const SERVICE: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
type Client = DesktopNotificationClient<DbusBackend>;

#[derive(Default)]
struct State {
    identities: HashMap<String, String>,
    calls: Vec<(String, String)>,
    payloads: Vec<HashMap<String, OwnedValue>>,
}
type Shared = Arc<Mutex<State>>;

#[derive(Clone, Copy)]
enum Registration {
    Allow,
    Deny,
    Hang,
}
struct Registry {
    state: Shared,
    policy: Registration,
}

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    async fn register(
        &self,
        id: &str,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let peer = header.sender().unwrap().to_string();
        self.state
            .lock()
            .unwrap()
            .calls
            .push(("register".into(), peer.clone()));
        if matches!(self.policy, Registration::Hang) {
            std::future::pending::<()>().await;
        }
        if !matches!(self.policy, Registration::Allow) {
            return Err(zbus::fdo::Error::AccessDenied("identity refused".into()));
        }
        let mut state = self.state.lock().unwrap();
        if state.identities.contains_key(&peer) {
            return Err(zbus::fdo::Error::Failed("duplicate registration".into()));
        }
        state.identities.insert(peer, id.into());
        Ok(())
    }
}

struct Notifications {
    state: Shared,
    require_identity: bool,
}

#[zbus::interface(name = "org.freedesktop.portal.Notification")]
impl Notifications {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    #[zbus(property, name = "SupportedOptions")]
    fn supported_options(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            (
                "category".into(),
                OwnedValue::try_from(Value::from(vec!["im.received"])).unwrap(),
            ),
            (
                "button-purpose".into(),
                OwnedValue::try_from(Value::from(vec!["im.reply-with-text"])).unwrap(),
            ),
        ])
    }

    fn add_notification(
        &self,
        _id: &str,
        payload: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let peer = header.sender().unwrap().to_string();
        let mut state = self.state.lock().unwrap();
        if self.require_identity && state.identities.get(&peer).map(String::as_str) != Some(APP) {
            return Err(zbus::fdo::Error::AccessDenied(
                "missing registered identity".into(),
            ));
        }
        state.calls.push(("add".into(), peer));
        state.payloads.push(payload);
        Ok(())
    }

    fn remove_notification(&self, _id: &str) {}
}

async fn serve(
    bus: &PrivateBus,
    state: &Shared,
    registry: Option<Registration>,
    require_identity: bool,
) -> zbus::Connection {
    let mut builder = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(
            PATH,
            Notifications {
                state: Arc::clone(state),
                require_identity,
            },
        )
        .unwrap();
    if let Some(policy) = registry {
        builder = builder
            .serve_at(
                PATH,
                Registry {
                    state: Arc::clone(state),
                    policy,
                },
            )
            .unwrap();
    }
    builder.build().await.unwrap()
}

fn client(bus: &PrivateBus, identity: Option<PortalIdentity>) -> Client {
    let backend = match identity {
        Some(identity) => DbusBackend::for_application_with_address(
            AppId::new(APP).unwrap(),
            identity,
            Some(bus.address.clone()),
        )
        .unwrap(),
        None => DbusBackend::with_address(Some(bus.address.clone())).unwrap(),
    };
    DesktopNotificationClient::new(AppId::new(APP).unwrap(), backend)
}

async fn ready(client: &mut Client) {
    loop {
        if matches!(
            event(client).await,
            NotificationEvent::ServiceChanged {
                available: true,
                ..
            }
        ) {
            break;
        }
    }
}

fn notice(id: &str) -> DesktopNotification {
    DesktopNotification::new(NotificationId::new(id).unwrap(), "Exported")
        .default_action(
            NotificationExportedAction::new("open")
                .unwrap()
                .target(NotificationActionTarget::Text("thread".into())),
        )
        .button(NotificationButton::new(
            "Archive",
            NotificationExportedAction::new("archive")
                .unwrap()
                .target(NotificationActionTarget::Unsigned(42)),
        ))
}

#[test]
fn exported_portal_registers_same_peer_once_per_owner_before_posting() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let first = Arc::new(Mutex::new(State::default()));
        let server = serve(&bus, &first, Some(Registration::Allow), true).await;
        let mut client = client(&bus, Some(PortalIdentity::Host));
        ready(&mut client).await;
        for id in ["one", "two"] {
            client.send(notice(id)).unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Sent
            );
        }
        server.release_name(SERVICE).await.unwrap();
        let second = Arc::new(Mutex::new(State::default()));
        let next = serve(&bus, &second, Some(Registration::Allow), true).await;
        ready(&mut client).await;
        for id in ["three", "four"] {
            client.send(notice(id)).unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Sent
            );
        }
        next.release_name(SERVICE).await.unwrap();
        next.request_name(SERVICE).await.unwrap();
        ready(&mut client).await;
        client.send(notice("five")).unwrap();
        assert_eq!(
            outcome(&mut client).await,
            NotificationOperationOutcome::Sent
        );
        next.release_name(SERVICE).await.unwrap();
        server.request_name(SERVICE).await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                assert_eq!(
                    first
                        .lock()
                        .unwrap()
                        .calls
                        .iter()
                        .filter(|call| call.0 == "register")
                        .count(),
                    1,
                    "a returning portal process must retain its connection registration"
                );
                if matches!(
                    client.poll_event(),
                    Some(NotificationEvent::ServiceChanged {
                        available: true,
                        ..
                    })
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        client.send(notice("six")).unwrap();
        assert_eq!(
            outcome(&mut client).await,
            NotificationOperationOutcome::Sent
        );
        let first = first.lock().unwrap();
        let second = second.lock().unwrap();
        for (state, additions) in [(&*first, 3), (&*second, 3)] {
            assert_eq!(state.calls.len(), additions + 1);
            assert_eq!(state.calls[0].0, "register");
            assert!(state.calls.iter().skip(1).all(|call| call.0 == "add"));
            let peer = &state.calls[0].1;
            assert!(state.calls.iter().all(|call| &call.1 == peer));
            assert_eq!(state.identities[peer], APP);
            let payload = &state.payloads[0];
            assert_eq!(
                <&str>::try_from(&payload["default-action"]).unwrap(),
                "app.open"
            );
            assert_eq!(
                <&str>::try_from(&payload["default-action-target"]).unwrap(),
                "thread"
            );
            let buttons = Vec::<HashMap<String, OwnedValue>>::try_from(
                payload["buttons"].try_clone().unwrap(),
            )
            .unwrap();
            assert_eq!(
                <&str>::try_from(&buttons[0]["action"]).unwrap(),
                "app.archive"
            );
            assert_eq!(
                u64::try_from(buttons[0]["target"].try_clone().unwrap()).unwrap(),
                42
            );
        }
        assert_eq!(first.calls[0].1, second.calls[0].1);
    });
}

#[test]
fn exported_portal_identity_refusal_missing_registry_and_timeout_fail_closed() {
    for registration in [None, Some(Registration::Deny), Some(Registration::Hang)] {
        let bus = PrivateBus::spawn();
        runtime().block_on(async {
            let state = Arc::new(Mutex::new(State::default()));
            let _server = serve(&bus, &state, registration, false).await;
            let mut bound = client(&bus, Some(PortalIdentity::Host));
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(4), event(&mut bound))
                    .await
                    .unwrap(),
                NotificationEvent::ServiceChanged {
                    available: false,
                    ..
                }
            ));
            assert!(matches!(
                bound.send(notice("blocked")),
                Err(NotificationError::Backend(
                    NotificationBackendError::Unavailable
                ))
            ));
            let mut legacy = client(&bus, None);
            ready(&mut legacy).await;
            legacy
                .send(DesktopNotification::new(
                    NotificationId::new("control").unwrap(),
                    "Control",
                ))
                .unwrap();
            assert_eq!(
                outcome(&mut legacy).await,
                NotificationOperationOutcome::Sent
            );
            let state = state.lock().unwrap();
            assert_eq!(state.payloads.len(), 1);
            assert_eq!(
                state
                    .calls
                    .iter()
                    .filter(|call| call.0 == "register")
                    .count(),
                usize::from(registration.is_some())
            );
        });
    }
}

#[test]
fn exported_portal_sandbox_identity_skips_host_registry_and_rejects_mismatch() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let state = Arc::new(Mutex::new(State::default()));
        let server = serve(&bus, &state, Some(Registration::Deny), false).await;
        let mut sandbox = client(&bus, Some(PortalIdentity::Sandbox));
        ready(&mut sandbox).await;
        sandbox.send(notice("sandbox")).unwrap();
        assert_eq!(
            outcome(&mut sandbox).await,
            NotificationOperationOutcome::Sent
        );
        sandbox
            .send(
                DesktopNotification::new(NotificationId::new("sandbox-reply").unwrap(), "Reply")
                    .inline_reply(NotificationInlineReply::new(
                        NotificationExportedAction::new("reply")
                            .unwrap()
                            .target(NotificationActionTarget::Text("thread".into())),
                        "Write",
                        "Send",
                    )),
            )
            .unwrap();
        assert_eq!(
            outcome(&mut sandbox).await,
            NotificationOperationOutcome::Sent
        );
        let peer = state.lock().unwrap().calls.last().unwrap().1.clone();
        for action in ["app.reply", "reply"] {
            let parameters = vec![
                OwnedValue::from(zbus::zvariant::Str::from("thread")),
                OwnedValue::try_from(Value::from(HashMap::<String, OwnedValue>::new())).unwrap(),
                OwnedValue::from(zbus::zvariant::Str::from("reply text")),
            ];
            server
                .emit_signal(
                    Some(peer.as_str()),
                    PATH,
                    "org.freedesktop.portal.Notification",
                    "ActionInvoked",
                    &("sandbox-reply", action, parameters),
                )
                .await
                .unwrap();
            assert!(matches!(
                event(&mut sandbox).await,
                NotificationEvent::IgnoredAction {
                    reason: IgnoredNotificationAction::UnknownAction,
                    ..
                }
            ));
        }
        let mut mismatched = DesktopNotificationClient::new(
            AppId::new("org.enzui.Wrong").unwrap(),
            DbusBackend::for_application_with_address(
                AppId::new(APP).unwrap(),
                PortalIdentity::Sandbox,
                Some(bus.address.clone()),
            )
            .unwrap(),
        );
        assert_eq!(
            mismatched.send(notice("mismatched")),
            Err(NotificationError::ApplicationIdentityMismatch)
        );
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), 2);
        assert!(state.calls.iter().all(|call| call.0 == "add"));
        let buttons = Vec::<HashMap<String, OwnedValue>>::try_from(
            state.payloads[1]["buttons"].try_clone().unwrap(),
        )
        .unwrap();
        assert_eq!(
            <&str>::try_from(&buttons[0]["action"]).unwrap(),
            "app.reply"
        );
        assert_eq!(
            <&str>::try_from(&buttons[0]["purpose"]).unwrap(),
            "im.reply-with-text"
        );
        assert_eq!(<&str>::try_from(&buttons[0]["target"]).unwrap(), "thread");
        assert_eq!(state.calls[0].0, "add");
    });
}

#[test]
fn exported_portal_unbound_connection_refuses_exports_and_keeps_callbacks() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let state = Arc::new(Mutex::new(State::default()));
        let _server = serve(&bus, &state, None, false).await;
        let mut client = client(&bus, None);
        ready(&mut client).await;
        client.send(notice("unbound")).unwrap();
        assert_eq!(
            outcome(&mut client).await,
            NotificationOperationOutcome::Unsupported(
                UnsupportedNotificationFeature::ExportedAction
            )
        );
        client
            .send(
                DesktopNotification::new(NotificationId::new("callback").unwrap(), "Callback")
                    .default_action(NotificationAction::new(
                        NotificationActionId::new("open").unwrap(),
                    )),
            )
            .unwrap();
        assert_eq!(
            outcome(&mut client).await,
            NotificationOperationOutcome::Sent
        );
        let state = state.lock().unwrap();
        assert_eq!(state.payloads.len(), 1);
        assert_eq!(
            <&str>::try_from(&state.payloads[0]["default-action"]).unwrap(),
            "open"
        );
    });
}

fn mock_client() -> DesktopNotificationClient<MockNotificationBackend> {
    DesktopNotificationClient::new(AppId::new(APP).unwrap(), MockNotificationBackend::new())
}

fn id(value: &str) -> NotificationId {
    NotificationId::new(value).unwrap()
}

#[test]
fn exported_actions_validate_bare_names_wire_bounds_and_targets() {
    for name in ["", "app.quit", "not valid", "a/b", "café", "line\nbreak"] {
        assert!(NotificationExportedAction::new(name).is_err());
    }
    assert!(NotificationExportedAction::new("a".repeat(124)).is_ok());
    assert!(NotificationExportedAction::new("a".repeat(125)).is_err());
    assert!(NotificationActionId::new("app.quit").is_err());
    assert!(NotificationActionId::new("local callback").is_ok());
    let mut client = mock_client();
    let result = client.send(
        DesktopNotification::new(id("oversized-target"), "Target").default_action(
            NotificationExportedAction::new("open")
                .unwrap()
                .target(NotificationActionTarget::Text("x".repeat(4097))),
        ),
    );
    assert!(matches!(
        result,
        Err(NotificationError::InvalidPayload(
            NotificationValidationError::ActionValueTooLarge
        ))
    ));
    assert!(client.backend().submitted().is_empty());
    client
        .send(
            DesktopNotification::new(id("valid-target"), "Target").default_action(
                NotificationExportedAction::new("open")
                    .unwrap()
                    .target(NotificationActionTarget::Text("thread".into())),
            ),
        )
        .unwrap();
    assert_eq!(client.backend().submitted().len(), 1);
}

#[test]
fn exported_actions_do_not_register_reducer_callbacks() {
    let mut client = mock_client();
    let receipt = client
        .send(
            DesktopNotification::new(id("exported"), "Exported")
                .default_action(NotificationExportedAction::new("open").unwrap()),
        )
        .unwrap();
    client
        .backend_mut()
        .complete(receipt.operation, NotificationBackendOutcome::Sent);
    assert!(matches!(
        client.poll_event(),
        Some(NotificationEvent::Operation {
            outcome: NotificationOperationOutcome::Sent,
            ..
        })
    ));
    client
        .backend_mut()
        .invoke("exported", "open", None, None, None);
    assert!(matches!(
        client.poll_event(),
        Some(NotificationEvent::IgnoredAction {
            reason: IgnoredNotificationAction::UnknownAction,
            ..
        })
    ));
    let receipt = client
        .send(
            DesktopNotification::new(id("callback"), "Callback").default_action(
                NotificationAction::new(NotificationActionId::new("open").unwrap()),
            ),
        )
        .unwrap();
    client
        .backend_mut()
        .complete(receipt.operation, NotificationBackendOutcome::Sent);
    assert!(matches!(
        client.poll_event(),
        Some(NotificationEvent::Operation {
            outcome: NotificationOperationOutcome::Sent,
            ..
        })
    ));
    client
        .backend_mut()
        .invoke("callback", "open", None, None, None);
    assert!(matches!(
        client.poll_event(),
        Some(NotificationEvent::Action(_))
    ));
}

#[test]
fn exported_actions_and_callbacks_have_distinct_wire_identities() {
    let mut client = mock_client();
    client
        .send(
            DesktopNotification::new(id("mixed"), "Mixed")
                .default_action(NotificationAction::new(
                    NotificationActionId::new("open").unwrap(),
                ))
                .button(NotificationButton::new(
                    "Open exported",
                    NotificationExportedAction::new("open").unwrap(),
                )),
        )
        .unwrap();
    assert_eq!(client.backend().submitted().len(), 1);
    let duplicate = DesktopNotification::new(id("duplicate"), "Duplicate")
        .default_action(NotificationExportedAction::new("open").unwrap())
        .button(NotificationButton::new(
            "Duplicate",
            NotificationExportedAction::new("open").unwrap(),
        ));
    assert_eq!(
        client.send(duplicate),
        Err(NotificationError::InvalidPayload(
            NotificationValidationError::DuplicateActionId
        ))
    );
    assert_eq!(client.backend().submitted().len(), 1);
}

#[test]
fn exported_portal_identity_owner_history_is_bounded() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let initial = Arc::new(Mutex::new(State::default()));
        let mut server = serve(&bus, &initial, Some(Registration::Allow), true).await;
        let mut bound = client(&bus, Some(PortalIdentity::Host));
        ready(&mut bound).await;
        let mut retired = Vec::new();
        for index in 1..64 {
            bound.send(notice(&format!("owner-{index}"))).unwrap();
            assert_eq!(
                outcome(&mut bound).await,
                NotificationOperationOutcome::Sent
            );
            server.release_name(SERVICE).await.unwrap();
            retired.push(server);
            let state = Arc::new(Mutex::new(State::default()));
            server = serve(&bus, &state, Some(Registration::Allow), true).await;
            ready(&mut bound).await;
            assert_eq!(state.lock().unwrap().identities.len(), 1);
        }
        bound.send(notice("owner-64")).unwrap();
        assert_eq!(
            outcome(&mut bound).await,
            NotificationOperationOutcome::Sent
        );
        server.release_name(SERVICE).await.unwrap();
        retired.push(server);
        let overflow = Arc::new(Mutex::new(State::default()));
        let overflow_server = serve(&bus, &overflow, Some(Registration::Allow), false).await;
        tokio::time::timeout(Duration::from_secs(4), async {
            while !enzui_core::portal::backend::PortalBackend::describe(bound.backend())
                .contains("owner history is full")
            {
                let _ = bound.poll_event();
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            bound.send(notice("overflow")),
            Err(NotificationError::Backend(
                NotificationBackendError::Unavailable
            ))
        ));
        let mut legacy = client(&bus, None);
        ready(&mut legacy).await;
        legacy
            .send(DesktopNotification::new(
                NotificationId::new("control").unwrap(),
                "Control",
            ))
            .unwrap();
        assert_eq!(
            outcome(&mut legacy).await,
            NotificationOperationOutcome::Sent
        );
        overflow_server.release_name(SERVICE).await.unwrap();
        retired[0].request_name(SERVICE).await.unwrap();
        ready(&mut bound).await;
        bound.send(notice("returning-known-owner")).unwrap();
        assert_eq!(
            outcome(&mut bound).await,
            NotificationOperationOutcome::Sent
        );
        assert_eq!(
            initial
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|call| call.0 == "register")
                .count(),
            1
        );
        let overflow = overflow.lock().unwrap();
        assert_eq!(overflow.calls.len(), 1);
        assert_eq!(overflow.calls[0].0, "add");
        assert!(overflow.identities.is_empty());
    });
}

#[test]
fn exported_portal_failed_owner_is_not_registered_twice() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let refused = Arc::new(Mutex::new(State::default()));
        let first = serve(&bus, &refused, Some(Registration::Deny), false).await;
        let mut bound = client(&bus, Some(PortalIdentity::Host));
        assert!(matches!(
            event(&mut bound).await,
            NotificationEvent::ServiceChanged {
                available: false,
                ..
            }
        ));
        first.release_name(SERVICE).await.unwrap();
        let accepted = Arc::new(Mutex::new(State::default()));
        let second = serve(&bus, &accepted, Some(Registration::Allow), true).await;
        ready(&mut bound).await;
        bound.send(notice("control-before")).unwrap();
        assert_eq!(
            outcome(&mut bound).await,
            NotificationOperationOutcome::Sent
        );
        second.release_name(SERVICE).await.unwrap();
        first.request_name(SERVICE).await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                assert_eq!(
                    refused.lock().unwrap().calls.len(),
                    1,
                    "a refused owner must not be registered again"
                );
                if enzui_core::portal::backend::PortalBackend::describe(bound.backend())
                    .contains("registration previously failed")
                {
                    break;
                }
                let _ = bound.poll_event();
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            bound.send(notice("refused-return")),
            Err(NotificationError::Backend(
                NotificationBackendError::Unavailable
            ))
        ));
        first.release_name(SERVICE).await.unwrap();
        second.request_name(SERVICE).await.unwrap();
        ready(&mut bound).await;
        bound.send(notice("control-after")).unwrap();
        assert_eq!(
            outcome(&mut bound).await,
            NotificationOperationOutcome::Sent
        );
        assert_eq!(refused.lock().unwrap().calls.len(), 1);
        assert_eq!(accepted.lock().unwrap().calls.len(), 3);
    });
}
