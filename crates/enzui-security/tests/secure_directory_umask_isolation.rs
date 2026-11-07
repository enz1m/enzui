//! `secure_dir::ensure_private_dir` yields 0700 whatever the process umask.

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

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use enzui_security::secure_dir::ensure_private_dir;

/// Scratch under `CARGO_TARGET_TMPDIR`, never the RAM-backed system temp directory.
fn scratch(tag: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "secure-directory-umask-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch base");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("normalise scratch base to a known owner-writable mode");
    path
}

fn mode_of(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .mode()
        & 0o7777
}

#[allow(unsafe_code, reason = "`umask(2)` has no safe std API")]
fn set_umask(mask: u32) -> u32 {
    unsafe extern "C" {
        fn umask(mask: u32) -> u32;
    }
    // SAFETY: `umask(2)` takes and returns a single word, touches no memory, and
    // its only effect is the process mask, which every caller restores.
    unsafe { umask(mask) }
}

static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Under a lax umask the created root is still exactly `0o700` — and therefore
/// still *accepted* by the same call's own adoption check.
#[test]
fn a_fresh_root_is_owner_only_under_a_lax_umask() {
    let base = scratch("lax");
    // Create the scratch base before touching the mask so its own mode is not
    // part of what is under test.
    let root = base.join("nested/root");
    let control = base.join("control");

    let guard = UMASK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = set_umask(0o002);
    let created = ensure_private_dir(&root);
    let control_result = std::fs::create_dir_all(&control);
    let control_mode = control_result.as_ref().ok().map(|()| mode_of(&control));
    set_umask(previous);
    drop(guard);

    let created = created.expect("a fresh root must be accepted under a lax umask");
    assert_eq!(created, root);
    assert_eq!(
        mode_of(&root),
        0o700,
        "the created root must be owner-only regardless of the umask; got {:o}",
        mode_of(&root)
    );
    assert_eq!(
        control_mode,
        Some(0o775),
        "the control proves the umask was really 0o002; if this is not 0o775 the \
         mask did not take effect and the assertion above would be vacuous"
    );
    // Re-provisioning the directory we just made must adopt it, not refuse it.
    assert_eq!(
        ensure_private_dir(&root),
        Ok(root.clone()),
        "the root we created must pass our own adoption check"
    );
}

/// Under an owner-clipping umask, `mkdir(0o700 & ~umask)` would leave a
/// directory we cannot write; `set_permissions` forces exactly `0o700`.
#[test]
fn a_fresh_root_is_owner_only_under_a_strict_umask() {
    let base = scratch("strict");
    let root = base.join("nested/root");
    let control = base.join("control");

    let guard = UMASK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = set_umask(0o277);
    let created = ensure_private_dir(&root);
    let control_result = std::fs::create_dir_all(&control);
    let control_mode = control_result.as_ref().ok().map(|()| mode_of(&control));
    set_umask(previous);
    drop(guard);

    created.expect("a fresh root must be accepted under a strict umask");
    assert_eq!(
        mode_of(&root),
        0o700,
        "the created root must be forced to exactly 0o700, writable by us; got \
         {:o}",
        mode_of(&root)
    );
    assert_eq!(
        control_mode,
        Some(0o500),
        "the control proves the umask was really 0o277 and that it clips an owner \
         bit — without that, this test could not tell set_permissions apart from \
         the mkdir mode argument"
    );
}
