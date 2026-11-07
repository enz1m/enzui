#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::app_data::AppId;
use enzui_core::portal::notification::freedesktop::FreedesktopNotificationBackend;
use enzui_core::portal::notification::*;
use enzui_test_support::test_scratch::ScratchDir;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedValue;

#[path = "support/activation_child.rs"]
mod activation_child;

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const WAIT: Duration = Duration::from_secs(8);
type Client = DesktopNotificationClient<FreedesktopNotificationBackend>;

struct Bus {
    child: Child,
    address: String,
    delivery: Option<PathBuf>,
    launches: Option<PathBuf>,
    shutdown: Option<PathBuf>,
    exited: Option<PathBuf>,
    _root: ScratchDir,
}
impl Bus {
    fn new() -> Self {
        Self::start(None)
    }

    fn activatable() -> Self {
        Self::start(Some(false))
    }

    fn exiting_activatable() -> Self {
        Self::start(Some(true))
    }

    fn start(exit_after_activation: Option<bool>) -> Self {
        let root = ScratchDir::new_restricted("ntf");
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        let service_dir = root.join("services");
        std::fs::create_dir(&service_dir).unwrap();
        let delivery = exit_after_activation.map(|_| root.join("delivered"));
        let launches = exit_after_activation.map(|_| root.join("launches"));
        let shutdown = exit_after_activation.map(|_| root.join("shutdown"));
        let exited = exit_after_activation.map(|_| root.join("exited"));
        if exit_after_activation.is_some() {
            let executable = std::env::current_exe().unwrap();
            assert!(!executable.to_string_lossy().contains(char::is_whitespace));
            std::fs::write(
                service_dir.join("org.freedesktop.Notifications.service"),
                format!(
                    "[D-BUS Service]\nName={SERVICE}\nExec={} --exact notification_daemon_fixture --nocapture\n",
                    executable.display()
                ),
            )
            .unwrap();
        }
        let config = root.join("bus.conf");
        std::fs::write(&config, format!("<busconfig><type>session</type><listen>{address}</listen><servicedir>{}</servicedir><policy context=\"default\"><allow send_destination=\"*\"/><allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>", service_dir.display())).unwrap();
        let mut command = Command::new("dbus-daemon");
        command
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--nopidfile"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = &delivery {
            command.env("ENZUI_NOTIFICATION_FIXTURE_DELIVERY", path);
        }
        if let Some(path) = &launches {
            command.env("ENZUI_NOTIFICATION_FIXTURE_LAUNCHES", path);
        }
        if let Some(path) = &shutdown {
            command.env("ENZUI_NOTIFICATION_FIXTURE_SHUTDOWN", path);
        }
        if let Some(path) = &exited {
            command.env("ENZUI_NOTIFICATION_FIXTURE_EXITED", path);
        }
        if exit_after_activation == Some(true) {
            command.env("ENZUI_NOTIFICATION_FIXTURE_EXIT_AFTER_CAPABILITIES", "1");
        }
        let child = command.spawn().unwrap();
        let mut bus = Self {
            child,
            address,
            delivery,
            launches,
            shutdown,
            exited,
            _root: root,
        };
        let deadline = Instant::now() + WAIT;
        while !socket.exists() {
            assert!(bus.child.try_wait().unwrap().is_none(), "bus exited");
            assert!(Instant::now() < deadline, "bus startup timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        bus
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        if let Some(shutdown) = &self.shutdown {
            let _ = std::fs::write(shutdown, []);
            if let Some(exited) = &self.exited {
                let deadline = Instant::now() + WAIT;
                while !exited.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct Posted {
    id: u32,
    replaces: u32,
    sender: String,
}
#[derive(Default)]
struct Records {
    posted: Vec<Posted>,
    closed: Vec<u32>,
    next_id: u32,
    zero_id: bool,
    deny: bool,
}
struct Server {
    capabilities: Vec<String>,
    records: Arc<Mutex<Records>>,
    pause: Option<Arc<tokio::sync::Notify>>,
    capability_pause: Option<Arc<tokio::sync::Notify>>,
    delivery: Option<PathBuf>,
    exit_after_capabilities: Option<Arc<AtomicBool>>,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    async fn get_capabilities(&self) -> Vec<String> {
        if let Some(pause) = &self.capability_pause {
            pause.notified().await;
        }
        if let Some(exit) = &self.exit_after_capabilities {
            exit.store(true, Ordering::Release);
        }
        self.capabilities.clone()
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "the `Notify` method's D-Bus signature"
    )]
    async fn notify(
        &self,
        _app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        _summary: &str,
        _body: &str,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<u32> {
        let id = {
            let mut records = self.records.lock().unwrap();
            if records.deny {
                return Err(zbus::fdo::Error::AccessDenied("Fixture denial".into()));
            }
            let id = if replaces_id == 0 {
                records.next_id += 1;
                records.next_id
            } else {
                replaces_id
            };
            records.posted.push(Posted {
                id,
                replaces: replaces_id,
                sender: header.sender().unwrap().to_string(),
            });
            if records.zero_id { 0 } else { id }
        };
        if let Some(delivery) = &self.delivery {
            std::fs::write(delivery, []).map_err(|error| {
                zbus::fdo::Error::Failed(format!("fixture delivery write failed: {error}"))
            })?;
        }
        if let Some(pause) = &self.pause {
            pause.notified().await;
        }
        Ok(id)
    }
    fn close_notification(&self, id: u32) {
        self.records.lock().unwrap().closed.push(id);
    }
}

async fn serve(bus: &Bus, capabilities: &[&str], records: Arc<Mutex<Records>>) -> zbus::Connection {
    tokio::time::timeout(
        WAIT,
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .serve_at(
                PATH,
                Server {
                    capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
                    records,
                    pause: None,
                    capability_pause: None,
                    delivery: None,
                    exit_after_capabilities: None,
                },
            )
            .unwrap()
            .build(),
    )
    .await
    .unwrap()
    .unwrap()
}
fn make_client(bus: &Bus) -> Client {
    let id = AppId::new("org.example.Notifications").unwrap();
    Client::new(
        id.clone(),
        FreedesktopNotificationBackend::with_address(id, Some(bus.address.clone())).unwrap(),
    )
}
async fn event(client: &mut Client) -> NotificationEvent {
    tokio::time::timeout(WAIT, async {
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
async fn ready(client: &mut Client) {
    loop {
        if matches!(
            event(client).await,
            NotificationEvent::ServiceChanged {
                available: true,
                ..
            }
        ) {
            return;
        }
    }
}
async fn sent(client: &mut Client) {
    assert!(matches!(
        event(client).await,
        NotificationEvent::Operation {
            outcome: NotificationOperationOutcome::Sent | NotificationOperationOutcome::Replaced,
            ..
        }
    ));
}
fn action(id: &str) -> NotificationAction {
    NotificationAction::new(NotificationActionId::new(id).unwrap())
        .target(NotificationActionTarget::Text(format!("target-{id}")))
}
fn notice() -> DesktopNotification {
    DesktopNotification::new(NotificationId::new("room").unwrap(), "Message")
        .body("Hello <friend> & all")
        .default_action(action("open"))
        .button(NotificationButton::new("Mark read", action("read")))
        .inline_reply(NotificationInlineReply::new(
            action("reply"),
            "Write a reply",
            "Send reply",
        ))
}
async fn emit(
    connection: &zbus::Connection,
    destination: &str,
    member: &str,
    id: u32,
    value: &str,
) {
    tokio::time::timeout(
        WAIT,
        connection.emit_signal(Some(destination), PATH, SERVICE, member, &(id, value)),
    )
    .await
    .unwrap()
    .unwrap();
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn launches(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |contents| contents.lines().count())
}

#[test]
fn notification_daemon_fixture() {
    let Ok(delivery) = std::env::var("ENZUI_NOTIFICATION_FIXTURE_DELIVERY") else {
        return;
    };
    runtime().block_on(async {
        let address = std::env::var("DBUS_STARTER_ADDRESS").unwrap();
        let launch_path = std::env::var("ENZUI_NOTIFICATION_FIXTURE_LAUNCHES").unwrap();
        let mut launch_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&launch_path)
            .unwrap();
        std::io::Write::write_all(&mut launch_log, b"launch\n").unwrap();
        let records = Arc::new(Mutex::new(Records::default()));
        let exit_after_capabilities =
            (std::env::var_os("ENZUI_NOTIFICATION_FIXTURE_EXIT_AFTER_CAPABILITIES").is_some()
                && launches(std::path::Path::new(&launch_path)) == 1)
                .then(|| Arc::new(AtomicBool::new(false)));
        let connection = tokio::time::timeout(
            WAIT,
            zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .name(SERVICE)
                .unwrap()
                .serve_at(
                    PATH,
                    Server {
                        capabilities: Vec::new(),
                        records,
                        pause: None,
                        capability_pause: None,
                        delivery: Some(PathBuf::from(&delivery)),
                        exit_after_capabilities: exit_after_capabilities.clone(),
                    },
                )
                .unwrap()
                .build(),
        )
        .await
        .unwrap()
        .unwrap();
        if let Some(exit) = exit_after_capabilities {
            let deadline = Instant::now() + WAIT;
            while !exit.load(Ordering::Acquire) {
                assert!(
                    Instant::now() < deadline,
                    "capability negotiation timed out"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        } else {
            let shutdown =
                PathBuf::from(std::env::var("ENZUI_NOTIFICATION_FIXTURE_SHUTDOWN").unwrap());
            let deadline = Instant::now() + WAIT;
            while !shutdown.exists() {
                assert!(Instant::now() < deadline, "fixture shutdown timed out");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        drop(connection);
        std::fs::write(
            std::env::var("ENZUI_NOTIFICATION_FIXTURE_EXITED").unwrap(),
            [],
        )
        .unwrap();
    });
}

#[test]
fn freedesktop_cold_activation_delivers_and_missing_service_is_unavailable() {
    let bus = Bus::activatable();
    runtime().block_on(async {
        let mut client = make_client(&bus);
        ready(&mut client).await;
        client
            .send(DesktopNotification::new(
                NotificationId::new("cold").unwrap(),
                "Cold activation",
            ))
            .unwrap();
        sent(&mut client).await;
        let delivery = bus.delivery.as_ref().unwrap();
        let deadline = Instant::now() + WAIT;
        while !delivery.exists() {
            assert!(Instant::now() < deadline, "activation delivery timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let launch_log = bus.launches.as_ref().unwrap();
    assert_eq!(launches(launch_log), 1);

    let missing_bus = Bus::new();
    runtime().block_on(async {
        let mut client = make_client(&missing_bus);
        loop {
            if matches!(
                event(&mut client).await,
                NotificationEvent::ServiceChanged {
                    available: false,
                    ..
                }
            ) {
                break;
            }
        }
        assert!(matches!(
            client.send(DesktopNotification::new(
                NotificationId::new("missing").unwrap(),
                "Unavailable",
            )),
            Err(NotificationError::Backend(
                NotificationBackendError::Unavailable
            ))
        ));
    });
}

#[test]
fn freedesktop_does_not_restart_an_exited_activatable_daemon_while_idle() {
    let bus = Bus::exiting_activatable();
    runtime().block_on(async {
        let mut client = make_client(&bus);
        let exited = bus.exited.as_ref().unwrap();
        let deadline = Instant::now() + WAIT;
        while !exited.exists() {
            assert!(Instant::now() < deadline, "activated daemon exit timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        loop {
            if matches!(
                event(&mut client).await,
                NotificationEvent::ServiceChanged {
                    available: false,
                    ..
                }
            ) {
                break;
            }
        }
        let launch_log = bus.launches.as_ref().unwrap();
        assert_eq!(launches(launch_log), 1);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(launches(launch_log), 1);
    });
}

#[test]
fn freedesktop_owner_restart_forgery_bounds_and_close_invalidation() {
    let bus = Bus::new();
    runtime().block_on(async {
        let records = Arc::new(Mutex::new(Records::default()));
        let server = serve(&bus, &["actions", "inline-reply"], Arc::clone(&records)).await;
        let mut client = make_client(&bus);
        ready(&mut client).await;
        client.send(notice()).unwrap();
        sent(&mut client).await;
        let (id, destination) = {
            let records = records.lock().unwrap();
            (records.posted[0].id, records.posted[0].sender.clone())
        };
        let impostor = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        emit(&impostor, &destination, "NotificationReplied", id, "Forged").await;
        emit(
            &server,
            &destination,
            "NotificationReplied",
            id,
            &"x".repeat(MAX_NOTIFICATION_ACTION_VALUE_BYTES + 1),
        )
        .await;
        emit(&server, &destination, "NotificationReplied", id, "Real").await;
        let NotificationEvent::Action(invocation) = event(&mut client).await else {
            panic!("missing real reply")
        };
        assert_eq!(invocation.response.as_deref(), Some("Real"));
        assert_eq!(client.backend_stats().rejected_actions, 2);
        client
            .send(
                DesktopNotification::new(NotificationId::new("barrier").unwrap(), "Barrier")
                    .default_action(action("barrier")),
            )
            .unwrap();
        sent(&mut client).await;
        let barrier_id = records.lock().unwrap().posted[1].id;
        server
            .emit_signal(
                Some(destination.as_str()),
                PATH,
                SERVICE,
                "NotificationClosed",
                &(id, 2u32),
            )
            .await
            .unwrap();
        emit(&server, &destination, "ActionInvoked", id, "default").await;
        emit(
            &server,
            &destination,
            "ActionInvoked",
            barrier_id,
            "default",
        )
        .await;
        let NotificationEvent::Closed {
            notification_id,
            reason,
            ..
        } = event(&mut client).await
        else {
            panic!("the closure reaches the reducer first")
        };
        assert_eq!(notification_id, NotificationId::new("room").unwrap());
        assert_eq!(reason, NotificationCloseReason::Dismissed);
        let NotificationEvent::Action(invocation) = event(&mut client).await else {
            panic!("missing close barrier")
        };
        assert_eq!(invocation.action_id.as_str(), "barrier");
        client.send(notice()).unwrap();
        sent(&mut client).await;
        assert_eq!(records.lock().unwrap().posted[2].replaces, 0);
        server.release_name(SERVICE).await.unwrap();
        loop {
            if matches!(
                event(&mut client).await,
                NotificationEvent::ServiceChanged {
                    available: false,
                    ..
                }
            ) {
                break;
            }
        }
        let next_records = Arc::new(Mutex::new(Records::default()));
        let next = serve(
            &bus,
            &["actions", "inline-reply"],
            Arc::clone(&next_records),
        )
        .await;
        ready(&mut client).await;
        emit(
            &server,
            &destination,
            "NotificationReplied",
            id,
            "Old owner",
        )
        .await;
        client.send(notice()).unwrap();
        sent(&mut client).await;
        assert_eq!(next_records.lock().unwrap().posted[0].replaces, 0);
        emit(&next, &destination, "NotificationReplied", 1, "New owner").await;
        let NotificationEvent::Action(invocation) = event(&mut client).await else {
            panic!("missing new owner reply")
        };
        assert_eq!(invocation.response.as_deref(), Some("New owner"));
    });
}

#[test]
fn activation_freedesktop_native_tokens_require_explicit_child_protocol_selection() {
    use activation_child::child_activation_environment;
    use activation_untyped::assert_untyped_context_needs_platform;
    use enzui_core::activation::{ActivationBroker, ActivationPlatform};
    let bus = Bus::new();
    runtime().block_on(async {
        let records = Arc::new(Mutex::new(Records::default()));
        let server = serve(&bus, &["actions"], Arc::clone(&records)).await;
        let mut client = make_client(&bus);
        ready(&mut client).await;
        let mut broker = ActivationBroker::default();
        for (index, (platform, expected)) in [
            (None, ""),
            (Some(ActivationPlatform::X11), "same-opaque-value\nunset\n"),
            (
                Some(ActivationPlatform::Wayland),
                "unset\nsame-opaque-value\n",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            client
                .send(
                    DesktopNotification::new(
                        NotificationId::new(format!("native-{index}")).unwrap(),
                        "Native activation",
                    )
                    .default_action(action("open")),
                )
                .unwrap();
            sent(&mut client).await;
            let (id, destination) = {
                let posted = records.lock().unwrap();
                let posted = posted.posted.last().unwrap();
                (posted.id, posted.sender.clone())
            };
            emit(
                &server,
                &destination,
                "ActivationToken",
                id,
                "same-opaque-value",
            )
            .await;
            emit(&server, &destination, "ActionInvoked", id, "default").await;
            let NotificationEvent::Action(mut invocation) = event(&mut client).await else {
                panic!("missing native notification action")
            };
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
            match platform {
                None => assert_untyped_context_needs_platform(context),
                Some(platform) => assert_eq!(
                    child_activation_environment(context.select_platform(platform).unwrap()),
                    expected
                ),
            }
        }
    });
}

#[path = "support/activation_untyped.rs"]
mod activation_untyped;

#[test]
fn freedesktop_encoded_icon_is_explicitly_unsupported() {
    let bus = Bus::new();
    runtime().block_on(async {
        let records = Arc::new(Mutex::new(Records::default()));
        let _server = serve(&bus, &[], Arc::clone(&records)).await;
        let mut client = make_client(&bus);
        ready(&mut client).await;
        client
            .send(
                DesktopNotification::new(NotificationId::new("encoded").unwrap(), "Encoded").icon(
                    NotificationIcon::Bytes(
                        br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"/>"#
                            .to_vec(),
                    ),
                ),
            )
            .unwrap();
        assert!(matches!(
            event(&mut client).await,
            NotificationEvent::Operation {
                outcome: NotificationOperationOutcome::Unsupported(
                    UnsupportedNotificationFeature::EncodedIcon
                ),
                ..
            }
        ));
        client
            .send(DesktopNotification::new(
                NotificationId::new("control").unwrap(),
                "Control",
            ))
            .unwrap();
        sent(&mut client).await;
        assert_eq!(records.lock().unwrap().posted.len(), 1);
    });
}

#[test]
fn freedesktop_exported_actions_are_explicitly_unsupported() {
    let bus = Bus::new();
    runtime().block_on(async {
        let records = Arc::new(Mutex::new(Records::default()));
        let _server = serve(&bus, &["actions", "inline-reply"], Arc::clone(&records)).await;
        let mut client = make_client(&bus);
        ready(&mut client).await;
        for notification in [
            DesktopNotification::new(NotificationId::new("default-export").unwrap(), "Default")
                .default_action(NotificationExportedAction::new("open").unwrap()),
            DesktopNotification::new(NotificationId::new("button-export").unwrap(), "Button")
                .button(NotificationButton::new(
                    "Open",
                    NotificationExportedAction::new("open").unwrap(),
                )),
            DesktopNotification::new(NotificationId::new("reply-export").unwrap(), "Reply")
                .inline_reply(NotificationInlineReply::new(
                    NotificationExportedAction::new("reply").unwrap(),
                    "Write",
                    "Send",
                )),
        ] {
            client.send(notification).unwrap();
            assert!(matches!(
                event(&mut client).await,
                NotificationEvent::Operation {
                    outcome: NotificationOperationOutcome::Unsupported(
                        UnsupportedNotificationFeature::ExportedAction
                    ),
                    ..
                }
            ));
        }
        client
            .send(DesktopNotification::new(
                NotificationId::new("control").unwrap(),
                "Control",
            ))
            .unwrap();
        sent(&mut client).await;
        assert_eq!(records.lock().unwrap().posted.len(), 1);
    });
}

/// A notice the desktop closed releases its registration: after every active
/// slot was used and all but one were dismissed, a new notice is admitted, and
/// the reducer hears each closure.
#[test]
fn freedesktop_closure_releases_active_capacity() {
    let bus = Bus::new();
    runtime().block_on(async {
        let records = Arc::new(Mutex::new(Records::default()));
        let server = serve(&bus, &["actions", "body"], Arc::clone(&records)).await;
        let mut client = make_client(&bus);
        ready(&mut client).await;
        let notice = |index: usize| {
            DesktopNotification::new(
                NotificationId::new(format!("notice-{index}")).unwrap(),
                "Notice",
            )
            .default_action(action("open"))
        };
        for index in 0..MAX_ACTIVE_NOTIFICATIONS {
            client.send(notice(index)).unwrap();
            sent(&mut client).await;
        }
        assert!(
            matches!(
                client.send(notice(MAX_ACTIVE_NOTIFICATIONS)),
                Err(NotificationError::Capacity { .. })
            ),
            "positive control: the active limit holds"
        );
        let (ids, destination) = {
            let records = records.lock().unwrap();
            (
                records
                    .posted
                    .iter()
                    .map(|posted| posted.id)
                    .collect::<Vec<_>>(),
                records.posted[0].sender.clone(),
            )
        };
        for id in &ids[1..] {
            server
                .emit_signal(
                    Some(destination.as_str()),
                    PATH,
                    SERVICE,
                    "NotificationClosed",
                    &(*id, 2u32),
                )
                .await
                .unwrap();
        }
        for _ in 1..ids.len() {
            assert!(
                matches!(
                    event(&mut client).await,
                    NotificationEvent::Closed {
                        reason: NotificationCloseReason::Dismissed,
                        ..
                    }
                ),
                "each closure reaches the reducer"
            );
        }
        client
            .send(notice(MAX_ACTIVE_NOTIFICATIONS))
            .expect("a closed notice no longer counts toward the limit");
        sent(&mut client).await;
        emit(&server, &destination, "ActionInvoked", ids[0], "default").await;
        assert!(
            matches!(event(&mut client).await, NotificationEvent::Action(_)),
            "the notice still showing keeps routing its actions"
        );
    });
}
