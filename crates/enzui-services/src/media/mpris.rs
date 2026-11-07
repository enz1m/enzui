//! MPRIS calls are translated into [`MediaCommand`] values and dispatched through [`MediaState`].
//!
//! Seek capability is read from [`Snapshot`], and transport integration is provided by [`MprisConnection`].
//! Queue boundaries are app-owned, so navigation capabilities come from a caller-supplied [`MprisNavigation`].

use crate::media::{
    Capabilities, CommandRequest, Generation, MediaCommand, MediaState, PlaybackState, Snapshot,
    sanitize_rate, sanitize_volume,
};
use std::time::Duration;

/// Object-path prefix of the `mpris:trackid` this player exports in `Metadata`.
pub const TRACK_ID_PREFIX: &str = "/org/enzui/track/";

/// The reserved MPRIS path meaning "no track", which the specification forbids
/// as a `SetPosition` argument. [`track_id`] never produces it, so it is stale.
pub const NO_TRACK_ID: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// The longest `TrackId` [`track_id`] can produce, being the prefix and the
/// twenty digits of [`u64::MAX`].
pub const MAX_MATCHABLE_TRACK_ID: usize = TRACK_ID_PREFIX.len() + 20;

/// The `mpris:trackid` object path naming `generation`.
///
/// The exported `Metadata` id and the identity [`MprisAdapter::translate`]
/// matches against are the same function, so the two cannot drift apart.
#[must_use]
pub fn track_id(generation: Generation) -> String {
    format!("{TRACK_ID_PREFIX}{}", generation.raw())
}

/// Narrows a peer-supplied `TrackId` to the form worth queueing.
///
/// A path longer than [`MAX_MATCHABLE_TRACK_ID`] cannot equal any [`track_id`],
/// so the empty string stands in for it and a queue of 64 calls cannot retain
/// megabytes of attacker-chosen path. Both forms compare unequal, so the
/// staleness verdict is unchanged.
#[must_use]
pub fn retained_track_id(supplied: &str) -> String {
    if supplied.len() > MAX_MATCHABLE_TRACK_ID {
        return String::new();
    }
    supplied.to_owned()
}

/// An MPRIS player call supported by the media command model.
#[derive(Debug, Clone, PartialEq)]
pub enum MprisCall {
    /// `Play` - start or resume.
    Play,
    /// `Pause` - pause in place.
    Pause,
    /// Toggles playback from the reducer-confirmed state.
    PlayPause,
    /// `Stop` - stop and rewind.
    Stop,
    /// `Next` - advance to the next entry of the caller's queue.
    Next,
    /// `Previous` - return to the previous entry of the caller's queue.
    Previous,
    /// Seeks by a relative MPRIS offset from the confirmed position.
    Seek {
        /// The offset; negative seeks backwards.
        offset: i64,
        /// Whether `offset` is expressed in MPRIS microseconds.
        micros: bool,
    },
    /// `SetPosition` - an absolute seek to a microsecond offset.
    SetPosition {
        /// The `TrackId` the caller believes is playing, as an object path. A
        /// value other than [`track_id`] of the current generation is stale.
        track: String,
        /// Target position in microseconds from the start.
        micros: i64,
    },
    /// The `Volume` property being written.
    SetVolume(f64),
    /// Toggles mute through the media command model; this is not a standard MPRIS property.
    SetMute(bool),
    /// The `Rate` property being written. A written zero is `Pause`, which is
    /// what MPRIS requires of a rate of zero.
    SetRate(f64),
}

/// A typed reason an MPRIS call was refused before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisRefusal {
    /// The source cannot seek, so a seek call is refused rather than dispatched.
    NotSeekable,
    /// There is no source loaded, so there is nothing to control.
    NoSource,
    /// The source duration is unknown, so a nonzero seek target cannot be bounded.
    UnknownDuration,
    /// The supplied `TrackId` does not name the loaded track, so the call is
    /// stale and must not reposition whatever replaced it.
    StaleTrack,
    /// The requested absolute position lies outside `0..=track_length`, which
    /// MPRIS ignores rather than clamping into range.
    PositionOutOfRange,
    /// The requested playback rate is outside
    /// `[MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE]`, which the backend would not
    /// accept. Zero is not in this set, because MPRIS defines it as `Pause`.
    InvalidRate,
}

impl core::fmt::Display for MprisRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MprisRefusal::NotSeekable => write!(f, "the current source is not seekable"),
            MprisRefusal::NoSource => write!(f, "no source is loaded"),
            MprisRefusal::UnknownDuration => {
                write!(f, "the current source has no known duration to seek within")
            }
            MprisRefusal::StaleTrack => {
                write!(f, "the supplied track id is not the current track")
            }
            MprisRefusal::PositionOutOfRange => {
                write!(f, "the requested position is outside the current track")
            }
            MprisRefusal::InvalidRate => {
                write!(
                    f,
                    "the requested playback rate is outside the supported range"
                )
            }
        }
    }
}

impl std::error::Error for MprisRefusal {}

/// An MPRIS playback status derived from [`PlaybackState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// `"Playing"`.
    Playing,
    /// `"Paused"`.
    Paused,
    /// `"Stopped"`.
    Stopped,
}

impl PlaybackStatus {
    /// The exact string MPRIS specifies.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PlaybackStatus::Playing => "Playing",
            PlaybackStatus::Paused => "Paused",
            PlaybackStatus::Stopped => "Stopped",
        }
    }

    /// Maps playback states to MPRIS; `EndOfStream` becomes `Stopped` because MPRIS has no ended status.
    #[must_use]
    pub fn from_playback(state: PlaybackState) -> PlaybackStatus {
        match state {
            PlaybackState::Playing => PlaybackStatus::Playing,
            PlaybackState::Paused => PlaybackStatus::Paused,
            PlaybackState::Stopped | PlaybackState::EndOfStream => PlaybackStatus::Stopped,
        }
    }
}

/// Playlist navigation availability published as `CanGoNext` and `CanGoPrevious`.
///
/// The queue lives in the application rather than in [`MediaState`], so the
/// framework cannot derive these. Every publishing path defaults to
/// [`MprisNavigation::NONE`], which advertises nothing instead of a call the
/// adapter would refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MprisNavigation {
    /// Whether another entry follows the loaded one.
    pub can_go_next: bool,
    /// Whether another entry precedes the loaded one.
    pub can_go_previous: bool,
}

impl MprisNavigation {
    /// No navigation in either direction, which is also [`Default`].
    pub const NONE: MprisNavigation = MprisNavigation {
        can_go_next: false,
        can_go_previous: false,
    };

    /// Availability in each direction.
    #[must_use]
    pub fn new(can_go_next: bool, can_go_previous: bool) -> MprisNavigation {
        MprisNavigation {
            can_go_next,
            can_go_previous,
        }
    }
}

/// MPRIS properties derived entirely from an authoritative [`Snapshot`].
#[derive(Debug, Clone, PartialEq)]
pub struct MprisProperties {
    /// `PlaybackStatus`.
    pub playback_status: PlaybackStatus,
    /// `Position`, in microseconds.
    pub position_micros: i64,
    /// `Metadata`'s `mpris:length`, in microseconds, or `None` when duration is unknown.
    pub length_micros: Option<i64>,
    /// `Volume`, 0.0–1.0.
    pub volume: f64,
    /// Whether audio is muted. Independent of `volume`, matching the reducer.
    pub muted: bool,
    /// `Rate` - the confirmed playback rate, inside
    /// `[MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE]`.
    pub rate: f64,
    /// `CanSeek`.
    pub can_seek: bool,
    /// Source availability for `CanPlay` and `CanPause`.
    ///
    /// The D-Bus transport's intrinsic `CanControl` remains true without a source.
    pub can_control: bool,
    /// `CanGoNext` - whether a client may call `Next` and expect the track to
    /// change.
    pub can_go_next: bool,
    /// `CanGoPrevious`.
    pub can_go_previous: bool,
    /// The redacted source title; paths and URL credentials are never published on the session bus.
    pub title: Option<String>,
    /// The generation these properties describe, so a caller can tell whether a
    /// published set is stale.
    pub generation: Generation,
}

impl MprisProperties {
    /// Derive the whole outbound property set from an authoritative snapshot.
    ///
    /// Navigation is [`MprisNavigation::NONE`], so `CanGoNext` and
    /// `CanGoPrevious` are false; a caller that owns a queue publishes it
    /// through [`MprisProperties::from_snapshot_with_navigation`].
    #[must_use]
    pub fn from_snapshot(snap: &Snapshot) -> MprisProperties {
        MprisProperties::from_snapshot_with_navigation(snap, MprisNavigation::NONE)
    }

    /// Derive the outbound property set from an authoritative snapshot and the
    /// caller's navigation availability.
    #[must_use]
    pub fn from_snapshot_with_navigation(
        snap: &Snapshot,
        navigation: MprisNavigation,
    ) -> MprisProperties {
        let can_control = snap.source.is_some();
        MprisProperties {
            playback_status: PlaybackStatus::from_playback(snap.playback),
            position_micros: duration_to_micros(snap.position),
            length_micros: snap.duration.map(duration_to_micros),
            volume: snap.volume,
            muted: snap.muted,
            rate: snap.rate,
            can_seek: snap.capabilities.seekable,
            can_control,
            // Navigation requires a loaded source because `translate` refuses calls without one.
            can_go_next: can_control && navigation.can_go_next,
            can_go_previous: can_control && navigation.can_go_previous,
            title: snap
                .source
                .as_ref()
                .map(enzui_core::media::MediaSource::redacted),
            generation: snap.generation,
        }
    }
}

fn duration_to_micros(d: Duration) -> i64 {
    i64::try_from(d.as_micros()).unwrap_or(i64::MAX)
}

/// Transport boundary for publishing MPRIS properties and reporting refusals.
pub trait MprisConnection {
    /// Publishes current properties without announcing a position discontinuity.
    fn publish(&mut self, properties: &MprisProperties);

    /// Publishes properties with the reducer's latest confirmed position-jump identity.
    ///
    /// Transports supporting `Seeked` deduplicate this identity. The default
    /// preserves property-only transports without emitting seek notifications.
    fn publish_with_discontinuity(
        &mut self,
        properties: &MprisProperties,
        _discontinuity: Option<enzui_core::media::PositionDiscontinuity>,
    ) {
        self.publish(properties);
    }

    /// Answer a method call that the adapter refused.
    fn refuse(&mut self, call: &MprisCall, refusal: MprisRefusal);
}

/// An app-scoped MPRIS adapter backed by authoritative media state.
#[derive(Debug, Clone)]
pub struct MprisAdapter {
    bus_name: String,
}

impl MprisAdapter {
    /// An adapter that would own `org.mpris.MediaPlayer2.<app>`.
    #[must_use]
    pub fn new(app_name: impl Into<String>) -> MprisAdapter {
        MprisAdapter {
            bus_name: format!("org.mpris.MediaPlayer2.{}", app_name.into()),
        }
    }

    /// The bus name this adapter would own.
    #[must_use]
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// Translates an MPRIS call against an authoritative snapshot without dispatching it.
    ///
    /// # Errors
    ///
    /// Returns [`MprisRefusal`] when the call is not valid for the current source.
    pub fn translate(
        &self,
        call: &MprisCall,
        snap: &Snapshot,
    ) -> Result<MediaCommand, MprisRefusal> {
        if snap.source.is_none() {
            return Err(MprisRefusal::NoSource);
        }

        match call {
            MprisCall::Play => Ok(MediaCommand::Play),
            MprisCall::Pause => Ok(MediaCommand::Pause),
            MprisCall::Stop => Ok(MediaCommand::Stop),
            // Queue position is app state, so the application decides which
            // entry a navigation command resolves to.
            MprisCall::Next => Ok(MediaCommand::NextTrack),
            MprisCall::Previous => Ok(MediaCommand::PreviousTrack),
            // Resolve against confirmed playback so unconfirmed consecutive toggles do not alternate from a local guess.
            MprisCall::PlayPause => match snap.playback {
                PlaybackState::Playing => Ok(MediaCommand::Pause),
                _ => Ok(MediaCommand::Play),
            },
            // A session-bus peer writes `Volume` with an arbitrary f64, so the
            // range documented on `MediaCommand::SetVolume` is enforced here.
            MprisCall::SetVolume(v) => Ok(MediaCommand::SetVolume(sanitize_volume(*v))),
            MprisCall::SetMute(m) => Ok(MediaCommand::SetMute(*m)),
            // MPRIS requires a written rate of zero to act as `Pause`, so it is
            // a transport control rather than an invalid rate.
            MprisCall::SetRate(rate) if *rate == 0.0 => Ok(MediaCommand::Pause),
            // Any other unsupported rate is refused rather than clamped: a
            // client that asked for reverse playback must not silently receive a
            // forward rate it did not request.
            MprisCall::SetRate(rate) => sanitize_rate(*rate)
                .map(MediaCommand::SetRate)
                .ok_or(MprisRefusal::InvalidRate),
            MprisCall::Seek { offset, micros } => {
                if !snap.capabilities.seekable {
                    return Err(MprisRefusal::NotSeekable);
                }
                let offset_micros = if *micros {
                    *offset
                } else {
                    offset.saturating_mul(1_000_000)
                };
                self.relative_seek_command(snap, offset_micros)
            }
            MprisCall::SetPosition { track, micros } => {
                // A `TrackId` naming anything but the loaded track is ignored as
                // stale, so an in-flight call cannot reposition its successor.
                if *track != track_id(snap.generation) {
                    return Err(MprisRefusal::StaleTrack);
                }
                if !snap.capabilities.seekable {
                    return Err(MprisRefusal::NotSeekable);
                }
                let target = self.exact_position_target(snap, *micros)?;
                Ok(MediaCommand::Seek(target))
            }
        }
    }

    fn relative_seek_command(
        &self,
        snap: &Snapshot,
        offset_micros: i64,
    ) -> Result<MediaCommand, MprisRefusal> {
        let target = duration_to_micros(snap.position)
            .saturating_add(offset_micros)
            .max(0);
        match snap.duration {
            Some(duration) if target > duration_to_micros(duration) => Ok(MediaCommand::NextTrack),
            Some(_) => Ok(MediaCommand::Seek(Duration::from_micros(
                target.unsigned_abs(),
            ))),
            None if target == 0 => Ok(MediaCommand::Seek(Duration::ZERO)),
            None => Err(MprisRefusal::UnknownDuration),
        }
    }

    // MPRIS ignores an absolute position outside `0..=track_length` instead of
    // moving it into range, so an out-of-range target is refused, not clamped.
    fn exact_position_target(
        &self,
        snap: &Snapshot,
        micros: i64,
    ) -> Result<Duration, MprisRefusal> {
        if micros < 0 {
            return Err(MprisRefusal::PositionOutOfRange);
        }
        match snap.duration {
            Some(duration) if micros <= duration_to_micros(duration) => {
                Ok(Duration::from_micros(micros.unsigned_abs()))
            }
            Some(_) => Err(MprisRefusal::PositionOutOfRange),
            // An unknown length cannot bound a nonzero target, but zero is
            // inside every track.
            None if micros == 0 => Ok(Duration::ZERO),
            None => Err(MprisRefusal::UnknownDuration),
        }
    }

    /// Translates and dispatches an MPRIS call through [`MediaState::dispatch`].
    ///
    /// The returned request is stamped with the current generation.
    ///
    /// # Errors
    ///
    /// Returns [`MprisRefusal`] without dispatching when the call is invalid.
    pub fn handle(
        &self,
        call: &MprisCall,
        state: &mut MediaState,
    ) -> Result<CommandRequest, MprisRefusal> {
        let snapshot = state.snapshot();
        let command = self.translate(call, &snapshot)?;
        Ok(state.dispatch(&command))
    }

    /// Handles a call and publishes the confirmed property snapshot.
    ///
    /// Refusals are reported and produce no property publication. Navigation
    /// is published as [`MprisNavigation::NONE`]; see
    /// [`Self::handle_and_publish_with_navigation`].
    ///
    /// # Errors
    ///
    /// Returns the [`MprisRefusal`] after reporting it to `connection`.
    pub fn handle_and_publish<C: MprisConnection + ?Sized>(
        &self,
        call: &MprisCall,
        state: &mut MediaState,
        connection: &mut C,
    ) -> Result<CommandRequest, MprisRefusal> {
        self.handle_and_publish_with_navigation(call, state, MprisNavigation::NONE, connection)
    }

    /// Handles a call and publishes the confirmed snapshot with the caller's
    /// navigation availability.
    ///
    /// # Errors
    ///
    /// Returns the [`MprisRefusal`] after reporting it to `connection`.
    pub fn handle_and_publish_with_navigation<C: MprisConnection + ?Sized>(
        &self,
        call: &MprisCall,
        state: &mut MediaState,
        navigation: MprisNavigation,
        connection: &mut C,
    ) -> Result<CommandRequest, MprisRefusal> {
        match self.handle(call, state) {
            Ok(request) => {
                // Dispatch is intent only; publish the unchanged confirmed snapshot until a backend event arrives.
                self.publish_state_with_navigation(state, navigation, connection);
                Ok(request)
            }
            Err(refusal) => {
                connection.refuse(call, refusal);
                Err(refusal)
            }
        }
    }

    /// Publishes authoritative properties and newly confirmed position discontinuities.
    ///
    /// Navigation is published as [`MprisNavigation::NONE`], which is also what
    /// [`AudioService::apply_events`](crate::media::AudioService::apply_events)
    /// publishes; a caller that owns a queue drains with
    /// [`AudioService::apply_events_no_mpris`](crate::media::AudioService::apply_events_no_mpris)
    /// and publishes through [`Self::publish_state_with_navigation`] instead.
    pub fn publish_state<C: MprisConnection + ?Sized>(
        &self,
        state: &MediaState,
        connection: &mut C,
    ) {
        self.publish_state_with_navigation(state, MprisNavigation::NONE, connection);
    }

    /// Publishes authoritative properties, confirmed discontinuities and navigation availability.
    pub fn publish_state_with_navigation<C: MprisConnection + ?Sized>(
        &self,
        state: &MediaState,
        navigation: MprisNavigation,
        connection: &mut C,
    ) {
        connection.publish_with_discontinuity(
            &MprisProperties::from_snapshot_with_navigation(&state.snapshot(), navigation),
            state.last_position_discontinuity(),
        );
    }
}

/// Maps media capabilities to the MPRIS capability set.
#[must_use]
pub fn can_seek(caps: Capabilities) -> bool {
    caps.seekable
}

/// An in-process MPRIS connection that records publications and refusals.
#[derive(Debug, Default)]
pub struct MockMprisClient {
    published: Vec<MprisProperties>,
    refused: Vec<(MprisCall, MprisRefusal)>,
}

impl MockMprisClient {
    /// A client that has seen nothing.
    #[must_use]
    pub fn new() -> MockMprisClient {
        MockMprisClient::default()
    }

    /// Every property set published, in order.
    #[must_use]
    pub fn published(&self) -> &[MprisProperties] {
        &self.published
    }

    /// The most recently published property set.
    #[must_use]
    pub fn last_published(&self) -> Option<&MprisProperties> {
        self.published.last()
    }

    /// Every refusal reported, in order.
    #[must_use]
    pub fn refused(&self) -> &[(MprisCall, MprisRefusal)] {
        &self.refused
    }

    /// Returns the number of property sets published.
    #[must_use]
    pub fn publish_count(&self) -> usize {
        self.published.len()
    }
}

impl MprisConnection for MockMprisClient {
    fn publish(&mut self, properties: &MprisProperties) {
        self.published.push(properties.clone());
    }

    fn refuse(&mut self, call: &MprisCall, refusal: MprisRefusal) {
        self.refused.push((call.clone(), refusal));
    }
}

/// zbus-backed MPRIS transport, available with the MPRIS feature.
pub mod zbus_transport;

#[cfg(test)]
mod tests;
