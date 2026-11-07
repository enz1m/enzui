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
#![cfg(all(target_os = "linux", feature = "fault-injection"))]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use enzui_security::quarantine::{
    ExpectedSourceIdentity, QuarantineOutcome, RecoveryCustody, quarantine_sibling,
    quarantine_sibling_expected,
};

const ORIGINAL: &[u8] = b"original recovery bytes";
const FOREIGN: &[u8] = b"foreign replacement bytes";
const PRIOR: &[u8] = b"prior quarantine bytes";

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let mut path = std::env::current_exe().expect("test executable path");
        path.pop();
        path.push(format!(
            "sibling-quarantine-{label}-{}-{}",
            std::process::id(),
            unique_number()
        ));
        fs::create_dir(&path).expect("create target-local scratch directory");
        Self { path }
    }

    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn expected_source(path: &Path) -> ExpectedSourceIdentity {
    let metadata = fs::metadata(path).expect("source metadata");
    ExpectedSourceIdentity::from_dev_ino(metadata.dev(), metadata.ino())
}

fn generated_names(root: &Path, source_name: &str) -> Vec<PathBuf> {
    fs::read_dir(root)
        .expect("read scratch directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&format!("{source_name}.recovery"))
                || name.starts_with(&format!("{source_name}.source-custody"))
        })
        .map(|entry| entry.path())
        .collect()
}

fn unique_number() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn custody(outcome: &QuarantineOutcome) -> &RecoveryCustody {
    match outcome {
        QuarantineOutcome::Moved { custody }
        | QuarantineOutcome::SourceReplaced { custody, .. }
        | QuarantineOutcome::DurabilityUncertain { custody, .. }
        | QuarantineOutcome::PartialCustody { custody, .. } => custody,
        other => panic!("expected retained custody, got {other:?}"),
    }
}

fn recovery_path(outcome: &QuarantineOutcome) -> &Path {
    custody(outcome)
        .source_paths()
        .next()
        .expect("at least one path must be verified as retained source")
}

#[test]
fn sibling_quarantine_moves_the_retained_inode_without_clobbering_prior_recovery() {
    let scratch = Scratch::new("ordinary");
    let source = scratch.join("session.json");
    let prior = scratch.join("session.json.recovered");
    fs::write(&source, ORIGINAL).expect("write source");
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).expect("set source mode");
    fs::write(&prior, PRIOR).expect("write prior recovery");
    let source_metadata = fs::metadata(&source).expect("source metadata");

    let outcome = quarantine_sibling(&source);
    let recovery = recovery_path(&outcome);
    let recovery_metadata = fs::metadata(recovery).expect("recovery metadata");

    assert!(matches!(&outcome, QuarantineOutcome::Moved { .. }));
    assert_eq!(custody(&outcome).source_paths().count(), 2);
    assert_eq!(source_metadata.ino(), recovery_metadata.ino());
    assert_eq!(source_metadata.uid(), recovery_metadata.uid());
    assert_eq!(source_metadata.mode(), recovery_metadata.mode());
    assert_eq!(fs::read(recovery).expect("read recovery"), ORIGINAL);
    assert_eq!(fs::read(prior).expect("read prior recovery"), PRIOR);
    assert!(!source.exists());
}

#[test]
fn sibling_quarantine_expected_refuses_substitution_before_call() {
    let scratch = Scratch::new("expected-before-call");
    let source = scratch.join("music-state.json");
    let original = scratch.join("original-state");
    fs::write(&source, ORIGINAL).expect("write source");
    let expected = expected_source(&source);
    fs::rename(&source, &original).expect("retain original source");
    fs::write(&source, FOREIGN).expect("write foreign source");

    let outcome = quarantine_sibling_expected(&source, expected);

    match outcome {
        QuarantineOutcome::SourceMismatch {
            expected: returned,
            observed: Some(observed),
            ..
        } => {
            assert_eq!(returned, expected);
            assert_ne!(observed, expected);
        }
        other => panic!("expected source mismatch, got {other:?}"),
    }
    assert_eq!(fs::read(&original).expect("read original"), ORIGINAL);
    assert_eq!(fs::read(&source).expect("read foreign"), FOREIGN);
    assert!(generated_names(&scratch.path, "music-state.json").is_empty());
}

#[test]
fn sibling_quarantine_expected_refuses_same_length_foreign_inode() {
    let scratch = Scratch::new("expected-same-length");
    let source = scratch.join("calendar-state.json");
    let original = scratch.join("original-state");
    fs::write(&source, ORIGINAL).expect("write source");
    let expected = expected_source(&source);
    fs::rename(&source, &original).expect("retain original source");
    let foreign = vec![b'F'; ORIGINAL.len()];
    fs::write(&source, &foreign).expect("write same-length foreign source");

    let outcome = quarantine_sibling_expected(&source, expected);

    assert!(matches!(
        outcome,
        QuarantineOutcome::SourceMismatch {
            observed: Some(_),
            ..
        }
    ));
    assert_eq!(fs::read(&original).expect("read original"), ORIGINAL);
    assert_eq!(fs::read(&source).expect("read foreign"), foreign);
    assert!(generated_names(&scratch.path, "calendar-state.json").is_empty());
}
