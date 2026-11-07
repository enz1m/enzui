//! Integration tests for media and content presentation widgets: avatars,
//! block and chat transcripts, icon sources, images and the image viewer,
//! media transport controls, and preview rows.
//!
//! Run a gate with `cargo wt --test media_content
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

mod animated_image;
mod animated_svg;

mod code_block_selection;

mod image_viewer_interaction;

mod media_controls_transport;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;
#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;

#[path = "../common/scratch.rs"]
pub mod scratch;
#[path = "../common/widget_population.rs"]
pub mod widget_population;
