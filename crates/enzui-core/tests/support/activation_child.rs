use enzui_test_support::test_scratch::ScratchDir;
use std::process::Child;
use std::time::{Duration, Instant};

pub(super) struct ReapedChild(pub(super) Child);

impl Drop for ReapedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                result => panic!("activation child cleanup did not settle: {result:?}"),
            }
        }
    }
}

pub(super) fn child_activation_environment(
    context: enzui_core::activation::ActivationContext,
) -> String {
    let root = ScratchDir::new_restricted("activation-child");
    let output = root.join("environment");
    let child = enzui_core::activation::ChildLaunch::new("/bin/sh")
        .args([
            "-c",
            "printf '%s\\n%s\\n' \"${DESKTOP_STARTUP_ID-unset}\" \"${XDG_ACTIVATION_TOKEN-unset}\" >\"$1\"",
            "activation-child",
        ])
        .arg(&output)
        .activation(context)
        .spawn(Instant::now())
        .unwrap();
    let mut child = ReapedChild(child);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            return std::fs::read_to_string(output).unwrap();
        }
        assert!(Instant::now() < deadline, "activation child did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
}
