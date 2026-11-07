//! The media backends: the GStreamer audio and video services, the worker
//! that drives a pipeline, the pinned HTTP proxy and curl redirect probe that
//! carry remote streams through `enzui-core`'s remote gate, and the MPRIS
//! adapter.
//!
//! Everything in `enzui_core::media` (the state model, frames, sinks and
//! the remote gate) is re-exported here, so a backend and the model it drives
//! are reached through one path.

pub use enzui_core::media::*;

#[cfg(feature = "gstreamer")]
pub mod audio_service;
#[cfg(feature = "gstreamer")]
mod bounded_http;
#[cfg(feature = "gstreamer")]
pub mod curl_redirect_probe;
#[cfg(feature = "gstreamer")]
pub mod gst_worker;
#[cfg(feature = "mpris")]
pub mod mpris;
#[cfg(feature = "gstreamer")]
mod pinned_proxy;
#[cfg(all(test, feature = "gstreamer"))]
mod remote_effect_tests;
#[cfg(feature = "gstreamer")]
pub mod video_service;

#[cfg(feature = "gstreamer")]
pub use audio_service::{
    AUDIO_EQUALIZER_BAND_COUNT, AUDIO_REAPER_CAPACITY, AudioEffects, AudioEffectsError,
    AudioEventReceiver, AudioService, AudioServiceConfig, AudioServiceWake,
    COMMAND_CHANNEL_CAPACITY, EVENT_CHANNEL_CAPACITY, LoadRefused, MAX_EQUALIZER_GAIN_DB,
    MAX_PROCESSING_GAIN, MIN_EQUALIZER_GAIN_DB, MIN_PROCESSING_GAIN, ReaperSnapshot, SendError,
    audio_reaper_snapshot,
};
#[cfg(feature = "gstreamer")]
pub use curl_redirect_probe::CurlRedirectProbe;
#[cfg(feature = "mpris")]
pub use mpris::{
    MAX_MATCHABLE_TRACK_ID, MockMprisClient, MprisAdapter, MprisCall, MprisConnection,
    MprisNavigation, MprisProperties, MprisRefusal, NO_TRACK_ID, PlaybackStatus, TRACK_ID_PREFIX,
    retained_track_id, track_id,
};
#[cfg(feature = "gstreamer")]
pub use video_service::{
    DiskReadAhead, LoadOptions, ReadAheadStatus, SeekAdmissionError, SeekMode, SeekOutcome,
    SeekRejection, SeekRequest, SubtitleSize, Track, VIDEO_SEEK_CAPACITY, VideoBuffering,
    VideoService, VideoServiceConfig, VideoStatus, VideoUpdate, VideoWake,
};
