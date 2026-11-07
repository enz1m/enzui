//! Shared test-scratch root and I/O-failure classification.
//!
//! # Why not `std::env::temp_dir()`
//!
//! On this project's dev/CI box `/tmp` is a 13 GiB tmpfs — every byte a
//! test writes there is resident RAM, not disk. The orchestrator's cargo
//! admission gate refuses a build below a 4096 MB `MemAvailable` floor (exit
//! 75), so scratch data written under `/tmp` is spent from the *same* budget
//! the build system rations. A gigabyte of stale fixtures left in `/tmp` by one
//! worker then refuses an unrelated `cargo` in another with a memory-pressure
//! error that names no offending directory — a genuinely confusing, entirely
//! avoidable failure mode.
//!
//! `CARGO_TARGET_TMPDIR` is the per-target scratch directory Cargo provides for
//! exactly this: it lives under `target/`, on real disk, and is cleaned when
//! the target dir is. This file is compiled into an integration test
//! binary, for which Cargo always sets the variable, so [`scratch_root`] reads
//! it with `env!` directly — no fallback is needed or wanted here (a `src/`
//! unit test, where the variable is *not* set, would need `option_env!` and a
//! repo-relative `target/` fallback instead; that is a separate helper).
//!
//! # Why I/O failure is classified
//!
//! When a fixture cannot create, write, archive or extract its scratch data the
//! failure is *infrastructure*, not the subject the test asserts about. A red
//! staleness gate whose real cause is "Disk quota exceeded" was read as "the
//! generated catalog is stale" and cost a wrong diagnosis. [`infra_failure`]
//! puts `INFRASTRUCTURE:` on the first line, ahead of any subject text, and
//! names the filesystem and its free space so a reader need not know that a tar
//! stderr is not a verdict.

use std::path::{Path, PathBuf};

/// Root for this integration suite's scratch data — under `target/`, on real
/// disk, never `/tmp`. See the module docs for why.
pub fn scratch_root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

/// Create a unique scratch subdirectory `<root>/<label>-<pid>-<seq>` under
/// `root`, returning an `INFRASTRUCTURE:`-classified `Err` if creation fails.
///
/// `root` is a parameter (rather than always [`scratch_root`]) so the positive
/// control in `scratch_root.rs` can point it at an unwritable path and
/// observe the classified message.
pub fn make_scratch_dir_in(root: &Path, label: &str, seq: u64) -> Result<PathBuf, String> {
    let path = root.join(format!("{label}-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&path)
        .map_err(|e| infra_failure("create test-scratch directory", &path, e))?;
    Ok(path)
}

/// [`make_scratch_dir_in`] rooted at [`scratch_root`], panicking (infra-
/// classified) on failure. The common case for a fixture.
pub fn make_scratch_dir(label: &str, seq: u64) -> PathBuf {
    match make_scratch_dir_in(&scratch_root(), label, seq) {
        Ok(p) => p,
        Err(message) => panic!("{message}"),
    }
}

/// Classify an I/O failure touching test scratch as infrastructure, not a
/// subject verdict. The returned string's first line starts with
/// `INFRASTRUCTURE:` and names the path; a filesystem/free-space note follows
/// when it can be determined.
pub fn infra_failure(action: &str, path: &Path, detail: impl std::fmt::Display) -> String {
    format!(
        "INFRASTRUCTURE: cannot {action} {} — test-scratch I/O failure, not a subject verdict.{}\n{detail}",
        path.display(),
        free_space_note(path),
    )
}

/// A ` [filesystem <fs> has <avail> free]` note for the filesystem holding
/// `path` (or its nearest existing ancestor), or an empty string if `df` is
/// unavailable. Best-effort and never itself panics — a classifier must not
/// fail while explaining a failure.
fn free_space_note(path: &Path) -> String {
    // Walk up to the nearest existing ancestor: the failing path itself often
    // does not exist yet (that is why creating it failed).
    let mut probe = path;
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return String::new(),
        }
    }
    let output = match std::process::Command::new("df")
        .arg("-Pk")
        .arg(probe)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return String::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    // POSIX `df -Pk`: header then one data line whose fields are
    // Filesystem, 1024-blocks, Used, Available, Capacity, Mounted-on.
    let Some(line) = text.lines().nth(1) else {
        return String::new();
    };
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 6 {
        return String::new();
    }
    let fs = fields[0];
    let mount = fields[5];
    let avail = fields[3]
        .parse::<u64>()
        .map_or_else(|_| fields[3].to_owned(), human_kib);
    format!(" [filesystem {fs} on {mount} has {avail} free]")
}

fn human_kib(kib: u64) -> String {
    let mib = kib as f64 / 1024.0;
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else {
        format!("{mib:.0} MiB")
    }
}
