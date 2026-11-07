use super::*;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrd};
use std::time::{Duration, Instant};

fn is_child(key: &str) -> bool {
    std::env::var_os(key).is_some()
}

fn bounded_child_test(env_key: &str, test_name: &str) -> std::process::Output {
    // Prevent recursive child spawning.
    assert!(
        std::env::var_os(env_key).is_none(),
        "bounded_child_test called inside a child process (recursive spawn)",
    );

    let child = std::process::Command::new(
        std::env::current_exe().expect("locate persistence test binary"),
    )
    .arg("--exact")
    .arg(test_name)
    .arg("--nocapture")
    .env(env_key, "1")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .expect("spawn reentry child process");

    wait_for_bounded_child(child)
}

fn wait_for_bounded_child(mut child: std::process::Child) -> std::process::Output {
    use std::io::Read;

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("poll reentry child") {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr);
                }
                return std::process::Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            None if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            None => {
                child.kill().expect("kill timed-out reentry child");
                let status = child.wait().expect("reap timed-out reentry child");
                return std::process::Output {
                    status,
                    stdout: Vec::new(),
                    stderr: b"child exceeded 10-second deadline (likely deadlock)".to_vec(),
                };
            }
        }
    }
}

// A Weak back-reference reenters Store::load on the same store, so the
// nested load meets the same SaveOrdering the outer save holds.
#[derive(Clone)]
struct LoadFromSaveBackend {
    inner: InMemoryBackend,
    trigger_load: Arc<AtomicBool>,
    same_store: Arc<Mutex<Option<std::sync::Weak<Store<TestSettings>>>>>,
    load_completed: Arc<AtomicBool>,
}

impl LoadFromSaveBackend {
    fn new() -> Self {
        Self {
            inner: InMemoryBackend::new(),
            trigger_load: Arc::new(AtomicBool::new(false)),
            same_store: Arc::new(Mutex::new(None)),
            load_completed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_store(&self, store: &Arc<Store<TestSettings>>) {
        *self.same_store.lock().unwrap() = Some(Arc::downgrade(store));
    }

    fn arm(&self) {
        self.trigger_load.store(true, AtomicOrd::Release);
    }

    fn load_completed(&self) -> bool {
        self.load_completed.load(AtomicOrd::Acquire)
    }
}

impl PersistBackend for LoadFromSaveBackend {
    fn load_raw(&self, format_id: &str, version: u32) -> BackendLoad {
        self.inner.load_raw(format_id, version)
    }

    fn save_raw(&self, format_id: &str, version: u32, body: &str) -> Result<(), PersistError> {
        self.inner.save_raw(format_id, version, body)?;
        if self.trigger_load.swap(false, AtomicOrd::AcqRel) {
            let weak = self
                .same_store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(store) = weak.and_then(|w| w.upgrade()) {
                let _outcome = store.load();
                self.load_completed.store(true, AtomicOrd::Release);
            }
        }
        Ok(())
    }
}

const LFS_CHILD_KEY: &str = "ENZUI_PERSIST_LFS_CHILD";
const LFS_CHILD_TEST: &str = "persist::tests::reentry::load_from_save_reentry_completes";

#[test]
fn load_from_save_reentry_completes() {
    if is_child(LFS_CHILD_KEY) {
        let backend = LoadFromSaveBackend::new();
        let store = Arc::new(Store::<TestSettings>::new(
            backend.clone(),
            "reentry-lfs",
            1,
        ));
        backend.set_store(&store);
        backend.arm();

        store
            .save_blocking(&TestSettings::default())
            .expect("save with reentrant load must complete");

        assert!(
            backend.load_completed(),
            "reentrant load on the same store must have run and completed",
        );

        let outcome = store.load();
        assert_eq!(outcome.into_value(), TestSettings::default());

        return;
    }

    let output = bounded_child_test(LFS_CHILD_KEY, LFS_CHILD_TEST);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "load-from-save reentry child failed (deadlock under 10 s?)\nstdout: {stdout}\nstderr: {stderr}",
    );
    assert!(
        stdout.contains("1 passed"),
        "child must have run exactly one test, got:\nstdout: {stdout}\nstderr: {stderr}",
    );
}

// Each token carries a one-shot GatedSaveOp from the same gate; the
// callback executes it through the public path into GateShared.

// A panic in save_raw without ActiveSaveGuard leaves active_save set;
// the next save waits forever, hanging std::thread::scope.
