use super::private_bus::PrivateBus;
use enzui_core::app_data::AppId;
use enzui_core::portal::dbus_backend::DbusBackend;
use enzui_core::portal::notification::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::zvariant::{OwnedValue, Str, Value};

type Client = DesktopNotificationClient<DbusBackend>;
type Posted = Arc<Mutex<Vec<(String, HashMap<String, OwnedValue>)>>>;
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Notification";

struct Server {
    version: u32,
    categories: Vec<String>,
    purposes: Vec<String>,
    posted: Posted,
}

#[zbus::interface(name = "org.freedesktop.portal.Notification")]
impl Server {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.version
    }

    #[zbus(property, name = "SupportedOptions")]
    fn supported_options(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("category".into(), owned(self.categories.clone())),
            ("button-purpose".into(), owned(self.purposes.clone())),
        ])
    }

    fn add_notification(
        &self,
        _id: &str,
        notification: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        self.posted
            .lock()
            .unwrap()
            .push((header.sender().unwrap().to_string(), notification));
    }

    fn remove_notification(&self, _id: &str) {}
}

fn owned<'a>(value: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).unwrap()
}

fn text(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

pub(super) async fn event(client: &mut Client) -> NotificationEvent {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(event) = client.poll_event() {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("notification event deadline")
}

async fn setup(
    bus: &PrivateBus,
    version: u32,
    category: bool,
    purpose: bool,
) -> (zbus::Connection, Client, Posted) {
    let posted = Arc::new(Mutex::new(Vec::new()));
    let server = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at(
            PATH,
            Server {
                version,
                categories: if category {
                    vec!["im.received".into()]
                } else {
                    vec![]
                },
                purposes: if purpose {
                    vec!["im.reply-with-text".into()]
                } else {
                    vec![]
                },
                posted: Arc::clone(&posted),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut client = DesktopNotificationClient::new(
        AppId::new("org.enzui.PortalReplies").unwrap(),
        DbusBackend::with_address(Some(bus.address.clone())).unwrap(),
    );
    loop {
        if matches!(
            event(&mut client).await,
            NotificationEvent::ServiceChanged {
                available: true,
                ..
            }
        ) {
            break;
        }
    }
    (server, client, posted)
}

fn notice(id: &str, target: Option<NotificationActionTarget>) -> DesktopNotification {
    let mut action = NotificationAction::new(NotificationActionId::new("reply").unwrap());
    if let Some(target) = target {
        action = action.target(target);
    }
    DesktopNotification::new(NotificationId::new(id).unwrap(), "Message").inline_reply(
        NotificationInlineReply::new(action, "Écrire une réponse", "Envoyer"),
    )
}

pub(super) async fn outcome(client: &mut Client) -> NotificationOperationOutcome {
    match event(client).await {
        NotificationEvent::Operation { outcome, .. } => outcome,
        other => panic!("missing operation: {other:?}"),
    }
}

async fn emit(server: &zbus::Connection, destination: &str, id: &str, parameters: Vec<OwnedValue>) {
    tokio::time::timeout(
        Duration::from_secs(3),
        server.emit_signal(
            Some(destination),
            PATH,
            INTERFACE,
            "ActionInvoked",
            &(id, "reply", parameters),
        ),
    )
    .await
    .unwrap()
    .unwrap();
}

pub(super) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn portal_inline_reply_round_trip_preserves_target_platform_and_text() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let (server, mut client, posted) = setup(&bus, 2, true, true).await;
        for (index, target) in [
            None,
            Some(NotificationActionTarget::Text("thread-42".into())),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("reply-{index}");
            let notification = notice(&id, target.clone());
            let notification = if index == 1 {
                notification.category("im.received")
            } else {
                notification
            };
            client.send(notification).unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Sent
            );
            let destination = {
                let posted = posted.lock().unwrap();
                let (destination, payload) = posted.last().unwrap();
                assert_eq!(
                    <&str>::try_from(&payload["category"]).unwrap(),
                    "im.received"
                );
                let buttons = Vec::<HashMap<String, OwnedValue>>::try_from(
                    payload["buttons"].try_clone().unwrap(),
                )
                .unwrap();
                assert_eq!(buttons.len(), 1);
                assert_eq!(<&str>::try_from(&buttons[0]["label"]).unwrap(), "Envoyer");
                assert_eq!(<&str>::try_from(&buttons[0]["action"]).unwrap(), "reply");
                assert_eq!(
                    <&str>::try_from(&buttons[0]["purpose"]).unwrap(),
                    "im.reply-with-text"
                );
                if target.is_some() {
                    assert_eq!(
                        <&str>::try_from(&buttons[0]["target"]).unwrap(),
                        "thread-42"
                    );
                } else {
                    assert!(!buttons[0].contains_key("target"));
                }
                destination.clone()
            };
            let platform = || {
                owned(HashMap::from([
                    ("activation-token", text("wayland-token")),
                    ("desktop-startup-id", text("x11-token")),
                ]))
            };
            let prefix = || {
                if target.is_some() {
                    vec![text("thread-42")]
                } else {
                    vec![]
                }
            };
            for suffix in [
                vec![text("wrong-order"), platform()],
                vec![platform(), platform(), text("duplicate-platform")],
                vec![platform(), text("extra"), text("response")],
                vec![platform(), OwnedValue::from(42_u32)],
            ] {
                let mut parameters = prefix();
                parameters.extend(suffix);
                emit(&server, &destination, &id, parameters).await;
                assert!(matches!(
                    event(&mut client).await,
                    NotificationEvent::IgnoredAction { .. }
                ));
            }
            let mut parameters = prefix();
            parameters.extend([platform(), text("Oui, à bientôt")]);
            emit(&server, &destination, &id, parameters).await;
            let NotificationEvent::Action(invocation) = event(&mut client).await else {
                panic!("missing inline reply")
            };
            assert_eq!(invocation.target, target);
            assert_eq!(invocation.response.as_deref(), Some("Oui, à bientôt"));
            let NotificationActivationContext::PlatformData(context) =
                invocation.activation_context
            else {
                panic!("missing platform data")
            };
            assert_eq!(context.xdg_activation_token(), Some("wayland-token"));
            assert_eq!(context.desktop_startup_id(), Some("x11-token"));
        }
    });
}

#[test]
fn portal_inline_reply_requires_both_capabilities_and_matching_category() {
    for (version, category, purpose) in [
        (1, true, true),
        (2, false, true),
        (2, true, false),
        (2, true, true),
    ] {
        let bus = PrivateBus::spawn();
        runtime().block_on(async {
            let (_server, mut client, posted) = setup(&bus, version, category, purpose).await;
            let mut notification = notice("unsupported", None);
            if version == 2 && category && purpose {
                notification = notification.category("alarm.ringing");
            }
            client.send(notification).unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Unsupported(
                    UnsupportedNotificationFeature::InlineReply
                )
            );
            client
                .send(DesktopNotification::new(
                    NotificationId::new("basic").unwrap(),
                    "Basic",
                ))
                .unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Sent
            );
            assert_eq!(posted.lock().unwrap().len(), 1);
        });
    }
}

#[test]
fn portal_inline_reply_counts_toward_total_button_limit() {
    for count in [7, 8] {
        let mut client = DesktopNotificationClient::new(
            AppId::new("org.enzui.ReplyLimit").unwrap(),
            UnavailableNotificationBackend,
        );
        let mut notification = notice("limit", None);
        for index in 0..count {
            notification = notification.button(NotificationButton::new(
                "Button",
                NotificationAction::new(
                    NotificationActionId::new(format!("button-{index}")).unwrap(),
                ),
            ));
        }
        let result = client.send(notification);
        if count == 8 {
            assert!(matches!(
                result,
                Err(NotificationError::InvalidPayload(
                    NotificationValidationError::TooManyButtons
                ))
            ));
        } else {
            assert!(!matches!(result, Err(NotificationError::InvalidPayload(_))));
        }
    }
}
