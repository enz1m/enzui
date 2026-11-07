//! Crash-safe, no-overwrite SQLite relocation.

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
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use enzui_core::db_migration::{MigrationError, MigrationPlan};

const MAIN: &[u8] = b"SQLite format 3\0-main-file-bytes";
const WAL: &[u8] = b"-wal-frames-holding-committed-rows";
const SHM: &[u8] = b"-shm-index";

/// Suffix -> bytes for the whole database file set (`""` is the main file).
const FILE_SET: &[(&str, &[u8])] = &[("", MAIN), ("-wal", WAL), ("-shm", SHM)];

fn scratch(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "db-migration-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("mkdir scratch");
    base
}

/// A source directory holding `connections.db` + `-wal` + `-shm`, and an
/// empty destination directory. Returns the two main-file paths.
fn fixture(tag: &str) -> (PathBuf, PathBuf) {
    let root = scratch(tag);
    let old = root.join("old");
    let new = root.join("new");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let source_main = old.join("connections.db");
    for (suffix, bytes) in FILE_SET {
        std::fs::write(with_suffix(&source_main, suffix), bytes).unwrap();
    }
    (source_main, new.join("connections.db"))
}

fn with_suffix(main: &Path, suffix: &str) -> PathBuf {
    with_raw_suffix(main, suffix.as_bytes())
}

fn with_raw_suffix(main: &Path, suffix: &[u8]) -> PathBuf {
    let mut name = main.file_name().unwrap().as_bytes().to_vec();
    name.extend_from_slice(suffix);
    main.parent().unwrap().join(OsString::from_vec(name))
}

#[test]
fn a_symlinked_source_is_refused() {
    let (source_main, destination_main) = fixture("symlink-source");
    let real = with_suffix(&source_main, ".real");
    std::fs::rename(&source_main, &real).unwrap();
    std::os::unix::fs::symlink(&real, &source_main).unwrap();

    let error = MigrationPlan::prepare(&source_main, &destination_main)
        .expect_err("a symlinked source must be refused");
    assert!(
        matches!(error, MigrationError::SourceNotRegularFile { .. }),
        "got {error}"
    );
}
