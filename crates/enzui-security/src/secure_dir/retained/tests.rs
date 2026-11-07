use super::*;
use crate::test_scratch::ScratchDir;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
    scratch: ScratchDir,
    source: RetainedDir,
    destination: RetainedDir,
}

impl Fixture {
    fn new() -> Self {
        let scratch = ScratchDir::new("retained-directory");
        let parent = RetainedDir::open(&scratch).unwrap();
        let source = parent.create_subroot(OsStr::new("source")).unwrap();
        let destination = parent.create_subroot(OsStr::new("destination")).unwrap();
        Self {
            scratch,
            source,
            destination,
        }
    }

    fn file(&self) -> RetainedEntry {
        std::fs::write(self.source.path().join("entry"), b"owned source bytes").unwrap();
        self.source.retain_entry(OsStr::new("entry")).unwrap()
    }
}

#[test]
fn ordinary_adoption_preserves_writable_mode_while_private_adoption_refuses() {
    let scratch = ScratchDir::new("ordinary-mode");
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o777)).unwrap();
    let ordinary = RetainedDir::open(&scratch).unwrap();
    assert_eq!(
        ordinary.identity(),
        open_file_identity(&File::open(&scratch).unwrap()).unwrap()
    );
    assert!(SecureRoot::open(&scratch).is_err());
    assert_eq!(
        std::fs::metadata(&scratch).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn root_open_refuses_a_symlink_and_regular_file() {
    let fixture = Fixture::new();
    let link = fixture.scratch.join("link");
    symlink(fixture.source.path(), &link).unwrap();
    assert!(RetainedDir::open(&link).is_err());
    let entry = fixture.file();
    assert!(RetainedDir::open(&entry.parent().path().join(entry.name())).is_err());
    assert!(RetainedDir::open(fixture.source.path()).is_ok());
}

#[test]
fn subroots_reject_traversal_nul_symlinks_and_occupied_names() {
    let fixture = Fixture::new();
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "nested/escape",
        "/escape",
        "nul\0name",
    ] {
        for result in [
            fixture.source.open_subroot(OsStr::new(name)),
            fixture.source.create_subroot(OsStr::new(name)),
            fixture.source.ensure_subroot(OsStr::new(name)),
        ] {
            assert_eq!(
                result.unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{name:?}"
            );
        }
    }
    symlink(
        fixture.destination.path(),
        fixture.source.path().join("link"),
    )
    .unwrap();
    assert!(fixture.source.ensure_subroot(OsStr::new("link")).is_err());
    let child = fixture.source.create_subroot(OsStr::new("child")).unwrap();
    assert_eq!(
        fixture
            .source
            .create_subroot(OsStr::new("child"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    std::fs::set_permissions(child.path(), std::fs::Permissions::from_mode(0o775)).unwrap();
    assert_eq!(
        fixture
            .source
            .ensure_subroot(OsStr::new("child"))
            .unwrap()
            .identity(),
        child.identity()
    );
    assert_eq!(
        std::fs::metadata(child.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o775
    );
    assert_eq!(
        std::fs::read_dir(fixture.destination.path())
            .unwrap()
            .count(),
        0
    );
}
