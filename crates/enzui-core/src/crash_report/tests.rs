use super::*;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "enzui-crash-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
enum ProbeMsg {
    Login {
        #[allow(dead_code, reason = "fixture fields are written but never read")]
        password: String,
    },
}

impl MessageKind for ProbeMsg {
    fn message_kind(&self) -> &'static str {
        match self {
            ProbeMsg::Login { .. } => "Login",
        }
    }
}

// Keep this frame uninlined so its distinctive symbol remains observable in captured backtraces.
#[inline(never)]
fn enzui_crash_probe_panicking_frame(message: &'static str) -> ! {
    std::hint::black_box(&message);
    panic!("{message}");
}

#[test]
fn child_process_entry() {
    let Ok(scenario) = std::env::var("ENZUI_CRASH_CHILD") else {
        return;
    };
    let root = PathBuf::from(std::env::var("ENZUI_CRASH_ROOT").expect("ENZUI_CRASH_ROOT"));

    match scenario.as_str() {
        "primary" => child_primary(&root),
        "held_lock" => child_held_lock(&root),
        "poisoned_ring" => child_poisoned_ring(&root),
        "two_panics" => child_two_panics(&root),
        other => panic!("unknown crash scenario {other:?}"),
    }
}

fn child_primary(root: &Path) {
    let sentinel = root.join("prev-hook-ran.marker");
    std::panic::set_hook(Box::new(move |_info| {
        let _ = std::fs::write(&sentinel, b"prev-hook ran");
    }));

    CrashReporter::new("probe-app", "9.9.9", root)
        .ring_capacity(4)
        .install();

    record("first_dropped");
    record("second_dropped");
    record("kept_a");
    record("kept_b");
    record("kept_c");
    record_kind(&ProbeMsg::Login {
        password: "hunter2SECRET".to_owned(),
    });

    let handle = std::thread::Builder::new()
        .name("probe-crash-thread".to_owned())
        .spawn(|| enzui_crash_probe_panicking_frame("probe boom marker"))
        .expect("spawn");
    let _ = handle.join();
}

fn child_held_lock(root: &Path) {
    CrashReporter::new("probe-app", "9.9.9", root).install();

    let handle = std::thread::Builder::new()
        .name("held-lock-thread".to_owned())
        .spawn(|| {
            let _guard = RING.lock().expect("lock ring");
            enzui_crash_probe_panicking_frame("held lock boom");
        })
        .expect("spawn");
    let _ = handle.join();
}

fn child_poisoned_ring(root: &Path) {
    CrashReporter::new("probe-app", "9.9.9", root)
        .ring_capacity(4)
        .install();

    record("survived_a");
    record("survived_b");

    let poisoner = std::thread::Builder::new()
        .name("ring-poisoner".to_owned())
        .spawn(|| {
            let _guard = RING.lock().expect("lock ring");
            enzui_crash_probe_panicking_frame("poisoning boom");
        })
        .expect("spawn");
    let _ = poisoner.join();

    let handle = std::thread::Builder::new()
        .name("poisoned-ring-thread".to_owned())
        .spawn(|| enzui_crash_probe_panicking_frame("poisoned boom"))
        .expect("spawn");
    let _ = handle.join();
}

fn child_two_panics(root: &Path) {
    CrashReporter::new("probe-app", "9.9.9", root).install();

    for message in ["boom one", "boom two"] {
        let handle = std::thread::Builder::new()
            .name("multi-panic-thread".to_owned())
            .spawn(move || enzui_crash_probe_panicking_frame(message))
            .expect("spawn");
        let _ = handle.join();
    }
}

fn spawn_child(scenario: &str, root: &Path, remove_backtrace: bool) -> Child {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(exe);
    cmd.args([
        "--exact",
        "crash_report::tests::child_process_entry",
        "--nocapture",
        "--test-threads=1",
    ]);
    cmd.env("ENZUI_CRASH_CHILD", scenario);
    cmd.env("ENZUI_CRASH_ROOT", root);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    if remove_backtrace {
        cmd.env_remove("RUST_BACKTRACE");
        cmd.env_remove("RUST_LIB_BACKTRACE");
    }
    cmd.spawn().expect("spawn child")
}

fn wait_or_kill(mut child: Child, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return false,
        }
    }
}

fn run(scenario: &str, root: &Path, remove_backtrace: bool) -> bool {
    let child = spawn_child(scenario, root, remove_backtrace);
    wait_or_kill(child, Duration::from_secs(30))
}

fn crash_files(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|error| {
        panic!(
            "crash_files: open crash-report directory {}: {error}",
            dir.display()
        )
    });
    collect_crash_files(dir, entries.map(|entry| entry.map(|entry| entry.path())))
        .unwrap_or_else(|error| panic!("{error}"))
}

fn collect_crash_files(
    dir: &Path,
    entries: impl IntoIterator<Item = io::Result<PathBuf>>,
) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "crash_files: read entry in crash-report directory {}: {error}",
                    dir.display()
                ),
            )
        })?;
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("crash-") && name.ends_with(".txt"))
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn read_only_report(dir: &Path) -> String {
    let files = crash_files(dir);
    assert_eq!(
        files.len(),
        1,
        "expected exactly one crash report in {dir:?}, found {files:?}"
    );
    std::fs::read_to_string(&files[0]).expect("read report")
}

#[test]
fn ring_redacts_message_payloads() {
    let td = TmpDir::new("redact");
    assert!(run("primary", td.path(), false), "child must complete");
    let report = read_only_report(td.path());

    assert!(
        !report.contains("hunter2SECRET"),
        "a secret-shaped message payload leaked into the crash file — the \
         ring must store kinds, never payloads:\n{report}"
    );
    assert!(
        report.contains("Login"),
        "positive control: the message KIND must be present, or the redaction \
         assertion above proves nothing:\n{report}"
    );
}

#[test]
fn the_next_start_lists_an_unseen_report_until_it_is_seen() {
    let tmp = TmpDir::new("unseen");
    assert!(run("primary", tmp.path(), false), "the child crashed");
    let unseen = unseen_reports(tmp.path());
    assert_eq!(unseen.len(), 1, "{unseen:?}");
    assert_eq!(unseen[0].panic_message(), "probe boom marker");
    assert!(unseen[0].read().unwrap().contains("probe-app 9.9.9"));
    let seen = unseen[0].mark_seen().unwrap();
    assert!(seen.exists(), "the report is kept");
    assert!(unseen_reports(tmp.path()).is_empty());
}
