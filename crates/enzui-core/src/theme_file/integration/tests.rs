use super::service::{ServiceConfig, ThemeServiceRecipe};
use super::*;
use crate::theme_file::ThemeDocument;
use enzui_theme::EnzThemeMode;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Change detection: no-op suppression vs real-change delivery (independently
// calibrated, per the acceptance's requirement).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Bounded probe ownership, now internal to the service.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Observation / debounce / diagnostics.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Runtime consumer proof: the production recipe stream, driven by its real
// background timer over several poll intervals, reports zero committed events
// for unchanged/absent candidates and exactly one coherent update on a change.
// ---------------------------------------------------------------------------

struct PollSignal {
    woken: Mutex<bool>,
    changed: Condvar,
}

impl std::task::Wake for PollSignal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.woken.lock().expect("poll signal") = true;
        self.changed.notify_all();
    }
}

use std::sync::{Condvar, Mutex};

/// Drives the real recipe stream on its own thread, forwarding every committed
/// event to a channel until told to stop (which drops the stream and cancels
/// the service).
struct StreamHarness {
    events: mpsc::Receiver<ThemeEvent>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StreamHarness {
    fn start(config: ServiceConfig) -> Self {
        use iced_futures::futures::StreamExt as _;
        use iced_futures::futures::stream;

        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            use std::task::{Context, Poll, Waker};
            let recipe = ThemeServiceRecipe { config };
            let mut streamed = iced_futures::subscription::Recipe::stream(
                Box::new(recipe),
                stream::pending().boxed(),
            );
            let signal = Arc::new(PollSignal {
                woken: Mutex::new(false),
                changed: Condvar::new(),
            });
            let waker = Waker::from(Arc::clone(&signal));
            let mut cx = Context::from_waker(&waker);
            loop {
                if stop_worker.load(Ordering::SeqCst) {
                    break;
                }
                *signal.woken.lock().expect("poll signal") = false;
                match streamed.as_mut().poll_next(&mut cx) {
                    Poll::Ready(Some(event)) => {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Poll::Ready(None) => break,
                    Poll::Pending => {
                        let mut woken = signal.woken.lock().expect("poll signal");
                        while !*woken && !stop_worker.load(Ordering::SeqCst) {
                            let (guard, timeout) = signal
                                .changed
                                .wait_timeout(woken, Duration::from_millis(20))
                                .expect("poll signal");
                            woken = guard;
                            if timeout.timed_out() {
                                break;
                            }
                        }
                    }
                }
            }
            // Dropping `streamed` here cancels the background service.
        });
        Self {
            events: rx,
            stop,
            handle: Some(handle),
        }
    }

    fn drain_for(&self, window: Duration) -> Vec<ThemeEvent> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.events.recv_timeout(remaining) {
                Ok(event) => out.push(event),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for StreamHarness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

const SEVERAL_INTERVALS: Duration = Duration::from_millis(160);

// A hermetic temp dir; the crate has no `tempfile` dependency.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::AtomicUsize;
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "enzui-theme-service-{}-{tag}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("enzui")).expect("create temp config dir");
        Self { path }
    }

    fn theme_file(&self) -> PathBuf {
        self.path.join("enzui").join("theme.toml")
    }

    fn write_theme(&self, source: &str) {
        std::fs::write(self.theme_file(), source).expect("write theme file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// A real on-disk config, watched by the real `FsBackend`, on a fast timer.
fn fs_config(dir: &TempDir) -> ServiceConfig {
    ServiceConfig {
        paths: ThemePaths::new(
            Some(dir.path.clone()),
            Vec::<PathBuf>::new(),
            None::<PathBuf>,
        ),
        backend: Arc::new(crate::file_watch::FsBackend),
        probe: ProbeOverride::None,
        initial_os_mode: EnzThemeMode::Dark,
        poll_interval: Duration::from_millis(5),
        debounce: Duration::from_millis(1),
        directory_watch: super::service::DirectoryWatch::Off,
    }
}

#[test]
fn a_real_edit_delivers_exactly_one_committed_update() {
    let dir = TempDir::new("edit");
    dir.write_theme("version = 1\n[spacing]\nspace6 = 23\n");
    let harness = StreamHarness::start(fs_config(&dir));

    // Settle the boot load.
    let boot = harness.drain_for(SEVERAL_INTERVALS);
    assert_eq!(boot.len(), 1, "premise: boot load delivered once: {boot:?}");

    // A real external edit (different length so the identity certainly moves).
    dir.write_theme("version = 1\n[spacing]\nspace6 = 37\nspace5 = 11\n");
    let delivered = harness.drain_for(SEVERAL_INTERVALS);
    assert_eq!(
        delivered.len(),
        1,
        "a real filesystem edit must deliver exactly one coherent committed \
         update, got {delivered:?}"
    );
    assert!(matches!(
        &delivered[0],
        ThemeEvent::Loaded(load)
            if matches!(&load.outcome, Ok(doc) if doc == &ThemeDocument::parse(
                "version = 1\n[spacing]\nspace6 = 37\nspace5 = 11\n"
            ).unwrap())
    ));

    // And returns to silence afterward.
    let after = harness.drain_for(SEVERAL_INTERVALS);
    assert!(
        after.is_empty(),
        "after delivering the change the feed must go quiet again: {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Recipe identity and cancellation.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Application-side committed-event folding (theme(), OS precedence, unmanaged).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// OS appearance subscription (the second recipe of a following integration).
// ---------------------------------------------------------------------------
