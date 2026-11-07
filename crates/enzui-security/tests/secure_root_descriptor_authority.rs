//! The retained descriptor is authoritative for `SecureRoot::open`.

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

use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use enzui_security::secure_dir::{SecureDirRefusal, SecureRoot};

/// Scratch on real disk under `target/`, never the RAM-backed system temp directory.
fn scratch(tag: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "secure-root-descriptor-{tag}-{}-{}",
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

/// Open `path` through the crate's `#[doc(hidden)]` opener so the fd's flags
/// match what `SecureRoot::open` retains. Tests never construct the fd
/// themselves; that would silently diverge from the constructor's flags.
fn openat_directory(path: &Path) -> OwnedFd {
    SecureRoot::openat_directory_for_test(path)
        .unwrap_or_else(|e| panic!("openat {}: {e}", path.display()))
}

fn dev_ino(path: &Path) -> (u64, u64) {
    let meta =
        std::fs::symlink_metadata(path).unwrap_or_else(|e| panic!("lstat {}: {e}", path.display()));
    (meta.dev(), meta.ino())
}

fn our_uid() -> u32 {
    enzui_security::runtime_dir::current_uid().expect("our own uid must be observable")
}

/// A directory owned by another uid is refused by the fd-side owner check.
#[test]
fn fd_side_owner_check_refuses_a_wrong_owner_directory() {
    let us = our_uid();
    let mut fixture: Option<(&Path, u32)> = None;
    for candidate in ["/usr", "/", "/etc", "/usr/lib"] {
        let path = Path::new(candidate);
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if meta.file_type().is_dir() && meta.uid() != us && meta.mode() & 0o022 == 0 {
            fixture = Some((path, meta.uid()));
            break;
        }
    }
    let Some((system, dir_uid)) = fixture else {
        eprintln!(
            "SKIP: every candidate system directory is owned by our own uid \
             ({us}), so no unprivileged wrong-owner fixture exists on this host"
        );
        return;
    };

    let fd = openat_directory(system);
    let (dev, ino) = dev_ino(system);
    // Feed the verifier the fd's own (dev, ino) so the identity check is
    // vacuous and only the uid check can refuse.
    let refusal = SecureRoot::verify_open_fd_for_test(&fd, dev, ino, us);
    assert_eq!(
        refusal,
        Err(SecureDirRefusal::WrongOwner {
            dir_uid,
            our_uid: us
        }),
        "the fd-side owner check must refuse a wrong-owner directory even \
         when the pathname identity is presented as matching"
    );
}

/// A directory chmod'd to 0o777 between openat and fstat is refused on mode.
#[test]
fn fd_side_mode_check_refuses_group_or_world_writable_after_open() {
    let base = scratch("chmod-race");
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

    // Open the fd while the directory is still 0o700, then chmod. The
    // pathname view saw 0o700; the fd's fstat now sees 0o777.
    let fd = openat_directory(&root);
    let (dev, ino) = dev_ino(&root);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777))
        .expect("chmod after open");

    // Feed the verifier the fd's own (dev, ino) so the identity check is
    // vacuous and only the mode check can refuse.
    match SecureRoot::verify_open_fd_for_test(&fd, dev, ino, our_uid()) {
        Err(SecureDirRefusal::GroupOrWorldWritable { mode }) => {
            assert_eq!(
                mode & 0o022,
                0o022,
                "the offending bits must be reported so a caller can log them"
            );
        }
        other => panic!("expected GroupOrWorldWritable refusal, got {other:?}"),
    }
}

/// The identity check catches a substitution even when every other check
/// would pass — same-uid, 0o700, but a different inode.
#[test]
fn fd_side_identity_check_refuses_same_uid_same_mode_substitute() {
    let base = scratch("identity-swap");
    let verified_side = base.join("verified");
    let substitute = base.join("substitute");
    for dir in [&verified_side, &substitute] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (verified_dev, verified_ino) = dev_ino(&verified_side);
    let (substitute_dev, substitute_ino) = dev_ino(&substitute);
    assert_ne!(
        (verified_dev, verified_ino),
        (substitute_dev, substitute_ino),
        "the two fixtures must have distinct inodes for this test to mean anything"
    );

    let fd = openat_directory(&substitute);
    match SecureRoot::verify_open_fd_for_test(&fd, verified_dev, verified_ino, our_uid()) {
        Err(SecureDirRefusal::RootIdentityChanged { original, current }) => {
            assert_eq!(original, (verified_dev, verified_ino));
            assert_eq!(current, Some((substitute_dev, substitute_ino)));
        }
        other => panic!("expected RootIdentityChanged, got {other:?}"),
    }
}
