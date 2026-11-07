//! Backend-neutral media state, commands, source policy, and playlists.
//!
//! Current-generation [`MediaEvent`] values update backend-derived state, while source replacement advances [`Generation`] and stale events are discarded. [`MediaState::dispatch`] emits intent without applying it, and remote sources require [`remote_gate`] approval.
//!
//! The backends that play media (the GStreamer audio and video services and
//! the MPRIS adapter) live in `enzui-services`, which builds on these types.

pub mod frame;
pub mod lifecycle;
mod reduced;
pub mod remote_gate;
pub mod rgba;
pub mod sink;
#[cfg(test)]
mod tests;
pub mod worker;

pub use frame::{FramePlan, FrameSeq, FrameVerdict};
pub use lifecycle::{Phase, WorkerLifecycle};
pub use rgba::{
    FrameFormatError, MAX_NATIVE_RGBA_BYTES, Rgba8Frame, RgbaGeometry, RgbaGeometryError,
    preflight_rgba8, preflight_rgba8_storage,
};
pub use sink::{BoundedFrameSink, Delivery};
pub use worker::{JoinOutcome, MediaWorker, StopSignal, WorkerError, WorkerFailure};

use crate::url_policy::{RemoteSource, UrlPolicy, UrlVerdict};
use reduced::Reduced;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

/// A wrapping media-session identifier used to discard events from superseded sources.
///
/// Zero is the first valid generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// The initial generation before any source replacement.
    pub const FIRST: Generation = Generation(0);

    /// The generation with this raw counter value, as another component
    /// numbered it (a web engine echoing a host's frame source, say).
    #[must_use]
    pub const fn from_raw(raw: u64) -> Generation {
        Generation(raw)
    }

    /// The next generation, for use when replacing the source.
    #[must_use]
    pub fn next(self) -> Generation {
        Generation(self.0.wrapping_add(1))
    }

    /// The raw counter value, for diagnostics.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A local file or validated remote URL whose `Display` and `Debug` forms redact sensitive source details.
#[derive(Clone, PartialEq, Eq)]
pub enum MediaSource {
    /// A local file path. Only the filename is shown in `Display`/`Debug`.
    Local(PathBuf),
    /// A URL accepted by [`UrlPolicy`], preserving its full transport URI while redacting rendered forms.
    Remote(RemoteSource),
}

impl MediaSource {
    /// Constructs a local source without checking existence or readability; rendered forms show only the filename.
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        MediaSource::Local(path.into())
    }

    /// Constructs a remote source after URL-string validation.
    ///
    /// The full transport URL is preserved but rendered forms are redacted. Resolved-destination approval remains the responsibility of [`remote_gate::MediaGate::approve`].
    ///
    /// # Errors
    ///
    /// The [`UrlVerdict`] when `policy` refuses `url`.
    pub fn remote(url: &str, policy: &UrlPolicy) -> Result<Self, UrlVerdict> {
        let remote = RemoteSource::checked(url, policy)?;
        Ok(MediaSource::Remote(remote))
    }

    /// The redacted display form, safe for logging.
    #[must_use]
    pub fn redacted(&self) -> String {
        match self {
            MediaSource::Local(path) => {
                let name = path
                    .file_name()
                    .map_or_else(|| "<no filename>".into(), |n| n.to_string_lossy());
                format!("local:{name}")
            }
            MediaSource::Remote(remote) => remote.redacted(),
        }
    }

    /// What a user interface names this source by when the caller gave no
    /// title: a local file's name (no scheme, no directory), or the remote
    /// source's redacted form. `None` for a local path with no file name.
    ///
    /// [`redacted`](Self::redacted) is the log form; it prefixes `local:`,
    /// which reads as a URI in a player's title line.
    #[must_use]
    pub fn display_name(&self) -> Option<String> {
        match self {
            MediaSource::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            MediaSource::Remote(remote) => Some(remote.redacted()),
        }
    }
}

impl fmt::Display for MediaSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

impl fmt::Debug for MediaSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaSource({})", self.redacted())
    }
}

/// Backend-reported source capabilities used to enable or hide media controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// Whether the source supports seeking.
    pub seekable: bool,
    /// Whether the source has an audio track.
    pub has_audio: bool,
    /// Whether the source has a video track.
    pub has_video: bool,
}

impl Capabilities {
    /// Unknown capabilities — the safe default before the backend reports.
    /// Everything is `false`, so controls start disabled rather than presenting
    /// actions that will fail.
    pub const UNKNOWN: Capabilities = Capabilities {
        seekable: false,
        has_audio: false,
        has_video: false,
    };
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::UNKNOWN
    }
}

/// Backend-confirmed playback state without optimistic transitional variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaybackState {
    /// The source is loaded but not playing.
    Stopped,
    /// The source is actively playing.
    Playing,
    /// Playback is paused at the current position.
    Paused,
    /// The source reached its end.
    EndOfStream,
}

impl PlaybackState {
    /// Returns whether the state is [`PlaybackState::EndOfStream`].
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, PlaybackState::EndOfStream)
    }
}

/// A redacted media error with a stable programmatic category.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaError {
    /// What category of failure this is.
    pub kind: MediaErrorKind,
    /// A human-readable message that callers must redact before construction.
    pub message: String,
}

/// The category of a [`MediaError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaErrorKind {
    /// The source could not be opened (file not found, network unreachable…).
    SourceNotFound,
    /// The source's format/codec is not supported.
    UnsupportedFormat,
    /// A seek to an invalid position was refused.
    SeekFailed,
    /// A generic backend error not covered above.
    Backend,
    /// A source refused by policy before connection, distinct from [`MediaErrorKind::SourceNotFound`].
    Refused,
    /// A fatal live-pipeline terminal caused by output-device loss, a clock
    /// disappearance, or a comparable device-level failure that the framework
    /// cannot safely recover in place. Callers must retire the current source
    /// and explicitly reload — a device-level terminal is authoritative for
    /// the exact live generation and no later command on that generation
    /// succeeds. Kept distinct from [`MediaErrorKind::Backend`] so a UI can
    /// tell an output-device loss from a transient decoder refusal without
    /// parsing a redacted message.
    DeviceLost,
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl fmt::Debug for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaError({:?}: {})", self.kind, self.message)
    }
}

/// The framework default playback rate applied on construction and after
/// [`MediaState::replace_source`]; a persisted user rate must be reissued
/// against the new generation.
pub const DEFAULT_PLAYBACK_RATE: f64 = 1.0;

/// The smallest playback rate accepted by [`MediaCommand::SetRate`]
/// admission and by [`sanitize_rate`].
pub const MIN_PLAYBACK_RATE: f64 = 0.5;

/// The largest playback rate accepted by [`MediaCommand::SetRate`]
/// admission and by [`sanitize_rate`]; the range is what real desktop
/// GStreamer audio pipelines change without a scaler reconfiguration.
pub const MAX_PLAYBACK_RATE: f64 = 2.0;

/// Returns the input rate when it is finite and lies in
/// `[MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE]`, otherwise `None`.
///
/// Zero, negative and non-finite inputs are refused rather than clamped
/// because a caller asking for reverse or stopped playback needs a
/// typed refusal, not a coerced forward rate.
#[must_use]
pub fn sanitize_rate(rate: f64) -> Option<f64> {
    if rate.is_finite() && (MIN_PLAYBACK_RATE..=MAX_PLAYBACK_RATE).contains(&rate) {
        Some(rate)
    } else {
        None
    }
}

/// An immutable view of the current authoritative media state.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// The current generation, for stamping commands.
    pub generation: Generation,
    /// The current source, if one is loaded.
    pub source: Option<MediaSource>,
    /// What the backend says is possible.
    pub capabilities: Capabilities,
    /// The confirmed playback state.
    pub playback: PlaybackState,
    /// The confirmed playback position.
    pub position: Duration,
    /// The total duration, or `None` when it is unknown.
    pub duration: Option<Duration>,
    /// The confirmed volume in the inclusive range `0.0..=1.0`.
    pub volume: f64,
    /// Whether audio output is muted (independent of volume).
    pub muted: bool,
    /// The confirmed playback rate. `1.0` after construction and after any
    /// source replacement; updated only when the backend has accepted a
    /// [`MediaCommand::SetRate`].
    pub rate: f64,
    /// The last error, if any. Cleared on source replacement or explicit clear.
    pub error: Option<MediaError>,
}

/// An event from the media backend, stamped with the [`Generation`] it belongs
/// to. The reducer discards events whose generation does not match the current
/// source.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaEvent {
    /// Which source-session this event belongs to.
    pub generation: Generation,
    /// What happened.
    pub kind: MediaEventKind,
}

/// What happened in the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaEventKind {
    /// The source was successfully opened and is ready.
    SourceReady {
        /// The source's capabilities.
        capabilities: Capabilities,
        /// The total duration, if known.
        duration: Option<Duration>,
    },
    /// Playback state changed.
    StateChanged(PlaybackState),
    /// The playback position was updated.
    PositionUpdated(Duration),
    /// The backend confirmed a discontinuous position change, such as a completed seek.
    PositionDiscontinuity(Duration),
    /// The volume was changed (e.g. by an external mixer).
    VolumeChanged {
        /// The new volume, 0.0–1.0.
        volume: f64,
    },
    /// Mute state changed.
    MuteChanged(bool),
    /// The playback rate was accepted by the backend.
    ///
    /// Emitted only after the pipeline confirmed the new rate; a failed
    /// native change leaves the previously published rate in place. The
    /// value lies in `[MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE]`.
    RateChanged {
        /// The confirmed rate.
        rate: f64,
    },
    /// An error occurred.
    Error(MediaError),
}

/// A backend command representing user intent; dispatching it does not mutate reduced state.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaCommand {
    /// Start or resume playback.
    Play,
    /// Pause playback.
    Pause,
    /// Stop playback (rewind to start).
    Stop,
    /// Seek to a position while retaining the generation's accepted playback rate.
    Seek(Duration),
    /// Sets volume in the inclusive range `0.0..=1.0`.
    SetVolume(f64),
    /// Mute or unmute.
    SetMute(bool),
    /// Requests a new playback rate.
    ///
    /// Admission accepts only finite values in
    /// `[MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE]`; anything else, including
    /// zero and negative rates, is refused with the audio service's
    /// `SendError::InvalidRate`. The framework default
    /// is [`DEFAULT_PLAYBACK_RATE`]; a persisted user rate is not carried
    /// across source replacement and must be reissued for the new
    /// generation.
    SetRate(f64),
    /// Skip to the previous track in the playlist.
    PreviousTrack,
    /// Skip to the next track in the playlist.
    NextTrack,
    /// Toggle shuffle mode on/off.
    ToggleShuffle,
    /// Cycle the repeat mode (Off → All → One → Off).
    CycleRepeat,
}

/// Playlist repeat mode with off, whole-playlist, and current-track states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RepeatMode {
    /// No repeat — stop at the end of the playlist.
    #[default]
    Off,
    /// Repeat the entire playlist.
    All,
    /// Repeat the current track.
    One,
}

impl RepeatMode {
    /// Advance to the next mode in the cycle: Off → All → One → Off.
    #[must_use]
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }

    /// Whether the mode is active (i.e. not `Off`).
    #[must_use]
    pub fn is_active(self) -> bool {
        !matches!(self, RepeatMode::Off)
    }
}

/// An ordered playlist in which an empty item list is a valid nothing-to-play state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    items: Vec<MediaSource>,
    current: Option<usize>,
}

impl Playlist {
    /// An empty playlist — a legitimate state meaning "nothing to play".
    #[must_use]
    pub fn empty() -> Self {
        Playlist {
            items: Vec::new(),
            current: None,
        }
    }

    /// A playlist from a list of sources. `current` starts at `0` if non-empty.
    #[must_use]
    pub fn from_items(items: Vec<MediaSource>) -> Self {
        let current = if items.is_empty() { None } else { Some(0) };
        Playlist { items, current }
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the playlist is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The current item, if any.
    #[must_use]
    pub fn current(&self) -> Option<&MediaSource> {
        self.current.and_then(|i| self.items.get(i))
    }

    /// The index of the current item (0-based), if any.
    #[must_use]
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// Advance to the next item. Returns `true` if there was a next item.
    pub fn advance(&mut self) -> bool {
        match self.current {
            Some(i) if i + 1 < self.items.len() => {
                self.current = Some(i + 1);
                true
            }
            _ => false,
        }
    }

    /// Go back to the previous item. Returns `true` if there was a previous.
    pub fn go_back(&mut self) -> bool {
        match self.current {
            Some(i) if i > 0 => {
                self.current = Some(i - 1);
                true
            }
            _ => false,
        }
    }

    /// All items, in order.
    pub fn items(&self) -> &[MediaSource] {
        &self.items
    }
}

/// Identity of the latest confirmed position discontinuity within a source generation.
///
/// Compare identities to avoid announcing the same seek on later progress updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionDiscontinuity {
    generation: Generation,
    revision: u64,
}

/// A pure media reducer that applies current-generation [`MediaEvent`] values and produces [`Snapshot`] values.
///
/// Events from superseded generations are discarded, and commands do not mutate reduced state.
#[derive(Debug, Clone)]
pub struct MediaState {
    generation: Generation,
    source: Option<MediaSource>,
    reduced: Reduced,
}

impl MediaState {
    /// A fresh state with no source loaded.
    #[must_use]
    pub fn new() -> Self {
        MediaState {
            generation: Generation::FIRST,
            source: None,
            reduced: Reduced::new(),
        }
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Replaces the source, advances the generation, resets per-source state, and preserves volume and mute.
    ///
    /// Backend events for the new source must carry the returned generation.
    pub fn replace_source(&mut self, source: MediaSource) -> Generation {
        self.generation = self.generation.next();
        self.source = Some(source);
        self.reduced.reset_for_new_source();
        self.generation
    }

    /// Replaces the source at an explicit target generation.
    ///
    /// This is the commit half of a prepare/commit load transaction used
    /// by `enzui_services::media::AudioService::load_into`.  The caller computes the target
    /// generation and is responsible for ensuring consistency with the
    /// service.  Prefer [`replace_source`](Self::replace_source) for
    /// standalone state management.
    pub fn replace_source_at(&mut self, source: MediaSource, target: Generation) -> Generation {
        self.generation = target;
        self.source = Some(source);
        self.reduced.reset_for_new_source();
        self.generation
    }

    /// Clear the source without loading a new one. Advances the generation.
    pub fn clear_source(&mut self) -> Generation {
        self.generation = self.generation.next();
        self.source = None;
        self.reduced.reset_for_new_source();
        self.generation
    }

    /// Applies a backend event when its generation is current.
    ///
    /// Returns `false` for a stale event; only accepted events mutate reduced state.
    pub fn apply(&mut self, event: &MediaEvent) -> bool {
        if event.generation != self.generation {
            return false;
        }
        self.reduced.apply_event(&event.kind);
        true
    }

    /// Converts a [`MediaCommand`] into a generation-stamped backend request without changing reduced state.
    ///
    /// Dispatch does not clear errors; use [`MediaState::clear_error`] for explicit dismissal.
    #[must_use]
    pub fn dispatch(&mut self, command: &MediaCommand) -> CommandRequest {
        CommandRequest {
            generation: self.generation,
            command: command.clone(),
        }
    }

    /// Explicitly dismisses the last error without dispatching a command.
    pub fn clear_error(&mut self) {
        self.reduced.clear_error();
    }

    /// Identifies the latest confirmed position jump, or `None` before one occurs.
    ///
    /// Source replacement clears this identity. Ordinary progress and command
    /// dispatch leave it unchanged; stale backend events cannot advance it.
    #[must_use]
    pub fn last_position_discontinuity(&self) -> Option<PositionDiscontinuity> {
        let revision = self.reduced.position_revision();
        (revision != 0).then_some(PositionDiscontinuity {
            generation: self.generation,
            revision,
        })
    }

    /// Produce an immutable snapshot for the view layer.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            generation: self.generation,
            source: self.source.clone(),
            capabilities: self.reduced.capabilities(),
            playback: self.reduced.playback(),
            position: self.reduced.position(),
            duration: self.reduced.duration(),
            volume: self.reduced.volume(),
            muted: self.reduced.muted(),
            rate: self.reduced.rate(),
            error: self.reduced.error().cloned(),
        }
    }
}

/// A generation-stamped backend request produced by [`MediaState::dispatch`].
///
/// Dispatch does not apply the request's effect; backend results return as [`MediaEvent`] values.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandRequest {
    /// The generation current when the command was dispatched. The worker
    /// stamps its resulting event with this so a command issued against a
    /// superseded source produces events the reducer discards.
    pub generation: Generation,
    /// The command to perform.
    pub command: MediaCommand,
}

impl Default for MediaState {
    fn default() -> Self {
        Self::new()
    }
}

/// A volume clamped to `0.0..=1.0`, with NaN read as silence.
#[must_use]
pub fn sanitize_volume(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}
