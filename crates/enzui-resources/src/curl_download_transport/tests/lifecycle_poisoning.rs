//! Lifecycle mutex-poison recovery and unwinding tests.

use super::*;

/// Poison the transport's real lifecycle mutex by panicking while its guard is
/// held, and prove the poisoning actually happened (an uncalibrated witness
/// would make every assertion below vacuous).
fn poison_state(transport: &CurlDownloadTransport) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = transport.state.lock().expect("a fresh mutex is unpoisoned");
        panic!("deliberate panic while the transport state guard is held");
    }));
    assert!(outcome.is_err(), "the poisoning witness must have panicked");
    assert!(
        transport.state.is_poisoned(),
        "the witness did not poison the real state mutex; the test proves nothing"
    );
}

/// The abort-sensitive witness. A panic in `Drop` *while another panic is
/// unwinding* is not a test failure — it aborts the process, so it can only be
/// observed from a parent. The child below panics with a live poisoned
/// transport in scope; a recovering `Drop` lets the child die the ordinary way
/// (harness exit code 101), a panicking one kills it with `SIGABRT`.
#[test]
fn drop_while_unwinding_does_not_abort_the_process() {
    use std::os::unix::process::ExitStatusExt;

    let exe = std::env::current_exe().expect("current test executable");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "--exact",
        "curl_download_transport::tests::lifecycle_poisoning::poisoned_drop_during_unwind_child_entry",
        "--nocapture",
        "--test-threads=1",
    ]);
    cmd.env("ENZUI_TRANSPORT_UNWIND_CHILD", "1");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().expect("spawn the unwinding child");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        match child.try_wait().expect("wait for the unwinding child") {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the unwinding child never exited");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };

    assert!(
        status.signal().is_none(),
        "Drop panicked while unwinding and aborted the child with signal {:?}",
        status.signal()
    );
    assert_eq!(
        status.code(),
        Some(101),
        "the child must die from its own single panic, not from a double panic"
    );
}

/// Child entry for [`drop_while_unwinding_does_not_abort_the_process`]. It is
/// inert unless the parent selects it through the environment, so an ordinary
/// run of this suite does not panic here.
#[test]
fn poisoned_drop_during_unwind_child_entry() {
    if std::env::var_os("ENZUI_TRANSPORT_UNWIND_CHILD").is_none() {
        return;
    }
    let transport = CurlDownloadTransport::new();
    poison_state(&transport);
    // `transport` is live: its `Drop` runs while this panic unwinds.
    panic!("simulated application panic with a poisoned transport in scope");
}
