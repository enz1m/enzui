//! The runtime-directory resolver's fallback ladder, gated at every rung.
//!
//! `enzui_core::runtime_dir` resolves `$XDG_RUNTIME_DIR` (0700, per-user,
//! owned by us) for sockets, credential files and per-session state, and falls
//! back to a self-created 0700 directory otherwise. This suite asserts the
//! consequence — the owner and mode of the directory the resolver actually
//! returns, `stat`ed, at each rung — not the shape of the path string.
//!
//! # Why the resolver is injected here
//!
//! The wrong-owner and group-writable rungs cannot be built by env-mutation in
//! a parallel test binary (setting `$XDG_RUNTIME_DIR` races other tests) and
//! cannot `chown` to another uid without privilege. So the tests drive
//! `runtime_dir::resolve`, the injected seam (the same shape
//! `persist::XdgResolver` uses): the wrong-owner rung is reached by passing an
//! `our_uid` that differs from the fixture's real owner, and every fixture is
//! built under `CARGO_TARGET_TMPDIR` (real disk, off the /tmp tmpfs — the very
//! rule the scratch-directory ratchet enforces), never in global process env.

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

use std::ffi::OsString;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use enzui_core::runtime_dir::{FallbackReason, RuntimeDir, RuntimeDirOrigin, resolve};

/// A fresh, unique scratch directory under `CARGO_TARGET_TMPDIR` (real disk).
fn scratch(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "runtime-dir-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("mkdir scratch");
    base
}

/// Our uid as ground truth, derived independently of the resolver: create
/// a file and read the uid the kernel stamped on it. Used so the positive
/// controls do not lean on the same `current_uid` they are validating.
fn our_uid_ground_truth() -> u32 {
    let dir = scratch("uid-truth");
    let f = dir.join("probe");
    std::fs::write(&f, b"x").expect("write probe");
    let uid = std::fs::metadata(&f).expect("stat probe").uid();
    let _ = std::fs::remove_file(&f);
    uid
}

/// Make `path` a directory with exactly `mode` (defeating umask).
fn make_dir(path: &Path, mode: u32) {
    std::fs::create_dir_all(path).expect("mkdir fixture");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod fixture");
}

/// Assert the resolved directory exists, is owned by `uid`, and is mode 0700 —
/// the property the whole change is for.
fn assert_owned_0700(dir: &RuntimeDir, uid: u32) {
    let meta = std::fs::symlink_metadata(dir.path())
        .unwrap_or_else(|e| panic!("stat resolved dir {}: {e}", dir.path().display()));
    assert!(
        meta.file_type().is_dir(),
        "resolved runtime dir {} is not a directory",
        dir.path().display()
    );
    assert_eq!(
        meta.uid(),
        uid,
        "resolved runtime dir {} is owned by uid {}, expected our uid {uid}",
        dir.path().display(),
        meta.uid(),
    );
    let mode = meta.mode() & 0o7777;
    assert_eq!(
        mode,
        0o700,
        "resolved runtime dir {} has mode {mode:04o}, expected 0700 \
         (a directory that is not 0700 is the whole defect this resolver fixes)",
        dir.path().display(),
    );
}

// ---------------------------------------------------------------------------
// Rung 1 — the happy path and the positive control.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Rung 2 — benign absences fall back to a created 0700 dir, no refusal.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Rung 3 — security refusals. These must be REFUSALS, not shrugs.
// ---------------------------------------------------------------------------

/// A `$XDG_RUNTIME_DIR` owned by another uid is refused. Built by passing an
/// `our_uid` that differs from the fixture's real owner (no privileged chown
/// needed). Dropping the ownership check in `verify_owned_0700` makes this rung
/// stop refusing — the intended ownership-check sabotage.
#[test]
fn wrong_owner_is_refused() {
    let real_uid = our_uid_ground_truth();
    let base = scratch("wrong-owner");
    let xdg = base.join("run");
    make_dir(&xdg, 0o700); // owned by us (real_uid), 0700 — otherwise perfect

    // Pretend our uid is someone else's: the dir's real owner != our_uid.
    let bogus_uid = real_uid.wrapping_add(1);
    let dir = resolve(Some(OsString::from(&xdg)), Some(bogus_uid), &base).expect("resolve");

    match dir.origin() {
        RuntimeDirOrigin::Fallback(FallbackReason::WrongOwner { dir_uid, our_uid }) => {
            assert_eq!(*dir_uid, real_uid, "reported dir owner");
            assert_eq!(*our_uid, bogus_uid, "reported our uid");
        }
        other => panic!("wrong-owner $XDG_RUNTIME_DIR must be refused, got {other:?}"),
    }
    assert!(
        dir.is_refusal(),
        "a wrong-owner runtime dir is a refusal the user should hear about"
    );
    // The fallback we then land on is still ours and 0700 (keyed on bogus uid
    // in its name, but created and owned by the real process uid).
    assert_owned_0700(&dir, real_uid);
}

// ---------------------------------------------------------------------------
// The fallback re-verifies an existing directory rather than trusting it.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// current_uid — the uid probe. /proc/self must agree with an fstat.
// ---------------------------------------------------------------------------
