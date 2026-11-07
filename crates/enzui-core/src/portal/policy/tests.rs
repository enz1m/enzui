use super::*;

#[test]
fn rejects_device_all() {
    let m = FlatpakManifest::from_finish_args(["--device=all"]);
    assert_eq!(
        review(&m),
        vec![PolicyViolation::DeviceAll("--device=all".into())]
    );
}
