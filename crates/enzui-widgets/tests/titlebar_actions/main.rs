//! Integration tests for action bars, titlebar actions, and header bars.
//!
//! Run a gate with `cargo wt --test titlebar_actions
//! <filter>`. Use the named Cargo target directly. Add a test by placing its file here and registering one `mod`
//! below; the `contracts` registration gate rejects missing and duplicate
//! modules.

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
mod header_bar_keyed_actions;
mod header_bar_narrow_actions;
#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
#[path = "../common/scratch.rs"]
pub mod scratch;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
