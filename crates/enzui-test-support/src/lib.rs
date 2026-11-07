//! Test-only source-tree audit support for enzUI applications.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::string_slice,
    clippy::unwrap_in_result,
    reason = "test tooling: it reports by panicking, and slices the repository's own \
              sources at offsets found by searching the same text"
)]

pub mod adoption;
pub mod chained_operation;
pub mod doc_links;
pub mod fake_executable;
pub mod hand_roll_scan;
/// The text of [`hand_roll_scan`]'s own source file, packaged with this
/// crate so a governance tool can compare embedded parser copies against it
/// without reading a sibling checkout.
pub const HAND_ROLL_SCAN_SOURCE: &str = include_str!("hand_roll_scan.rs");
#[cfg(feature = "rust-syntax")]
pub mod rust_syntax;
pub mod sabotage_manifest;
pub mod strict_fs;
pub mod test_target_inventory;
pub mod titlebar_adoption;

pub mod test_scratch;
