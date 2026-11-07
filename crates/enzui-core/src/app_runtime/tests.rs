#[cfg(all(feature = "accesskit", target_os = "linux"))]
mod enabled {

    use crate::app_runtime::enabled::*;

    #[test]
    fn either_platform_half_counts_as_an_existing_owner() {
        assert!(!platform_registration_exists(false, false, false));
        assert!(
            platform_registration_exists(false, true, false),
            "a foreign request handler alone must count: our own handler \
                 would be silently discarded while it kept routing actions",
        );
        assert!(platform_registration_exists(true, false, false));
        assert!(
            platform_registration_exists(false, false, true),
            "a foreign window-closed callback alone must count: it would \
                 keep releasing another owner's per-window state",
        );
        assert!(platform_registration_exists(true, true, true));
    }
}
