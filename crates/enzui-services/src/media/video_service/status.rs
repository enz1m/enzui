//! Playback observations and requested settings reported by a video service.

use crate::media::{Capabilities, Generation, MediaError, MediaSource, PlaybackState, Snapshot};
use std::time::Duration;

/// One audio or subtitle track of the current source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// The index to select it by.
    pub index: i32,
    /// Container language code or language name, if provided.
    pub language: Option<String>,
    /// The track's own title ("Commentary", "SDH"), if any.
    pub title: Option<String>,
    /// The codec, as GStreamer describes it ("Opus", "AC-3").
    pub codec: Option<String>,
}

impl Track {
    /// A name for menus: the language and title when known ("English ·
    /// Commentary"), otherwise "Track 2".
    #[must_use]
    pub fn label(&self) -> String {
        let language = self
            .language
            .as_deref()
            .map(|code| language_name(code).map_or_else(|| code.to_uppercase(), str::to_owned));
        // Muxers name untitled tracks after their kind; that says nothing.
        let title = self.title.as_deref().map(str::trim).filter(|title| {
            !title.is_empty()
                && !GENERIC_TITLES
                    .iter()
                    .any(|generic| title.eq_ignore_ascii_case(generic))
        });
        match (language, title) {
            (Some(language), Some(title)) if !title.eq_ignore_ascii_case(&language) => {
                format!("{language} · {title}")
            }
            (Some(language), _) => language,
            (None, Some(title)) => title.to_owned(),
            (None, None) => format!("Track {}", self.index + 1),
        }
    }
}

const GENERIC_TITLES: &[&str] = &["audio", "sound", "video", "subtitle", "subtitles", "text"];

/// Subtitle text size, relative to the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SubtitleSize {
    /// Smaller than the default.
    Small,
    /// The default.
    #[default]
    Medium,
    /// Larger than the default.
    Large,
    /// For reading from across a room.
    ExtraLarge,
}

impl SubtitleSize {
    /// Every size, smallest first.
    pub const ALL: [SubtitleSize; 4] = [
        SubtitleSize::Small,
        SubtitleSize::Medium,
        SubtitleSize::Large,
        SubtitleSize::ExtraLarge,
    ];

    /// The size's name for menus.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SubtitleSize::Small => "Small",
            SubtitleSize::Medium => "Medium",
            SubtitleSize::Large => "Large",
            SubtitleSize::ExtraLarge => "Extra large",
        }
    }

    // The overlay scales these font sizes relative to the picture.
    pub(crate) fn font(self) -> &'static str {
        match self {
            SubtitleSize::Small => "Sans Bold 14",
            SubtitleSize::Medium => "Sans Bold 18",
            SubtitleSize::Large => "Sans Bold 23",
            SubtitleSize::ExtraLarge => "Sans Bold 30",
        }
    }
}

/// The latest observations and settings of a [`VideoService`](super::VideoService).
///
/// Loading and unloading reset this immediately. Most fields update on worker
/// commands or bus messages; position can be a requested seek target, and
/// playback remains Playing during buffering or a flushing seek when playing
/// was requested. These fields do not uniformly confirm pipeline completion.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoStatus {
    /// The generation this status describes.
    pub generation: Generation,
    /// The source's display name (its file name), if one is loaded.
    pub source_name: Option<String>,
    /// The title the file's tags declare, if any.
    pub title: Option<String>,
    /// Whether the source is remote.
    pub remote: bool,
    /// Initial preroll or the start-position seek has not completed.
    pub loading: bool,
    /// Reported playback state, including requested Playing while buffering or seeking.
    pub playback: PlaybackState,
    /// Last queried position or accepted seek target.
    pub position: Duration,
    /// The total duration, `None` when unknown (a live stream).
    pub duration: Option<Duration>,
    /// Whether the source can seek.
    pub seekable: bool,
    /// Whether the source has a video track.
    pub has_video: bool,
    /// Whether the source has an audio track.
    pub has_audio: bool,
    /// The picture's display size (square pixels), as the source declares it.
    pub natural_size: Option<(u32, u32)>,
    /// Frames per second, when the source declares it.
    pub frame_rate: Option<f64>,
    /// The video codec, as GStreamer describes it.
    pub video_codec: Option<String>,
    /// The audio tracks.
    pub audio_tracks: Vec<Track>,
    /// The selected audio track's index.
    pub current_audio: Option<i32>,
    /// The embedded subtitle tracks.
    pub subtitle_tracks: Vec<Track>,
    /// The shown embedded subtitle track's index; `None` when subtitles are
    /// off or a subtitle file is shown instead.
    pub current_subtitle: Option<i32>,
    /// The subtitle file shown, if any.
    pub subtitle_file: Option<std::path::PathBuf>,
    /// The subtitle text size.
    pub subtitle_size: SubtitleSize,
    /// How far subtitles are shifted; positive is later.
    pub subtitle_delay_ms: i64,
    /// Slider volume; load options must be finite to keep it in `0.0..=1.0`.
    pub volume: f64,
    /// Whether audio is muted.
    pub muted: bool,
    /// The confirmed playback rate.
    pub rate: f64,
    /// Buffering progress below 100 percent; None once buffering completes.
    pub buffering: Option<u8>,
    /// Confirmed native unread disk retention; absent until adopted or when disabled.
    pub read_ahead: Option<super::ReadAheadStatus>,
    /// Why the source cannot play, once it cannot.
    pub error: Option<MediaError>,
    /// Decoders GStreamer lacks for streams of this source ("H.264 (High
    /// Profile) decoder"). A source can play without one of its streams: a
    /// missing video decoder leaves the sound, and the UI should say why
    /// there is no picture rather than show none.
    pub missing_decoders: Vec<String>,
}

impl Default for VideoStatus {
    fn default() -> Self {
        Self::empty(Generation::FIRST)
    }
}

impl VideoStatus {
    /// No source.
    #[must_use]
    pub fn empty(generation: Generation) -> Self {
        Self {
            generation,
            source_name: None,
            title: None,
            remote: false,
            loading: false,
            playback: PlaybackState::Stopped,
            position: Duration::ZERO,
            duration: None,
            seekable: false,
            has_video: false,
            has_audio: false,
            natural_size: None,
            frame_rate: None,
            video_codec: None,
            audio_tracks: Vec::new(),
            current_audio: None,
            subtitle_tracks: Vec::new(),
            current_subtitle: None,
            subtitle_file: None,
            subtitle_size: SubtitleSize::Medium,
            subtitle_delay_ms: 0,
            volume: 1.0,
            muted: false,
            rate: 1.0,
            buffering: None,
            read_ahead: None,
            error: None,
            missing_decoders: Vec::new(),
        }
    }

    /// A source that is opening.
    #[must_use]
    pub fn loading(generation: Generation, source_name: Option<String>, remote: bool) -> Self {
        Self {
            source_name,
            remote,
            loading: true,
            ..Self::empty(generation)
        }
    }

    /// Whether a source is loaded or opening.
    #[must_use]
    pub fn has_source(&self) -> bool {
        self.loading || self.source_name.is_some()
    }

    /// Whether the picture is moving (or about to: buffering while playing).
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playback == PlaybackState::Playing
    }

    /// The best name for the source: its tagged title, else its file name.
    #[must_use]
    pub fn display_title(&self) -> Option<&str> {
        self.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .or(self.source_name.as_deref())
    }

    /// The selected audio track.
    #[must_use]
    pub fn audio_track(&self) -> Option<&Track> {
        let index = self.current_audio?;
        self.audio_tracks.iter().find(|track| track.index == index)
    }

    /// The shown embedded subtitle track.
    #[must_use]
    pub fn subtitle_track(&self) -> Option<&Track> {
        let index = self.current_subtitle?;
        self.subtitle_tracks
            .iter()
            .find(|track| track.index == index)
    }

    /// The status as a media [`Snapshot`], for [`MediaState`]-driven widgets
    /// such as the media controls.
    ///
    /// [`MediaState`]: crate::media::MediaState
    #[must_use]
    pub fn snapshot(&self, source: Option<MediaSource>) -> Snapshot {
        Snapshot {
            generation: self.generation,
            source,
            capabilities: Capabilities {
                seekable: self.seekable,
                has_audio: self.has_audio,
                has_video: self.has_video,
            },
            playback: self.playback,
            position: self.position,
            duration: self.duration,
            volume: self.volume,
            muted: self.muted,
            rate: self.rate,
            error: self.error.clone(),
        }
    }
}

/// The English name of an ISO 639-1 or 639-2 language code, for the codes
/// in the lookup table; `None` for unknown codes. Trims whitespace, ignores
/// ASCII case, and removes region subtags separated by `-` or `_`.
#[must_use]
pub fn language_name(code: &str) -> Option<&'static str> {
    let code = code.trim().to_ascii_lowercase();
    // Region subtags ("en-US", "pt_BR") name the same language.
    let code = code.split(['-', '_']).next().unwrap_or_default();
    LANGUAGES
        .iter()
        .find(|(two, three, _)| *two == code || three.contains(&code))
        .map(|(_, _, name)| *name)
}

// ISO 639-1, ISO 639-2 aliases, and English name.
const LANGUAGES: &[(&str, &[&str], &str)] = &[
    ("ar", &["ara"], "Arabic"),
    ("bg", &["bul"], "Bulgarian"),
    ("ca", &["cat"], "Catalan"),
    ("cs", &["cze", "ces"], "Czech"),
    ("da", &["dan"], "Danish"),
    ("de", &["ger", "deu"], "German"),
    ("el", &["gre", "ell"], "Greek"),
    ("en", &["eng"], "English"),
    ("es", &["spa"], "Spanish"),
    ("et", &["est"], "Estonian"),
    ("fa", &["per", "fas"], "Persian"),
    ("fi", &["fin"], "Finnish"),
    ("fr", &["fre", "fra"], "French"),
    ("ga", &["gle"], "Irish"),
    ("he", &["heb"], "Hebrew"),
    ("hi", &["hin"], "Hindi"),
    ("hr", &["hrv"], "Croatian"),
    ("hu", &["hun"], "Hungarian"),
    ("id", &["ind"], "Indonesian"),
    ("is", &["ice", "isl"], "Icelandic"),
    ("it", &["ita"], "Italian"),
    ("ja", &["jpn"], "Japanese"),
    ("ko", &["kor"], "Korean"),
    ("lt", &["lit"], "Lithuanian"),
    ("lv", &["lav"], "Latvian"),
    ("ms", &["may", "msa"], "Malay"),
    ("nb", &["nob"], "Norwegian Bokmål"),
    ("nl", &["dut", "nld"], "Dutch"),
    ("no", &["nor"], "Norwegian"),
    ("pl", &["pol"], "Polish"),
    ("pt", &["por"], "Portuguese"),
    ("ro", &["rum", "ron"], "Romanian"),
    ("ru", &["rus"], "Russian"),
    ("sk", &["slo", "slk"], "Slovak"),
    ("sl", &["slv"], "Slovenian"),
    ("sr", &["srp"], "Serbian"),
    ("sv", &["swe"], "Swedish"),
    ("th", &["tha"], "Thai"),
    ("tr", &["tur"], "Turkish"),
    ("uk", &["ukr"], "Ukrainian"),
    ("vi", &["vie"], "Vietnamese"),
    ("zh", &["chi", "zho"], "Chinese"),
];
