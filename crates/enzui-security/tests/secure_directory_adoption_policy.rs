//! `secure_dir::ensure_private_dir` — create privately, adopt only after verification.

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

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use enzui_security::secure_dir::{SecureDirRefusal, ensure_private_dir};

/// Scratch under `CARGO_TARGET_TMPDIR` (real disk under `target/`, never the
/// RAM-backed `/tmp`), unique per test and per run.
fn scratch(tag: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "secure-directory-adoption-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch base");
    path
}

fn mode_of(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .mode()
        & 0o7777
}

/// Make a fixture directory with exactly `mode`, defeating the umask.
fn make_dir_with_mode(path: &Path, mode: u32) {
    std::fs::create_dir_all(path).expect("create fixture dir");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod fixture");
    assert_eq!(
        mode_of(path),
        mode,
        "the fixture must really carry mode {mode:o} for the test below to mean \
         anything"
    );
}

/// Group-writable is refused, and the refusal does not repair the mode.
#[test]
fn a_group_writable_root_is_refused_without_being_chmodded() {
    let base = scratch("group-write");
    let root = base.join("root");
    make_dir_with_mode(&root, 0o770);
    match ensure_private_dir(&root) {
        Err(SecureDirRefusal::GroupOrWorldWritable { mode }) => {
            assert_eq!(mode & 0o022, 0o020, "group write is the offending bit");
        }
        other => panic!("a 0770 root must be refused as writable, got {other:?}"),
    }
    assert_eq!(
        mode_of(&root),
        0o770,
        "a refusal must not silently chmod the directory it refused"
    );
}

/// World-writable is refused too (the `/tmp`-style 0777 case).
#[test]
fn a_world_writable_root_is_refused() {
    let base = scratch("world-write");
    let root = base.join("root");
    make_dir_with_mode(&root, 0o777);
    match ensure_private_dir(&root) {
        Err(SecureDirRefusal::GroupOrWorldWritable { mode }) => {
            assert_eq!(mode & 0o022, 0o022, "group and other write are both set");
        }
        other => panic!("a 0777 root must be refused as writable, got {other:?}"),
    }
}

/// A symlink at the leaf is refused before it can be followed; the private
/// target is the witness that we never wrote through the link.
#[test]
fn a_symlinked_root_is_refused_and_never_followed() {
    let base = scratch("symlink");
    let target = base.join("real-target");
    make_dir_with_mode(&target, 0o700);
    let link = base.join("link-root");
    symlink(&target, &link).expect("plant symlink");
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat link")
            .file_type()
            .is_symlink(),
        "the fixture must actually be a symlink"
    );

    assert_eq!(
        ensure_private_dir(&link),
        Err(SecureDirRefusal::NotADirectory),
        "a symlink at the root must be refused, never followed to its target"
    );
    assert_eq!(
        std::fs::read_dir(&target).expect("read target").count(),
        0,
        "nothing may have been created through the refused link"
    );
}
