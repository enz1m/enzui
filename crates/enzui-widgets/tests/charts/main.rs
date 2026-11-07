//! Integration tests for the `charts` widget area: charts, gauges, grids,
//! calendars, schedules, annotations, and swatch/token galleries.
//!
//! Run a gate with `cargo wt --test charts <filter>`.
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

mod bar_chart_keyboard;

mod cartesian_host;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;

#[path = "../common/scratch.rs"]
pub mod scratch;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
