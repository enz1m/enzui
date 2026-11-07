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

use enzui_services::document_preview::offline_video::*;
use enzui_test_support::test_scratch::ScratchDir;
use std::time::Duration;

use support::request;

#[test]
fn offline_video_reserved_or_overflowing_timestamps_are_refused_before_file_access() {
    let scratch = ScratchDir::new("offline-video-time-range");
    for timestamp in [Duration::from_nanos(u64::MAX), Duration::MAX] {
        let operation = OfflineVideoOperation::Frame {
            timestamp,
            policy: FramePolicy::Accurate,
            size: FrameSize::Source,
        };
        assert_eq!(
            run(
                request(
                    &scratch.path().join("missing.webm"),
                    &scratch.path().join("snapshots"),
                    operation
                ),
                || false
            )
            .unwrap_err(),
            OfflineVideoError::InvalidRequest
        );
        assert!(matches!(
            enzui_services::document_preview::gst_poster::extract_poster_frame(
                &scratch.path().join("missing.webm"),
                timestamp,
                &enzui_core::decode_limits::DecodeLimits::DEFAULT
            ),
            Err(enzui_services::document_preview::PreviewError::DecodeFailed { .. })
        ));
    }
}
#[path = "support/offline_video.rs"]
mod support;
