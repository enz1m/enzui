//! Rendered-tree contracts for the whole widget population.
//!
//! This target owns the product contracts that only a built widget tree can
//! settle: layout geometry and spill, clipping, paint and interaction-state
//! ink, hit targets and focus rings, measured control heights, motion latching
//! and reduced-motion behaviour, and the semantics a rendered node publishes.
//! Contracts decided by reading source, exports or generated documentation
//! belong to `--test contracts`.
//!
//! Run a gate with `cargo wt --test rendering <filter>`.
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

mod animated_replay;
mod clipped_mesh_indices;
#[path = "../common/corner_probe.rs"]
pub mod corner_probe;
mod custom_track_replay;
mod glass_first_frames;
mod hit_target_sweep;
#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
mod idle_frames;
mod keyed_moves_replay;
mod layout_spill_widget_sweep;
mod leaf_duplicate_label_sweep;
mod live_chart_replay;
mod partial_image_cache;
mod progress_sweep_replay;
mod reveal_replay;
mod rounded_clip_widget_sweep;
mod shader_layout;
mod skeleton_replay;
mod software_damage_replay;
mod spinner_replay;
mod svg_translucency;
#[path = "../common/widget_population.rs"]
pub mod widget_population;
