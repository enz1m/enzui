//! Launcher progress where the activity broker cannot be reached.
//!
//! A private `dbus-daemon` stands in for the session bus. The publisher sees
//! no broker socket, so its live jobs must reach the bus as one
//! `com.canonical.Unity.LauncherEntry.Update` signal per change, at most
//! twice a second, hidden again when the last job ends. With a broker serving
//! the same jobs, no signal may be sent.

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
use enzui_activity::{Outcome, Progress, Publisher, Status, Unit};
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const APP: &str = "io.github.enzui.Files";

struct PrivateBus {
    child: Child,
    address: String,
    _root: tempfile::TempDir,
}

impl PrivateBus {
    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("bus");
        let address = format!("unix:path={}", socket.display());
        let config = root.path().join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig>\n  <type>session</type>\n  <listen>{address}</listen>\n  \
                 <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    \
                 <allow receive_sender=\"*\"/>\n    <allow own=\"*\"/>\n  \
                 <allow eavesdrop=\"true\"/>\n  </policy>\n</busconfig>\n"
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

/// One received `Update`: the app URI, `progress` and `progress-visible`.
#[derive(Debug)]
struct Update {
    uri: String,
    progress: f64,
    visible: bool,
    at: Instant,
}

/// Collects `LauncherEntry.Update` signals on `address` for `window`.
async fn collect(address: &str, window: Duration) -> Vec<Update> {
    let connection = zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap();
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("com.canonical.Unity.LauncherEntry")
        .unwrap()
        .member("Update")
        .unwrap()
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await
        .unwrap();
    let mut updates = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, stream.next()).await {
        let (uri, properties): (String, HashMap<String, zbus::zvariant::OwnedValue>) =
            message.body().deserialize().unwrap();
        updates.push(Update {
            uri,
            progress: properties
                .get("progress")
                .and_then(|value| f64::try_from(value).ok())
                .unwrap(),
            visible: properties
                .get("progress-visible")
                .and_then(|value| bool::try_from(value).ok())
                .unwrap(),
            at: Instant::now(),
        });
    }
    updates
}

fn copying(done: u64) -> Status {
    Status {
        progress: Some(Progress {
            done,
            total: 10,
            unit: Unit::Items,
        }),
        ..Status::running("Copying files")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreachable_broker_shows_launcher_progress_and_hides_it_at_the_end() {
    let bus = PrivateBus::start();
    let missing = tempfile::tempdir().unwrap();
    let address = bus.address.clone();
    let listener = tokio::spawn(async move { collect(&address, Duration::from_secs(4)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let publisher = Publisher::at_socket_with_launcher_bus(
        APP,
        missing.path().join("service.sock"),
        bus.address.clone(),
    )
    .unwrap();
    let job = publisher.start(copying(3)).unwrap();
    // A burst of measurements: the dock must not receive one per update.
    for done in 4..=9 {
        tokio::time::sleep(Duration::from_millis(40)).await;
        job.set_progress(
            Some(Progress {
                done,
                total: 10,
                unit: Unit::Items,
            }),
            None,
        )
        .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1200)).await;
    job.finish(Outcome::Succeeded).unwrap();
    let updates = listener.await.unwrap();

    let first = updates.first().expect("no launcher signal was sent");
    assert_eq!(first.uri, format!("application://{APP}.desktop"));
    assert!(first.visible);
    assert!((0.29..=0.91).contains(&first.progress), "{updates:?}");
    let shown: Vec<&Update> = updates.iter().filter(|update| update.visible).collect();
    assert!(
        shown
            .iter()
            .any(|update| (update.progress - 0.9).abs() < 1e-9),
        "the latest measurement must reach the dock: {updates:?}"
    );
    for pair in shown.windows(2) {
        assert!(
            pair[1].at.duration_since(pair[0].at) >= Duration::from_millis(400),
            "signals closer than the 2 Hz bound: {updates:?}"
        );
    }
    assert!(shown.len() <= 4, "{} signals for one burst", shown.len());
    let last = updates.last().unwrap();
    assert!(
        !last.visible,
        "the bar must hide when the job ends: {updates:?}"
    );
}

/// A broker that accepts the publisher and acknowledges every command.
fn broker(path: std::path::PathBuf) -> std::thread::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            let reply = if line.contains("\"publish\"") {
                r#"{"op":"ready","version":1,"lease_ms":90000,"heartbeat_ms":30000,"rate_per_second":4,"burst":8}"#
            } else {
                r#"{"op":"ack","revision":1,"changed":true}"#
            };
            if writeln!(writer, "{reply}").is_err() {
                return;
            }
            line.clear();
        }
    })
}

#[tokio::test(flavor = "current_thread")]
async fn a_serving_broker_sends_no_launcher_progress() {
    let bus = PrivateBus::start();
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("service.sock");
    let _broker = broker(socket.clone());
    let address = bus.address.clone();
    let listener = tokio::spawn(async move { collect(&address, Duration::from_secs(2)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let publisher =
        Publisher::at_socket_with_launcher_bus(APP, socket, bus.address.clone()).unwrap();
    let job = publisher.start(copying(3)).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    job.finish(Outcome::Succeeded).unwrap();
    let updates = listener.await.unwrap();

    assert!(
        updates.is_empty(),
        "the broker already shows these jobs: {updates:?}"
    );
}

/// Without any reachable bus the publisher retries at the signal rate instead
/// of spinning: its worker thread stays nearly idle.
///
/// Sabotage: stop counting a failed connection against the rate in
/// `Launcher::show`; the worker spins at full speed.
#[test]
fn a_missing_bus_is_retried_at_the_signal_rate() {
    fn cpu_seconds() -> f64 {
        // utime + stime of this process, in clock ticks (100 Hz on Linux).
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let fields: Vec<&str> = stat
            .rsplit(')')
            .next()
            .unwrap()
            .split_whitespace()
            .collect();
        let ticks: f64 = fields[11].parse::<f64>().unwrap() + fields[12].parse::<f64>().unwrap();
        ticks / 100.0
    }
    let missing = tempfile::tempdir().unwrap();
    let publisher = Publisher::at_socket_with_launcher_bus(
        APP,
        missing.path().join("service.sock"),
        format!("unix:path={}", missing.path().join("no-bus").display()),
    )
    .unwrap();
    let _job = publisher.start(copying(1)).unwrap();
    let before = cpu_seconds();
    std::thread::sleep(Duration::from_secs(2));
    let spent = cpu_seconds() - before;
    assert!(spent < 0.5, "the worker spent {spent:.2} s of CPU in 2 s");
}

/// A launcher bus that accepts the connection and then never answers cannot
/// hold the publisher: it still reaches a broker that appears afterwards.
#[test]
fn a_stalled_launcher_bus_does_not_stop_the_broker_retry() {
    let dir = tempfile::tempdir().unwrap();
    // Accepts every connection and says nothing, as a wedged daemon would.
    let stalled = UnixListener::bind(dir.path().join("stalled-bus")).unwrap();
    let _held = std::thread::spawn(move || {
        let mut kept = Vec::new();
        for stream in stalled.incoming() {
            kept.push(stream);
        }
    });
    let socket = dir.path().join("service.sock");
    let publisher = Publisher::at_socket_with_launcher_bus(
        APP,
        socket.clone(),
        format!("unix:path={}", dir.path().join("stalled-bus").display()),
    )
    .unwrap();
    let _job = publisher.start(copying(1)).unwrap();
    std::thread::sleep(Duration::from_millis(600));

    let broker = UnixListener::bind(&socket).unwrap();
    broker.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if broker.accept().is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the publisher never retried the broker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
