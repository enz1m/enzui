#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::app_data::AppId;
use enzui_core::portal::notification::{
    DesktopNotification, DesktopNotificationClient, NotificationId, NotificationValidationError,
};

#[cfg(feature = "portal-dbus")]
#[path = "desktop_notification_portal/activation.rs"]
mod activation;

#[cfg(feature = "portal-dbus")]
#[path = "desktop_notification_portal/replies.rs"]
mod replies;

#[cfg(feature = "portal-dbus")]
#[path = "desktop_notification_portal/exported_policy.rs"]
mod exported_policy;

#[cfg(all(feature = "portal-dbus", target_os = "linux"))]
#[path = "desktop_notification_portal/icons.rs"]
mod icons;

#[cfg(feature = "portal-dbus")]
#[path = "support/private_bus.rs"]
mod private_bus;

#[cfg(feature = "portal-dbus")]
#[path = "support/activation_child.rs"]
mod activation_child;

#[cfg(all(
    feature = "portal-dbus",
    feature = "application-dbus",
    target_os = "linux"
))]
#[path = "desktop_notification_portal/exported_actions.rs"]
mod exported_actions;

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
struct BusStartupFailure {
    child_pid: u32,
    error: BusStartupError,
}

#[allow(
    dead_code,
    reason = "the payload is read only through `Debug`, in the failure message"
)]
#[derive(Debug)]
enum BusStartupError {
    SocketTimeout,
    ChildExited(std::process::ExitStatus),
}

#[test]
fn invalid_payload_is_rejected_before_private_dbus_submission() {
    let backend = enzui_core::portal::notification::UnavailableNotificationBackend;
    let mut client =
        DesktopNotificationClient::new(AppId::new("org.enzui.Invalid").unwrap(), backend);
    let error = client
        .send(DesktopNotification::new(
            NotificationId::new("invalid").unwrap(),
            "",
        ))
        .unwrap_err();
    assert!(matches!(
        error,
        enzui_core::portal::notification::NotificationError::InvalidPayload(
            NotificationValidationError::InvalidTitle
        )
    ));
}
