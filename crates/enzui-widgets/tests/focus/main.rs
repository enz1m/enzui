//! One binary for the focus integration tests.
//!
//! Each `tests/focus/<x>.rs` is a module declared here, so Cargo links one
//! binary against the widgets crate instead of one per file.

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

mod exclusivity;
mod overlay_press_focus;
mod ring_modality_additional_widgets;
