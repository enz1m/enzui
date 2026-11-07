//! One async runtime for the whole process.
//!
//! Three shipped features meet in every runnable target of this workspace:
//! `a11y` links `accesskit_unix`, whose `tokio` selection compiles the single
//! `zbus` in the graph with its Tokio executor; `linux-theme-detection` drives
//! `mundy`'s preference stream, which reaches that zbus; and the facade's
//! executor feature decides which reactor those futures are polled inside.
//! An executor that is not Tokio kills the binary before its first frame with
//! "there is no reactor running, must be called from the context of a Tokio
//! 1.x runtime" — a startup crash, not a degraded feature.
//!
//! Applications carry `iced/tokio` for it. This file is the gate for the
//! framework's own targets, the examples, which share the workspace facade
//! declaration rather than an application's.

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

use iced::executor::{Default as ShippedExecutor, Executor};

#[test]
fn the_shipped_executor_enters_a_tokio_reactor_for_the_preference_stream() {
    // The control, in the other direction: a bare thread has no reactor, so
    // the probe below reports the executor rather than the ambient process.
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "a plain test thread must have no reactor, or this probe answers yes \
         whatever the executor is"
    );

    let executor = ShippedExecutor::new().expect("the shipped executor must build");

    // `iced_winit` spawns `mundy::Preferences::stream` on this executor and
    // snapshots inside this same `enter`, which is where zbus asks.
    // Named through the trait: when the executor resolves to Tokio's runtime
    // its inherent `enter` shadows this one, and the two mean different things.
    assert!(
        Executor::enter(&executor, || tokio::runtime::Handle::try_current().is_ok()),
        "the shipped facade executor does not enter a Tokio reactor, so the \
         preference stream will reach zbus with no reactor running and every \
         window in this workspace will die before its first frame; restore \
         the `tokio` feature on the workspace `iced` dependency"
    );

    assert!(
        std::any::type_name::<ShippedExecutor>().contains("tokio::runtime"),
        "the default backend resolved to {}, not Tokio's runtime",
        std::any::type_name::<ShippedExecutor>()
    );
}
