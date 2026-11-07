use super::*;
use enzui_test_support::test_scratch::ScratchDir;
use std::fs;
use std::os::unix::fs::symlink;

fn scratch(tag: &str) -> ScratchDir {
    ScratchDir::new(&format!("user-file-{tag}"))
}

#[test]
fn intermediate_directory_symlinks_are_refused() {
    let dir = scratch("intermediate-symlink");
    let real = dir.path().join("real");
    let linked = dir.path().join("linked");
    fs::create_dir(&real).expect("real directory");
    fs::write(real.join("document"), b"old").expect("seed");
    symlink(&real, &linked).expect("directory symlink");

    assert!(
        save(
            &linked.join("document"),
            SavePolicy::ReplaceExisting,
            b"new"
        )
        .is_err()
    );
    assert_eq!(fs::read(real.join("document")).expect("old"), b"old");
}
