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

use enzui_services::desktop_icons::portal::{
    IconThemeObservation, PortalIconContext, PortalIconWatch,
};
use enzui_services::desktop_icons::{
    DesktopIcon, DesktopIconService, IconCacheLimits, IconContext,
};
use iced_futures::futures::StreamExt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedValue, Value};

static NEXT: AtomicU64 = AtomicU64::new(0);
const NS: &str = "org.gnome.desktop.interface";
const KEY: &str = "icon-theme";

struct Bus {
    child: std::process::Child,
    root: PathBuf,
    address: String,
}
impl Bus {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "enzui-icon-bus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile"])
            .arg(format!("--address={address}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("private dbus-daemon is required");
        until(|| {
            assert!(child.try_wait().unwrap().is_none(), "private bus exited");
            socket.exists()
        });
        Self {
            child,
            root,
            address,
        }
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone)]
struct Settings {
    name: Arc<Mutex<String>>,
    calls: Arc<AtomicU64>,
    legacy: bool,
    queued_old: Arc<AtomicBool>,
    // Holding the re-read keeps latest-value coalescing from hiding its consequence.
    pause_second: bool,
    resume: Arc<tokio::sync::Notify>,
}
impl Settings {
    fn value(&self, namespace: &str, key: &str, nested: bool) -> zbus::fdo::Result<OwnedValue> {
        if namespace != NS || key != KEY {
            return Err(zbus::fdo::Error::Failed("not found".into()));
        }
        let name = self.name.lock().unwrap();
        let value = Value::from(name.as_str());
        OwnedValue::try_from(if nested {
            Value::Value(Box::new(value))
        } else {
            value
        })
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}
#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl Settings {
    async fn read_one(
        &self,
        namespace: &str,
        key: &str,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::fdo::Result<OwnedValue> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pause_second && call == 1 {
            self.resume.notified().await;
        }
        if self.legacy {
            return Err(zbus::fdo::Error::UnknownMethod("v1 fixture".into()));
        }
        if self.queued_old.swap(false, Ordering::SeqCst) {
            Self::setting_changed(&emitter, NS, KEY, Value::from("Light"))
                .await
                .unwrap();
        }
        self.value(namespace, key, false)
    }
    fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.value(namespace, key, true)
    }
    #[zbus(signal)]
    async fn setting_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;
}

enum Command {
    Emit(String, String, String, std::sync::mpsc::SyncSender<()>),
    Stop,
}
struct Portal {
    commands: tokio::sync::mpsc::Sender<Command>,
    worker: Option<std::thread::JoinHandle<()>>,
    settings: Settings,
    bus: Bus,
}
impl Portal {
    fn new(legacy: bool, queued_old: bool) -> Self {
        let bus = Bus::new();
        let settings = Settings {
            name: Arc::new(Mutex::new(if queued_old { "Dark" } else { "Light" }.into())),
            calls: Arc::new(AtomicU64::new(0)),
            legacy,
            queued_old: Arc::new(AtomicBool::new(queued_old)),
            pause_second: queued_old,
            resume: Arc::new(tokio::sync::Notify::new()),
        };
        let served = settings.clone();
        let address = bus.address.clone();
        let (commands, mut receiver) = tokio::sync::mpsc::channel(16);
        let (ready, acknowledgement) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let connection = zbus::connection::Builder::address(address.as_str())
                    .unwrap()
                    .name("org.freedesktop.portal.Desktop")
                    .unwrap()
                    .serve_at("/org/freedesktop/portal/desktop", served)
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                ready.send(()).unwrap();
                while let Some(command) = receiver.recv().await {
                    match command {
                        Command::Stop => break,
                        Command::Emit(namespace, key, name, done) => {
                            let interface = connection
                                .object_server()
                                .interface::<_, Settings>("/org/freedesktop/portal/desktop")
                                .await
                                .unwrap();
                            Settings::setting_changed(
                                interface.signal_emitter(),
                                &namespace,
                                &key,
                                Value::from(name.as_str()),
                            )
                            .await
                            .unwrap();
                            done.send(()).unwrap();
                        }
                    }
                }
            });
        });
        acknowledgement
            .recv_timeout(Duration::from_secs(3))
            .expect("private portal startup");
        Self {
            commands,
            worker: Some(worker),
            settings,
            bus,
        }
    }
    fn emit(&self, namespace: &str, key: &str, name: &str) {
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        self.commands
            .blocking_send(Command::Emit(
                namespace.into(),
                key.into(),
                name.into(),
                done,
            ))
            .unwrap();
        wait.recv_timeout(Duration::from_secs(3))
            .expect("private signal emission");
    }
    fn set(&self, name: &str) {
        *self.settings.name.lock().unwrap() = name.into();
        self.emit(NS, KEY, name);
    }
    fn service(&self) -> DesktopIconService {
        for name in ["Light", "Dark"] {
            let root = self.bus.root.join(name);
            std::fs::create_dir_all(root.join("16")).unwrap();
            std::fs::write(
                root.join("index.theme"),
                "[Icon Theme]\nDirectories=16\n[16]\nSize=16\nType=Fixed\n",
            )
            .unwrap();
            std::fs::write(root.join("16/folder.svg"), format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><!--{name}--></svg>"#)).unwrap();
        }
        DesktopIconService::new(
            IconContext::new(vec![self.bus.root.clone()]),
            "hicolor",
            IconCacheLimits::default(),
        )
        .unwrap()
    }
}
impl Drop for Portal {
    fn drop(&mut self) {
        let _ = self.commands.blocking_send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "private portal fixture deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn wait<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    let mut answer = None;
    until(|| match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => {
            answer = Some(value);
            true
        }
        Poll::Pending => false,
    });
    answer.unwrap()
}
fn stream(watch: &PortalIconWatch) -> iced_futures::BoxStream<IconThemeObservation> {
    let mut recipes = iced_futures::subscription::into_recipes(watch.subscription());
    assert_eq!(recipes.len(), 1);
    recipes
        .pop()
        .unwrap()
        .stream(Box::pin(iced_futures::futures::stream::pending()))
}
fn selected(stream: &mut iced_futures::BoxStream<IconThemeObservation>, expected: &str) -> u64 {
    loop {
        match wait(stream.next()).expect("live observation") {
            IconThemeObservation::Waiting => {}
            IconThemeObservation::Selected { theme, epoch } => {
                assert_eq!(theme, expected);
                return epoch;
            }
            other => panic!("unexpected observation: {other:?}"),
        }
    }
}
#[derive(Default)]
struct Wakes(AtomicU64);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn desktop_icon_portal_live_transport_invalidates_handles_and_wakes_subscription() {
    for legacy in [false, true] {
        let portal = Portal::new(legacy, false);
        let service = portal.service();
        let watch = service
            .watch_portal(PortalIconContext::new(&portal.bus.address))
            .unwrap();
        let mut events = stream(&watch);
        selected(&mut events, "Light");
        let old = wait(service.resolve(1, "folder", 16, 1))
            .into_result()
            .unwrap()
            .unwrap();
        assert!(service.is_current(&old));
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&wakes));
        let mut context = Context::from_waker(&waker);
        let mut next = Box::pin(events.next());
        assert!(matches!(next.as_mut().poll(&mut context), Poll::Pending));
        portal.set("Dark");
        until(|| wakes.0.load(Ordering::SeqCst) > 0);
        let observation = wait(next).unwrap();
        assert!(
            matches!(observation, IconThemeObservation::Selected { ref theme, .. } if theme == "Dark")
        );
        assert!(!service.is_current(&old));
        let new = wait(service.resolve(2, "folder", 16, 1))
            .into_result()
            .unwrap()
            .unwrap();
        let DesktopIcon::Svg(handle) = new.icon().unwrap() else {
            panic!("SVG artwork");
        };
        let iced_core::svg::Data::Bytes(bytes) = handle.data() else {
            panic!("cached memory");
        };
        assert!(
            std::str::from_utf8(bytes.as_ref())
                .unwrap()
                .contains("<!--Dark-->")
        );
        let before = portal.settings.calls.load(Ordering::SeqCst);
        portal.emit("org.example.unrelated", KEY, "Light");
        portal.emit(NS, "unrelated", "Light");
        portal.emit(NS, KEY, "Dark");
        selected(&mut events, "Dark");
        assert_eq!(
            portal.settings.calls.load(Ordering::SeqCst) - before,
            if legacy { 2 } else { 1 }
        );
        for i in 0..32 {
            let name = format!("Theme{i}");
            portal.set(&name);
            until(
                || matches!(watch.latest(), IconThemeObservation::Selected { theme, .. } if theme == name),
            );
        }
        portal.set("Dark");
        until(
            || matches!(watch.latest(), IconThemeObservation::Selected { theme, .. } if theme == "Dark"),
        );
        selected(&mut events, "Dark");
        let mut next = Box::pin(events.next());
        assert!(matches!(next.as_mut().poll(&mut context), Poll::Pending));
        drop(next);
        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            wait(events.next()).is_none(),
            "watch shutdown closes its stream"
        );
    }
}
