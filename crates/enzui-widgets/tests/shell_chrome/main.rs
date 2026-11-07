//! Integration tests for the app-shell chrome: window controls and their
//! activation/hover/tint policy, the window title's theming, tone and
//! typography, the sidebar toggle's ownership, ink, seam travel and opt-out
//! census, the shell's numeric/raw/struct token guards, its titlebar tone, and
//! its decorations/anchor and window-resize frame.
//!
//! Run a gate with `cargo wt --test shell_chrome
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
// Shared harness modules are path-included in each target. No gate in this
// suite consumes one yet, so each helper's own dead-code allowance applies.

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;

mod desktop_decorations;

#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;

#[path = "../common/scratch.rs"]
pub mod scratch;

mod titlebar_ownership;
#[path = "../common/widget_population.rs"]
pub mod widget_population;
mod window_controls_policy;
mod window_resize_frame;
