//! Integration tests for text editing, fields, and text grids.
//!
//! Run a gate with
//! `cargo wt --test text_editing <filter>`.
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

mod bidi_caret;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

mod edit_menu;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;

mod primary_selection;
mod replace_batch_history;
#[path = "../common/scratch.rs"]
pub mod scratch;

mod spell_check_menu;

mod text_area_host_hooks;
mod text_area_paste_hook;
mod text_field_interaction;
mod text_field_paste;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
