//! `$XDG_DATA_HOME` — the durable app-data directory capability.

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

use enzui_core::app_data::{AppDataError, AppDataResolver, AppId};

fn app() -> AppId {
    AppId::new("enzui-remote").expect("valid app id")
}

#[test]
fn relative_roots_on_both_rungs_refuse_and_say_why() {
    let r = AppDataResolver::new(Some("relative/data"), Some("also/relative"));
    let err = r.resolve(&app()).expect_err("must refuse");
    assert_eq!(
        err,
        AppDataError::NoDataHome {
            data_home_was_relative: true,
            home_was_relative: true,
        }
    );
    let msg = err.to_string();
    assert!(
        msg.contains("relative"),
        "the refusal must name the relative root it ignored, got {msg}"
    );
}
