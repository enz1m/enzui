//! A lease crossing the task/state boundary is delivered exactly once, even if
//! the carrying message is cloned or delivered twice.

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

use enzui_parent_lease::Handoff;

#[test]
fn the_value_is_delivered_to_exactly_one_taker() {
    let handoff = Handoff::new(String::from("exported-token"));
    let duplicate = handoff.clone();

    assert!(handoff.is_pending());
    assert!(duplicate.is_pending());

    assert_eq!(handoff.take().as_deref(), Some("exported-token"));
    assert!(
        !duplicate.is_pending(),
        "a clone shares the slot, so the lease cannot be installed twice"
    );
    assert!(duplicate.take().is_none());
    assert!(handoff.take().is_none());
}
