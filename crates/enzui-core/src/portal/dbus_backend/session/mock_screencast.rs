use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::*;
use crate::portal::session::{HandshakeStep, SessionBackend, SessionError};

const PRIVATE_BUS_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const PRIVATE_BUS_TEARDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

struct ReapRequest {
    child: std::process::Child,
    completion: Option<std::sync::mpsc::Sender<ReapResult>>,
}

#[allow(dead_code, reason = "tests only wait for the reap report")]
struct ReapResult {
    pid: u32,
    status: std::process::ExitStatus,
}

// Owns any child that bounded teardown cannot reap and performs the blocking wait.
struct ChildReaper {
    tx: std::sync::mpsc::Sender<ReapRequest>,
    // Dropping the handle detaches the live reaper; dropping `tx` then lets it exit.
    _thread: std::thread::JoinHandle<()>,
}

impl ChildReaper {
    fn start() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<ReapRequest>();
        let thread = std::thread::Builder::new()
            .name("enzui-private-bus-reaper".into())
            .spawn(move || {
                while let Ok(mut request) = rx.recv() {
                    let pid = request.child.id();
                    let status = loop {
                        match request.child.wait() {
                            Ok(status) => break status,
                            Err(_) => {
                                // Only the dedicated reaper thread may perform this unbounded wait.
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        }
                    };
                    if let Some(completion) = request.completion {
                        let _ = completion.send(ReapResult { pid, status });
                    }
                }
            })
            .unwrap_or_else(|error| {
                panic!("private-bus fixture: could not establish child reaper: {error}")
            });
        Self {
            tx,
            _thread: thread,
        }
    }

    fn transfer(
        &self,
        child: std::process::Child,
        completion: Option<std::sync::mpsc::Sender<ReapResult>>,
    ) {
        if let Err(error) = self.tx.send(ReapRequest { child, completion }) {
            // If the reaper is unavailable, retain the child and wait for it before failing.
            let mut request = error.0;
            let pid = request.child.id();
            let status = loop {
                match request.child.wait() {
                    Ok(status) => break status,
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            };
            if let Some(completion) = request.completion {
                let _ = completion.send(ReapResult { pid, status });
            }
            panic!("private-bus fixture: child reaper stopped before handoff");
        }
    }
}

struct PrivateBus {
    child: Option<std::process::Child>,
    reaper: ChildReaper,
    address: String,
    _dir: std::path::PathBuf,
}

impl PrivateBus {
    fn spawn() -> Option<Self> {
        // `ENZUI_TEST_DBUS_DAEMON` overrides the executable for the unavailable-daemon fixture.
        let daemon = match std::env::var("ENZUI_TEST_DBUS_DAEMON") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "dbus-daemon".to_owned(),
            Err(error) => panic!("private-bus fixture: ENZUI_TEST_DBUS_DAEMON is invalid: {error}"),
        };
        Self::spawn_with_daemon(&daemon, true, &[], PRIVATE_BUS_READY_TIMEOUT)
    }

    fn spawn_with_daemon(
        daemon: &str,
        expect_socket: bool,
        extra_args: &[&str],
        ready_timeout: std::time::Duration,
    ) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!(
            "enzui-portal-sc-mock-{}-{}",
            std::process::id(),
            next_counter()
        ));
        create_private_bus_dir(&dir);
        let socket = dir.join("bus");
        let address = format!("unix:path={}", socket.display());

        // Establish the fallback reaper before spawning so every child has a wait owner.
        let reaper = ChildReaper::start();
        let mut cmd = std::process::Command::new(daemon);
        if expect_socket {
            let service_dir = dir.join("services");
            create_private_bus_dir(&service_dir);
            let config = dir.join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    "<busconfig>\n  <type>session</type>\n  <listen>{address}</listen>\n  <servicedir>{}</servicedir>\n  <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    <allow receive_sender=\"*\"/>\n    <allow own=\"*\"/>\n    <allow eavesdrop=\"true\"/>\n  </policy>\n</busconfig>\n",
                    service_dir.display()
                ),
            )
            .expect("private bus configuration");
            cmd.arg(format!("--config-file={}", config.display()))
                .arg("--nofork")
                .arg("--nopidfile");
        } else {
            cmd.args(extra_args);
        }
        let child = match cmd
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if expect_socket && error.kind() == std::io::ErrorKind::NotFound => {
                return None;
            }
            Err(error) => {
                panic!("private-bus fixture: failed to spawn {daemon:?}: {error}")
            }
        };

        let mut bus = Self {
            child: Some(child),
            reaper,
            address,
            _dir: dir,
        };

        let deadline = std::time::Instant::now() + ready_timeout;
        while !socket.exists() {
            if std::time::Instant::now() >= deadline {
                // On readiness timeout, kill and transfer or reap the child before panicking.
                bus.kill_and_wait_bounded();
                panic!(
                    "private-bus fixture: daemon spawned but its socket \
                     ({}) did not appear within {}ms — fail-closed fixture bug",
                    socket.display(),
                    ready_timeout.as_millis()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Some(bus)
    }

    fn kill_and_wait_bounded(&mut self) {
        self.kill_and_wait_with(PRIVATE_BUS_TEARDOWN_TIMEOUT, None);
    }

    fn kill_and_wait_with(
        &mut self,
        timeout: std::time::Duration,
        completion: Option<std::sync::mpsc::Sender<ReapResult>>,
    ) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let pid = child.id();
        let _ = child.kill();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                self.reaper.transfer(child, completion);
                return;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(completion) = completion {
                        let _ = completion.send(ReapResult { pid, status });
                    }
                    return;
                }
                Ok(None) => {}
                Err(_) => {
                    self.reaper.transfer(child, completion);
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        self.kill_and_wait_bounded();
    }
}

static COUNTER: AtomicU32 = AtomicU32::new(0);
fn next_counter() -> u32 {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn create_private_bus_dir(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|error| {
        panic!(
            "private-bus fixture: could not create temporary directory {}: {error}",
            dir.display()
        )
    });
}

#[derive(Clone)]
enum Script {
    Success,
    HangAtStart,
}

#[derive(Default, Debug)]
struct MockState {
    create_calls: u32,
    select_calls: u32,
    start_calls: u32,
    close_calls: u32,
    open_pw_calls: u32,
    session_handle: Option<String>,
    // The `parent_window` argument the Start call carried, as it arrived on
    // the wire. `None` means Start was never called.
    start_parent_window: Option<String>,
}

// Atomics allow timeout observers to run without the mock-state lock; guard drop records handler retirement.
#[derive(Default)]
struct PipeWireControl {
    stall_ms: AtomicU64,
    entered: AtomicU64,
    active: AtomicU64,
    completed: AtomicU64,
    // An injected fd is consumed once; later calls use a fresh pipe.
    injected_fd: Mutex<Option<std::os::fd::OwnedFd>>,
}

struct PipeWireHandlerGuard {
    pw: Arc<PipeWireControl>,
}

impl PipeWireHandlerGuard {
    fn enter(pw: &Arc<PipeWireControl>) -> Self {
        pw.active.fetch_add(1, Ordering::SeqCst);
        pw.entered.fetch_add(1, Ordering::SeqCst);
        Self { pw: Arc::clone(pw) }
    }
}

impl Drop for PipeWireHandlerGuard {
    fn drop(&mut self) {
        self.pw.active.fetch_sub(1, Ordering::SeqCst);
        self.pw.completed.fetch_add(1, Ordering::SeqCst);
    }
}

struct ScreenCastMock {
    connection: zbus::Connection,
    script: Script,
    state: Arc<Mutex<MockState>>,
    pw: Arc<PipeWireControl>,
}

#[zbus::interface(name = "org.freedesktop.portal.ScreenCast", crate = "ashpd::zbus")]
impl ScreenCastMock {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }

    #[zbus(property, name = "AvailableSourceTypes")]
    fn available_source_types(&self) -> u32 {
        7
    }

    async fn create_session(
        &self,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.state.lock().unwrap().create_calls += 1;
        let sender = sender_of(&header);
        let session_token = options
            .get("session_handle_token")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok())
            .unwrap_or_else(|| "sess".into());
        let sanitized = sender.trim_start_matches(':').replace('.', "_");
        let session_path =
            format!("/org/freedesktop/portal/desktop/session/{sanitized}/{session_token}");
        self.state.lock().unwrap().session_handle = Some(session_path.clone());

        let session_mock = SessionMock {
            state: Arc::clone(&self.state),
        };
        self.connection
            .object_server()
            .at(session_path.as_str(), session_mock)
            .await
            .expect("Session mock must register at the returned session path");

        let mut results: HashMap<String, OwnedValue> = HashMap::new();
        results.insert(
            "session_handle".into(),
            OwnedValue::try_from(Value::from(session_path.as_str())).unwrap(),
        );
        self.emit_response(&options, &header, 0, results).await;
        request_path_ret(&options, &header)
    }

    async fn select_sources(
        &self,
        _session_handle: OwnedObjectPath,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.state.lock().unwrap().select_calls += 1;
        let code = self.code_for(HandshakeStep::SelectSources);
        self.emit_response(&options, &header, code, HashMap::new())
            .await;
        request_path_ret(&options, &header)
    }

    async fn start(
        &self,
        _session_handle: OwnedObjectPath,
        parent_window: String,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        {
            // Recorded before any scripted early return, so the observation is
            // of what the caller sent rather than of how the mock answered.
            let mut state = self.state.lock().unwrap();
            state.start_calls += 1;
            state.start_parent_window = Some(parent_window);
        }
        if matches!(self.script, Script::HangAtStart) {
            return request_path_ret(&options, &header);
        }
        let code = self.code_for(HandshakeStep::Start);
        if code == 0 {
            let mut props: HashMap<String, OwnedValue> = HashMap::new();
            props.insert(
                "position".into(),
                OwnedValue::try_from(Value::from((0i32, 0i32))).unwrap(),
            );
            props.insert(
                "size".into(),
                OwnedValue::try_from(Value::from((1920i32, 1080i32))).unwrap(),
            );
            let streams = vec![(99u32, props)];
            let mut results: HashMap<String, OwnedValue> = HashMap::new();
            results.insert(
                "streams".into(),
                OwnedValue::try_from(Value::from(streams)).unwrap(),
            );
            self.emit_response(&options, &header, 0, results).await;
        } else {
            self.emit_response(&options, &header, code, HashMap::new())
                .await;
        }
        request_path_ret(&options, &header)
    }

    // zbus's default conversion spells `Pipewire`; pin the portal's exact `PipeWire` member name.
    #[zbus(name = "OpenPipeWireRemote")]
    async fn open_pipewire_remote(
        &self,
        _session_handle: OwnedObjectPath,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<ashpd::zvariant::OwnedFd> {
        let _lifecycle = PipeWireHandlerGuard::enter(&self.pw);
        self.state.lock().unwrap().open_pw_calls += 1;
        let stall_ms = self.pw.stall_ms.load(Ordering::SeqCst);
        if stall_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(stall_ms)).await;
        }
        let injected = self.pw.injected_fd.lock().unwrap().take();
        if let Some(owned) = injected {
            return Ok(ashpd::zvariant::OwnedFd::from(owned));
        }
        use std::os::fd::FromRawFd;
        let (read, write) = crate::portal::pipewire::test_support::make_pipe();
        crate::portal::pipewire::test_support::close_raw(write);
        // SAFETY: `pipe2` returned `read` as a fresh descriptor with no existing owner; this constructs its sole owning wrapper.
        #[allow(unsafe_code, reason = "adopts the fresh pipe descriptor")]
        let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(read) };
        Ok(ashpd::zvariant::OwnedFd::from(owned))
    }
}

impl ScreenCastMock {
    fn code_for(&self, _step: HandshakeStep) -> u32 {
        0
    }

    // Emit inline before returning the method reply so a late subscription deterministically misses the signal.
    async fn emit_response(
        &self,
        options: &HashMap<String, Value<'_>>,
        header: &zbus::message::Header<'_>,
        code: u32,
        results: HashMap<String, OwnedValue>,
    ) {
        let request_path = request_path_for(options, header);
        let _ = self
            .connection
            .emit_signal(
                None::<&str>,
                request_path.as_str(),
                "org.freedesktop.portal.Request",
                "Response",
                &(code, results),
            )
            .await;
    }
}

struct SessionMock {
    state: Arc<Mutex<MockState>>,
}

#[zbus::interface(name = "org.freedesktop.portal.Session", crate = "ashpd::zbus")]
impl SessionMock {
    fn close(&self) {
        self.state.lock().unwrap().close_calls += 1;
    }
}

fn sender_of(header: &zbus::message::Header<'_>) -> String {
    header
        .sender()
        .map(|s| s.as_str().to_owned())
        .unwrap_or_default()
}

fn request_path_for(
    options: &HashMap<String, Value<'_>>,
    header: &zbus::message::Header<'_>,
) -> OwnedObjectPath {
    let handle_token = options
        .get("handle_token")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_else(|| "unknown".into());
    let sender = sender_of(header);
    let sanitized = sender.trim_start_matches(':').replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sanitized}/{handle_token}");
    OwnedObjectPath::try_from(path).expect("valid request path")
}

fn request_path_ret(
    options: &HashMap<String, Value<'_>>,
    header: &zbus::message::Header<'_>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    Ok(request_path_for(options, header))
}

const PRIVATE_BUS_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const PRIVATE_BUS_REGISTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const PRIVATE_BUS_REQUEST_NAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

// After the daemon starts, setup failures panic and every async setup step is bounded.
async fn attach_mock(
    address: &str,
    script: Script,
    state: Arc<Mutex<MockState>>,
    pw: Arc<PipeWireControl>,
) -> zbus::Connection {
    let conn = tokio::time::timeout(
        PRIVATE_BUS_CONNECT_TIMEOUT,
        zbus::connection::Builder::address(address)
            .expect("private bus address must be valid")
            .build(),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "private-bus fixture: zbus connect exceeded {}s — fail-closed \
             (dbus-daemon alive but handshake hung)",
            PRIVATE_BUS_CONNECT_TIMEOUT.as_secs()
        )
    })
    .expect("ScreenCast mock must connect to its private bus");

    let screencast = ScreenCastMock {
        connection: conn.clone(),
        script,
        state: Arc::clone(&state),
        pw: Arc::clone(&pw),
    };
    tokio::time::timeout(
        PRIVATE_BUS_REGISTER_TIMEOUT,
        conn.object_server()
            .at("/org/freedesktop/portal/desktop", screencast),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "private-bus fixture: registering the ScreenCast mock exceeded {}s",
            PRIVATE_BUS_REGISTER_TIMEOUT.as_secs()
        )
    })
    .expect("ScreenCast mock must register at the portal path");

    tokio::time::timeout(
        PRIVATE_BUS_REQUEST_NAME_TIMEOUT,
        conn.request_name("org.freedesktop.portal.Desktop"),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "private-bus fixture: RequestName exceeded {}s — fail-closed",
            PRIVATE_BUS_REQUEST_NAME_TIMEOUT.as_secs()
        )
    })
    .expect("ScreenCast mock must own the well-known name");

    conn
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn drive_session(
    session: &mut crate::portal::session_op::ScreencastSession,
    backend: &mut DbusSessionBackend,
    timeout: std::time::Duration,
) -> Option<crate::portal::session_op::SessionOutcome> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(out) = session.poll(backend) {
            return Some(out);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

use crate::portal::session_op::{ScreencastRequest, SessionOutcome};

#[test]
fn session_late_owner_loss_and_replacement_recover_without_restart() {
    let Some(bus) = PrivateBus::spawn() else {
        println!("skipping: dbus-daemon unavailable");
        return;
    };
    let runtime = rt();
    let mut backend =
        DbusSessionBackend::with_address(Some(bus.address.clone())).expect("session backend");
    assert!(backend.wait_until_settled(std::time::Duration::from_secs(5)));
    assert_eq!(backend.create_session(), Err(SessionError::Unavailable));

    let old_state = Arc::new(Mutex::new(MockState::default()));
    let old_connection = runtime.block_on(attach_mock(
        &bus.address,
        Script::HangAtStart,
        Arc::clone(&old_state),
        Arc::new(PipeWireControl::default()),
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut old_session = loop {
        match ScreencastRequest::new().start(&mut backend) {
            Ok(session) => break session,
            Err(SessionOutcome::Unavailable) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => panic!("late owner did not become usable: {other:?}"),
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while old_state.lock().unwrap().start_calls == 0 {
        let outcome = old_session.poll(&mut backend);
        assert!(outcome.is_none(), "hanging owner returned {outcome:?}");
        assert!(
            std::time::Instant::now() < deadline,
            "Start must reach old owner"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    runtime
        .block_on(old_connection.release_name(PORTAL_DEST))
        .expect("release old portal name");
    assert_eq!(
        drive_session(
            &mut old_session,
            &mut backend,
            std::time::Duration::from_secs(5)
        ),
        Some(SessionOutcome::Revoked),
        "owner loss must terminate the old session"
    );

    let new_state = Arc::new(Mutex::new(MockState::default()));
    let _new_connection = runtime.block_on(attach_mock(
        &bus.address,
        Script::Success,
        Arc::clone(&new_state),
        Arc::new(PipeWireControl::default()),
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut replacement = loop {
        match ScreencastRequest::new().start(&mut backend) {
            Ok(session) => break session,
            Err(SessionOutcome::Unavailable) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => panic!("replacement owner did not become usable: {other:?}"),
        }
    };
    let outcome = drive_session(
        &mut replacement,
        &mut backend,
        std::time::Duration::from_secs(8),
    );
    assert!(
        matches!(outcome, Some(SessionOutcome::Completed(_))),
        "replacement generation must complete independently, got {outcome:?}"
    );
    assert_eq!(new_state.lock().unwrap().start_calls, 1);
}
