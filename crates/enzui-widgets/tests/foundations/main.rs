//! Integration tests for the `foundations` widget area.
//!
//! Run a gate with `cargo wt --test foundations <filter>`.
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

mod app_setup_localizer;
mod builder_option_reads_reach_paint;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
mod ink_for_floor;

mod localization;
mod rtl_mirroring;
#[path = "../common/scratch.rs"]
pub mod scratch;
mod testing_guide;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
