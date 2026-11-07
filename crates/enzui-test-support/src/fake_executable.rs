//! Fake helper programs for tests that run them.
//!
//! A script a test writes and then runs can fail to start with "Text file
//! busy" (ETXTBSY) when tests run in parallel. A child that another test
//! thread forks while the script is open for writing inherits that
//! descriptor until its own `exec`, and the kernel refuses to execute a file
//! anyone holds open for writing. [`write_executable`] never opens the script
//! for writing in the test process: a short-lived `sh` child writes it from a
//! pipe and has exited before this returns, so no thread of the test process
//! can pass a write descriptor on.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `body` to `path` as an executable (mode 0755) that a parallel test
/// suite can run at once.
///
/// # Panics
/// When `/bin/sh` cannot write the file.
pub fn write_executable(path: &Path, body: &str) {
    write_executable_with_mode(path, body, 0o755);
}

/// [`write_executable`] with an explicit permission `mode`, for a test whose
/// subject checks it (an owner-only `0o700` script, say).
///
/// # Panics
/// When `/bin/sh` cannot write the file.
pub fn write_executable_with_mode(path: &Path, body: &str, mode: u32) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" \"$1\"", "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(Stdio::piped())
        .spawn()
        .expect("sh writes the test executable");
    {
        let mut stdin = writer.stdin.take().expect("the writer's stdin");
        stdin
            .write_all(body.as_bytes())
            .expect("the executable's body");
    }
    let status = writer.wait().expect("the writer exits");
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
}
