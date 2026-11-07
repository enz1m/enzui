use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const OS_SERVICE: &str = r#"{
  "schema": 1,
  "executable": "/usr/lib/enzimos-background/enzimos-background-service",
  "bus_name": "org.enzimos.Background1",
  "unit": "enzimos-background-apps.service",
  "witness": {
    "bus": "org.enzimos.System1",
    "path": "/org/enzimos/System1",
    "interface": "org.enzimos.System1",
    "method": "InspectBackgroundBroker",
    "schema": 1
  }
}"#;

const NAME: &str = "org.enzimos.Background1.json";

/// A view in which files this test creates count as owned outside the
/// namespace, the way root-owned files look from inside one. In the real
/// view a user-owned file never qualifies (see the control below).
fn outside_owner_view() -> OwnerView {
    let me = rustix::process::getuid().as_raw();
    let own = me.wrapping_add(1);
    OwnerView::from_map(own, me, &format!("{own} {own} 1"))
}

/// A fresh private directory beside the test binary, whose ancestors are
/// the build tree, the home directory and root-owned system directories.
fn scratch(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let base = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let dir = base.join(format!(
        "lifecycle-callers-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn write(dir: &Path, name: &str, body: &str, mode: u32) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn unsafe_descriptor_files_are_skipped() {
    let view = outside_owner_view();
    let dir = scratch("files");
    let writable = write(&dir, NAME, OS_SERVICE, 0o664);
    assert_eq!(installed(view, &dir), Some(vec![]), "group writable");
    std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(installed(view, &dir).map(|p| p.len()), Some(1), "control");

    std::fs::hard_link(&writable, dir.join("second-link")).unwrap();
    assert_eq!(installed(view, &dir), Some(vec![]), "two links");
    std::fs::remove_file(dir.join("second-link")).unwrap();

    std::fs::rename(&writable, dir.join("target")).unwrap();
    std::os::unix::fs::symlink(dir.join("target"), dir.join(NAME)).unwrap();
    assert_eq!(installed(view, &dir), Some(vec![]), "symlink");
    std::fs::remove_file(dir.join(NAME)).unwrap();

    write(&dir, NAME, "", 0o644);
    assert_eq!(installed(view, &dir), Some(vec![]), "empty");
    let padded = format!("{OS_SERVICE}{}", " ".repeat(4096));
    write(&dir, NAME, &padded, 0o644);
    assert_eq!(installed(view, &dir), Some(vec![]), "over 4096 bytes");
    std::fs::remove_dir_all(&dir).unwrap();
}
