//! Exercises [`export_toplevel`] against an in-process `wayland-server`
//! xdg-foreign compositor.
//!
//! The compositor is a protocol fixture wired to clients over a `socketpair(2)`,
//! not a real desktop portal and not an iced-owned window. These tests
//! establish that the export state machine speaks the xdg-foreign wire
//! protocol, owns and tears down its lease, resolves a token across two client
//! connections, and stays bounded under a stalled or vanished compositor. They
//! do not establish the modal behavior of any installed portal, nor iced
//! runtime ownership.

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
#![cfg(target_os = "linux")]

use enzui_wayland::owned_window_from_payload;
use std::any::Any;

#[test]
fn bridge_rejects_absent_or_foreign_payload() {
    assert!(owned_window_from_payload(None).is_none());
    let wrong: Option<Box<dyn Any + Send>> = Some(Box::new(42u64));
    assert!(
        owned_window_from_payload(wrong).is_none(),
        "a payload of the wrong type must not be misread as a window"
    );
}
