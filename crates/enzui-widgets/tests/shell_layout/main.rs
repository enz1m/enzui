//! Integration tests for the app-shell layout: the split geometry host and
//! its fragments; the responsive, mobile and RTL arrangements; content width,
//! slot and band-stack geometry; the drag region and resize edges; the first
//! frame, flat page, build-count and layout-cache probes; the activity surface;
//! and the redraw focus-probe skip and seam-travel diagnostics.
//!
//! Run a gate with `cargo wt --test shell_layout
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
// Shared harness modules are path-included in each target. Only `corner_probe`
// is consumed by the geometry/layout gates retained here; the other helpers'
// own dead-code allowances apply.

mod app_shell_content_identity;
mod app_shell_geometry;
#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
#[path = "../common/scratch.rs"]
pub mod scratch;
#[path = "../common/widget_population.rs"]
pub mod widget_population;
