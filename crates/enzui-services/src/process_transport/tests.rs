use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn sh(script: &str, stdout: usize, stderr: usize, deadline: Duration) -> ProcessRequest {
    ProcessRequest::new("/bin/sh", StdinPolicy::Null, stdout, stderr, deadline)
        .arg("-c")
        .arg(script)
}

struct Marker {
    _scratch: enzui_test_support::test_scratch::ScratchDir,
    path: PathBuf,
}

impl std::ops::Deref for Marker {
    type Target = Path;
    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for Marker {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

fn marker(tag: &str) -> Marker {
    let sequence = MARKER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let scratch =
        enzui_test_support::test_scratch::ScratchDir::new(&format!("pt-{tag}-{sequence}"));
    let path = scratch.join(format!(
        "enzui-process-transport-{tag}-{}-{sequence}",
        std::process::id()
    ));
    Marker {
        _scratch: scratch,
        path,
    }
}

fn fixture_pids(marker: &Path) -> (u32, u32) {
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        if let Ok(text) = std::fs::read_to_string(marker) {
            let mut fields = text.split_whitespace();
            let leader = fields
                .next()
                .expect("fixture leader pid")
                .parse()
                .expect("numeric leader pid");
            let descendant = fields
                .next()
                .expect("fixture descendant pid")
                .parse()
                .expect("numeric descendant pid");
            return (leader, descendant);
        }
        assert!(Instant::now() < until, "fixture did not publish its PIDs");
        thread::sleep(POLL_INTERVAL);
    }
}

#[test]
fn explicit_stdin_bytes_are_written_then_closed() {
    let output = run(
        ProcessRequest::new(
            "/bin/cat",
            StdinPolicy::Bytes(b"structured stdin".to_vec()),
            16,
            0,
            Duration::from_secs(2),
        ),
        &CancelToken::new(),
    )
    .expect("cat observes bytes followed by EOF");
    assert_eq!(output.stdout, b"structured stdin");
    assert!(output.stderr.is_empty());
}

fn proc_state(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    Some(
        stat.rsplit_once(") ")?
            .1
            .split_whitespace()
            .next()?
            .to_owned(),
    )
}

struct ReaperFixture(&'static ReaperRegistry);

impl ReaperFixture {
    fn service(&self) -> &ReaperService {
        self.0
            .service
            .get()
            .expect("fixture initialized the reaper")
    }

    fn shutdown_and_join(&self) {
        self.service().request_shutdown();
        reaper_wait_until("owned rescue worker must finish", || {
            self.service().join_if_finished()
        });
        assert!(self.service().worker.lock().unwrap().is_none());
    }
}

impl Drop for ReaperFixture {
    fn drop(&mut self) {
        let Some(service) = self.0.service.get() else {
            return;
        };
        service.request_shutdown();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            for slot in &service.shared.slots {
                let mut pending = match slot.child.try_lock() {
                    Ok(pending) => pending,
                    Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => continue,
                };
                if let Some(owned) = pending.as_mut() {
                    if let Some(seam) = &owned.seam {
                        seam.reaper_wait_recovered.store(true, Ordering::Release);
                    }
                    let _ = owned.child.kill();
                }
            }
            if service.join_if_finished() || Instant::now() >= until {
                return;
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn reaper_wait_until(context: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(Instant::now() < until, "{context}");
        thread::sleep(POLL_INTERVAL);
    }
}

fn reaper_sleep_owner(registry: &'static ReaperRegistry, seam: &CleanupSeam) -> ProcessOwner {
    let faults = Faults {
        reaper_registry: Some(registry),
        cleanup_seam: Some(seam.clone()),
        ..Faults::default()
    };
    let permit = establish_reaper(&faults).expect("reserve exact-child custody");
    let mut command = Command::new("/bin/sleep");
    command
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    platform::configure(&mut command);
    let child = command.spawn().expect("spawn owned sleep fixture");
    ProcessOwner::new(child, permit, &faults)
}

mod background_scope;
mod lifecycle;

mod sensitive;
