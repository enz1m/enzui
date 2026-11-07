use super::activation_child::{ReapedChild, child_activation_environment};
use super::private_bus::PrivateBus;
use super::replies::{event, outcome, runtime};
use enzui_core::activation::dbus::ApplicationService;
use enzui_core::activation::{ActivationBroker, ApplicationIntent};
use enzui_core::app_data::AppId;
use enzui_core::portal::dbus_backend::{DbusBackend, PortalIdentity};
use enzui_core::portal::notification::*;
use enzui_test_support::test_scratch::ScratchDir;
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedValue;

const APP: &str = "org.enzui.ExportedNotificationTest";

fn exported_notice(id: &str) -> DesktopNotification {
    DesktopNotification::new(NotificationId::new(id).unwrap(), "Message")
        .default_action(
            NotificationExportedAction::new("open-conversation")
                .unwrap()
                .target(NotificationActionTarget::Text("thread-42".into())),
        )
        .button(NotificationButton::new(
            "Archive",
            NotificationExportedAction::new("archive")
                .unwrap()
                .target(NotificationActionTarget::Unsigned(42)),
        ))
}

#[test]
fn exported_notification_process_fixture() {
    let Ok(mode) = std::env::var("ENZUI_EXPORTED_FIXTURE") else {
        return;
    };
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    let mut application = ApplicationService::connect_to_address(APP, &address).unwrap();
    if mode == "poster" {
        let mut client = DesktopNotificationClient::new(
            AppId::new(APP).unwrap(),
            DbusBackend::for_application_with_address(
                AppId::new(APP).unwrap(),
                PortalIdentity::Host,
                Some(address),
            )
            .unwrap(),
        );
        runtime().block_on(async {
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
            for id in ["persistent-default", "persistent-button"] {
                client.send(exported_notice(id)).unwrap();
                assert_eq!(
                    outcome(&mut client).await,
                    NotificationOperationOutcome::Sent
                );
            }
        });
        client.begin_shutdown(NotificationShutdown::LeavePosted);
    } else {
        assert_eq!(mode, "activated");
        let mut broker = ActivationBroker::default();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let mut calls = application.drain(&mut broker, Instant::now(), 1);
            if let Some(call) = calls.pop() {
                let ApplicationIntent::ActivateAction { name } = call.intent else {
                    panic!("not ActivateAction")
                };
                let parameters: Vec<serde_json::Value> = call
                    .parameters
                    .into_iter()
                    .map(|value| {
                        if let Ok(text) = <&str>::try_from(&value) {
                            serde_json::json!(text)
                        } else {
                            serde_json::json!(u64::try_from(value).unwrap())
                        }
                    })
                    .collect();
                assert!(call.context_error.is_none());
                let environment =
                    child_activation_environment(call.context.expect("activation context"));
                let report = serde_json::json!({"pid": std::process::id(), "name": name, "parameters": parameters, "environment": environment});
                let destination =
                    std::path::PathBuf::from(std::env::var("ENZUI_EXPORTED_REPORT").unwrap());
                let temporary = destination.with_extension("partial");
                std::fs::write(&temporary, report.to_string()).unwrap();
                std::fs::rename(temporary, destination).unwrap();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "activated child received no action"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[derive(Clone)]
struct Posted {
    id: u32,
    destination: String,
    actions: Vec<String>,
    application_id: String,
}

struct Daemon {
    posted: Arc<Mutex<Vec<Posted>>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Daemon {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["actions", "body", "body-markup"]
    }
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("Private", "enzui", "1", "1.2")
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "the standard Notify method has eight wire arguments"
    )]
    fn notify(
        &self,
        _app_name: &str,
        _replaces_id: u32,
        _app_icon: &str,
        _summary: &str,
        _body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> u32 {
        let id = 41 + u32::try_from(self.posted.lock().unwrap().len()).unwrap();
        self.posted.lock().unwrap().push(Posted {
            id,
            destination: header.sender().unwrap().to_string(),
            actions,
            application_id: hints
                .get("desktop-entry")
                .map(|value| <&str>::try_from(value).unwrap().to_owned())
                .unwrap_or_default(),
        });
        id
    }
    fn close_notification(&self, _id: u32) {}
}

struct Desktop {
    _processes: Vec<ReapedChild>,
    display: String,
    root: ScratchDir,
}

impl Desktop {
    fn new(bus: &PrivateBus) -> Self {
        let root = ScratchDir::new_restricted("exported-desktop");
        for path in [
            "home",
            "runtime",
            "data/applications",
            "config/xdg-desktop-portal",
            "portals",
            "cache",
            "state",
            "data-dirs",
            "config-dirs",
        ] {
            std::fs::create_dir_all(root.path().join(path)).unwrap();
        }
        std::fs::set_permissions(
            root.path().join("runtime"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(root.path().join("data/applications").join(format!("{APP}.desktop")),
            "[Desktop Entry]\nType=Application\nName=Exported notification test\nExec=/bin/true\nDBusActivatable=true\nNoDisplay=true\n").unwrap();
        std::fs::write(
            root.path().join("config/xdg-desktop-portal/portals.conf"),
            "[preferred]\ndefault=none\norg.freedesktop.impl.portal.Notification=gtk\n",
        )
        .unwrap();
        std::fs::write(root.path().join("portals/gtk.portal"),
            "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.gtk\nInterfaces=org.freedesktop.impl.portal.Notification;\nUseIn=enzui-test\n").unwrap();
        let xvfb_log = std::fs::File::create(root.path().join("xvfb.log")).unwrap();
        let mut xvfb = ReapedChild(
            Command::new("Xvfb")
                .args([
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    "800x600x24",
                    "-nolisten",
                    "tcp",
                    "-ac",
                ])
                .stdout(Stdio::piped())
                .stderr(xvfb_log)
                .spawn()
                .unwrap(),
        );
        let mut stdout = xvfb.0.stdout.take().unwrap();
        let flags = rustix::fs::fcntl_getfl(&stdout).unwrap();
        rustix::fs::fcntl_setfl(&stdout, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
        let mut display = String::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match stdout.read_to_string(&mut display) {
                Ok(_) if display.contains('\n') => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("display pipe: {error}"),
            }
            assert!(xvfb.0.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline, "Xvfb startup deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut desktop = Self {
            _processes: vec![xvfb],
            display: format!(":{}", display.trim()),
            root,
        };
        for (binary, name) in [
            ("/usr/lib/xdg-permission-store", "permission"),
            ("/usr/lib/xdg-desktop-portal-gtk", "gtk"),
            ("/usr/lib/xdg-desktop-portal", "portal"),
        ] {
            let mut command = desktop.command(binary, bus);
            let log =
                std::fs::File::create(desktop.root.path().join(format!("{name}.log"))).unwrap();
            command.stdout(log.try_clone().unwrap()).stderr(log);
            desktop
                ._processes
                .push(ReapedChild(command.spawn().unwrap()));
        }
        desktop
    }

    fn command(&self, binary: impl AsRef<std::ffi::OsStr>, bus: &PrivateBus) -> Command {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_RUNTIME_DIR", self.root.path().join("runtime"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_DATA_DIRS", self.root.path().join("data-dirs"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_CONFIG_DIRS", self.root.path().join("config-dirs"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_DESKTOP_PORTAL_DIR", self.root.path().join("portals"))
            .env("XDG_CURRENT_DESKTOP", "enzui-test")
            .env("XDG_SESSION_TYPE", "x11")
            .env("GDK_BACKEND", "x11")
            .env("DISPLAY", &self.display)
            .env("GTK_USE_PORTAL", "0")
            .env("GIO_USE_VFS", "local")
            .env("GSETTINGS_BACKEND", "memory")
            .env("GSETTINGS_SCHEMA_DIR", "/usr/share/glib-2.0/schemas")
            .env("NO_AT_BRIDGE", "1")
            .env("GTK_A11Y", "none");
        command
    }
}

async fn owner(connection: &zbus::Connection, name: &str, expected: bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        let proxy = zbus::fdo::DBusProxy::new(connection).await.unwrap();
        loop {
            if proxy
                .name_has_owner(name.try_into().unwrap())
                .await
                .unwrap()
                == expected
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn exported_notification_reactivates_after_poster_exit_through_real_portals() {
    let root = ScratchDir::new_restricted("exported-activation");
    let report = root.path().join("activated.json");
    let executable = std::env::current_exe().unwrap();
    assert!(!executable.to_string_lossy().contains(char::is_whitespace));
    assert!(!report.to_string_lossy().contains(char::is_whitespace));
    let service = format!(
        "[D-BUS Service]\nName={APP}\nExec=/usr/bin/env ENZUI_EXPORTED_FIXTURE=activated ENZUI_EXPORTED_REPORT={} {} --exact exported_actions::exported_notification_process_fixture --nocapture\n",
        report.display(),
        executable.display()
    );
    let bus = PrivateBus::with_service(Some(&service));
    runtime().block_on(async {
        let posted = Arc::new(Mutex::new(Vec::new()));
        let daemon = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at(
                "/org/freedesktop/Notifications",
                Daemon {
                    posted: Arc::clone(&posted),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let desktop = Desktop::new(&bus);
        owner(&daemon, "org.freedesktop.portal.Desktop", true).await;
        owner(&daemon, "org.freedesktop.impl.portal.desktop.gtk", true).await;
        let poster_log = std::fs::File::create(root.path().join("poster.log")).unwrap();
        let mut command = desktop.command(&executable, &bus);
        command
            .args([
                "--exact",
                "exported_actions::exported_notification_process_fixture",
                "--nocapture",
            ])
            .env("ENZUI_EXPORTED_FIXTURE", "poster")
            .stdout(poster_log.try_clone().unwrap())
            .stderr(poster_log);
        let mut poster = ReapedChild(command.spawn().unwrap());
        let poster_pid = poster.0.id();
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Some(status) = poster.0.try_wait().unwrap() {
                    assert!(
                        status.success(),
                        "poster failed: {}",
                        std::fs::read_to_string(root.path().join("poster.log")).unwrap()
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        owner(&daemon, APP, false).await;
        let notifications = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if posted.lock().unwrap().len() == 2 {
                    break posted.lock().unwrap().clone();
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        for (index, notification) in notifications.into_iter().enumerate() {
            assert_eq!(notification.application_id, APP);
            let action = if index == 0 {
                "default".to_owned()
            } else {
                notification
                    .actions
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .find(|pair| pair[1] == "Archive")
                    .unwrap()[0]
                    .clone()
            };
            let token = format!("desktop-token-{index}");
            daemon
                .emit_signal(
                    Some(notification.destination.as_str()),
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                    "ActivationToken",
                    &(notification.id, &token),
                )
                .await
                .unwrap();
            daemon
                .emit_signal(
                    Some(notification.destination.as_str()),
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                    "ActionInvoked",
                    &(notification.id, action),
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while !report.exists() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            let result: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
            assert_ne!(result["pid"].as_u64().unwrap(), u64::from(poster_pid));
            assert_eq!(
                result["name"],
                if index == 0 {
                    "open-conversation"
                } else {
                    "archive"
                }
            );
            assert_eq!(
                result["parameters"],
                if index == 0 {
                    serde_json::json!(["thread-42"])
                } else {
                    serde_json::json!([42])
                }
            );
            assert_eq!(result["environment"], format!("{token}\n{token}\n"));
            owner(&daemon, APP, false).await;
            std::fs::rename(
                &report,
                root.path().join(format!("activation-{index}.json")),
            )
            .unwrap();
        }
    });
}
