//! Integration tests for editable fields and suggestions.
//!
//! Run a gate with `cargo wt --test fields <filter>`.
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
// Shared harness modules are path-included in each target. Only
// `widget_population` needs an outer dead-code allowance.

mod autocomplete_field;

mod chip_input;
#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

mod form_choice_end_to_end;
#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
mod input_otp_editing;

#[path = "../common/label_clicks.rs"]
pub mod label_clicks;

mod quiet_inline_value_editor;

#[path = "../common/scratch.rs"]
pub mod scratch;
mod search_field_composition;
mod search_field_focus_retention;

mod time_field_editing;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
