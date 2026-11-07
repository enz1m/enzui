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

use enzui_activity::{Error, Outcome, Progress, Publisher, Status, Unit, Visibility};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

#[allow(dead_code, reason = "fixture fields are written but never read")]
struct Frame {
    connection: u64,
    at: Instant,
    value: Value,
}

struct Broker {
    directory: tempfile::TempDir,
    frames: mpsc::Receiver<Frame>,
    hold: Arc<AtomicBool>,
    reset: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Broker {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(directory.path().join("service.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (tx, frames) = mpsc::channel();
        let hold = Arc::new(AtomicBool::new(false));
        let reset = Arc::new(AtomicU64::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let (stop, held, generation) =
            (Arc::clone(&stopped), Arc::clone(&hold), Arc::clone(&reset));
        let thread = thread::spawn(move || {
            let mut connection = 0;
            while !stop.load(Ordering::SeqCst) {
                let (stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(30)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                connection += 1;
                let accepted = generation.load(Ordering::SeqCst);
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                while !stop.load(Ordering::SeqCst) && generation.load(Ordering::SeqCst) == accepted
                {
                    match stream.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            continue;
                        }
                        Err(_) => break,
                    }
                    assert!(line.len() <= 32769);
                    let value: Value = serde_json::from_str(&line).unwrap();
                    let reply = if value["op"] == "publish" {
                        assert_eq!(
                            value,
                            json!({"op":"publish", "version":1, "app_id":"io.github.enzui.Files"})
                        );
                        json!({"op":"ready", "version":1, "lease_ms":90000, "heartbeat_ms":30000, "rate_per_second":4, "burst":8})
                    } else {
                        json!({"op":"ack", "revision":1, "changed":true})
                    };
                    tx.send(Frame {
                        connection,
                        at: Instant::now(),
                        value,
                    })
                    .unwrap();
                    while held.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    if writeln!(stream.get_mut(), "{reply}").is_err() {
                        break;
                    }
                    line.clear();
                }
                let _ = tx.send(Frame {
                    connection,
                    at: Instant::now(),
                    value: json!({"op":"disconnected"}),
                });
            }
        });
        Self {
            directory,
            frames,
            hold,
            reset,
            stopped,
            thread: Some(thread),
        }
    }

    fn publisher(&self) -> Publisher {
        Publisher::at_socket(
            "io.github.enzui.Files",
            self.directory.path().join("service.sock"),
        )
        .unwrap()
    }

    fn next(&self, operation: &str) -> Frame {
        self.next_any(&[operation])
    }

    fn next_any(&self, operations: &[&str]) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let frame = self
                .frames
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("waiting for {operations:?}: {error}"));
            if operations
                .iter()
                .any(|operation| frame.value["op"] == *operation)
            {
                return frame;
            }
        }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.hold.store(false, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn status(done: u64, token: &str) -> Status {
    Status {
        progress: Some(Progress {
            done,
            total: 100_000,
            unit: Unit::Bytes,
        }),
        window_token: Some(token.into()),
        ..Status::running("Copying files")
    }
}

#[test]
fn activity_reconnects_only_live_work_without_resurrecting_completion() {
    let broker = Broker::new();
    let publisher = broker.publisher();
    assert!(
        matches!(
            publisher.start(Status::running("Copying \u{200e}files")),
            Err(Error::InvalidMetadata)
        ),
        "reject metadata that would invalidate the shell subscriber's snapshot"
    );
    let live = publisher.start(status(10, "files-live")).unwrap();
    let first = broker.next("batch");
    assert!(first.value["works"][0].get("visibility").is_none());
    let brief = publisher
        .start_after(
            Status::running("Brief operation"),
            Visibility::WhenUnfocused,
            Duration::from_millis(800),
        )
        .unwrap();
    brief.finish(Outcome::Succeeded).unwrap();
    let started = Instant::now();
    let completed = publisher
        .start_after(
            Status::running("Deferred operation"),
            Visibility::WhenUnfocused,
            Duration::from_millis(1250),
        )
        .unwrap();
    live.set_visibility(Visibility::Always).unwrap();
    let visible = broker.next_any(&["batch", "finish"]);
    assert_eq!(visible.value["op"], "batch");
    assert_eq!(
        visible.value["works"].as_array().unwrap().len(),
        1,
        "brief and unrevealed work stays silent"
    );
    assert_eq!(visible.value["works"][0]["visibility"], "always");
    assert_eq!(visible.value["works"][0]["window_token"], "files-live");
    assert_eq!(visible.value["works"][0]["progress"]["done"], 10);
    let revealed = broker.next("batch");
    assert!(revealed.at.duration_since(started) >= Duration::from_millis(1250));
    assert_eq!(revealed.value["works"][0]["label"], "Deferred operation");
    completed.finish(Outcome::Succeeded).unwrap();
    broker.next("finish");
    broker.reset.fetch_add(1, Ordering::SeqCst);
    let replacement = broker.next("batch");
    assert!(replacement.connection > first.connection);
    assert_eq!(replacement.value["works"][0]["visibility"], "always");
    live.set_visibility(Visibility::WhenUnfocused).unwrap();
    assert!(
        broker.next("batch").value["works"][0]
            .get("visibility")
            .is_none()
    );
    assert_eq!(replacement.value["works"].as_array().unwrap().len(), 1);
    assert_eq!(replacement.value["works"][0]["window_token"], "files-live");
    live.update(status(11, "files-live")).unwrap();
    assert_eq!(
        broker.next("batch").value["works"][0]["progress"]["done"],
        11
    );
    assert_eq!(completed.finish(Outcome::Failed), Err(Error::Finished));
}

#[cfg(feature = "operation-center")]
#[test]
fn activity_center_publication_follows_live_attempts_without_replaying_private_history() {
    use enzui_activity::CenterJobs;
    use enzui_core::operation_activity::{
        Activity, ActivityCenter, ActivityKey, ActivityKind, ActivityProgress, Generation,
        LivePhase, ProviderId, TerminalOutcome,
    };
    let broker = Broker::new();
    let publisher = broker.publisher();
    let mut center = ActivityCenter::new();
    center.register_provider(ProviderId("downloads"), true);
    let key = ActivityKey::new("downloads", 1);
    let historical = ActivityKey::new("downloads", 2);
    let row = |key| {
        Activity::new(key, ActivityKind::Download, "private-filename")
            .detail("private-path-and-error")
            .progress(ActivityProgress::Bytes {
                done: 25,
                total: Some(100),
            })
    };
    center.publish(row(key), 0);
    center.publish(row(historical), 0);
    center.finish(historical, Generation::FIRST, TerminalOutcome::Completed, 1);
    let mut jobs = CenterJobs::default();
    let describe = |_: &Activity| Some("Downloading".to_owned());
    jobs.sync(
        &publisher,
        &center,
        None,
        Visibility::WhenUnfocused,
        describe,
    );
    let control = publisher
        .start(Status::running("Control operation"))
        .unwrap();
    let first = broker.next("batch");
    assert_eq!(
        first.value["works"].as_array().unwrap().len(),
        1,
        "unrevealed work and history stay silent"
    );
    assert_eq!(first.value["works"][0]["label"], "Control operation");
    center.prune(800);
    jobs.sync(
        &publisher,
        &center,
        Some("download-window"),
        Visibility::WhenUnfocused,
        describe,
    );
    let frame = broker.next("batch");
    let work = &frame.value["works"][0];
    let original_job = work["job"].clone();
    assert_eq!(work["label"], "Downloading");
    assert_eq!(work["detail"], "");
    assert_eq!(
        work["progress"],
        json!({"done":25,"total":100,"unit":"bytes"})
    );
    assert_eq!(work["window_token"], "download-window");
    center.progress(key, Generation::FIRST, ActivityProgress::Fraction(0.4));
    center.set_live_phase(key, Generation::FIRST, LivePhase::Paused);
    jobs.sync(
        &publisher,
        &center,
        Some("download-window"),
        Visibility::WhenUnfocused,
        describe,
    );
    let frame = broker.next("batch");
    assert_eq!(frame.value["works"][0]["state"], "paused");
    assert_eq!(frame.value["works"][0]["progress"]["done"], 400_000);
    assert_eq!(frame.value["works"][0]["progress"]["unit"], "steps");
    center.progress(key, Generation::FIRST, ActivityProgress::Indeterminate);
    center.set_live_phase(key, Generation::FIRST, LivePhase::Running);
    jobs.sync(
        &publisher,
        &center,
        Some("download-window"),
        Visibility::Always,
        describe,
    );
    let frame = broker.next("batch");
    assert_eq!(frame.value["works"][0]["state"], "running");
    assert!(frame.value["works"][0]["progress"].is_null());
    assert_eq!(frame.value["works"][0]["visibility"], "always");
    center.finish(key, Generation::FIRST, TerminalOutcome::Failed, 900);
    jobs.sync(
        &publisher,
        &center,
        None,
        Visibility::WhenUnfocused,
        describe,
    );
    let finished = broker.next("finish");
    assert_eq!(finished.value["job"], original_job);
    assert_eq!(finished.value["outcome"], "failed");
    jobs.sync(
        &publisher,
        &center,
        None,
        Visibility::WhenUnfocused,
        describe,
    );
    control
        .update(Status::running("Control after completion"))
        .unwrap();
    let barrier = broker.next("batch");
    assert_eq!(barrier.value["works"].as_array().unwrap().len(), 1);
    assert_eq!(
        barrier.value["works"][0]["label"],
        "Control after completion"
    );
    center.publish(row(key).generation(Generation::FIRST.next()), 1000);
    center.prune(1800);
    jobs.sync(
        &publisher,
        &center,
        None,
        Visibility::WhenUnfocused,
        describe,
    );
    let retried = broker.next("batch").value["works"][0]["job"].clone();
    assert_ne!(retried, original_job, "a retry owns a new broker job");
    center.publish(row(key).generation(Generation::FIRST.next().next()), 2000);
    center.prune(2800);
    jobs.sync(
        &publisher,
        &center,
        None,
        Visibility::WhenUnfocused,
        describe,
    );
    let changes = [
        broker.next_any(&["batch", "remove"]),
        broker.next_any(&["batch", "remove"]),
    ];
    let replacement = changes
        .iter()
        .find(|frame| frame.value["op"] == "batch")
        .expect("replacement publication")
        .value["works"][0]["job"]
        .clone();
    assert_eq!(
        changes
            .iter()
            .find(|frame| frame.value["op"] == "remove")
            .expect("old attempt removal")
            .value["job"],
        retried
    );
    assert_ne!(
        replacement, retried,
        "replacing a live attempt must retire its job"
    );
    center.set_provider_enabled(ProviderId("downloads"), false);
    jobs.sync(
        &publisher,
        &center,
        None,
        Visibility::WhenUnfocused,
        describe,
    );
    assert_eq!(broker.next("remove").value["job"], replacement);
}
