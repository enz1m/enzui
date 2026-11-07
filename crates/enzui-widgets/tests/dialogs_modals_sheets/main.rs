//! Integration tests for dialogs, modal hosts, and sheets.
//!
//! Run a gate with `cargo wt --test dialogs_modals_sheets <filter>`.
//! Use the named Cargo target directly.
//! Register test modules below; the workspace source-reachability check catches
//! files that are not compiled by any target.

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

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

mod dialog_host_keeps_page_state;
mod dialog_stack;
mod form_dialog_enter_rules;

mod modal_focus_lifecycle;

mod settings_dialog_page_scroll;

mod sheet_gesture_cancel_interaction;
