use super::activation_child::child_activation_environment;
use super::private_bus::PrivateBus;
use enzui_core::activation::ActivationBroker;
use enzui_core::app_data::AppId;
use enzui_core::portal::dbus_backend::DbusBackend;
use enzui_core::portal::notification::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedValue, Str, Value};

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Notification";
type Client = DesktopNotificationClient<DbusBackend>;
type Posted = Arc<Mutex<Vec<(String, String)>>>;

struct Server {
    posted: Posted,
}

#[zbus::interface(name = "org.freedesktop.portal.Notification")]
impl Server {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    #[zbus(property, name = "SupportedOptions")]
    fn supported_options(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }

    fn add_notification(
        &self,
        id: &str,
        _notification: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        self.posted
            .lock()
            .unwrap()
            .push((id.to_owned(), header.sender().unwrap().to_string()));
    }

    fn remove_notification(&self, _id: &str) {}
}

async fn event(client: &mut Client) -> NotificationEvent {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(event) = client.poll_event() {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("portal notification deadline")
}

async fn emit(
    connection: &zbus::Connection,
    destination: &str,
    id: &str,
    data: HashMap<String, OwnedValue>,
) {
    let platform = OwnedValue::try_from(Value::from(data)).unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        connection.emit_signal(
            Some(destination),
            PATH,
            INTERFACE,
            "ActionInvoked",
            &(id, "open", vec![platform]),
        ),
    )
    .await
    .unwrap()
    .unwrap();
}

fn field(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

#[test]
fn activation_portal_callbacks_retain_x11_wayland_and_combined_context() {
    let bus = PrivateBus::spawn();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let posted = Arc::new(Mutex::new(Vec::new()));
        let server = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .serve_at(
                PATH,
                Server {
                    posted: Arc::clone(&posted),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let impostor = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let mut client = DesktopNotificationClient::new(
            AppId::new("org.enzui.ActivationPortal").unwrap(),
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
        let mut broker = ActivationBroker::default();
        for (index, (data, expected)) in [
            (
                HashMap::from([("activation-token".into(), field("same-opaque-value"))]),
                "unset\nsame-opaque-value\n",
            ),
            (
                HashMap::from([("desktop-startup-id".into(), field("same-opaque-value"))]),
                "same-opaque-value\nunset\n",
            ),
            (
                HashMap::from([
                    ("activation-token".into(), field("wayland-field")),
                    ("desktop-startup-id".into(), field("x11-field")),
                ]),
                "x11-field\nwayland-field\n",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("platform-{index}");
            client
                .send(
                    DesktopNotification::new(NotificationId::new(&id).unwrap(), "Activation")
                        .default_action(NotificationAction::new(
                            NotificationActionId::new("open").unwrap(),
                        )),
                )
                .unwrap();
            assert!(matches!(
                event(&mut client).await,
                NotificationEvent::Operation {
                    outcome: NotificationOperationOutcome::Sent,
                    ..
                }
            ));
            let (received_id, destination) = posted.lock().unwrap().last().cloned().unwrap();
            assert_eq!(received_id, id);
            emit(
                &impostor,
                &destination,
                &id,
                HashMap::from([("desktop-startup-id".into(), field("forged"))]),
            )
            .await;
            for value in [OwnedValue::from(17_u32), field("")] {
                emit(
                    &server,
                    &destination,
                    &id,
                    HashMap::from([("desktop-startup-id".into(), value)]),
                )
                .await;
                assert!(matches!(
                    event(&mut client).await,
                    NotificationEvent::IgnoredAction {
                        reason: IgnoredNotificationAction::MalformedPayload,
                        ..
                    }
                ));
            }
            emit(
                &server,
                &destination,
                &id,
                HashMap::from([("desktop-startup-id".into(), field(&"x".repeat(4097)))]),
            )
            .await;
            emit(&server, &destination, &id, data).await;
            let mut invocation = match event(&mut client).await {
                NotificationEvent::Action(invocation) => invocation,
                other => panic!("missing portal action: {other:?}"),
            };
            assert_eq!(invocation.action_id.as_str(), "open");
            if index == 0 {
                assert!(matches!(
                    &invocation.activation_context,
                    NotificationActivationContext::XdgActivationToken(_)
                ));
            } else {
                assert!(matches!(
                    &invocation.activation_context,
                    NotificationActivationContext::PlatformData(_)
                ));
            }
            let context = broker
                .admit_notification_invocation(&mut invocation, Instant::now())
                .unwrap()
                .unwrap();
            assert!(
                broker
                    .admit_notification_invocation(&mut invocation, Instant::now())
                    .unwrap()
                    .is_none()
            );
            assert_eq!(child_activation_environment(context), expected);
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while client.backend_stats().rejected_actions < 6 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(client.backend_stats().rejected_actions, 6);
    });
}
