//! Integration tests for the app-shell surfaces: the drawer's header,
//! state and teardown; the sidebar's landmark, motion and titlebar drop; the
//! secondary-leading slot, subtitlebar and footer scrims; the shell gallery,
//! overlay and editor ink; focus traversal, retention and text-field focus;
//! the chained-operation and embedded-surface proofs; and the multi-window,
//! scratch-root and shell-window-state auxiliaries.
//!
//! Run a gate with `cargo wt --test shell_surfaces
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
// Shared harness modules are path-included in each target. `corner_probe` and
// `scratch` are consumed here; the `honest_frame_driver` and
// `widget_population` modules are not, so their own dead-code allowances apply.

mod shell_window_state;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;
#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
#[path = "../common/scratch.rs"]
pub mod scratch;
#[path = "../common/widget_population.rs"]
pub mod widget_population;
