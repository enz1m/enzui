use super::*;

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        // Scratch stays beside the test binary under `target/`, never in
        // the world-writable system temp base.
        let mut p = std::env::current_exe().expect("current test executable");
        p.pop();
        p.push(format!(
            "enzui-tempstage-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).expect("scratch dir");
        Self(p)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Strict directory-entry collection: discovery failures must fail closed.

fn directory_stage_parent(tag: &str) -> (TmpDir, crate::secure_dir::SecureRoot) {
    let dir = TmpDir::new(tag);
    let root = crate::secure_dir::SecureRoot::open(&dir.0).expect("open stage parent");
    (dir, root)
}

#[test]
fn directory_stage_drop_removes_an_ordinary_owned_tree() {
    let (dir, parent) = directory_stage_parent("directory-tree");
    let stage = DirectoryStage::create(&parent, OsStr::new("stage")).expect("create stage");
    let stage_path = stage.path().to_path_buf();
    std::fs::create_dir(stage_path.join("nested")).expect("create owned descendant");
    std::fs::write(stage_path.join("nested/payload"), b"owned").expect("write owned file");
    std::fs::write(stage_path.join("top-level"), b"owned").expect("write owned file");
    let unrelated = dir.join("unrelated");
    std::fs::write(&unrelated, b"keep").expect("write parent control");

    drop(stage);

    assert!(
        !stage_path.exists(),
        "the complete owned tree must be removed"
    );
    assert_eq!(
        std::fs::read(unrelated).expect("read parent control"),
        b"keep"
    );
}

#[test]
fn directory_stage_drop_retains_a_root_after_private_mode_drift() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, parent) = directory_stage_parent("directory-mode-drift");
    let stage = DirectoryStage::create(&parent, OsStr::new("stage")).expect("create stage");
    let stage_path = stage.path().to_path_buf();
    let marker = stage_path.join("foreign-marker");
    std::fs::write(&marker, b"foreign").expect("seed marker");
    std::fs::set_permissions(&stage_path, std::fs::Permissions::from_mode(0o777))
        .expect("make stage root unsafe");

    drop(stage);

    assert_eq!(std::fs::read(&marker).expect("marker survives"), b"foreign");
    std::fs::set_permissions(&stage_path, std::fs::Permissions::from_mode(0o700))
        .expect("restore stage mode");
    std::fs::remove_dir_all(&stage_path).expect("remove retained stage fixture");
}

fn assert_directory_stage_preserves_root_replacement(nonempty: bool) {
    use std::os::unix::fs::MetadataExt;
    use std::sync::mpsc;
    use std::time::Duration;

    let (dir, parent) = directory_stage_parent(if nonempty {
        "root-replacement-nonempty"
    } else {
        "root-replacement-empty"
    });
    let stage = DirectoryStage::create(&parent, OsStr::new("stage")).expect("create stage");
    let stage_path = stage.path().to_path_buf();
    let original_identity = stage.root().identity();
    let displaced = dir.join("displaced-owned-stage");
    let foreign_marker = stage_path.join("foreign/marker");
    let (start_tx, start_rx) = mpsc::sync_channel(0);
    let (done_tx, done_rx) = mpsc::sync_channel(0);
    let actor_target = stage_path.clone();
    let actor = std::thread::spawn(move || {
        start_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("Drop did not reach the root substitution seam");
        std::fs::rename(&actor_target, &displaced).expect("displace owned stage");
        std::fs::create_dir(&actor_target).expect("create foreign replacement");
        if nonempty {
            std::fs::create_dir(actor_target.join("foreign")).expect("create foreign descendant");
            std::fs::write(actor_target.join("foreign/marker"), b"foreign")
                .expect("write foreign descendant");
        }
        done_tx.send(()).expect("report completed substitution");
    });
    fault::arm_directory_cleanup(
        stage_path.clone(),
        Box::new(move |_| {
            start_tx.send(()).expect("wake substitution actor");
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("substitution actor did not finish");
        }),
    );

    drop(stage);
    actor.join().expect("substitution actor");
    assert!(!fault::is_armed(), "the Drop race seam was not consumed");
    let replacement = std::fs::symlink_metadata(&stage_path).expect("replacement survived");
    assert_ne!(
        (replacement.dev(), replacement.ino()),
        original_identity,
        "the foreign replacement must be distinguishable from the retained identity"
    );
    if nonempty {
        assert_eq!(
            std::fs::read(foreign_marker).expect("foreign descendant survived"),
            b"foreign"
        );
    }
}

#[test]
fn directory_stage_drop_preserves_empty_root_replacement() {
    assert_directory_stage_preserves_root_replacement(false);
}

// strict_entry_names: fail-closed seam and positive-control tests.

// ==== Root-descriptor staging (the HTTP cache's route) ===================
