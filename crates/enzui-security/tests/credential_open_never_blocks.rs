//! The credential loader's `open(2)` must never wait for a FIFO peer.

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
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use enzui_security::secret::{SecretError, load_secret};

/// The bounded wait for one open. Generous enough that ordinary scheduling
/// jitter on a loaded builder cannot exceed it, while an unbounded FIFO open
/// exceeds it by construction (it never returns at all).
const BUDGET: Duration = Duration::from_secs(5);

/// The control's budget. Shorter only because the control is *expected* to
/// exhaust it, and a blocked open never finishes no matter how long we wait.
const CONTROL_BUDGET: Duration = Duration::from_secs(2);

/// Scratch on real disk under `target/`, never the RAM-backed system temp base.
fn scratch(tag: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "credential-open-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create scratch base");
    path
}

/// Create a mode-0600 FIFO at `path`, owned by this uid.
fn make_private_fifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg("-m")
        .arg("600")
        .arg(path)
        .status()
        .expect("run mkfifo(1)");
    assert!(status.success(), "mkfifo {} failed", path.display());
    let mode = std::fs::symlink_metadata(path)
        .expect("lstat fifo")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode, 0o600, "the fixture must be an owner-only FIFO");
}

/// Run `open` on a detached thread and wait at most `budget` for it.
fn bounded<T: Send + 'static>(
    budget: Duration,
    open: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(open());
    });
    rx.recv_timeout(budget).ok()
}

/// The positive control: a plain blocking `File::open` of `fifo` must not
/// complete within the control budget.
fn assert_a_blocking_open_of_this_fifo_hangs(fifo: &Path) {
    let path = fifo.to_path_buf();
    let outcome = bounded(CONTROL_BUDGET, move || {
        std::fs::File::open(&path).map(|_| ()).map_err(|e| e.kind())
    });
    assert!(
        outcome.is_none(),
        "control: a plain read-only open of {} returned {outcome:?} instead of \
         blocking, so the fixture cannot prove anything about O_NONBLOCK",
        fifo.display()
    );
}

/// A FIFO planted at a credential path is refused as `NotRegularFile` without
/// the loader ever parking in `open(2)`.
#[test]
fn a_planted_fifo_credential_is_refused_without_blocking_the_loader() {
    let base = scratch("fifo");
    let credential = base.join("agent.key");
    make_private_fifo(&credential);

    assert_a_blocking_open_of_this_fifo_hangs(&credential);

    let path = credential.clone();
    let outcome = bounded(BUDGET, move || load_secret(&path).map(|s| s.len()));
    let Some(result) = outcome else {
        panic!(
            "load_secret blocked inside open(2) on the writer-less FIFO {} for \
             longer than {BUDGET:?}; the descriptor checks that would refuse it \
             are unreachable",
            credential.display()
        );
    };
    assert!(
        matches!(result, Err(SecretError::NotRegularFile)),
        "a FIFO at a credential path must be refused as NotRegularFile, got {result:?}"
    );

    let _ = std::fs::remove_file(&credential);
    let _ = std::fs::remove_dir(&base);
}

/// The ordinary credential still loads through the same non-blocking open, so
/// the flag is a refusal for planted objects and inert for a regular file.
#[test]
fn an_ordinary_private_credential_still_loads_through_the_nonblocking_open() {
    let base = scratch("regular");
    let credential = base.join("agent.key");
    std::fs::write(&credential, b"correct horse battery staple").expect("write credential");
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600))
        .expect("chmod credential");

    let path = credential.clone();
    let outcome = bounded(BUDGET, move || {
        load_secret(&path).map(|s| s.as_bytes().to_vec())
    });
    let result = outcome.unwrap_or_else(|| {
        panic!(
            "load_secret did not return for the ordinary regular credential {} \
             within {BUDGET:?}",
            credential.display()
        )
    });
    assert_eq!(
        result
            .expect("a 0600 regular credential must load")
            .as_slice(),
        b"correct horse battery staple",
        "O_NONBLOCK must be inert for a regular file: the full payload must be read"
    );

    let _ = std::fs::remove_file(&credential);
    let _ = std::fs::remove_dir(&base);
}

/// A directory is still the plain, already-covered `NotRegularFile` refusal.
#[test]
fn a_directory_credential_path_remains_a_not_regular_file_refusal() {
    let base = scratch("directory");
    let credential = base.join("agent.key");
    std::fs::create_dir(&credential).expect("create directory fixture");
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o700))
        .expect("chmod directory fixture");

    let result = load_secret(&credential);
    assert!(
        matches!(result, Err(SecretError::NotRegularFile)),
        "a directory at a credential path must be refused as NotRegularFile, got {result:?}"
    );

    let _ = std::fs::remove_dir(&credential);
    let _ = std::fs::remove_dir(&base);
}
