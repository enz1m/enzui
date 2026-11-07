//! Integration tests for tab surfaces: document tabs (rows, pinned grids,
//! keyed identity, hit testing, context menus) and the view switchers —
//! inline, routed and scrollable — that publish tab lists.
//!
//! Run a gate with `cargo wt --test tabs <filter>`.
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
// The pinned software renderer is consumed from two slices
// (`tabs::switcher_engine_parity` and the three `input` tab-pill render gates),
// so it lives in `tests/common/` and is declared by exactly the
// targets that render pixels through it, verbatim from main. Its own
// `#[test]` asserts a property of this test process (`ICED_TEST_BACKEND` and
// the resolved backend name), so it is correct for it to run once per binary
// that renders with it rather than once for the whole crate.

mod document_tabs_keyed_identity;

mod responsive_focus_handoff;

mod view_switcher_routed_tab_activation;

#[path = "../common/corner_probe.rs"]
pub mod corner_probe;
#[path = "../common/honest_frame_driver.rs"]
pub mod honest_frame_driver;
#[path = "../common/pinned_test_renderer.rs"]
pub mod pinned_test_renderer;
#[path = "../common/scratch.rs"]
pub mod scratch;

#[path = "../common/widget_population.rs"]
pub mod widget_population;
