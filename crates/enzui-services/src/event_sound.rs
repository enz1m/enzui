//! Play a desktop event sound in one call.
//!
//! [`play`] finds the named sound in the sound theme
//! (`enzui_core::sound_theme`) and plays it once on a thread of its own,
//! marked as an event sound so the sound server applies the user's
//! event-sound volume. Use it for feedback the user expects to hear: a
//! message arrived (`message-new-instant`), a long job finished
//! (`complete`), a photo was taken (`camera-shutter`). Respect the app's own
//! "play sounds" setting before calling it.

use enzui_core::sound_theme::{FALLBACK_THEME, event_sound_path};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The longest an event sound may play before it is stopped.
pub const MAX_EVENT_SOUND: Duration = Duration::from_secs(10);

/// Why an event sound did not play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventSoundError {
    /// No theme has the sound, or the theme switched it off.
    NotFound(String),
    /// GStreamer could not play it; the text says why.
    Playback(String),
}

impl std::fmt::Display for EventSoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "no event sound named {name:?}"),
            Self::Playback(reason) => write!(f, "event sound playback failed: {reason}"),
        }
    }
}

impl std::error::Error for EventSoundError {}

/// Plays event sound `name` from the `freedesktop` theme, in the background.
///
/// # Errors
/// [`EventSoundError::NotFound`] when there is no such sound; playback
/// errors after that are logged under `enzui::sound`.
pub fn play(name: &str) -> Result<(), EventSoundError> {
    play_from_theme(name, FALLBACK_THEME)
}

/// Plays event sound `name` from `theme` (falling back through the themes it
/// inherits to `freedesktop`), in the background.
///
/// # Errors
/// [`EventSoundError::NotFound`] when there is no such sound.
pub fn play_from_theme(name: &str, theme: &str) -> Result<(), EventSoundError> {
    let path =
        event_sound_path(name, theme).ok_or_else(|| EventSoundError::NotFound(name.to_owned()))?;
    let spawned = std::thread::Builder::new()
        .name("enzui-event-sound".into())
        .spawn(move || {
            if let Err(error) = play_file(&path, None) {
                log::warn!(target: "enzui::sound", "{}: {error}", path.display());
            }
        });
    spawned
        .map(|_| ())
        .map_err(|error| EventSoundError::Playback(error.to_string()))
}

/// Plays the sound file at `path` to its end (at most [`MAX_EVENT_SOUND`]),
/// blocking: through the default audio output as an event sound, or through
/// the GStreamer element `sink` when given.
///
/// # Errors
/// The file could not be decoded or played.
pub fn play_file(path: &Path, sink: Option<&str>) -> Result<Duration, EventSoundError> {
    let fail = |reason: String| EventSoundError::Playback(reason);
    gst::init().map_err(|error| fail(error.to_string()))?;
    let uri = gst::glib::filename_to_uri(path, None).map_err(|error| fail(error.to_string()))?;
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", uri.as_str())
        .build()
        .map_err(|error| fail(error.to_string()))?;
    // No video; and the stream is an event sound, so the sound server files
    // it under the user's event-sound volume.
    playbin.set_property_from_str("flags", "audio");
    let output = match sink {
        // Kept in time, so a test sink plays for the sound's length too.
        Some(name) => gst::ElementFactory::make(name)
            .build()
            .ok()
            .inspect(|sink| {
                if sink.has_property("sync", None) {
                    sink.set_property("sync", true);
                }
            }),
        None => event_sink(),
    };
    if let Some(output) = output {
        playbin.set_property("audio-sink", &output);
    }
    let bus = playbin.bus().ok_or_else(|| fail("no bus".into()))?;
    let started = std::time::Instant::now();
    playbin
        .set_state(gst::State::Playing)
        .map_err(|error| fail(error.to_string()))?;
    let result = loop {
        let remaining = MAX_EVENT_SOUND.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break Ok(started.elapsed());
        }
        let timeout =
            gst::ClockTime::from_nseconds(u64::try_from(remaining.as_nanos()).unwrap_or(u64::MAX));
        let Some(message) =
            bus.timed_pop_filtered(timeout, &[gst::MessageType::Eos, gst::MessageType::Error])
        else {
            break Ok(started.elapsed());
        };
        match message.view() {
            gst::MessageView::Eos(_) => break Ok(started.elapsed()),
            gst::MessageView::Error(error) => break Err(fail(error.error().to_string())),
            _ => {}
        }
    };
    let _ = playbin.set_state(gst::State::Null);
    result
}

/// A PulseAudio or PipeWire sink tagged `media.role=event`, when the
/// system has one; otherwise playbin picks its own output.
fn event_sink() -> Option<gst::Element> {
    let sink = gst::ElementFactory::make("pulsesink").build().ok()?;
    let properties = gst::Structure::builder("props")
        .field("media.role", "event")
        .build();
    if sink.has_property("stream-properties", None) {
        sink.set_property("stream-properties", &properties);
    }
    Some(sink)
}

/// The event sound file `play` would use, for an app that plays it itself.
#[must_use]
pub fn sound_file(name: &str, theme: &str) -> Option<PathBuf> {
    event_sound_path(name, theme)
}
