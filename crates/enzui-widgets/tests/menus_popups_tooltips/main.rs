//! Integration tests for menus, anchored popups, and tooltips.
//!
//! Run a gate with `cargo wt --test menus_popups_tooltips <filter>`.
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

mod combo_box_popup;
mod combo_box_search;

mod context_menu_host_interaction;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;
mod fade_tooltip_cancel_interaction;
mod menu_bar;

mod menu_button_submenu;

mod popover;

mod typed_context_menu;
mod wheel_containment;
