//! The tray item and launcher badge as a tray host and a dock see them, on a
//! private bus: registration with the watcher (again when a host restarts),
//! the item's properties, the dbusmenu layout, clicks back to the app, and
//! the launcher's count and urgent mark.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]
use enzui_core::launcher_badge::LauncherBadge;
use enzui_core::tray::{Tray, TrayEvent, TrayIcon, TrayItem, TrayMenuItem, TrayState, TrayStatus};
use iced_futures::futures::StreamExt as _;
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedValue, Value};

struct PrivateBus {
    child: Child,
    address: String,
    root: std::path::PathBuf,
}

impl PrivateBus {
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("enzui-tray-{}-{n}", std::process::id()));
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

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
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

struct FakeWatcher {
    registered: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl FakeWatcher {
    fn register_status_notifier_item(&self, service: String) {
        self.registered.lock().unwrap().push(service);
    }
}

/// A tray host's watcher on `address`, until the connection is dropped.
async fn watcher(address: &str, registered: &Arc<Mutex<Vec<String>>>) -> zbus::Connection {
    zbus::connection::Builder::address(address)
        .unwrap()
        .name("org.kde.StatusNotifierWatcher")
        .unwrap()
        .serve_at(
            "/StatusNotifierWatcher",
            FakeWatcher {
                registered: Arc::clone(registered),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap()
}

fn item(muted: bool) -> TrayItem {
    TrayItem::new(
        "org.example.Mail",
        "Mail",
        TrayIcon::Named("mail-unread-symbolic".into()),
    )
    .tooltip("Mail", "3 unread")
    .menu(vec![
        TrayMenuItem::action("open", "Open Mail"),
        TrayMenuItem::check("mute", "Mute notifications", muted),
        TrayMenuItem::separator(),
        TrayMenuItem::submenu(
            "Account",
            vec![
                TrayMenuItem::radio("work", "Work", true),
                TrayMenuItem::radio("home", "Home", false).enabled(false),
            ],
        ),
        TrayMenuItem::action("quit", "Quit"),
    ])
}

fn events_of(tray: &Tray) -> iced_futures::BoxStream<TrayEvent> {
    let mut recipes = iced_futures::subscription::into_recipes(tray.subscription());
    assert_eq!(recipes.len(), 1);
    recipes
        .remove(0)
        .stream(Box::pin(iced_futures::futures::stream::pending()))
}

type Pixmap = (i32, i32, Vec<u8>);

type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn label(node: &Node) -> String {
    node.1
        .get("label")
        .and_then(|value| String::try_from(value.try_clone().unwrap()).ok())
        .unwrap_or_default()
}

fn int(node: &Node, key: &str) -> Option<i32> {
    node.1.get(key).and_then(|value| i32::try_from(value).ok())
}

fn children(node: &Node) -> Vec<Node> {
    node.2
        .iter()
        .map(|child| Node::try_from(child.try_clone().unwrap()).unwrap())
        .collect()
}

async fn layout(host: &zbus::Connection, service: &str) -> (u32, Node) {
    host.call_method(
        Some(service),
        "/MenuBar",
        Some("com.canonical.dbusmenu"),
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()),
    )
    .await
    .unwrap()
    .body()
    .deserialize()
    .unwrap()
}

async fn property<T: TryFrom<OwnedValue>>(host: &zbus::Connection, service: &str, name: &str) -> T
where
    T::Error: std::fmt::Debug,
{
    let proxy = zbus::fdo::PropertiesProxy::builder(host)
        .destination(service.to_owned())
        .unwrap()
        .path("/StatusNotifierItem")
        .unwrap()
        .build()
        .await
        .unwrap();
    let value = proxy
        .get(
            zbus::names::InterfaceName::try_from("org.kde.StatusNotifierItem").unwrap(),
            name,
        )
        .await
        .unwrap();
    T::try_from(value).unwrap()
}

#[test]
fn a_tray_host_sees_the_item_and_its_menu_and_clicks_reach_the_app() {
    let bus = PrivateBus::start();
    let rt = runtime();
    let _context = rt.enter();
    let registered = Arc::new(Mutex::new(Vec::new()));
    let host_watcher = rt.block_on(watcher(&bus.address, &registered));
    let mut tray = Tray::on_bus(item(false), bus.address.clone());
    let service = wait("registration", || {
        registered.lock().unwrap().first().cloned()
    });
    assert!(
        service.starts_with("org.kde.StatusNotifierItem-"),
        "{service}"
    );
    wait("shown", || (tray.state() == TrayState::Shown).then_some(()));

    let host = rt.block_on(async {
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    });
    rt.block_on(async {
        assert_eq!(property::<String>(&host, &service, "Title").await, "Mail");
        assert_eq!(
            property::<String>(&host, &service, "Status").await,
            "Active"
        );
        assert_eq!(
            property::<String>(&host, &service, "IconName").await,
            "mail-unread-symbolic"
        );
        let tooltip: (String, Vec<Pixmap>, String, String) =
            property(&host, &service, "ToolTip").await;
        assert_eq!(
            (tooltip.2.as_str(), tooltip.3.as_str()),
            ("Mail", "3 unread")
        );
    });

    let (revision, root) = rt.block_on(layout(&host, &service));
    let rows = children(&root);
    let labels: Vec<String> = rows.iter().map(label).collect();
    assert_eq!(
        labels,
        ["Open Mail", "Mute notifications", "", "Account", "Quit"]
    );
    assert_eq!(int(&rows[1], "toggle-state"), Some(0));
    assert!(rows[2].1.contains_key("type"), "a separator row");
    let account = children(&rows[3]);
    assert_eq!(
        account.iter().map(label).collect::<Vec<_>>(),
        ["Work", "Home"]
    );
    assert_eq!(int(&account[0], "toggle-state"), Some(1));

    // The host clicks rows and the icon; the app hears about it.
    let mut events = events_of(&tray);
    let click = |id: i32| {
        rt.block_on(host.call_method(
            Some(service.as_str()),
            "/MenuBar",
            Some("com.canonical.dbusmenu"),
            "Event",
            &(id, "clicked", Value::from(0i32), 0u32),
        ))
        .unwrap();
    };
    click(rows[1].0);
    click(account[1].0); // disabled: nothing
    click(rows[4].0);
    rt.block_on(host.call_method(
        Some(service.as_str()),
        "/StatusNotifierItem",
        Some("org.kde.StatusNotifierItem"),
        "Activate",
        &(10i32, 10i32),
    ))
    .unwrap();
    rt.block_on(host.call_method(
        Some(service.as_str()),
        "/StatusNotifierItem",
        Some("org.kde.StatusNotifierItem"),
        "ContextMenu",
        &(10i32, 10i32),
    ))
    .unwrap();
    let mut next = || {
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), events.next())
                .await
                .unwrap()
                .unwrap()
        })
    };
    assert_eq!(next(), TrayEvent::MenuItem("mute".into()));
    assert_eq!(next(), TrayEvent::MenuItem("quit".into()));
    assert_eq!(next(), TrayEvent::Activated);
    assert_eq!(next(), TrayEvent::MenuRequested);

    // The app checks the row and asks for attention: the host is told.
    let changes = rt.block_on(async {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(service.as_str())
            .unwrap()
            .build();
        zbus::MessageStream::for_match_rule(rule, &host, None)
            .await
            .unwrap()
    });
    tray.set(item(true).status(TrayStatus::NeedsAttention));
    let mut changes = changes;
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let message = rt
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), changes.next()).await })
            .unwrap()
            .unwrap()
            .unwrap();
        let member = message.header().member().unwrap().to_string();
        if member == "PropertiesChanged" {
            continue;
        }
        seen.push(member);
    }
    seen.sort();
    assert_eq!(seen, ["LayoutUpdated", "NewStatus"]);
    let (after, root) = rt.block_on(layout(&host, &service));
    assert!(after > revision);
    assert_eq!(int(&children(&root)[1], "toggle-state"), Some(1));
    rt.block_on(async {
        assert_eq!(
            property::<String>(&host, &service, "Status").await,
            "NeedsAttention"
        );
    });

    // The tray host restarts: the item registers with the new one.
    drop(host_watcher);
    wait("no host", || {
        (tray.state() == TrayState::NoHost).then_some(())
    });
    let _again = rt.block_on(watcher(&bus.address, &registered));
    wait("registered again", || {
        (registered.lock().unwrap().len() == 2).then_some(())
    });
    wait("shown again", || {
        (tray.state() == TrayState::Shown).then_some(())
    });

    // Dropping the tray takes the item off the bus.
    drop(tray);
    rt.block_on(async {
        let dbus = zbus::fdo::DBusProxy::new(&host).await.unwrap();
        let name = zbus::names::BusName::try_from(service.as_str()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while dbus.name_has_owner(name.clone()).await.unwrap() {
            assert!(Instant::now() < deadline, "the item left the bus");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

#[test]
fn without_a_tray_host_the_item_says_so() {
    let bus = PrivateBus::start();
    let tray = Tray::on_bus(item(false), bus.address.clone());
    wait("no host", || {
        (tray.state() == TrayState::NoHost).then_some(())
    });
}

#[test]
fn rgba_icons_reach_the_host_as_argb() {
    let bus = PrivateBus::start();
    let rt = runtime();
    let _context = rt.enter();
    let registered = Arc::new(Mutex::new(Vec::new()));
    let _watcher = rt.block_on(watcher(&bus.address, &registered));
    let icon = TrayIcon::Rgba {
        width: 1,
        height: 2,
        pixels: vec![10, 20, 30, 255, 1, 2, 3, 128],
    };
    let _tray = Tray::on_bus(TrayItem::new("x", "X", icon), bus.address.clone());
    let service = wait("registration", || {
        registered.lock().unwrap().first().cloned()
    });
    let pixmaps: Vec<Pixmap> = rt.block_on(async {
        let host = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        property(&host, &service, "IconPixmap").await
    });
    assert_eq!(pixmaps, [(1, 2, vec![255, 10, 20, 30, 128, 1, 2, 3])]);
}

#[test]
fn the_launcher_shows_the_count_and_urgent_mark() {
    let bus = PrivateBus::start();
    let rt = runtime();
    let _context = rt.enter();
    let mut updates = rt.block_on(async {
        let dock = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("com.canonical.Unity.LauncherEntry")
            .unwrap()
            .build();
        zbus::MessageStream::for_match_rule(rule, &dock, None)
            .await
            .unwrap()
    });
    let mut next = || -> (String, HashMap<String, OwnedValue>, String) {
        let message = rt
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), updates.next()).await })
            .expect("an update")
            .unwrap()
            .unwrap();
        let path = message.header().path().unwrap().to_string();
        let (uri, properties): (String, HashMap<String, OwnedValue>) =
            message.body().deserialize().unwrap();
        (uri, properties, path)
    };
    let mut badge = LauncherBadge::on_bus("org.example.Mail", bus.address.clone());
    badge.set_count(Some(3));
    let (uri, properties, path) = next();
    assert_eq!(uri, "application://org.example.Mail.desktop");
    assert_eq!(path, "/com/canonical/unity/launcherentry/org_example_Mail");
    assert_eq!(i64::try_from(&properties["count"]).unwrap(), 3);
    assert!(bool::try_from(&properties["count-visible"]).unwrap());
    assert!(!properties.contains_key("urgent"), "only what changed");

    badge.set_count(Some(3)); // unchanged: nothing sent
    badge.set_urgent(true);
    let (_, properties, _) = next();
    assert!(bool::try_from(&properties["urgent"]).unwrap());
    assert!(!properties.contains_key("count"));

    badge.set_count(None);
    let (_, properties, _) = next();
    assert!(!bool::try_from(&properties["count-visible"]).unwrap());

    // Dropping the badge clears what is left.
    drop(badge);
    let (_, properties, _) = next();
    assert!(!bool::try_from(&properties["urgent"]).unwrap());
}
