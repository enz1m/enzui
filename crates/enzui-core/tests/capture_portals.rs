//! Screenshot, colour picking and global shortcuts against a fake portal on
//! a private bus.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::float_cmp,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]
use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, OwnedValue, Value};
use enzui_core::portal::global_shortcuts::{
    BoundShortcut, GlobalShortcut, ShortcutEvent, run_shortcuts,
};
use enzui_core::portal::parent::ParentHandle;
use enzui_core::portal::screenshot::{
    PortalOutcome, ScreenshotRequest, pick_color_on, take_screenshot_on,
};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

struct PrivateBus {
    child: Child,
    address: String,
    root: std::path::PathBuf,
}

impl PrivateBus {
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("enzui-capture-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile"])
            .arg(format!("--address={address}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon is required for this test");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "private bus startup");
            std::thread::sleep(Duration::from_millis(10));
        }
        Self {
            child,
            address,
            root,
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, Default)]
struct Log {
    /// The response code the next request answers with (0 ok, 1 cancelled).
    code: u32,
    parents: Vec<String>,
    interactive: Vec<Option<bool>>,
    /// (id, description, preferred trigger) per bound shortcut.
    bound: Vec<(String, String, Option<String>)>,
    sessions: Vec<String>,
    closed: Vec<String>,
}

fn caller(header: &zbus::message::Header<'_>) -> String {
    header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_")
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|value| String::try_from(value.try_clone().unwrap()).ok())
}

fn respond(
    connection: &zbus::Connection,
    path: String,
    code: u32,
    results: HashMap<&'static str, Value<'static>>,
) {
    let connection = connection.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = connection
            .emit_signal(
                None::<&str>,
                path.as_str(),
                "org.freedesktop.portal.Request",
                "Response",
                &(code, results),
            )
            .await;
    });
}

struct FakeScreenshot {
    log: Arc<Mutex<Log>>,
}

#[zbus::interface(name = "org.freedesktop.portal.Screenshot", crate = "ashpd::zbus")]
impl FakeScreenshot {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    fn screenshot(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        parent: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        let code = {
            let mut log = self.log.lock().unwrap();
            log.parents.push(parent);
            log.interactive.push(
                options
                    .get("interactive")
                    .and_then(|value| bool::try_from(value).ok()),
            );
            log.code
        };
        let path = format!(
            "{PORTAL_PATH}/request/{}/{}",
            caller(&header),
            string(&options, "handle_token").unwrap()
        );
        let mut results = HashMap::new();
        results.insert(
            "uri",
            Value::from("file:///home/ada/Pictures/Screenshot.png"),
        );
        respond(connection, path.clone(), code, results);
        OwnedObjectPath::try_from(path).unwrap()
    }

    fn pick_color(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        parent: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        let code = {
            let mut log = self.log.lock().unwrap();
            log.parents.push(parent);
            log.code
        };
        let path = format!(
            "{PORTAL_PATH}/request/{}/{}",
            caller(&header),
            string(&options, "handle_token").unwrap()
        );
        let mut results = HashMap::new();
        results.insert("color", Value::from((1.0f64, 0.5f64, 0.25f64)));
        respond(connection, path.clone(), code, results);
        OwnedObjectPath::try_from(path).unwrap()
    }
}

struct FakeSession {
    path: String,
    log: Arc<Mutex<Log>>,
}

#[zbus::interface(name = "org.freedesktop.portal.Session", crate = "ashpd::zbus")]
impl FakeSession {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    fn close(&self) {
        self.log.lock().unwrap().closed.push(self.path.clone());
    }
}

struct FakeShortcuts {
    log: Arc<Mutex<Log>>,
}

#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts", crate = "ashpd::zbus")]
impl FakeShortcuts {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    async fn create_session(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        let who = caller(&header);
        let request = format!(
            "{PORTAL_PATH}/request/{who}/{}",
            string(&options, "handle_token").unwrap()
        );
        let session = format!(
            "{PORTAL_PATH}/session/{who}/{}",
            string(&options, "session_handle_token").unwrap()
        );
        let _ = connection
            .object_server()
            .at(
                session.as_str(),
                FakeSession {
                    path: session.clone(),
                    log: Arc::clone(&self.log),
                },
            )
            .await;
        self.log.lock().unwrap().sessions.push(session.clone());
        let mut results = HashMap::new();
        results.insert("session_handle", Value::from(session));
        respond(connection, request.clone(), 0, results);
        OwnedObjectPath::try_from(request).unwrap()
    }

    fn bind_shortcuts(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        _session: OwnedObjectPath,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        let request = format!(
            "{PORTAL_PATH}/request/{}/{}",
            caller(&header),
            string(&options, "handle_token").unwrap()
        );
        let mut bound = Vec::new();
        for (id, info) in &shortcuts {
            let description = string(info, "description").unwrap_or_default();
            let preferred = string(info, "preferred_trigger");
            self.log.lock().unwrap().bound.push((
                id.clone(),
                description.clone(),
                preferred.clone(),
            ));
            let mut out: HashMap<&str, Value<'_>> = HashMap::new();
            out.insert("description", Value::from(description));
            out.insert(
                "trigger_description",
                Value::from(preferred.map(|_| "Ctrl+Alt+R").unwrap_or_default()),
            );
            bound.push((id.clone(), out));
        }
        let mut results = HashMap::new();
        results.insert("shortcuts", Value::from(bound));
        respond(connection, request.clone(), 0, results);
        OwnedObjectPath::try_from(request).unwrap()
    }
}

struct Portal {
    log: Arc<Mutex<Log>>,
    connection: zbus::Connection,
}

async fn portal(address: &str) -> Portal {
    let log = Arc::new(Mutex::new(Log::default()));
    let connection = zbus::connection::Builder::address(address)
        .unwrap()
        .name(PORTAL_DEST)
        .unwrap()
        .serve_at(
            PORTAL_PATH,
            FakeScreenshot {
                log: Arc::clone(&log),
            },
        )
        .unwrap()
        .serve_at(
            PORTAL_PATH,
            FakeShortcuts {
                log: Arc::clone(&log),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    Portal { log, connection }
}

async fn client(address: &str) -> zbus::Connection {
    zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn capture_and_shortcut_requests_deliver_outcomes_and_release_sessions() {
    // a_screenshot_returns_the_desktops_file_and_a_cancel_says_so
    {
        let bus = PrivateBus::start();
        let rt = runtime();
        rt.block_on(async {
            let portal = portal(&bus.address).await;
            let shot = take_screenshot_on(
                client(&bus.address).await,
                ScreenshotRequest::new()
                    .interactive(true)
                    .parent(ParentHandle::x11(0x2a)),
            )
            .await;
            assert_eq!(
                shot,
                PortalOutcome::Done("file:///home/ada/Pictures/Screenshot.png".to_owned())
            );
            {
                let log = portal.log.lock().unwrap();
                assert_eq!(log.parents, ["x11:2a"]);
                assert_eq!(log.interactive, [Some(true)]);
            }

            portal.log.lock().unwrap().code = 1;
            let cancelled =
                take_screenshot_on(client(&bus.address).await, ScreenshotRequest::new()).await;
            assert_eq!(cancelled, PortalOutcome::Cancelled);
            assert_eq!(portal.log.lock().unwrap().parents[1], "", "no parent");
        });
    }

    // a_picked_colour_arrives_as_an_iced_colour
    {
        let bus = PrivateBus::start();
        let rt = runtime();
        rt.block_on(async {
            let portal = portal(&bus.address).await;
            let picked = pick_color_on(
                client(&bus.address).await,
                ParentHandle::wayland("exported-token"),
            )
            .await;
            assert_eq!(
                picked,
                PortalOutcome::Done(iced_core::Color::from_rgb(1.0, 0.5, 0.25))
            );
            assert_eq!(
                portal.log.lock().unwrap().parents,
                ["wayland:exported-token"]
            );
        });
    }

    // without_the_portal_the_outcome_is_unavailable
    {
        let bus = PrivateBus::start();
        let rt = runtime();
        rt.block_on(async {
            let outcome =
                take_screenshot_on(client(&bus.address).await, ScreenshotRequest::new()).await;
            assert!(
                matches!(outcome, PortalOutcome::Unavailable(_)),
                "{outcome:?}"
            );
        });
    }

    // global_shortcuts_bind_report_presses_for_their_session_and_close_it
    {
        let bus = PrivateBus::start();
        let rt = runtime();
        rt.block_on(async {
            let portal = portal(&bus.address).await;
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            let connection = client(&bus.address).await;
            let task = tokio::spawn(run_shortcuts(
                connection,
                vec![
                    GlobalShortcut::new("record", "Start or stop recording")
                        .preferred_trigger("CTRL+ALT+r"),
                    GlobalShortcut::new("mark", "Add a marker"),
                ],
                sender,
            ));
            let mut next = async || {
                tokio::time::timeout(Duration::from_secs(5), events.recv())
                    .await
                    .unwrap()
                    .unwrap()
            };
            assert_eq!(
                next().await,
                ShortcutEvent::Bound(vec![
                    BoundShortcut {
                        id: "record".into(),
                        description: "Start or stop recording".into(),
                        trigger: "Ctrl+Alt+R".into(),
                    },
                    BoundShortcut {
                        id: "mark".into(),
                        description: "Add a marker".into(),
                        trigger: String::new(),
                    },
                ])
            );
            assert_eq!(
                portal.log.lock().unwrap().bound,
                [
                    (
                        "record".to_owned(),
                        "Start or stop recording".to_owned(),
                        Some("CTRL+ALT+r".to_owned())
                    ),
                    ("mark".to_owned(), "Add a marker".to_owned(), None),
                ]
            );
            let session = portal.log.lock().unwrap().sessions[0].clone();
            // Another app's session: not ours.
            emit(
                &portal,
                "Activated",
                &format!("{PORTAL_PATH}/session/other/x"),
                "record",
            )
            .await;
            emit(&portal, "Activated", &session, "record").await;
            emit(&portal, "Deactivated", &session, "record").await;
            assert_eq!(next().await, ShortcutEvent::Activated("record".into()));
            assert_eq!(next().await, ShortcutEvent::Deactivated("record".into()));

            // The user rebinds it in the desktop's settings.
            let mut info: HashMap<&str, Value<'_>> = HashMap::new();
            info.insert("description", Value::from("Start or stop recording"));
            info.insert("trigger_description", Value::from("Super+R"));
            portal
                .connection
                .emit_signal(
                    None::<&str>,
                    PORTAL_PATH,
                    "org.freedesktop.portal.GlobalShortcuts",
                    "ShortcutsChanged",
                    &(
                        OwnedObjectPath::try_from(session.as_str()).unwrap(),
                        vec![("record", info)],
                    ),
                )
                .await
                .unwrap();
            assert_eq!(
                next().await,
                ShortcutEvent::Changed(vec![BoundShortcut {
                    id: "record".into(),
                    description: "Start or stop recording".into(),
                    trigger: "Super+R".into(),
                }])
            );

            // The app goes away: the session is closed.
            drop(events);
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(portal.log.lock().unwrap().closed, [session]);
        });
    }
}

async fn emit(portal: &Portal, member: &str, session: &str, id: &str) {
    let session = OwnedObjectPath::try_from(session).unwrap();
    let options: HashMap<String, OwnedValue> = HashMap::new();
    portal
        .connection
        .emit_signal(
            None::<&str>,
            PORTAL_PATH,
            "org.freedesktop.portal.GlobalShortcuts",
            member,
            &(session, id, 1234u64, options),
        )
        .await
        .unwrap();
}
