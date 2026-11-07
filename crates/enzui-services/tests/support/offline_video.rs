use super::{OfflineVideoLimits, OfflineVideoOperation, OfflineVideoRequest};

use enzui_core::decode_limits::DecodeLimits;
use gst::prelude::*;
use gstreamer as gst;
use std::{path::Path, time::Duration};

#[allow(
    dead_code,
    reason = "shared by several tests, and each uses only part of it"
)]
pub(crate) fn fixture(path: &Path) {
    gst::init().unwrap();
    let caps = "video/x-raw,format=I420,width=64,height=48,framerate=10/1";
    let launch = format!(
        "concat name=join ! {caps} ! vp8enc deadline=1 keyframe-max-dist=100 ! queue ! mux. videotestsrc pattern=red num-buffers=10 ! {caps} ! join. videotestsrc pattern=green num-buffers=10 ! {caps} ! join. videotestsrc pattern=blue num-buffers=10 ! {caps} ! join. audiotestsrc wave=silence num-buffers=130 ! vorbisenc ! queue ! mux. webmmux name=mux ! filesink location=\"{}\"",
        path.display()
    );
    let pipeline = gst::parse::launch(&launch)
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let message = pipeline
        .bus()
        .unwrap()
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .expect("fixture deadline");
    pipeline.set_state(gst::State::Null).unwrap();
    assert!(
        matches!(message.view(), gst::MessageView::Eos(_)),
        "fixture encoding failed: {message:?}"
    );
}
pub(crate) fn request(
    path: &Path,
    root: &Path,
    operation: OfflineVideoOperation,
) -> OfflineVideoRequest {
    OfflineVideoRequest {
        generation: 17,
        path: path.into(),
        snapshot_root: root.into(),
        operation,
        limits: OfflineVideoLimits {
            max_file_bytes: 1024 * 1024,
            max_snapshot_copy_bytes: 1024 * 1024,
            frames: DecodeLimits {
                max_encoded_bytes: 1,
                ..DecodeLimits::DEFAULT
            },
            timeout: Duration::from_secs(3),
            max_frames: 4,
            max_retained_rgba_bytes: 1024 * 1024,
        },
    }
}
