use super::*;
use enzui_test_support::test_scratch::ScratchDir;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn scratch(tag: &str) -> ScratchDir {
    ScratchDir::new(&format!("state-authority-{tag}"))
}

fn seed(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("seed");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("owner-only seed");
}

#[test]
fn a_broadened_state_mode_is_refused_at_open() {
    let dir = scratch("open-mode");
    for mode in [0o640, 0o604, 0o666, 0o2600] {
        let path = dir.path().join(format!("session-{mode:o}"));
        seed(&path, b"old");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("broadened seed");
        assert_eq!(
            fs::metadata(&path).expect("seed mode").mode() & 0o7777,
            mode,
            "the fixture must carry the mode under test"
        );
        let error = StateAuthority::open_existing(&path).expect_err("broadened mode");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(StateAuthority::open_or_create(&path).is_err());
        assert_eq!(fs::read(&path).expect("untouched"), b"old");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("owner-only");
        let mut authority = StateAuthority::open_existing(&path).expect("owner-only opens");
        authority.publish(b"new").expect("publish");
        assert_eq!(fs::read(&path).expect("published"), b"new");
    }
}

// ---------------------------------------------------------------------------
// read_bounded tests
// ---------------------------------------------------------------------------
