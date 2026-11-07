//! Session lifecycle against fake services on private buses: typed
//! inhibitors, the session monitor's query-end answer, and logind's
//! prepare-for-sleep with its delay lock.

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
use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, OwnedValue, Value};
use enzui_core::portal::inhibit::{InhibitKinds, InhibitState, Inhibitor};
use enzui_core::portal::session_monitor::{SessionEvent, SessionState, run_monitor};
use enzui_core::system_sleep::{SleepEvent, run_watch};
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
        let root = std::env::temp_dir().join(format!("enzui-session-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        let config = root.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig>\n  <type>session</type>\n  <listen>{address}</listen>\n  \
                 <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    \
                 <allow receive_sender=\"*\"/>\n    <allow own=\"*\"/>\n  \
                 </policy>\n</busconfig>\n"
            ),
        )
        .unwrap();
        let child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--nopidfile"])
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
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

fn caller(header: &zbus::message::Header<'_>) -> String {
    header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_")
}

fn token(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options
        .get(key)
        .and_then(|value| String::try_from(value.try_clone().unwrap()).ok())
        .unwrap()
}

#[derive(Debug, Default)]
struct PortalLog {
    inhibit_flags: Vec<u32>,
    closed: usize,
    session: Option<String>,
    query_end_responses: Vec<Instant>,
    /// Session states (`session-state` values) waiting to be emitted.
    emit: Vec<u32>,
}

struct FakeRequest {
    log: Arc<Mutex<PortalLog>>,
}

#[zbus::interface(name = "org.freedesktop.portal.Request", crate = "ashpd::zbus")]
impl FakeRequest {
    fn close(&self) {
        self.log.lock().unwrap().closed += 1;
    }
}

struct FakeInhibit {
    log: Arc<Mutex<PortalLog>>,
}

fn respond(
    connection: &zbus::Connection,
    path: String,
    results: HashMap<&'static str, Value<'static>>,
) {
    let connection = connection.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = connection
            .emit_signal(
                None::<&str>,
                path.as_str(),
                "org.freedesktop.portal.Request",
                "Response",
                &(0u32, results),
            )
            .await;
    });
}

#[zbus::interface(name = "org.freedesktop.portal.Inhibit", crate = "ashpd::zbus")]
impl FakeInhibit {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        3
    }

    async fn inhibit(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        _window: String,
        flags: u32,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        self.log.lock().unwrap().inhibit_flags.push(flags);
        let path = format!(
            "{PORTAL_PATH}/request/{}/{}",
            caller(&header),
            token(&options, "handle_token")
        );
        let _ = connection
            .object_server()
            .at(
                path.as_str(),
                FakeRequest {
                    log: Arc::clone(&self.log),
                },
            )
            .await;
        respond(connection, path.clone(), HashMap::new());
        OwnedObjectPath::try_from(path).unwrap()
    }

    fn create_monitor(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        _window: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        let who = caller(&header);
        let request = format!(
            "{PORTAL_PATH}/request/{who}/{}",
            token(&options, "handle_token")
        );
        let session = format!(
            "{PORTAL_PATH}/session/{who}/{}",
            token(&options, "session_handle_token")
        );
        self.log.lock().unwrap().session = Some(session.clone());
        let mut results = HashMap::new();
        results.insert("session_handle", Value::from(session));
        respond(connection, request.clone(), results);
        OwnedObjectPath::try_from(request).unwrap()
    }

    fn query_end_response(&self, _session: OwnedObjectPath) {
        self.log
            .lock()
            .unwrap()
            .query_end_responses
            .push(Instant::now());
    }
}

/// Serves the fake portal; returns its log and a stop guard.
fn serve_portal(address: &str) -> (Arc<Mutex<PortalLog>>, std::sync::mpsc::Sender<()>) {
    let log = Arc::new(Mutex::new(PortalLog::default()));
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let (ready, started) = std::sync::mpsc::channel();
    let (address, served) = (address.to_owned(), Arc::clone(&log));
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let connection = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            connection
                .object_server()
                .at(
                    PORTAL_PATH,
                    FakeInhibit {
                        log: Arc::clone(&served),
                    },
                )
                .await
                .unwrap();
            connection.request_name(PORTAL_DEST).await.unwrap();
            ready.send(()).unwrap();
            while stopped.try_recv().is_err() {
                let (session, states) = {
                    let mut log = served.lock().unwrap();
                    (log.session.clone(), std::mem::take(&mut log.emit))
                };
                for state in states {
                    let mut details: HashMap<&str, Value<'_>> = HashMap::new();
                    details.insert("screensaver-active", Value::from(false));
                    details.insert("session-state", Value::from(state));
                    let session = OwnedObjectPath::try_from(session.clone().unwrap()).unwrap();
                    connection
                        .emit_signal(
                            None::<&str>,
                            PORTAL_PATH,
                            "org.freedesktop.portal.Inhibit",
                            "StateChanged",
                            &(session, details),
                        )
                        .await
                        .unwrap();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    (log, stop)
}

fn wait<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(Instant::now() < deadline, "waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_unsaved_work_inhibitor_holds_off_logout_switching_and_suspend() {
    let bus = PrivateBus::start();
    let (log, _stop) = serve_portal(&bus.address);
    let mut inhibitor =
        Inhibitor::new(InhibitKinds::UNSAVED_WORK, "Unsaved changes").on_bus(bus.address.clone());
    inhibitor.set(true);
    wait("active", || {
        (inhibitor.state() == InhibitState::Active).then_some(())
    });
    // Logout 1, user switch 2, suspend 4; not idle 8.
    assert_eq!(log.lock().unwrap().inhibit_flags, [1 | 2 | 4]);
    inhibitor.set(false);
    wait("closed", || (log.lock().unwrap().closed == 1).then_some(()));
    assert!(InhibitKinds::UNSAVED_WORK.contains(InhibitKinds::SUSPEND));
    assert!(!InhibitKinds::UNSAVED_WORK.contains(InhibitKinds::IDLE));
}

/// Runs the session monitor against the fake portal, handing back the
/// events and the portal's log.
fn monitor(
    bus: &PrivateBus,
) -> (
    tokio::runtime::Runtime,
    tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    Arc<Mutex<PortalLog>>,
    std::sync::mpsc::Sender<()>,
) {
    let (log, stop) = serve_portal(&bus.address);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let address = bus.address.clone();
    runtime.spawn(async move {
        let connection = zbus::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        run_monitor(connection, sender).await.unwrap();
    });
    wait("monitor session", || log.lock().unwrap().session.clone());
    (runtime, receiver, log, stop)
}

fn next_event(
    runtime: &tokio::runtime::Runtime,
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
) -> SessionEvent {
    runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await })
        .unwrap()
        .unwrap()
}

#[test]
fn a_query_end_is_answered_when_the_application_is_done() {
    let bus = PrivateBus::start();
    let (runtime, mut events, log, _stop) = monitor(&bus);
    log.lock().unwrap().emit.push(2);
    let event = next_event(&runtime, &mut events);
    assert_eq!(event.state, SessionState::QueryEnd);
    let ack = event.ack.clone().expect("a query carries an ack");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        log.lock().unwrap().query_end_responses.is_empty(),
        "no answer while the application holds the ack"
    );
    let answered_at = Instant::now();
    ack.done();
    let response = wait("answer", || {
        log.lock().unwrap().query_end_responses.first().copied()
    });
    assert!(response.duration_since(answered_at) < Duration::from_millis(500));

    // Dropping every copy answers too.
    log.lock().unwrap().emit.push(2);
    let dropped = next_event(&runtime, &mut events);
    drop(ack);
    drop(event);
    drop(dropped);
    wait("answer on drop", || {
        (log.lock().unwrap().query_end_responses.len() == 2).then_some(())
    });

    // Ending carries no ack.
    log.lock().unwrap().emit.push(3);
    let ending = next_event(&runtime, &mut events);
    assert_eq!(ending.state, SessionState::Ending);
    assert!(ending.ack.is_none());
}

#[test]
fn an_unanswered_query_end_is_answered_before_the_portal_gives_up() {
    let bus = PrivateBus::start();
    let (runtime, mut events, log, _stop) = monitor(&bus);
    log.lock().unwrap().emit.push(2);
    let event = next_event(&runtime, &mut events);
    let asked = Instant::now();
    let _held = event;
    let response = wait("deadline answer", || {
        log.lock().unwrap().query_end_responses.first().copied()
    });
    let waited = response.duration_since(asked);
    assert!(
        waited > Duration::from_millis(600) && waited < Duration::from_millis(1000),
        "answered within the portal's second: {waited:?}"
    );
}

#[derive(Debug, Default)]
struct LoginLog {
    inhibits: Vec<(String, String)>,
    /// The other ends of handed-out delay locks.
    locks: Vec<std::os::unix::net::UnixStream>,
    emit: Vec<bool>,
}

struct FakeLogin1 {
    log: Arc<Mutex<LoginLog>>,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager", crate = "ashpd::zbus")]
impl FakeLogin1 {
    fn inhibit(
        &self,
        what: String,
        _who: String,
        _why: String,
        mode: String,
    ) -> ashpd::zvariant::OwnedFd {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        let mut log = self.log.lock().unwrap();
        log.inhibits.push((what, mode));
        log.locks.push(ours);
        ashpd::zvariant::OwnedFd::from(std::os::fd::OwnedFd::from(theirs))
    }
}

/// Whether the delay lock at `index` was released (its peer closed).
fn released(log: &Mutex<LoginLog>, index: usize) -> bool {
    use std::io::Read as _;
    let mut byte = [0u8; 1];
    matches!(log.lock().unwrap().locks[index].read(&mut byte), Ok(0))
}

#[test]
fn suspend_waits_for_the_application_and_resume_retakes_the_lock() {
    let system = PrivateBus::start();
    let log = Arc::new(Mutex::new(LoginLog::default()));
    let (address, served) = (system.address.clone(), Arc::clone(&log));
    let (ready, started) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let connection = zbus::connection::Builder::address(address.as_str())
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                connection
                    .object_server()
                    .at(
                        "/org/freedesktop/login1",
                        FakeLogin1 {
                            log: Arc::clone(&served),
                        },
                    )
                    .await
                    .unwrap();
                connection
                    .request_name("org.freedesktop.login1")
                    .await
                    .unwrap();
                ready.send(()).unwrap();
                loop {
                    let pending = std::mem::take(&mut served.lock().unwrap().emit);
                    for start in pending {
                        connection
                            .emit_signal(
                                None::<&str>,
                                "/org/freedesktop/login1",
                                "org.freedesktop.login1.Manager",
                                "PrepareForSleep",
                                &(start,),
                            )
                            .await
                            .unwrap();
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let address = system.address.clone();
    runtime.spawn(async move {
        let connection = zbus::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        run_watch(connection, "Notes", sender).await.unwrap();
    });
    wait("delay lock", || {
        (!log.lock().unwrap().inhibits.is_empty()).then_some(())
    });
    assert_eq!(
        log.lock().unwrap().inhibits[0],
        ("sleep".to_owned(), "delay".to_owned())
    );
    assert!(!released(&log, 0), "the lock is held before any suspend");

    log.lock().unwrap().emit.push(true);
    let event = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), events.recv()).await })
        .unwrap()
        .unwrap();
    let SleepEvent::Suspending(ack) = event else {
        panic!("expected a suspend")
    };
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !released(&log, 0),
        "suspend waits while the application works"
    );
    ack.done();
    assert!(released(&log, 0), "done releases the lock");

    log.lock().unwrap().emit.push(false);
    let resumed = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), events.recv()).await })
        .unwrap()
        .unwrap();
    assert!(matches!(resumed, SleepEvent::Resumed));
    assert_eq!(
        log.lock().unwrap().inhibits.len(),
        2,
        "the lock is taken again"
    );
}
