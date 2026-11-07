//! Network, memory and power-saver monitors against fake portals and fake
//! system services on private buses, with the portal first and the system
//! services as the fallback.

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
use ashpd::zbus;
use ashpd::zvariant::OwnedValue;
use enzui_core::system_status::{
    Connectivity, MemoryPressure, NetworkStatus, run_memory, run_network, run_power_saver,
};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

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
        let root = std::env::temp_dir().join(format!("enzui-status-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        // No service directories: nothing (such as the real portal) is
        // activated on this bus.
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

    async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn next<T>(events: &mut UnboundedReceiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), events.recv())
        .await
        .expect("an event in time")
        .expect("the monitor is still running")
}

async fn quiet<T: std::fmt::Debug>(events: &mut UnboundedReceiver<T>, for_: Duration) {
    if let Ok(Some(event)) = tokio::time::timeout(for_, events.recv()).await {
        panic!("unexpected {event:?}");
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

struct FakeNetworkPortal {
    available: bool,
    metered: bool,
    connectivity: u32,
}

#[zbus::interface(name = "org.freedesktop.portal.NetworkMonitor", crate = "ashpd::zbus")]
impl FakeNetworkPortal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        3
    }

    fn get_status(&self) -> HashMap<String, OwnedValue> {
        let mut status = HashMap::new();
        status.insert("available".into(), OwnedValue::from(self.available));
        status.insert("metered".into(), OwnedValue::from(self.metered));
        status.insert("connectivity".into(), OwnedValue::from(self.connectivity));
        status
    }

    #[zbus(signal, name = "changed")]
    async fn changed(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
}

#[test]
fn status_monitors_use_portals_fallbacks_and_terminate_without_sources_1() {
    // the_network_portal_reports_the_status_and_each_real_change
    {
        let session = PrivateBus::start();
        runtime().block_on(async {
            let portal = zbus::connection::Builder::address(session.address.as_str())
                .unwrap()
                .name(PORTAL_DEST)
                .unwrap()
                .serve_at(
                    PORTAL_PATH,
                    FakeNetworkPortal {
                        available: true,
                        metered: false,
                        connectivity: 4,
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(run_network(Some(session.connect().await), None, sender));
            assert_eq!(
                next(&mut events).await,
                NetworkStatus {
                    available: true,
                    metered: false,
                    connectivity: Connectivity::Full
                }
            );
            let iface = portal
                .object_server()
                .interface::<_, FakeNetworkPortal>(PORTAL_PATH)
                .await
                .unwrap();
            // A change signal with nothing changed is not passed on.
            FakeNetworkPortal::changed(iface.signal_emitter())
                .await
                .unwrap();
            quiet(&mut events, Duration::from_millis(200)).await;
            {
                let mut fake = iface.get_mut().await;
                fake.metered = true;
                fake.connectivity = 3;
            }
            FakeNetworkPortal::changed(iface.signal_emitter())
                .await
                .unwrap();
            assert_eq!(
                next(&mut events).await,
                NetworkStatus {
                    available: true,
                    metered: true,
                    connectivity: Connectivity::CaptivePortal
                }
            );
        });
    }

    // without_the_portal_networkmanager_is_read
    {
        let (session, system) = (PrivateBus::start(), PrivateBus::start());
        runtime().block_on(async {
            let nm = zbus::connection::Builder::address(system.address.as_str())
                .unwrap()
                .name("org.freedesktop.NetworkManager")
                .unwrap()
                .serve_at(
                    "/org/freedesktop/NetworkManager",
                    FakeNetworkManager {
                        connectivity: 4,
                        metered: 4,
                        state: 70,
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(run_network(
                Some(session.connect().await),
                Some(system.connect().await),
                sender,
            ));
            assert_eq!(
                next(&mut events).await,
                NetworkStatus {
                    available: true,
                    metered: false,
                    connectivity: Connectivity::Full
                }
            );
            let iface = nm
                .object_server()
                .interface::<_, FakeNetworkManager>("/org/freedesktop/NetworkManager")
                .await
                .unwrap();
            {
                let mut fake = iface.get_mut().await;
                fake.connectivity = 1;
                fake.state = 20;
                fake.connectivity_changed(iface.signal_emitter())
                    .await
                    .unwrap();
            }
            assert_eq!(
                next(&mut events).await,
                NetworkStatus {
                    available: false,
                    metered: false,
                    connectivity: Connectivity::Local
                }
            );
        });
    }

    // memory_warnings_come_from_the_portal
    {
        let session = PrivateBus::start();
        runtime().block_on(async {
            let portal = zbus::connection::Builder::address(session.address.as_str())
                .unwrap()
                .name(PORTAL_DEST)
                .unwrap()
                .serve_at(PORTAL_PATH, FakeMemoryPortal)
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            // The pressure file is not read while the portal answers.
            tokio::spawn(run_memory(
                Some(session.connect().await),
                Some("/nonexistent/pressure".into()),
                sender,
            ));
            let iface = portal
                .object_server()
                .interface::<_, FakeMemoryPortal>(PORTAL_PATH)
                .await
                .unwrap();
            // Let the monitor subscribe before the signals go out.
            tokio::time::sleep(Duration::from_millis(200)).await;
            for level in [50u8, 100, 255] {
                FakeMemoryPortal::low_memory_warning(iface.signal_emitter(), level)
                    .await
                    .unwrap();
            }
            assert_eq!(next(&mut events).await, MemoryPressure::Low);
            assert_eq!(next(&mut events).await, MemoryPressure::Medium);
            assert_eq!(next(&mut events).await, MemoryPressure::Critical);
        });
    }

    // without_the_portal_rising_memory_stalls_are_reported_once
    {
        let session = PrivateBus::start();
        let file = std::env::temp_dir().join(format!("enzui-pressure-{}", std::process::id()));
        std::fs::write(&file, stall(15.0, 0.0)).unwrap();
        runtime().block_on(async {
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(run_memory(
                Some(session.connect().await),
                Some(file.clone()),
                sender,
            ));
            assert_eq!(next(&mut events).await, MemoryPressure::Low);
            // Holding at the same level stays quiet; a rise is reported.
            quiet(&mut events, Duration::from_millis(2500)).await;
            std::fs::write(&file, stall(50.0, 12.0)).unwrap();
            assert_eq!(next(&mut events).await, MemoryPressure::Critical);
        });
        let _ = std::fs::remove_file(&file);
    }
}

struct FakeNetworkManager {
    connectivity: u32,
    metered: u32,
    state: u32,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager", crate = "ashpd::zbus")]
impl FakeNetworkManager {
    #[zbus(property)]
    fn connectivity(&self) -> u32 {
        self.connectivity
    }

    #[zbus(property)]
    fn metered(&self) -> u32 {
        self.metered
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state
    }
}

struct FakeMemoryPortal;

#[zbus::interface(name = "org.freedesktop.portal.MemoryMonitor", crate = "ashpd::zbus")]
impl FakeMemoryPortal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    #[zbus(signal)]
    async fn low_memory_warning(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        level: u8,
    ) -> zbus::Result<()>;
}

fn stall(some: f64, full: f64) -> String {
    format!(
        "some avg10={some:.2} avg60=0.00 avg300=0.00 total=1\n\
         full avg10={full:.2} avg60=0.00 avg300=0.00 total=1\n"
    )
}

struct FakePowerPortal {
    saver: bool,
}

#[zbus::interface(
    name = "org.freedesktop.portal.PowerProfileMonitor",
    crate = "ashpd::zbus"
)]
impl FakePowerPortal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    #[zbus(property, name = "power-saver-enabled")]
    fn power_saver_enabled(&self) -> bool {
        self.saver
    }
}

struct FakePowerProfiles {
    profile: String,
}

#[zbus::interface(name = "org.freedesktop.UPower.PowerProfiles", crate = "ashpd::zbus")]
impl FakePowerProfiles {
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.profile.clone()
    }
}

#[test]
fn status_monitors_use_portals_fallbacks_and_terminate_without_sources_2() {
    // power_saver_comes_from_the_portal_or_power_profiles_daemon
    {
        let (session, system) = (PrivateBus::start(), PrivateBus::start());
        runtime().block_on(async {
            // From the portal.
            let portal = zbus::connection::Builder::address(session.address.as_str())
                .unwrap()
                .name(PORTAL_DEST)
                .unwrap()
                .serve_at(PORTAL_PATH, FakePowerPortal { saver: false })
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(run_power_saver(Some(session.connect().await), None, sender));
            assert!(!next(&mut events).await);
            let iface = portal
                .object_server()
                .interface::<_, FakePowerPortal>(PORTAL_PATH)
                .await
                .unwrap();
            {
                let mut fake = iface.get_mut().await;
                fake.saver = true;
                fake.power_saver_enabled_changed(iface.signal_emitter())
                    .await
                    .unwrap();
            }
            assert!(next(&mut events).await);
            drop(events);
            // The portal goes away (a dropped connection may linger).
            portal.release_name(PORTAL_DEST).await.unwrap();

            // Without the portal, from power-profiles-daemon.
            let daemon = zbus::connection::Builder::address(system.address.as_str())
                .unwrap()
                .name("org.freedesktop.UPower.PowerProfiles")
                .unwrap()
                .serve_at(
                    "/org/freedesktop/UPower/PowerProfiles",
                    FakePowerProfiles {
                        profile: "power-saver".into(),
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(run_power_saver(
                Some(session.connect().await),
                Some(system.connect().await),
                sender,
            ));
            assert!(next(&mut events).await);
            let iface = daemon
                .object_server()
                .interface::<_, FakePowerProfiles>("/org/freedesktop/UPower/PowerProfiles")
                .await
                .unwrap();
            {
                let mut fake = iface.get_mut().await;
                fake.profile = "balanced".into();
                fake.active_profile_changed(iface.signal_emitter())
                    .await
                    .unwrap();
            }
            assert!(!next(&mut events).await);
        });
    }

    // with_no_source_nothing_arrives_and_the_monitor_ends
    {
        let session = PrivateBus::start();
        runtime().block_on(async {
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel::<NetworkStatus>();
            run_network(Some(session.connect().await), None, sender).await;
            assert!(events.recv().await.is_none());
        });
    }
}
