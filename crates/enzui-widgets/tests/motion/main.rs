//! Integration tests for motion effects, the frame clock and self-animating
//! widgets.
//!
//! Run a gate with `cargo wt --test motion <filter>`.
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

mod frame_clock_offscreen;

mod frame_route_reentry;
mod frame_routing_wrappers;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;

mod motion_fixture;

#[path = "../common/scratch.rs"]
pub mod scratch;

mod settled_widgets_leave_frame_routes;

mod sidebar_route_availability;
mod sidebar_tree_scroll_retention;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
