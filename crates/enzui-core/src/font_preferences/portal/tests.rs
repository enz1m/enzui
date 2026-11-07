//! Real-transport tests for the settings-portal font source.
//!
//! Every test here drives the shipped [`PortalFontBackend`] against a private
//! `dbus-daemon` and a `zbus` server that implements
//! `org.freedesktop.portal.Settings`. No host service is contacted, no host
//! preference is read or written, and the bus dies with the fixture.
//!
//! These compile only under the `desktop-font-portal` feature (enabled by widgets defaults),
//! the same gate the other live bus backends use. A host without
//! `dbus-daemon` fails the fixture rather than skipping it: a test that
//! reports nothing must not read as a green transport.

use super::*;
use enzui_theme::font_source::DesktopFontBackend;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static BUS_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A private session bus, torn down with this value.
struct PrivateBus {
    child: std::process::Child,
    address: String,
    directory: std::path::PathBuf,
}

impl PrivateBus {
    fn spawn() -> Option<PrivateBus> {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)?;
        let directory = workspace.join(format!(
            ".fp-{}-{}",
            std::process::id(),
            BUS_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&directory).expect("create the private-bus directory");
        let runtime = directory.join("runtime");
        std::fs::create_dir(&runtime).expect("create the private XDG runtime");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
                .expect("set private XDG runtime permissions");
        }
        let socket = directory.join("bus");
        let address = format!("unix:path={}", socket.display());

        let mut child = match std::process::Command::new("dbus-daemon")
            .env("XDG_RUNTIME_DIR", &runtime)
            .arg("--session")
            .arg(format!("--address={address}"))
            .arg("--nofork")
            .arg("--nopidfile")
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_dir_all(&directory);
                return None;
            }
            Err(error) => panic!("spawn dbus-daemon: {error}"),
        };

        let deadline = Instant::now() + Duration::from_secs(3);
        while !socket.exists() {
            if let Some(status) = child.try_wait().expect("inspect dbus-daemon") {
                let _ = std::fs::remove_dir_all(&directory);
                panic!("dbus-daemon exited before binding its socket: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "dbus-daemon did not create its socket within 3s"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        Some(PrivateBus {
            child,
            address,
            directory,
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The values a fake portal serves, and the signal it emits on change.
#[derive(Clone)]
struct FakeSettings {
    values: Arc<std::sync::Mutex<HashMap<String, String>>>,
    namespace: String,
    supports_read_one: bool,
    // Delay each method response beyond the call timeout to exercise the
    // TimedOut → Failed mapping.
    delay: Option<Duration>,
}

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl FakeSettings {
    async fn read_one(
        &self,
        namespace: &str,
        key: &str,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedValue> {
        if !self.supports_read_one {
            return Err(zbus::fdo::Error::UnknownMethod("ReadOne".into()));
        }
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        self.lookup(namespace, key)
    }

    async fn read(
        &self,
        namespace: &str,
        key: &str,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedValue> {
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        // The v1 method wraps its answer in a second variant.
        let inner = self.lookup(namespace, key)?;
        zbus::zvariant::OwnedValue::try_from(Value::new(Value::from(inner)))
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;
}

impl FakeSettings {
    fn new(namespace: &str, entries: &[(&str, &str)]) -> Self {
        Self {
            values: Arc::new(std::sync::Mutex::new(
                entries
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect(),
            )),
            namespace: namespace.to_owned(),
            supports_read_one: true,
            delay: None,
        }
    }

    fn lookup(&self, namespace: &str, key: &str) -> zbus::fdo::Result<zbus::zvariant::OwnedValue> {
        if namespace != self.namespace {
            return Err(zbus::fdo::Error::Failed(
                "org.freedesktop.portal.Error.NotFound".into(),
            ));
        }
        let values = self
            .values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match values.get(key) {
            Some(text) => zbus::zvariant::OwnedValue::try_from(Value::from(text.as_str()))
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string())),
            None => Err(zbus::fdo::Error::Failed(
                "org.freedesktop.portal.Error.NotFound".into(),
            )),
        }
    }
}

/// A running fake portal: a dedicated thread owning a current-thread runtime,
/// the served connection, and the well-known portal name.
///
/// The thread is what keeps the fixture honest on a *current-thread* runtime:
/// the server's tasks only progress while something is awaited on that
/// runtime, so the loop awaits its command channel instead of blocking the
/// thread, and the connection keeps answering the whole time.
#[allow(dead_code, reason = "fixture fields are written but never read")]
struct FakePortal {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    worker: Option<std::thread::JoinHandle<()>>,
    namespace: String,
    address: String,
    shared: Arc<std::sync::Mutex<HashMap<String, String>>>,
    _bus: PrivateBus,
}

#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Command {
    /// Set a key and emit `SettingChanged` for it.
    Set {
        namespace: String,
        key: String,
        value: String,
        done: std::sync::mpsc::SyncSender<()>,
    },
    Stop,
}

impl FakePortal {
    fn start(settings: FakeSettings) -> Option<FakePortal> {
        let bus = PrivateBus::spawn()?;
        let address = bus.address.clone();
        let namespace = settings.namespace.clone();
        let shared = Arc::clone(&settings.values);
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Command>();
        let (ready, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let served = settings;
        let values = Arc::clone(&shared);
        let thread_address = address.clone();

        let worker = std::thread::Builder::new()
            .name("enzui-fake-portal".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build the fixture runtime");
                runtime.block_on(async move {
                    let connection =
                        match zbus::connection::Builder::address(thread_address.as_str())
                            .and_then(|builder| builder.name("org.freedesktop.portal.Desktop"))
                            .and_then(|builder| {
                                builder.serve_at("/org/freedesktop/portal/desktop", served)
                            }) {
                            Ok(builder) => match builder.build().await {
                                Ok(connection) => connection,
                                Err(error) => {
                                    let _ = ready.send(Err(error.to_string()));
                                    return;
                                }
                            },
                            Err(error) => {
                                let _ = ready.send(Err(error.to_string()));
                                return;
                            }
                        };
                    let _ = ready.send(Ok(()));

                    while let Some(command) = receiver.recv().await {
                        match command {
                            Command::Stop => break,
                            Command::Set {
                                namespace,
                                key,
                                value,
                                done,
                            } => {
                                values.lock().unwrap().insert(key.clone(), value.clone());
                                let emitter = zbus::object_server::SignalEmitter::new(
                                    &connection,
                                    "/org/freedesktop/portal/desktop",
                                )
                                .expect("build the signal emitter");
                                FakeSettings::setting_changed(
                                    &emitter,
                                    &namespace,
                                    &key,
                                    Value::from(value.as_str()),
                                )
                                .await
                                .expect("emit SettingChanged");
                                let _ = done.send(());
                            }
                        }
                    }
                });
            })
            .expect("spawn the fixture thread");

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("the fake portal failed to start: {error}"),
            Err(_) => panic!("the fake portal did not start within 5s"),
        }

        Some(FakePortal {
            commands,
            worker: Some(worker),
            namespace,
            address,
            shared,
            _bus: bus,
        })
    }

    fn backend(&self) -> PortalFontBackend {
        PortalFontBackend::with_namespace(
            self.namespace.clone(),
            GNOME_INTERFACE_KEYS.iter().map(|(role, key)| (*role, *key)),
        )
        .on_bus(self.address.clone())
    }
}

impl Drop for FakePortal {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn gnome_portal() -> Option<FakePortal> {
    FakePortal::start(FakeSettings::new(
        GNOME_INTERFACE_NAMESPACE,
        &[
            ("font-name", "Adwaita Sans 11"),
            ("document-font-name", "Adwaita Sans 12"),
            ("monospace-font-name", "Adwaita Mono 11"),
        ],
    ))
}

#[test]
fn the_portal_delivers_all_three_gnome_roles() {
    if let Some(root) = std::env::var_os("ENZUI_DEFAULT_FONT_FIXTURE") {
        default_font_host(std::path::Path::new(&root));
        return;
    }
    let Some(portal) = gnome_portal() else {
        panic!("dbus-daemon is required for this test");
    };
    let backend = portal.backend();
    let read = backend.read();
    let preferences = read.preferences().expect("the portal answered");
    assert_eq!(
        preferences.family(FontRole::Ui),
        Some(enzui_theme::FontFamily::Named("Adwaita Sans".into()))
    );
    assert_eq!(
        preferences.family(FontRole::Monospace),
        Some(enzui_theme::FontFamily::Named("Adwaita Mono".into()))
    );
    assert_eq!(
        preferences
            .description(FontRole::Document)
            .map(enzui_theme::RawFontDescription::as_str),
        Some("Adwaita Sans 12"),
        "the description must survive the transport verbatim"
    );
    check_default_font_host(&portal, false);
    portal.shared.lock().unwrap().clear();
    check_default_font_host(&portal, true);
}

fn default_font_host(root: &std::path::Path) {
    use crate::theme_file::integration::ThemeIntegration;
    use iced_futures::futures::Stream;
    use std::task::{Context, Poll, Waker};
    let (mut themes, _boot) = ThemeIntegration::boot();
    let streams = iced_futures::subscription::into_recipes(themes.subscription())
        .into_iter()
        .map(|recipe| recipe.stream(Box::pin(iced_futures::futures::stream::empty())));
    let mut events = Box::pin(iced_futures::futures::stream::select_all(streams));
    let mut context = Context::from_waker(Waker::noop());
    let deadline = Instant::now() + Duration::from_secs(15);
    let fallback = std::env::var_os("ENZUI_DEFAULT_FONT_FILE_FALLBACK").is_some();
    let initial = if fallback {
        "File Fallback"
    } else {
        "Adwaita Sans"
    };
    let changed = if fallback {
        "File Updated"
    } else {
        "DejaVu Sans Mono"
    };
    let mut ready = false;
    loop {
        if let Poll::Ready(Some(event)) = events.as_mut().poll_next(&mut context) {
            let _ = themes.update(event);
        }
        let font = themes.theme().font_catalog.family(FontRole::Ui).clone();
        if font == enzui_theme::FontFamily::Named(initial.into()) && !ready {
            assert!(
                (themes.theme().typography.body_size
                    - (if fallback { 10.0 } else { 11.0 }) * 96.0 / 72.0)
                    .abs()
                    < 0.01
            );
            std::fs::write(root.join("ready"), b"ready").unwrap();
            ready = true;
        }
        if font == enzui_theme::FontFamily::Named(changed.into()) {
            assert!(
                ready,
                "the default host must observe both initial and changed portal fonts"
            );
            assert!(
                (themes.theme().typography.body_size
                    - (if fallback { 12.0 } else { 14.0 }) * 96.0 / 72.0)
                    .abs()
                    < 0.01
            );
            assert_eq!(
                themes.theme().typography.weight_regular,
                if fallback { 400 } else { 700 }
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "default host did not follow live portal font: {font:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn check_default_font_host(portal: &FakePortal, fallback: bool) {
    let root = &portal._bus.directory;
    let _ = std::fs::remove_file(root.join("ready"));
    let config = root.join("config");
    std::fs::create_dir_all(config.join("gtk-4.0")).unwrap();
    std::fs::write(
        config.join("gtk-4.0/settings.ini"),
        "[Settings]\ngtk-font-name=File Fallback 10\n",
    )
    .unwrap();
    let name = format!(
        "{}::the_portal_delivers_all_three_gnome_roles",
        module_path!().strip_prefix("enzui_core::").unwrap()
    );
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    if fallback {
        command.env("ENZUI_DEFAULT_FONT_FILE_FALLBACK", "1");
    }
    let mut child = Child(
        command
            .args(["--exact", &name, "--nocapture"])
            .env("ENZUI_DEFAULT_FONT_FIXTURE", root)
            .env("DBUS_SESSION_BUS_ADDRESS", &portal.address)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_CONFIG_DIRS", root.join("empty"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(root.join("child.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "default font host failed: {}",
            std::fs::read_to_string(root.join("child.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "default font host did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    if fallback {
        std::fs::write(
            config.join("gtk-4.0/settings.ini"),
            "[Settings]\ngtk-font-name=File Updated 12\n",
        )
        .unwrap();
    } else {
        let (done, ack) = std::sync::mpsc::sync_channel(1);
        portal
            .commands
            .send(Command::Set {
                namespace: GNOME_INTERFACE_NAMESPACE.into(),
                key: "font-name".into(),
                value: "DejaVu Sans Mono Bold 14 @wght=700".into(),
                done,
            })
            .unwrap();
        ack.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "default font host failed: {}",
                std::fs::read_to_string(root.join("child.log")).unwrap()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "default font host did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
