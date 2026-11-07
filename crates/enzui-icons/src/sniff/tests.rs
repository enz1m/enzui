//! Tests for the sniff policy (cache, single flight, generation invalidation,
//! bounded eviction) and the real `FsSniffer` (bounded reads, regular-file-only
//! refusal).

use super::*;

// ---- Single flight: COUNT invocations, do not compare results ----

// ---- Negative caching: Unknown / Failed must not re-sniff ----

// ---- Exact attempt identity: fingerprint supersession and terminal-once ----

// ---- Bounded in-flight: cap refuses without retaining; churn stays bounded ----

// ---- Bounded cache: more keys than capacity, live entry survives ----

// ==== FsSniffer: the real filesystem seam ====

// A unique temp directory under the OS temp dir, removed on drop. Hand-rolled
// because `tempfile` is not a dev-dependency.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut base = std::env::temp_dir();
        let uniq = format!(
            "enzui-sniff-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        base.push(uniq);
        std::fs::create_dir_all(&base).expect("create temp dir");
        Self { path: base }
    }

    fn child(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---- Bounded reads: assert the byte count read is capped ----

// ---- Regular-file-only: one negative test per rejected kind, each asserting
//      a refusal rather than an empty Unknown ----

#[cfg(unix)]
#[test]
fn fifo_is_refused_without_blocking() {
    let dir = TempDir::new("fifo-refuse");
    let fifo = dir.child("pipe");
    let status = std::process::Command::new("mkfifo").arg(&fifo).status();
    match status {
        Ok(s) if s.success() && fifo.exists() => {}
        _ => {
            println!("skipping fifo test: mkfifo unavailable");
            return;
        }
    }

    let sniffer = FsSniffer::new(vec![]);
    // This call must return rather than hang, and the answer must be a refusal.
    let out = sniffer.sniff(&fifo);
    assert_eq!(
        out,
        SniffOutcome::Failed(SniffError::NotRegularFile(FileKind::Fifo)),
        "a FIFO must be refused (and must not block on open)"
    );
}
