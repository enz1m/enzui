//! Descriptors handed to one native helper and to no other process.
#![cfg(target_os = "linux")]
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

use enzui_core::native_child::{NativeCommand, SpawnError, Stdio};
use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::fd::{AsFd as _, OwnedFd};

/// The tests count this process's descriptors, so they run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn open_fds() -> BTreeSet<i32> {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

/// A secret readable from one pipe end, and an empty output pipe.
fn secret_and_output() -> (OwnedFd, OwnedFd, std::io::PipeReader) {
    let (secret, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"recovery-secret").unwrap();
    drop(writer);
    let (output_reader, output_writer) = std::io::pipe().unwrap();
    (
        OwnedFd::from(secret),
        OwnedFd::from(output_writer),
        output_reader,
    )
}

#[test]
fn a_failed_launch_closes_every_descriptor_it_opened() {
    let _serial = serial();
    let (secret, output, _document) = secret_and_output();
    let before = open_fds();
    let failed = NativeCommand::new("/nonexistent/enzui-helper")
        .stdout(Stdio::Piped)
        .descriptor(secret.as_fd(), 3)
        .unwrap()
        .descriptor(output.as_fd(), 4)
        .unwrap()
        .spawn();
    assert!(
        matches!(&failed, Err(SpawnError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound),
        "{failed:?}"
    );
    assert_eq!(open_fds(), before, "a failed launch left descriptors open");
}
