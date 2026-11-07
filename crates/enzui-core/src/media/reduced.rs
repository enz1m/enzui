//! Backend-event reduction for media state, with source reset and explicit error dismissal as the non-event update paths.

use super::{
    Capabilities, DEFAULT_PLAYBACK_RATE, MediaError, MediaEventKind, PlaybackState, sanitize_rate,
    sanitize_volume,
};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(super) struct Reduced {
    capabilities: Capabilities,
    playback: PlaybackState,
    position: Duration,
    position_revision: u64,
    duration: Option<Duration>,
    volume: f64,
    muted: bool,
    rate: f64,
    error: Option<MediaError>,
}

impl Reduced {
    pub(super) fn new() -> Self {
        Reduced {
            capabilities: Capabilities::UNKNOWN,
            playback: PlaybackState::Stopped,
            position: Duration::ZERO,
            position_revision: 0,
            duration: None,
            volume: 1.0,
            muted: false,
            rate: DEFAULT_PLAYBACK_RATE,
            error: None,
        }
    }

    // A persisted user rate is not carried across source replacement;
    // the caller reissues `SetRate` against the new generation when
    // desired. Volume and mute persist because that is user-audio state,
    // not per-source policy.
    pub(super) fn reset_for_new_source(&mut self) {
        self.capabilities = Capabilities::UNKNOWN;
        self.playback = PlaybackState::Stopped;
        self.position = Duration::ZERO;
        self.position_revision = 0;
        self.duration = None;
        self.rate = DEFAULT_PLAYBACK_RATE;
        self.error = None;
    }

    pub(super) fn apply_event(&mut self, kind: &MediaEventKind) {
        match kind {
            MediaEventKind::SourceReady {
                capabilities,
                duration,
            } => {
                self.capabilities = *capabilities;
                self.duration = *duration;
            }
            MediaEventKind::StateChanged(state) => {
                self.playback = *state;
            }
            MediaEventKind::PositionUpdated(pos) => {
                self.position = *pos;
            }
            MediaEventKind::PositionDiscontinuity(pos) => {
                self.position = *pos;
                self.position_revision = self.position_revision.saturating_add(1);
            }
            MediaEventKind::VolumeChanged { volume } => {
                self.volume = sanitize_volume(*volume);
            }
            MediaEventKind::MuteChanged(m) => {
                self.muted = *m;
            }
            MediaEventKind::RateChanged { rate } => {
                // A worker only emits `RateChanged` after the pipeline
                // accepts the seek; an out-of-range value here means the
                // event was constructed by hand, so drop it rather than
                // silently store a value the admission gate would reject.
                if let Some(r) = sanitize_rate(*rate) {
                    self.rate = r;
                }
            }
            MediaEventKind::Error(err) => {
                self.error = Some(err.clone());
            }
        }
    }

    // Error dismissal is the only non-event mutation and can only erase state.
    pub(super) fn clear_error(&mut self) {
        self.error = None;
    }

    pub(super) fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
    pub(super) fn playback(&self) -> PlaybackState {
        self.playback
    }
    pub(super) fn position(&self) -> Duration {
        self.position
    }
    pub(super) fn position_revision(&self) -> u64 {
        self.position_revision
    }
    pub(super) fn duration(&self) -> Option<Duration> {
        self.duration
    }
    pub(super) fn volume(&self) -> f64 {
        self.volume
    }
    pub(super) fn muted(&self) -> bool {
        self.muted
    }
    pub(super) fn rate(&self) -> f64 {
        self.rate
    }
    pub(super) fn error(&self) -> Option<&MediaError> {
        self.error.as_ref()
    }
}
