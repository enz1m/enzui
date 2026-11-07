//! Source, export, documentation and test-infrastructure contracts for the
//! widget kit.
//!
//! This target owns the product contracts that are decided by reading the
//! crate rather than by rendering it: module boundaries, the public export
//! surface and its naming grammar, the generated catalog/token documentation,
//! and the generated catalog.
//! Contracts that only a built widget tree can settle — geometry, paint,
//! motion and rendered semantics — belong to `--test rendering`.
//!
//! Run a gate with `cargo wt --test contracts <filter>`.
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
// Shared harness modules are path-included in each target.

mod icon_name_resolution;
#[path = "../common/scratch.rs"]
pub mod scratch;

mod tiers;
mod token_index;
#[path = "../fixtures/widget_catalog.rs"]
mod widget_catalog_examples;
mod widget_catalog_page;

mod widget_catalog_page_contracts;
mod widget_catalog_page_discovery;
mod widget_catalog_page_staleness;
mod widgets_do_not_depend_on_resources;
