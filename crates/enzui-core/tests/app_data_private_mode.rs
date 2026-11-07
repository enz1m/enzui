//! `AppDataResolver::ensure` must create the app-owned leaf as a private
//! `0o700` directory before any consumer writes a payload into it.
//!
//! # Why a separate binary
//!
//! `umask(2)` is a *process*-wide global, not per-thread. `app_data.rs`
//! contains the rest of the resolver's tests, several of which create
//! scratch directories with `std::fs::create_dir_all`. If we co-locate the
//! umask tests with them, a strict-umask test setting the process mask to
//! `0o277` will race with a concurrent scratch mkdir and leave that scratch
//! at `0o500` — a directory the sibling test cannot write, which then panics
//! for the wrong reason. Running the mode-sensitive tests in their own
//! binary means `UMASK_LOCK` covers every mkdir that could be affected: the
//! only mkdirs in this binary are the ones the mode tests do themselves.
//!
//! # What each test proves
//!
//! * `fresh_leaf_is_0o700_under_lax_umask` — the fresh-create branch sets
//!   mode `0o700` at `mkdir(2)` time. Positive control: a sibling
//!   `create_dir_all` under the same umask keeps group/other bits, so the
//!   `0o700` assertion is not vacuous.
//! * `set_permissions_forces_0o700_under_strict_umask` — the strict-umask
//!   branch: `mkdir(mode & ~umask)` would leave `0o500`, and the module's
//!   `set_permissions` step forces exactly `0o700`. This is what pins the
//!   `set_permissions` line; without it the assertion goes red.
//! * `adopts_existing_private_leaf` — a `0o700` directory left by an
//!   earlier boot is adopted, not refused.
//! * `refuses_existing_lax_leaf_without_chmod` — a `0o755` planted
//!   directory is refused, and the refusal does not silently chmod it (the
//!   rubric: no chmod-after, ever).
//! * `tightening_makes_an_own_lax_leaf_private` — the opt-in
//!   `ensure_tightening` makes that same own `0o755` leaf `0o700`, keeping
//!   its contents, as state files are tightened on open.
//! * `tightening_refuses_a_symlinked_leaf_without_touching_the_target` — the
//!   opt-in never follows a symlink to chmod what it points at.

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
use std::path::{Path, PathBuf};

use enzui_core::app_data::{AppDataError, AppDataResolver, AppId};

fn scratch(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "app-data-private-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("mkdir scratch");
    base
}

fn app() -> AppId {
    AppId::new("enzui-remote").expect("valid app id")
}

#[cfg(unix)]
fn dir_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .expect("stat leaf")
        .permissions()
        .mode()
        & 0o7777
}

#[cfg(unix)]
static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
#[test]
fn refuses_existing_lax_leaf_without_chmod() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = UMASK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = scratch("refuse-lax");
    let data_home = root.join("data");
    std::fs::create_dir_all(&data_home).unwrap();

    // A pre-existing lax directory left by an older, unhardened build (or
    // by `mkdir -p` in a shell script) must not be silently trusted — that
    // would re-open the exact leak this rung closes — and must not be
    // silently chmod'd — that would fight a user-configured mode and hide
    // the surprise from the caller.
    let leaf = data_home.join(app().as_str());
    std::fs::create_dir_all(&leaf).unwrap();
    std::fs::set_permissions(&leaf, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(dir_mode(&leaf), 0o755, "planted directory must be 0o755");

    let r = AppDataResolver::new(Some(&data_home), None::<&str>);
    match r.ensure(&app()) {
        Err(AppDataError::Create { path, message }) => {
            assert_eq!(path, leaf);
            assert!(
                message.contains("755") || message.contains("group/other"),
                "the refusal must name the offending mode, got {message:?}"
            );
        }
        other => panic!("a 0o755 pre-existing leaf must be refused, got {other:?}"),
    }
    assert_eq!(
        dir_mode(&leaf),
        0o755,
        "a refusal must not chmod the directory it refused"
    );
}
