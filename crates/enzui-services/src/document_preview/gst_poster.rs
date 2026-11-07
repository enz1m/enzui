//! Blocking video-poster extraction through the feature-gated GStreamer backend.
//!
//! The encoded file and decoded frame are limit-checked, and pipeline waits use finite
//! timeouts.

use std::path::Path;
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;

use crate::decode_limits::DecodeLimits;
use crate::frame_plan::FrameSeq;
use crate::media::Generation;
use crate::media::gst_worker::{
    GstWorkerConfig, PreparedSource, SampleConversionError, build_offline_pipeline,
    ensure_initialised, sample_to_frame_with_limits,
};

use super::{PreviewError, RenderedPreview};

const PREROLL_TIMEOUT: Duration = Duration::from_secs(10);

const PULL_TIMEOUT: Duration = Duration::from_secs(5);

fn sample_to_preview(
    sample: &gst::Sample,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    match sample_to_frame_with_limits(sample, Generation::FIRST, FrameSeq(1), limits) {
        Ok(frame) => RenderedPreview::new(frame.width(), frame.height(), frame.into_pixels()),
        Err(SampleConversionError::Geometry(crate::media::RgbaGeometryError::DecodeLimit(
            verdict,
        ))) => Err(PreviewError::LimitExceeded { verdict }),
        Err(error) => Err(PreviewError::DecodeFailed {
            message: format!("frame validation failed: {error}"),
        }),
    }
}

/// Opens a video, attempts a nonzero seek, and returns one limit-checked RGBA8 frame.
///
/// This is a blocking operation. A failed seek falls back to a frame available from initial
/// playback.
///
/// # Errors
///
/// Returns a [`PreviewError`] when file access, GStreamer setup, decoding, or limit
/// validation fails.
pub fn extract_poster_frame(
    path: &Path,
    timestamp: Duration,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    extract_poster_frame_cancellable(path, timestamp, limits, || false)
}

/// Extract a silent best-effort poster, checking cancellation during staging
/// and sliced preroll/seek waits. A failed seek retains the initial preroll.
///
/// # Errors
/// Returns file, codec, limit, timeout or cancellation errors.
pub fn extract_poster_frame_cancellable(
    path: &Path,
    timestamp: Duration,
    limits: &DecodeLimits,
    cancelled: impl Fn() -> bool,
) -> Result<RenderedPreview, PreviewError> {
    let time = gst::ClockTime::try_from(timestamp).map_err(|_| PreviewError::DecodeFailed {
        message: "poster timestamp exceeds backend range".into(),
    })?;
    let (checked, _) = super::open_checked_file_cancellable(path, limits, &cancelled)?;
    ensure_initialised().map_err(|error| PreviewError::DecodeFailed {
        message: error.to_string(),
    })?;
    let built = build_offline_pipeline(
        &PreparedSource::local(checked.decoder_path()),
        &GstWorkerConfig::default(),
    )
    .map_err(|error| PreviewError::DecodeFailed {
        message: error.to_string(),
    })?;
    #[cfg(all(test, target_os = "linux"))]
    super::offline_video::observe_pipeline_for_test(&built.pipeline);
    let result = (|| {
        built
            .pipeline
            .set_state(gst::State::Paused)
            .map_err(|_| PreviewError::DecodeFailed {
                message: "poster pipeline would not pause".into(),
            })?;
        poster_preroll(&built, &cancelled)?;
        let initial = poster_sample(&built, &cancelled)?;
        let sample = if !timestamp.is_zero()
            && built
                .pipeline
                .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, time)
                .is_ok()
        {
            poster_preroll(&built, &cancelled)?;
            poster_sample(&built, &cancelled)?
        } else {
            initial
        };
        let preview = sample_to_preview(&sample, limits)?;
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        Ok(preview)
    })();
    built.shutdown();
    result
}
fn poster_sample(
    built: &crate::media::gst_worker::BuiltPipeline,
    cancelled: &dyn Fn() -> bool,
) -> Result<gst::Sample, PreviewError> {
    let deadline = std::time::Instant::now() + PULL_TIMEOUT;
    loop {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let left = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| PreviewError::DecodeFailed {
                message: "poster sample timed out".into(),
            })?;
        if let Some(sample) = built
            .video_sink
            .try_pull_preroll(gst::ClockTime::from_nseconds(
                u64::try_from(left.min(Duration::from_millis(25)).as_nanos()).unwrap_or(u64::MAX),
            ))
        {
            return Ok(sample);
        }
    }
}
fn poster_preroll(
    built: &crate::media::gst_worker::BuiltPipeline,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PreviewError> {
    let deadline = std::time::Instant::now() + PREROLL_TIMEOUT;
    loop {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let left = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| PreviewError::DecodeFailed {
                message: "poster preroll timed out".into(),
            })?;
        let (result, state, pending) = built.pipeline.state(gst::ClockTime::from_nseconds(
            u64::try_from(left.min(Duration::from_millis(25)).as_nanos()).unwrap_or(u64::MAX),
        ));
        if result.is_err() {
            return Err(PreviewError::DecodeFailed {
                message: "poster preroll failed".into(),
            });
        }
        if state == gst::State::Paused && pending == gst::State::VoidPending {
            return Ok(());
        }
    }
}
