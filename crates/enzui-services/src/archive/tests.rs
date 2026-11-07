use super::*;
use enzui_test_support::test_scratch::ScratchDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/archive")
        .join(name)
}

/// `name` copied into a fresh scratch folder, with an empty `out` beside it.
struct Case {
    scratch: ScratchDir,
    archive: PathBuf,
}

impl Case {
    fn new(names: &[&str]) -> Self {
        let scratch = ScratchDir::new("archive");
        for name in names {
            let file = Path::new(name).file_name().unwrap();
            fs::copy(fixture(name), scratch.path().join(file)).unwrap();
        }
        fs::create_dir(scratch.path().join("out")).unwrap();
        let archive = scratch
            .path()
            .join(Path::new(names[0]).file_name().unwrap());
        Self { scratch, archive }
    }

    fn out(&self) -> PathBuf {
        self.scratch.path().join("out")
    }

    fn request(&self) -> ExtractRequest {
        ExtractRequest::new(&self.archive, self.out())
    }

    /// Everything in `out`, hidden staging folders included.
    fn listing(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.out())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn require_libarchive() {
    assert!(
        capabilities().library().is_some(),
        "these tests need the system libarchive"
    );
}

#[test]
fn cancellation_at_any_point_removes_the_staging_folder() {
    require_libarchive();
    let case = Case::new(&["safe.tar.bz2"]);
    for after in [0, 1, 3, 6] {
        let calls = std::cell::Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() > after
        };
        assert!(
            matches!(
                extract(&case.request(), &ExtractProgress::default(), &cancelled),
                Err(ArchiveError::Cancelled)
            ),
            "after {after}"
        );
        assert!(case.listing().is_empty(), "after {after}");
    }
}

/// Renames the staging folder away the first time it is seen and plants a
/// foreign folder, holding `keep`, under its name.
fn swap_staging(out: &Path) -> bool {
    let Some(stage) = fs::read_dir(out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .find(|name| name.to_string_lossy().contains(".extracting-"))
    else {
        return false;
    };
    fs::rename(out.join(&stage), out.join("moved")).unwrap();
    fs::create_dir(out.join(&stage)).unwrap();
    fs::write(out.join(&stage).join("keep"), b"foreign").unwrap();
    true
}

#[test]
fn cleanup_leaves_a_folder_planted_at_the_staging_name() {
    require_libarchive();
    let case = Case::new(&["safe.tar.bz2"]);
    let out = case.out();
    let swapped = std::cell::Cell::new(false);
    let cancelled = || {
        if swapped.get() {
            return true;
        }
        swapped.set(swap_staging(&out));
        false
    };
    assert!(matches!(
        extract(&case.request(), &ExtractProgress::default(), &cancelled),
        Err(ArchiveError::Cancelled)
    ));
    assert!(swapped.get(), "the stage was swapped mid-extraction");
    let planted = case
        .listing()
        .into_iter()
        .find(|name| name.contains(".extracting-"))
        .expect("the planted folder is still there");
    assert_eq!(
        fs::read(out.join(planted).join("keep")).unwrap(),
        b"foreign",
        "cleanup must not remove what another process put at the old name"
    );
}

#[test]
fn publication_refuses_a_folder_planted_at_the_staging_name() {
    require_libarchive();
    let case = Case::new(&["safe.tar.bz2"]);
    let out = case.out();
    let swapped = std::cell::Cell::new(false);
    let cancelled = || {
        if !swapped.get() {
            swapped.set(swap_staging(&out));
        }
        false
    };
    assert!(
        extract(&case.request(), &ExtractProgress::default(), &cancelled).is_err(),
        "the planted folder is not published as the archive's output"
    );
    assert!(swapped.get());
    assert!(
        !case.listing().iter().any(|name| name.starts_with("safe")),
        "nothing published under the archive's name: {:?}",
        case.listing()
    );
}
