//! Cancellable local video probing and sampling without a playback owner.
//!
//! Requests run synchronously on an owned worker. File admission and isolated
//! disk snapshots have separate limits from decoded frames. A filmstrip reuses
//! one silent decoder session; results retain the caller's generation and each
//! returned buffer's presentation timestamp. Accurate requests never fall back
//! to an initial poster. Snapshot roots must be caller-owned private directories.
//!
//! ```no_run
//! use enzui_core::{blocking_task::BlockingTaskOwner, decode_limits::DecodeLimits};
//! use enzui_services::document_preview::offline_video::*;
//! use std::{num::NonZeroUsize, time::Duration};
//!
//! # async fn sample() -> Result<(), Box<dyn std::error::Error>> {
//! let owner = BlockingTaskOwner::new("video-preview", NonZeroUsize::new(1).unwrap(),
//!     NonZeroUsize::new(8).unwrap())?;
//! let request = OfflineVideoRequest {
//!     generation: 7, path: "clip.webm".into(), snapshot_root: "private-scratch".into(),
//!     operation: OfflineVideoOperation::Frame {
//!         timestamp: Duration::from_secs(1), policy: FramePolicy::Accurate,
//!         size: FrameSize::Bounds { width: 320, height: 180 },
//!     },
//!     limits: OfflineVideoLimits {
//!         max_file_bytes: 2_000_000_000, max_snapshot_copy_bytes: 100_000_000,
//!         frames: DecodeLimits::DEFAULT, timeout: Duration::from_secs(5),
//!         max_frames: 1, max_retained_rgba_bytes: 320 * 180 * 4,
//!     },
//! };
//! let settled = dispatch(&owner.dispatcher(), request).await;
//! let result = settled.into_result()??;
//! assert_eq!(result.generation, 7); // The application compares its current generation.
//! # Ok(()) }
//! ```
//!
//! Applications retain the worker owner, cancel superseded tasks and reject stale
//! generations at delivery. Raster dimensions and pixels can form an iced RGBA
//! image handle for the optional `enzui_resources::image_ops` export service.
//! Native plugins run in-process; cancellation is cooperative at copy/wait boundaries.
//! Encoded video caps must report dimensions before decoder construction, otherwise
//! requests return [`OfflineVideoError::Unsupported`] rather than decode without admission.

use super::RenderedPreview;
use crate::decode_limits::{DecodeLimits, RefusedVerdict};
use std::path::PathBuf;
use std::time::Duration;

/// Temporal selection policy for an offline frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramePolicy {
    /// Snap to a preceding keyframe; failed seeks may return the initial poster.
    Poster,
    /// Seek accurately; seek failures and unavailable timestamps are errors.
    Accurate,
}

/// Output geometry, independent from encoded file admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameSize {
    /// Preserve aspect ratio within positive bounds without enlargement.
    Bounds {
        /// Maximum width in pixels.
        width: u32,
        /// Maximum height in pixels.
        height: u32,
    },
    /// Retain admitted natural dimensions.
    Source,
}

/// Source size, scratch-copy and per-frame allocation policy.
#[derive(Clone, Debug)]
pub struct OfflineVideoLimits {
    /// Largest regular source file accepted, independently of image encoded limits.
    pub max_file_bytes: u64,
    /// Largest fallback disk copy when reflinking is unavailable; zero requires reflink.
    pub max_snapshot_copy_bytes: u64,
    /// Native preroll and requested RGBA frame ceilings; encoded-byte limit is unused.
    pub frames: DecodeLimits,
    /// Maximum wait for each pipeline preroll or seek, checked in cancellation slices.
    pub timeout: Duration,
    /// Maximum number of retained filmstrip frames.
    pub max_frames: usize,
    /// Maximum sum of retained RGBA filmstrip bytes.
    pub max_retained_rgba_bytes: u64,
}

/// A generation-owned offline request over a local path and private scratch root.
#[derive(Clone, Debug)]
pub struct OfflineVideoRequest {
    /// View/editor generation returned with successful results.
    pub generation: u64,
    /// Local regular-file source; no URI/network source is accepted.
    pub path: PathBuf,
    /// Private scratch root, preferably on the source filesystem for reflinks.
    pub snapshot_root: PathBuf,
    /// Requested probe, frame or bounded series.
    pub operation: OfflineVideoOperation,
    /// Independent source and raster admission ceilings.
    pub limits: OfflineVideoLimits,
}

/// Explicit work; probing does not retain a raster.
#[derive(Clone, Debug)]
pub enum OfflineVideoOperation {
    /// Discover natural video size, duration, rate and decoded stream descriptions.
    Probe,
    /// Extract one timestamp according to the supplied selection policy.
    Frame {
        /// Requested position.
        timestamp: Duration,
        /// Seek semantics.
        policy: FramePolicy,
        /// Output bounds.
        size: FrameSize,
    },
    /// Sample the supplied times in order using one decoder session.
    Filmstrip {
        /// Requested positions; bounded by `max_frames` before file access.
        timestamps: Vec<Duration>,
        /// Seek semantics.
        policy: FramePolicy,
        /// Output bounds.
        size: FrameSize,
    },
}

/// A decoded stream's identifier and bounded caps description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfflineTrack {
    /// Backend stream identifier; labels/tags remain optional application concerns.
    pub id: String,
    /// Negotiated caps, identifying audio/video/text and format properties.
    pub caps: String,
}

/// Natural video metadata, independent from requested output scaling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoProbe {
    /// Natural decoded width.
    pub width: u32,
    /// Natural decoded height.
    pub height: u32,
    /// Duration when the container/backend reports it.
    pub duration: Option<Duration>,
    /// Declared frame-rate numerator and denominator; absent when caps provide no positive rate.
    pub frame_rate: Option<(u32, u32)>,
    /// Timestamp units used by this API: one nanosecond per tick.
    pub timestamp_time_base: (u32, u32),
    /// At most 64 streams, with IDs capped at 512 characters and caps at 1024.
    pub tracks: Vec<OfflineTrack>,
}

/// One admitted frame with its actual backend presentation timestamp.
#[derive(Clone, Debug)]
pub struct OfflineFrame {
    /// Requested position.
    pub requested: Duration,
    /// Returned buffer presentation timestamp, which may be clipped by the segment.
    pub actual: Duration,
    /// Whether a poster request used the initial preroll after a failed seek.
    pub poster_fallback: bool,
    /// Exact validated RGBA storage.
    pub raster: RenderedPreview,
}

/// Metadata and generation-owned frames from one isolated decoder session.
#[derive(Clone, Debug)]
pub struct OfflineVideoResult {
    /// Caller generation for rejection of obsolete editor/viewer deliveries.
    pub generation: u64,
    /// Natural dimensions and backend metadata.
    pub probe: VideoProbe,
    /// Empty for probes, one for frame extraction, bounded for filmstrips.
    pub frames: Vec<OfflineFrame>,
}

/// Typed admission, backend and extraction failures.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfflineVideoError {
    /// The build or host lacks the requested backend/plugin.
    Unsupported(String),
    /// Source access or private scratch staging failed.
    Io(String),
    /// The source is not a regular file or changed while its snapshot was created.
    SourceChanged,
    /// Independent encoded-file admission refused the source.
    FileLimit {
        /// Observed source size.
        bytes: u64,
        /// Caller ceiling.
        limit: u64,
    },
    /// Source snapshot cannot reflink and exceeds the permitted fallback disk copy.
    SnapshotUnavailable,
    /// Native or output RGBA geometry exceeded an admitted frame ceiling.
    FrameLimit(RefusedVerdict),
    /// Cancellation was observed; no filmstrip is returned partially.
    Cancelled,
    /// A bounded pipeline wait expired.
    Timeout,
    /// A target or filmstrip request is degenerate or exceeds retained budgets.
    InvalidRequest,
    /// No video stream was discovered.
    NoVideo,
    /// An accurate requested time exceeds the known duration.
    OutOfRange(Duration),
    /// The backend refused an accurate seek.
    SeekFailed(Duration),
    /// The returned sample has no presentation timestamp.
    MissingTimestamp,
    /// Backend parsing, negotiation or sample conversion failed.
    Decode(String),
}
impl std::fmt::Display for OfflineVideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for OfflineVideoError {}
impl From<std::io::Error> for OfflineVideoError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// Run an offline request on an existing worker, forwarding cancellation to staging
/// and bounded backend waits. Requires the optional `gstreamer` backend on Linux.
///
/// # Errors
/// Returns typed source, request, codec, seek, timeout or cancellation errors.
pub fn run(
    request: OfflineVideoRequest,
    cancelled: impl Fn() -> bool,
) -> Result<OfflineVideoResult, OfflineVideoError> {
    if cancelled() {
        return Err(OfflineVideoError::Cancelled);
    }
    #[cfg(all(feature = "gstreamer", target_os = "linux"))]
    {
        native::run(request, &cancelled)
    }
    #[cfg(not(all(feature = "gstreamer", target_os = "linux")))]
    {
        let _ = request;
        Err(OfflineVideoError::Unsupported(
            "offline video requires Linux and the gstreamer feature".into(),
        ))
    }
}

/// Submit without spawning another thread or interacting with live playback.
pub fn dispatch(
    worker: &crate::blocking_task::BlockingTaskDispatcher,
    request: OfflineVideoRequest,
) -> crate::blocking_task::BlockingTask<Result<OfflineVideoResult, OfflineVideoError>> {
    worker.submit(request.generation, move |context| {
        run(request, || context.is_cancelled())
    })
}

#[cfg(all(feature = "gstreamer", target_os = "linux"))]
mod native;

#[cfg(all(test, feature = "gstreamer", target_os = "linux"))]
pub(super) fn observe_pipeline_for_test(pipeline: &gstreamer::Pipeline) {
    native::observations::attach(pipeline);
}
