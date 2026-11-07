//! In-flight shape across a real font-system mutation.
//!
//! Thread A begins `baseline_offset_px` under generation G, finishes the
//! shape, and blocks at a test-only post-shape handshake. The main thread
//! then loads a real font, drives a lookup that retires the cache to G+1,
//! and releases A. A's `try_seat` must refuse the stale observation, so a
//! subsequent lookup at A's key reshapes and agrees with an independently-
//! uncached reference through the same backend generation.
//!
//! Isolated in its own integration process because `FontSystem::load_font`
//! mutates process-global state that the concurrent widget-lib warm gates
//! rely on (`revision_text_font_generation` isolates for the same reason).
//! The `__test_seams` module is compiled only under the `internal-test-seams`
//! feature; without the feature cargo skips this target and no seam ships
//! in normal builds.
//!
//! Every wait in this target is bounded by [`WATCHDOG`]: a disconnected
//! seam, a panicked shape thread, or an unreached handshake surface as
//! ordinary test failures rather than hangs. The timeout is a failure
//! guard, not a scheduling contract.

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

use enzui_widgets::text_baseline::{
    __metric_cache_generation, __metric_shape_calls, __test_seams, baseline_offset_px,
};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::Duration;

const WATCHDOG: Duration = Duration::from_secs(30);

fn load_fira_as_owned() {
    // `Cow::Owned` bypasses the borrowed-pointer dedupe so the load bumps
    // the version even when Fira has already been registered.
    let _ = iced::advanced::graphics::text::font_system()
        .write()
        .expect("write font system")
        .load_font(std::borrow::Cow::Owned(
            iced::advanced::graphics::text::FIRA_SANS_REGULAR.to_vec(),
        ));
}

#[test]
fn in_flight_shape_across_font_load_does_not_resurrect_stale_value() {
    let font = iced::Font::DEFAULT;
    let line_box = 16.0_f32;
    let a_size = 15.75_f32;
    let probe_size = 12.5_f32;

    let (arrived_tx, arrived_rx) = sync_channel::<()>(0);
    let (release_tx, release_rx) = sync_channel::<()>(0);
    let (result_tx, result_rx) = sync_channel::<f32>(1);
    __test_seams::install_post_shape_gate(arrived_tx, release_rx, WATCHDOG);

    let a_font = font;
    std::thread::spawn(move || {
        let out = baseline_offset_px(a_font, a_size, line_box);
        let _ = result_tx.send(out);
    });

    // Bounded wait for A to reach the gate; the arrival signal fires only
    // after A's shape has completed and A is blocked before `try_seat`.
    match arrived_rx.recv_timeout(WATCHDOG) {
        Ok(()) => {}
        Err(RecvTimeoutError::Timeout) => {
            panic!("thread A did not reach the post-shape gate within {WATCHDOG:?}")
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("post-shape gate arrived-channel disconnected before A signalled arrival")
        }
    }

    let gen_before = __metric_cache_generation();
    load_fira_as_owned();
    let _ = baseline_offset_px(font, probe_size, line_box);
    let gen_after_load = __metric_cache_generation();
    assert_ne!(
        gen_before, gen_after_load,
        "font load must advance the cache generation before A is released",
    );

    release_tx
        .send(())
        .expect("post-shape release channel closed before send");

    let a_offset = match result_rx.recv_timeout(WATCHDOG) {
        Ok(v) => v,
        Err(RecvTimeoutError::Timeout) => {
            panic!("thread A did not return a result within {WATCHDOG:?}")
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("thread A panicked before sending its result")
        }
    };

    // Post-race lookup at A's key: with the observation guard, A's insert
    // was refused, so this lookup misses and reshapes under the current
    // generation. The shape counter is the discriminating claim; the
    // uncached reference proves the retained value is the one the current
    // shaper produces.
    let shapes_before_fresh = __metric_shape_calls();
    let fresh_offset = baseline_offset_px(font, a_size, line_box);
    let shapes_after_fresh = __metric_shape_calls();
    assert!(
        shapes_after_fresh > shapes_before_fresh,
        "fresh lookup at A's key returned {fresh_offset} without reshaping; \
         A's pre-load insert was resurrected into the G+1 cache",
    );

    let reference = __test_seams::shape_uncached(font, a_size, line_box).expect("uncached shape");
    assert_eq!(
        fresh_offset, reference,
        "post-race retained value {fresh_offset} disagrees with an \
         independently-uncached shape {reference} through the same backend \
         generation",
    );

    // Numeric equality of `a_offset` and `reference` is host-dependent: if
    // Fira does not change the resolved SansSerif face, both are the same
    // shaper output. Reported for the log; not asserted.
    eprintln!(
        "in_flight_shape: A returned {a_offset}, post-race reshape \
         returned {fresh_offset}, uncached reference {reference}",
    );
}
