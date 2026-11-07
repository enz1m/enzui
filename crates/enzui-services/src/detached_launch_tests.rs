use super::*;
use enzui_test_support::test_scratch::ScratchDir;

const SH: &str = "/bin/sh";

#[test]
fn a_relative_program_or_a_missing_folder_is_refused_before_spawn() {
    let scratch = ScratchDir::new("detached-refuse");
    assert_eq!(
        launch(&DetachedLaunch::new("sh", scratch.path())),
        Err(LaunchError::NotAbsolute)
    );
    assert_eq!(
        launch(&DetachedLaunch::new(SH, scratch.path().join("gone"))),
        Err(LaunchError::WorkingDirectory)
    );
    assert!(matches!(
        launch(&DetachedLaunch::new(
            scratch.path().join("missing-program"),
            scratch.path()
        )),
        Err(LaunchError::Spawn(_))
    ));
}
