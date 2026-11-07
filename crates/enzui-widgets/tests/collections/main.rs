//! Integration tests for keyed collections, reconciliation and retained content.
//!
//! Run a gate with
//! `cargo wt --test collections <filter>`.
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

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;

mod keyed_grid_reorder_identity;
mod keyed_identity;

#[path = "../common/label_clicks.rs"]
pub mod label_clicks;

mod reorder_handles;
mod restore_focus_shared;
#[path = "../common/scratch.rs"]
pub mod scratch;
#[path = "../common/widget_population.rs"]
pub mod widget_population;
