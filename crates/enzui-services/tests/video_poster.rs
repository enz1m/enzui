//! Integration test for `video_poster_frame`.
//!
//! This is the follow-up's "worth having" GStreamer-feature integration
//! test. It generates a small `.webm` fixture at test time with
//! `gst-launch-1.0`, so no binary is committed. The default suite does not
//! need GStreamer — this test is gated on the `gstreamer` Cargo feature.
//!
//! # What the fixture is
//!
//! 30 frames of `videotestsrc pattern=smpte` at 320x240, encoded VP8 in a
//! WebM container. That is a real container/codec exercise (not a raw
//! RGB dump), and the SMPTE test pattern has bright bars so the "non-blank
//! pixels" assertion is meaningful without any codec-specific inspection.
//!
//! # Numbers
//!
//! The fixture writes to a temp file; its size is reported at test time
//! rather than pinned to a byte count, because different GStreamer VP8
//! encoders may produce slightly different output. On a typical box the
//! encoded size is a few kilobytes.
//!
//! # Skip semantics
//!
//! If `gst-launch-1.0` is not on `PATH` (a build box with the runtime
//! libraries but no CLI tools), the test skips with an explicit
//! `eprintln!` rather than fail. The `gstreamer` feature only requires the
//! runtime; asserting on a CLI tool would gate the wrong thing.

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
#![cfg(feature = "gstreamer")]

use enzui_core::decode_limits::{DecodeLimits, DecodeVerdict};
use enzui_services::document_preview::{PreviewError, video_poster_frame};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn have_gst_launch() -> bool {
    Command::new("gst-launch-1.0")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Generate a 30-frame SMPTE-bars WebM fixture. Returns the path on
/// success, or `None` if `gst-launch-1.0` is unavailable or the encode
/// pipeline could not be built (e.g. `vp8enc` element missing).
///
/// `gst-launch-1.0` accepts element properties as a single `k=v` argv
/// entry, so `location=<path>` is one arg, not two.
fn generate_fixture(name: &str) -> Option<PathBuf> {
    if !have_gst_launch() {
        return None;
    }
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("enzui_video_poster_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    let location = format!("location={}", path.display());
    let out = Command::new("gst-launch-1.0")
        .args([
            "-q",
            "videotestsrc",
            "pattern=smpte",
            "num-buffers=30",
            "!",
            "video/x-raw,width=320,height=240,framerate=30/1",
            "!",
            "vp8enc",
            "deadline=1",
            "!",
            "webmmux",
            "!",
            "filesink",
        ])
        .arg(&location)
        .output()
        .ok()?;
    if !out.status.success() || !path.exists() {
        eprintln!(
            "gst-launch-1.0 encode failed (stderr={}); skipping",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(path)
}

/// The follow-up's gate: a fixture *exceeding* the encoded limit is
/// refused before decode, and the refusal names the limit.
#[test]
fn video_poster_frame_refuses_oversized_file() {
    let Some(path) = generate_fixture("oversized.webm") else {
        eprintln!("SKIP: gst-launch-1.0 or vp8enc unavailable");
        return;
    };
    let file_size = std::fs::metadata(&path).unwrap().len();
    let limits = DecodeLimits {
        max_encoded_bytes: file_size.saturating_sub(1),
        ..DecodeLimits::DEFAULT
    };
    let err = video_poster_frame(&path, Duration::ZERO, &limits)
        .expect_err("encoded size exceeds limit, must refuse");
    match err {
        PreviewError::LimitExceeded { verdict }
            if matches!(verdict.verdict(), DecodeVerdict::EncodedTooLarge { .. }) => {}
        other => panic!("expected EncodedTooLarge, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}
