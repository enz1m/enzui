//! integration-target consolidation: one binary for the a11y_* integration tests.
//!
//! Each `tests/a11y/<x>.rs` module is declared here.
//! Cargo links this file once instead of linking 15 separate binaries against
//! the whole 254k-line widgets crate. The test sources are unchanged.

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

mod action_routing;
mod app_setup_dispatch;
mod atspi_adapter_registry;
mod capability_matrix;
mod framework_control_ids;

mod no_widget_lost_set_value;

mod routed_action_applier;
mod text_runs;
