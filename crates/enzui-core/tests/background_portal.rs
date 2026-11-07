//! The Background portal helper against a fake portal on a private bus.
//!
//! The fake `org.freedesktop.portal.Background` records what the helper sends
//! and answers through the request object, as xdg-desktop-portal does.

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
#[path = "background_portal/file_transfer.rs"]
mod file_transfer;

use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, OwnedValue, Value};
use enzui_core::portal::background::{BackgroundPortal, BackgroundRequest, BackgroundState};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

/// A private directory removed on drop.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("enzui-bg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct PrivateBus {
    child: Child,
    address: String,
    _root: Scratch,
}

impl PrivateBus {
    fn start() -> Self {
        let root = Scratch::new();
        let socket = root.path().join("bus");
        let address = format!("unix:path={}", socket.display());
        let config = root.path().join("bus.conf");
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
            .arg("--nofork")
            .arg("--nopidfile")
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
            _root: root,
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Default)]
struct Calls {
    requests: Vec<HashMap<String, OwnedValue>>,
    statuses: Vec<String>,
}

struct FakeBackground {
    calls: Arc<Mutex<Calls>>,
    /// The `response` code the fake answers with: 0 granted, 1 cancelled.
    response: u32,
}

#[zbus::interface(name = "org.freedesktop.portal.Background", crate = "ashpd::zbus")]
impl FakeBackground {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    fn request_background(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
    ) -> OwnedObjectPath {
        let sender = header
            .sender()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let token = options
            .get("handle_token")
            .and_then(|value| String::try_from(value.try_clone().unwrap()).ok())
            .unwrap();
        let autostart = options
            .get("autostart")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false);
        self.calls.lock().unwrap().requests.push(options);
        let path =
            OwnedObjectPath::try_from(format!("{PORTAL_PATH}/request/{sender}/{token}")).unwrap();
        let (connection, reply_path, response) = (connection.clone(), path.clone(), self.response);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut results: HashMap<&str, Value<'_>> = HashMap::new();
            results.insert("background", Value::from(response == 0));
            results.insert("autostart", Value::from(autostart && response == 0));
            let _ = connection
                .emit_signal(
                    None::<&str>,
                    reply_path.as_str(),
                    "org.freedesktop.portal.Request",
                    "Response",
                    &(response, results),
                )
                .await;
        });
        path
    }

    fn set_status(&self, options: HashMap<String, OwnedValue>) {
        let message = options
            .get("message")
            .and_then(|value| String::try_from(value.try_clone().unwrap()).ok())
            .unwrap_or_default();
        self.calls.lock().unwrap().statuses.push(message);
    }
}

/// Serves the fake portal on its own thread until the returned guard drops.
fn serve(address: &str, response: u32) -> (Arc<Mutex<Calls>>, std::sync::mpsc::Sender<()>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let (ready, started) = std::sync::mpsc::channel();
    let (address, served) = (address.to_owned(), Arc::clone(&calls));
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
                    FakeBackground {
                        calls: served,
                        response,
                    },
                )
                .await
                .unwrap();
            connection.request_name(PORTAL_DEST).await.unwrap();
            ready.send(()).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    (calls, stop)
}

fn wait_for(portal: &BackgroundPortal, done: impl Fn(BackgroundState) -> bool) -> BackgroundState {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = portal.state();
        if done(state) {
            return state;
        }
        assert!(Instant::now() < deadline, "stuck at {state:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_refusal_is_denied_and_a_missing_portal_is_unavailable() {
    let bus = PrivateBus::start();
    let (_calls, _stop) = serve(&bus.address, 1);
    let mut portal = BackgroundPortal::with_address(Some(bus.address.clone()));
    portal.request(BackgroundRequest::new("Syncing"));
    assert_eq!(
        wait_for(&portal, |state| state != BackgroundState::Requested),
        BackgroundState::Denied
    );

    let empty = PrivateBus::start();
    let mut portal = BackgroundPortal::with_address(Some(empty.address.clone()));
    portal.request(BackgroundRequest::new("Syncing"));
    assert_eq!(
        wait_for(&portal, |state| state != BackgroundState::Requested),
        BackgroundState::Unavailable
    );
}
