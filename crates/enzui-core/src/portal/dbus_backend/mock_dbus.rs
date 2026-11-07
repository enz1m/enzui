use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, Value};

use super::*;

struct PrivateBus {
    child: Option<std::process::Child>,
    address: String,
    _dir: enzui_test_support::test_scratch::ScratchDir,
}

impl PrivateBus {
    // Return `None` only when `dbus-daemon` is not installed; all other setup failures must panic.
    fn spawn() -> Option<Self> {
        Self::spawn_with_activation(None)
    }

    fn spawn_with_activation(command_line: Option<String>) -> Option<Self> {
        let dir = enzui_test_support::test_scratch::ScratchDir::new_restricted("pact");
        let socket = dir.join("bus");
        let address = format!("unix:path={}", socket.display());
        let service_dir = dir.join("services");
        std::fs::create_dir_all(&service_dir).expect("private service directory");
        if let Some(command_line) = command_line {
            let service = format!("[D-BUS Service]\nName={PORTAL_DEST}\nExec={command_line}\n");
            std::fs::write(
                service_dir.join("org.freedesktop.portal.Desktop.service"),
                service,
            )
            .expect("activation service file");
        }

        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig>\n  <type>session</type>\n  <listen>{address}</listen>\n  <servicedir>{}</servicedir>\n  <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    <allow receive_sender=\"*\"/>\n    <allow own=\"*\"/>\n    <allow eavesdrop=\"true\"/>\n  </policy>\n</busconfig>\n",
                service_dir.display()
            ),
        )
        .expect("private bus configuration");
        let daemon =
            std::env::var("ENZUI_TEST_DBUS_DAEMON").unwrap_or_else(|_| "dbus-daemon".to_owned());
        let mut command = std::process::Command::new(&daemon);
        command
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--nopidfile")
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => panic!(
                "dbus-daemon spawn failed with a non-NotFound error: {e}; \
                 only executable-not-found may skip, all other failures are broken fixtures"
            ),
        };

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !socket.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "dbus-daemon socket did not appear within 3 s at {}; \
                 this is a fixture failure, not a missing executable",
                socket.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        Some(Self {
            child: Some(child),
            address,
            _dir: dir,
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                _ => {
                    CHILD_REAPER.send(child).expect("private bus reaper");
                    return;
                }
            }
        }
    }
}

static CHILD_REAPER: LazyLock<std::sync::mpsc::Sender<std::process::Child>> = LazyLock::new(|| {
    let (sender, receiver) = std::sync::mpsc::channel::<std::process::Child>();
    std::thread::Builder::new()
        .name("enzui-portal-bus-reaper".to_owned())
        .spawn(move || {
            while let Ok(mut child) = receiver.recv() {
                let _ = child.wait();
            }
        })
        .expect("private bus reaper thread");
    sender
});

#[derive(Clone)]
enum Script {
    Success { uris: Vec<String>, delay_ms: u64 },
    Hang,
}

#[derive(Default, Debug)]
struct MockState {
    open_calls: u32,
    save_calls: u32,
    close_calls: u32,
    close_entry_at: Vec<std::time::Instant>,
    last_parent_window: Option<String>,
    last_title: Option<String>,
    last_current_folder: Option<Vec<u8>>,
    current_folder_wrong_type: bool,
    last_filters: Option<Vec<WireFilter>>,
    last_current_filter: Option<WireFilter>,
    last_current_name: Option<String>,
    /// OpenURI members called, in order, with the identity of the descriptor
    /// each one received.
    open_uri_calls: Vec<(&'static str, Option<(u64, u64)>)>,
}

type WireFilter = (String, Vec<(u32, String)>);

struct FileChooserMock {
    connection: zbus::Connection,
    script: Script,
    state: Arc<Mutex<MockState>>,
    gate: Option<Arc<OverlapGate>>,
    tag_counter: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser", crate = "ashpd::zbus")]
impl FileChooserMock {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }

    async fn open_file(
        &self,
        parent_window: String,
        title: String,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        {
            let mut s = self.state.lock().unwrap();
            s.open_calls += 1;
            record_call(&mut s, &parent_window, &title, &options);
        }
        self.dispatch(&options, &header).await
    }

    async fn save_file(
        &self,
        parent_window: String,
        title: String,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        {
            let mut s = self.state.lock().unwrap();
            s.save_calls += 1;
            record_call(&mut s, &parent_window, &title, &options);
        }
        self.dispatch(&options, &header).await
    }
}

/// The OpenURI interface, answering every request with success.
struct OpenUriMock {
    chooser: FileChooserMock,
}

fn descriptor_identity(fd: &zbus::zvariant::OwnedFd) -> Option<(u64, u64)> {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;
    let file = std::fs::File::from(fd.as_fd().try_clone_to_owned().ok()?);
    file.metadata().ok().map(|m| (m.dev(), m.ino()))
}

#[zbus::interface(name = "org.freedesktop.portal.OpenURI", crate = "ashpd::zbus")]
impl OpenUriMock {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }

    #[zbus(name = "OpenURI")]
    async fn open_uri(
        &self,
        _parent_window: String,
        _uri: String,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.chooser
            .state
            .lock()
            .unwrap()
            .open_uri_calls
            .push(("OpenURI", None));
        self.chooser.dispatch(&options, &header).await
    }

    async fn open_file(
        &self,
        _parent_window: String,
        fd: zbus::zvariant::OwnedFd,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let identity = descriptor_identity(&fd);
        self.chooser
            .state
            .lock()
            .unwrap()
            .open_uri_calls
            .push(("OpenFile", identity));
        self.chooser.dispatch(&options, &header).await
    }

    async fn open_directory(
        &self,
        _parent_window: String,
        fd: zbus::zvariant::OwnedFd,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let identity = descriptor_identity(&fd);
        self.chooser
            .state
            .lock()
            .unwrap()
            .open_uri_calls
            .push(("OpenDirectory", identity));
        self.chooser.dispatch(&options, &header).await
    }
}

impl FileChooserMock {
    // Reconstruct the request path from the caller sender and handle token, then schedule the scripted response there.
    async fn dispatch(
        &self,
        options: &HashMap<String, Value<'_>>,
        header: &zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let handle_token = options
            .get("handle_token")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| String::try_from(v).ok())
            .unwrap_or_else(|| "unknown".into());

        let sender = header
            .sender()
            .map(|s| s.as_str().to_owned())
            .unwrap_or_default();

        let request_path = reconstruct_request_path(&sender, &handle_token);
        let request_path_ret = request_path.clone();

        let conn = self.connection.clone();
        let script = self.script.clone();
        let state = Arc::clone(&self.state);
        let path_for_task = request_path.clone();
        let gate = self.gate.clone();
        let tag = self.tag_counter.fetch_add(1, Ordering::SeqCst);
        // Do not return the request path until its Request interface is registered, or an immediate Close can race registration.
        let (reg_tx, reg_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            // Register the Request interface at the computed path because zbus routes Close by exact object path.
            match gate {
                None => {
                    conn.object_server()
                        .at(path_for_task.as_str(), RequestMock { state })
                        .await
                        .expect("Request mock must register at the computed chooser path");
                }
                Some(gate) => {
                    conn.object_server()
                        .at(path_for_task.as_str(), SlowRequestMock { state, gate, tag })
                        .await
                        .expect("SlowRequest mock must register at the computed chooser path");
                }
            }
            let _ = reg_tx.send(());
            emit_scripted_response(&conn, &path_for_task, &script).await;
        });
        let _ = reg_rx.await;

        Ok(request_path_ret)
    }
}

// Decode values received by the mock frontend so assertions verify bytes that actually crossed D-Bus.
fn record_call(
    s: &mut MockState,
    parent_window: &str,
    title: &str,
    options: &HashMap<String, Value<'_>>,
) {
    s.last_parent_window = Some(parent_window.to_owned());
    s.last_title = Some(title.to_owned());

    s.current_folder_wrong_type = false;
    s.last_current_folder = None;
    if let Some(v) = options.get("current_folder") {
        match v.try_clone().ok().and_then(|v| Vec::<u8>::try_from(v).ok()) {
            Some(bytes) => s.last_current_folder = Some(bytes),
            // Present but not `ay` — a real portal would ignore it silently.
            None => s.current_folder_wrong_type = true,
        }
    }

    s.last_filters = options
        .get("filters")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<WireFilter>::try_from(v).ok());
    s.last_current_filter = options
        .get("current_filter")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| WireFilter::try_from(v).ok());
    s.last_current_name = options
        .get("current_name")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| String::try_from(v).ok());
}

// Reconstruct the subscribed request object path from sender and handle token.
fn reconstruct_request_path(sender: &str, handle_token: &str) -> OwnedObjectPath {
    let sanitized = sender.trim_start_matches(':').replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sanitized}/{handle_token}");
    OwnedObjectPath::try_from(path).expect("valid request path")
}

async fn emit_scripted_response(
    conn: &zbus::Connection,
    request_path: &OwnedObjectPath,
    script: &Script,
) {
    let (code, uris, delay): (u32, Vec<String>, u64) = match script {
        Script::Success { uris, delay_ms } => (0, uris.clone(), *delay_ms),
        Script::Hang => return,
    };
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    let mut results: HashMap<&str, Value<'_>> = HashMap::new();
    if !uris.is_empty() {
        results.insert("uris", Value::from(uris));
    }
    let _ = conn
        .emit_signal(
            None::<&str>,
            request_path.as_str(),
            "org.freedesktop.portal.Request",
            "Response",
            &(code, results),
        )
        .await;
}

struct RequestMock {
    state: Arc<Mutex<MockState>>,
}

#[zbus::interface(name = "org.freedesktop.portal.Request", crate = "ashpd::zbus")]
impl RequestMock {
    fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.close_calls += 1;
        s.close_entry_at.push(std::time::Instant::now());
    }
}

// Coordinate Close handlers with an explicit release gate rather than timing sleeps.
#[derive(Default)]
struct CloseOverlap {
    active: u32,
    max_active: u32,
    entered: Vec<u32>,
    // Entry timestamps distinguish concurrent closes from calls separated by the per-close timeout.
    entered_at: Vec<std::time::Instant>,
    completed: u32,
    released: bool,
}

// Pair a condition variable for the test thread with a Tokio notification for async Close handlers.
type OverlapGate = (Mutex<CloseOverlap>, std::sync::Condvar, tokio::sync::Notify);

// Decrement the active count and record completion exactly once on every handler exit.
struct ActiveGuard {
    gate: Arc<OverlapGate>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let (lock, cvar, _notify) = &*self.gate;
        let mut ov = lock.lock().unwrap();
        ov.active -= 1;
        ov.completed += 1;
        cvar.notify_all();
    }
}

// Block each Close handler on the shared gate so two entries can overlap before release.
struct SlowRequestMock {
    state: Arc<Mutex<MockState>>,
    gate: Arc<OverlapGate>,
    tag: u32,
}

#[zbus::interface(name = "org.freedesktop.portal.Request", crate = "ashpd::zbus")]
impl SlowRequestMock {
    async fn close(&self) {
        // Record handler entry immediately and hold a guard that decrements `active` on every exit.
        let _guard = {
            let (lock, cvar, _notify) = &*self.gate;
            let mut ov = lock.lock().unwrap();
            ov.active += 1;
            if ov.active > ov.max_active {
                ov.max_active = ov.active;
            }
            ov.entered.push(self.tag);
            ov.entered_at.push(std::time::Instant::now());
            cvar.notify_all();
            ActiveGuard {
                gate: Arc::clone(&self.gate),
            }
        };

        // Enable notification interest before checking `released` so a concurrent release wake cannot be lost.
        let (lock, _cvar, notify) = &*self.gate;
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if lock.lock().unwrap().released {
                break;
            }
            notified.await;
        }

        self.state.lock().unwrap().close_calls += 1;
    }
}

// Start a private mock portal and return `None` only when `dbus-daemon` is unavailable.
async fn start_mock(
    script: Script,
    state: Arc<Mutex<MockState>>,
) -> Option<(PrivateBus, zbus::Connection, String)> {
    start_mock_gated(script, state, None)
        .await
        .map(|(bus, conn, addr, _gate)| (bus, conn, addr))
}

// With a gate, each Request.Close blocks until the test releases it.
async fn start_mock_gated(
    script: Script,
    state: Arc<Mutex<MockState>>,
    gate: Option<Arc<OverlapGate>>,
) -> Option<(
    PrivateBus,
    zbus::Connection,
    String,
    Option<Arc<OverlapGate>>,
)> {
    let bus = PrivateBus::spawn()?;
    let address = bus.address.clone();

    // Once the daemon starts, treat every connection, registration, or name-ownership failure as a broken fixture rather than a skip.
    let conn = zbus::connection::Builder::address(address.as_str())
        .expect("private bus address must be valid")
        .build()
        .await
        .expect("mock portal must connect to its private bus");

    let file_chooser = FileChooserMock {
        connection: conn.clone(),
        script,
        state: Arc::clone(&state),
        gate: gate.clone(),
        tag_counter: Arc::new(AtomicU32::new(0)),
    };
    conn.object_server()
        .at("/org/freedesktop/portal/desktop", file_chooser)
        .await
        .expect("FileChooser mock must register at the portal path");
    let open_uri = OpenUriMock {
        chooser: FileChooserMock {
            connection: conn.clone(),
            script: Script::Success {
                uris: Vec::new(),
                delay_ms: 0,
            },
            state: Arc::clone(&state),
            gate: None,
            tag_counter: Arc::new(AtomicU32::new(1_000)),
        },
    };
    conn.object_server()
        .at("/org/freedesktop/portal/desktop", open_uri)
        .await
        .expect("OpenURI mock must register at the portal path");

    conn.request_name("org.freedesktop.portal.Desktop")
        .await
        .expect("mock portal must own the well-known name");

    Some((bus, conn, address, gate))
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime")
}

#[test]
fn activation_service_child() {
    if std::env::var_os("ENZUI_PORTAL_ACTIVATION_FIXTURE").is_none() {
        return;
    }
    let address = std::env::var("DBUS_STARTER_ADDRESS")
        .expect("activation fixture must receive DBUS_STARTER_ADDRESS");
    let runtime = rt();
    runtime.block_on(async move {
        let state = Arc::new(Mutex::new(MockState::default()));
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("activation address")
            .build()
            .await
            .expect("activated portal connection");
        connection
            .object_server()
            .at(
                PORTAL_PATH,
                FileChooserMock {
                    connection: connection.clone(),
                    script: Script::Success {
                        uris: vec!["file:///tmp/activated-portal.txt".to_owned()],
                        delay_ms: 0,
                    },
                    state: Arc::clone(&state),
                    gate: None,
                    tag_counter: Arc::new(AtomicU32::new(0)),
                },
            )
            .await
            .expect("activated FileChooser registration");
        connection
            .request_name(PORTAL_DEST)
            .await
            .expect("activated portal name");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while state.lock().unwrap().open_calls == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "activated portal did not receive its first request"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });
}

fn start_when_ready(
    backend: &mut DbusBackend,
    operation: PortalOperation,
) -> Result<RequestId, BackendError> {
    start_with_options_when_ready(
        backend,
        operation,
        &ChooserOptions::new(),
        &ParentHandle::none(),
    )
}

fn start_with_options_when_ready(
    backend: &mut DbusBackend,
    operation: PortalOperation,
    options: &ChooserOptions,
    parent: &ParentHandle,
) -> Result<RequestId, BackendError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match backend.start_with_options(operation, options, parent) {
            Err(BackendError::Initialising) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

macro_rules! require_mock {
    ($script:expr, $state:expr) => {{
        let runtime = rt();
        let started = runtime.block_on(start_mock($script, Arc::clone(&$state)));
        match started {
            Some(v) => (runtime, v),
            None => {
                println!("skipping: dbus-daemon unavailable");
                return;
            }
        }
    }};
}

mod flows;
