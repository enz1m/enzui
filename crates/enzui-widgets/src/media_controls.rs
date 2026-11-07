//! Media transport controls driven by a confirmed [`Snapshot`].
//!
//! The host dispatches [`MediaControlsMessage`] values to its reducer or backend.
//! [`crate::video`] embeds this bar; it can also accompany
//! a host-owned media surface. Controls wrap when their measured minimum width does
//! not fit. Control IDs derive from a stable transport namespace and semantic keys.
//!
//! The scrubber requires a known duration and seekability; volume and mute require
//! audio. These gates omit controls entirely. Play/pause remains available, and
//! errors appear above the controls. [`EnzMediaControls::playlist_controls`] adds
//! shuffle, previous, next and repeat; shuffle and repeat expose selected state
//! to accessibility clients. Transport controls are quiet icon buttons in
//! primary ink, play/pause the largest, sized from [`ButtonSize::Sm`] by
//! default; [`EnzMediaControls::transport_button_size`] scales them up for a
//! full player panel.
//!
//! [`TimeDisplay`] chooses the readout independently of seekability:
//!
//! | Mode | Known duration | Missing duration |
//! |---|---|---|
//! | [`TimeDisplay::Auto`] (default) | elapsed / total | `● LIVE` |
//! | [`TimeDisplay::Elapsed`] | elapsed / total | no readout |
//! | [`TimeDisplay::Live`] | `● LIVE` | `● LIVE` |
//! | [`TimeDisplay::Hidden`] | no readout | no readout |
//!
//! Use `Elapsed` when a missing duration means unknown rather than live. Times
//! truncate whole seconds and use `M:SS` or `H:MM:SS`; displayed positions and seek
//! values clamp to the duration. Hiding the readout leaves scrubber and audio gates
//! unchanged; an otherwise empty transport row is omitted.
//!
//! Muting emits [`MediaControlsMessage::ToggleMute`] without changing volume.
//! Muted and zero-volume states have distinct icons. The widget uses icon-font
//! names; [`volume_icon_text`] and [`playback_icon_text`] provide text alternatives.
//!
//! The default flat fill follows the runtime theme unless tokens are pinned.
//! The outer border is invisible by default. [`EnzMediaControls::material`] uses
//! a sampled backdrop with the surface's opaque accessibility/renderer fallback.

use crate::button::ButtonSize;
use crate::enz_text::enz_text;
use crate::slider::{SliderTokens, enz_slider};
use enzui_core::identity::derive_id;
use enzui_core::media::{PlaybackState, RepeatMode, Snapshot};
use enzui_theme::{MaterialPolicy, MaterialStyle, Palette};
use iced::widget::{column, container, row, space};
use iced::{Color, Element, Length, Padding, Theme};
use std::time::Duration;

/// Palette-derived colors for the media controls bar.
#[derive(Debug, Clone, Copy)]
pub struct MediaControlsTokens {
    /// Background of the control bar (flat fallback for glass).
    pub background: Color,
    /// Primary text/icon colour.
    pub text_color: Color,
    /// Muted/secondary text colour (elapsed/total time).
    pub text_muted: Color,
    /// Accent colour for the active play/pause icon.
    pub accent: Color,
    /// Scrubber track (unfilled) colour.
    pub track_color: Color,
    /// Scrubber fill colour.
    pub fill_color: Color,
    /// Error text colour.
    pub error_color: Color,
    /// Error background colour.
    pub error_bg: Color,
    /// Border colour.
    pub border_color: Color,
}

impl MediaControlsTokens {
    /// Derive tokens from a theme palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_elevated,
            text_color: p.text,
            text_muted: p.text_muted,
            accent: p.accent,
            // The scrub/volume track is an interactive extent, so it takes the 3:1 control border.
            track_color: p.control_border,
            fill_color: p.accent,
            error_color: p.danger,
            error_bg: p.danger_soft,
            border_color: p.control_border,
        }
    }

    /// Tokens for dark theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Tokens for light theme.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Messages emitted by the media controls widget.
///
/// The app's `update` dispatches these as [`MediaCommand`](enzui_core::media::MediaCommand)s
/// to the reducer or the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaControlsMessage {
    /// Toggle play/pause.
    PlayPause,
    /// Seek to a position (only emitted when seekable + finite duration).
    Seek(Duration),
    /// Set volume (0.0–1.0).
    SetVolume(f64),
    /// Toggle mute.
    ToggleMute,
    /// Skip to the previous track (only emitted when playlist controls are
    /// enabled).
    PreviousTrack,
    /// Skip to the next track (only emitted when playlist controls are
    /// enabled).
    NextTrack,
    /// Toggle shuffle mode (only emitted when playlist controls are enabled).
    ToggleShuffle,
    /// Cycle the repeat mode (only emitted when playlist controls are
    /// enabled).
    CycleRepeat,
}

/// Playlist state enabling shuffle, previous, play/pause, next and repeat controls.
///
/// The default disables shuffle and repeat. Playlist controls are opt-in and are not
/// inferred from source capabilities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaylistTransport {
    /// Whether shuffle is currently active.
    pub shuffle: bool,
    /// The current repeat mode.
    pub repeat_mode: RepeatMode,
}

impl Default for PlaylistTransport {
    fn default() -> Self {
        Self {
            shuffle: false,
            repeat_mode: RepeatMode::Off,
        }
    }
}

/// Choose the time readout independently of seeking capabilities.
///
/// The default is [`TimeDisplay::Auto`]. Use `Elapsed` when missing duration means
/// unknown rather than live, or `Hidden` when the host owns the readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeDisplay {
    /// Infer from `duration`: `Some(d)` → elapsed / total, `None` → `● LIVE`.
    #[default]
    Auto,
    /// Show elapsed / total when duration is known; omit the readout otherwise.
    Elapsed,
    /// Show `● LIVE` even when duration is known.
    Live,
    /// Omit the readout while retaining scrubber and audio controls.
    Hidden,
}

impl TimeDisplay {
    /// Whether the elapsed/total pair should be rendered for `duration`.
    #[must_use]
    pub fn shows_elapsed(self, duration: Option<Duration>) -> bool {
        match self {
            TimeDisplay::Auto | TimeDisplay::Elapsed => duration.is_some(),
            TimeDisplay::Live | TimeDisplay::Hidden => false,
        }
    }

    /// Whether the `● LIVE` caption should be rendered for `duration`.
    #[must_use]
    pub fn shows_live(self, duration: Option<Duration>) -> bool {
        match self {
            TimeDisplay::Auto => duration.is_none(),
            TimeDisplay::Live => true,
            TimeDisplay::Elapsed | TimeDisplay::Hidden => false,
        }
    }
}

/// A media transport control bar driven by a [`Snapshot`].
///
/// Controls wrap when the allocated width cannot fit the timestamps and usable ranges.
/// Construct with [`enz_media_controls`], optionally override tokens.
#[derive(Clone)]
pub struct EnzMediaControls {
    id_prefix: String,
    snapshot: Snapshot,
    tokens: Option<MediaControlsTokens>,
    playlist: Option<PlaylistTransport>,
    transport_button_size: ButtonSize,
    time_display: TimeDisplay,
    bordered: bool,
    material: Option<MaterialStyle>,
    material_policy: Option<MaterialPolicy>,
}

impl EnzMediaControls {
    /// Pin visual tokens instead of resolving the runtime theme.
    #[must_use]
    pub fn tokens(mut self, tokens: MediaControlsTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Enable the five-button playlist row using the supplied shuffle and repeat state.
    #[must_use]
    pub fn playlist_controls(mut self, transport: PlaylistTransport) -> Self {
        self.playlist = Some(transport);
        self
    }

    /// Select play/pause and playlist-button geometry; defaults to [`ButtonSize::Sm`].
    ///
    /// Audio mute remains compact so its volume cluster does not grow with the
    /// primary transport.
    #[must_use]
    pub fn transport_button_size(mut self, size: ButtonSize) -> Self {
        self.transport_button_size = size;
        self
    }

    /// Show the flat bar’s one-pixel outer border; disabled by default.
    ///
    /// Toggling visibility retains the border slot and eight-pixel corner radius.
    /// This setting has no effect on a material bar.
    #[must_use]
    pub fn bordered(mut self, on: bool) -> Self {
        self.bordered = on;
        self
    }

    /// Replace the flat fill with a sampled-backdrop surface.
    ///
    /// The surface supplies an opaque fallback when sampling is unavailable or its
    /// accessibility policy requires one. Padding and radius are retained.
    #[must_use]
    pub fn material(mut self, style: MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Override the material surface’s accessibility policy.
    ///
    /// Without an override, material surfaces follow the live policy.
    #[must_use]
    pub fn material_policy(mut self, policy: MaterialPolicy) -> Self {
        self.material_policy = Some(policy);
        self
    }

    /// Choose the time readout; defaults to [`TimeDisplay::Auto`].
    #[must_use]
    pub fn time_display(mut self, display: TimeDisplay) -> Self {
        self.time_display = display;
        self
    }
}

/// Construct transport controls with a stable owner namespace.
///
/// Use a domain identity rather than a rendered index: child IDs derive from this
/// namespace and semantic keys, preserving focus targets across surrounding reorders.
#[must_use]
pub fn enz_media_controls(id_prefix: impl Into<String>, snapshot: Snapshot) -> EnzMediaControls {
    EnzMediaControls {
        id_prefix: id_prefix.into(),
        snapshot,
        tokens: None,
        playlist: None,
        transport_button_size: ButtonSize::Sm,
        time_display: TimeDisplay::Auto,
        bordered: false,
        material: None,
        material_policy: None,
    }
}

const BAR_RADIUS: f32 = 8.0;

fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

/// Compute the scrubber fraction, clamping position to duration.
///
/// Returns `None` without a known duration or seekability, and zero for a zero duration.
pub fn scrubber_fraction(snap: &Snapshot) -> Option<f32> {
    if !snap.capabilities.seekable {
        return None;
    }
    let dur = snap.duration?;
    if dur.is_zero() {
        return Some(0.0);
    }
    let pos = snap.position.min(dur);
    Some((pos.as_secs_f64() / dur.as_secs_f64()) as f32)
}

/// Choose the icon-font name for the volume level (non-muted).
///
/// The widget calls this to select among the three Lucide volume icons:
///
/// | volume | icon name | visual |
/// |---|---|---|
/// | `<= 0.0` | `"volume"` | speaker cone, no sound waves |
/// | `0.0 < v < 0.5` | `"volume-1"` | speaker cone + one arc |
/// | `>= 0.5` | `"volume-2"` | speaker cone + two arcs |
///
/// For the muted state (`volume-x`), the widget handles that case directly
/// before calling this function.
pub fn volume_icon_name(volume: f64) -> &'static str {
    if volume <= 0.0 {
        "volume"
    } else if volume < 0.5 {
        "volume-1"
    } else {
        "volume-2"
    }
}

/// Choose the volume icon text (emoji) based on mute and volume state.
///
/// Text alternative to [`volume_icon_name`]; the widget uses icon-font names.
///
/// `muted` and `volume == 0.0` are distinct states with distinct icons.
pub fn volume_icon_text(muted: bool, volume: f64) -> &'static str {
    if muted {
        "🔇"
    } else if volume <= 0.0 {
        "🔈"
    } else if volume < 0.5 {
        "🔉"
    } else {
        "🔊"
    }
}

/// Choose the play/pause icon text based on playback state.
pub fn playback_icon_text(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Playing => "⏸",
        _ => "▶",
    }
}

/// Whether the scrubber should be shown.
///
/// The scrubber requires both a finite duration and seekability.
/// A live stream (`duration: None`) or a non-seekable source hides it.
pub fn should_show_scrubber(snap: &Snapshot) -> bool {
    snap.capabilities.seekable && snap.duration.is_some()
}

/// Whether audio controls (volume/mute) should be shown.
pub fn should_show_audio(snap: &Snapshot) -> bool {
    snap.capabilities.has_audio
}

/// Choose the repeat button’s accessible name for the given mode.
pub fn repeat_label(mode: RepeatMode) -> String {
    match mode {
        RepeatMode::Off => enzui_core::localization::text("enzui-media-controls-repeat"),
        RepeatMode::All => enzui_core::localization::text("enzui-media-controls-repeat-all"),
        RepeatMode::One => enzui_core::localization::text("enzui-media-controls-repeat-one"),
    }
}

/// Choose the repeat button’s icon name for the given mode.
pub fn repeat_icon_name(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::One => "repeat-1",
        _ => "repeat",
    }
}

/// Choose the shuffle button’s accessible name.
pub fn shuffle_label(shuffle_on: bool) -> String {
    if shuffle_on {
        enzui_core::localization::text("enzui-media-controls-shuffle-on")
    } else {
        enzui_core::localization::text("enzui-media-controls-shuffle")
    }
}

fn resolve_tokens(theme: &Theme, pinned: Option<MediaControlsTokens>) -> MediaControlsTokens {
    crate::theme_lookup::resolve_pinned(
        crate::theme_lookup::PIN_SITE_MEDIA_CONTROLS,
        pinned,
        theme,
        MediaControlsTokens::from_palette,
    )
}

fn slider_tokens(tokens: MediaControlsTokens) -> SliderTokens {
    // Mirror the theme’s interaction alphas using the pinned palette’s foreground.
    let hover_wash = Color {
        a: 0.07,
        ..tokens.text_color
    };
    let pressed_wash = Color {
        a: 0.12,
        ..tokens.text_color
    };
    SliderTokens {
        track: tokens.track_color,
        track_border: tokens.track_color,
        fill: tokens.fill_color,
        fill_hover: crate::surface_interaction::composite(tokens.fill_color, hover_wash),
        thumb: tokens.background,
        thumb_border: tokens.border_color,
        thumb_shadow: Color::TRANSPARENT,
        thumb_dragging: crate::surface_interaction::composite(tokens.background, pressed_wash),
        text_color: tokens.text_color,
        value_text_color: tokens.text_muted,
        focus_ring: tokens.accent,
        ..SliderTokens::dark()
    }
}

/// Toolbar-icon colours from pinned media tokens: primary glyph ink and
/// washes of it.
fn icon_tokens(tokens: MediaControlsTokens) -> crate::toolbar_icon::ToolbarIconTokens {
    let wash = |a: f32| Color {
        a,
        ..tokens.text_color
    };
    crate::toolbar_icon::ToolbarIconTokens {
        icon_color: tokens.text_color,
        hover_color: tokens.text_color,
        pressed_color: tokens.text_color,
        active_color: tokens.text_color,
        disabled_color: tokens.text_color,
        active_background: wash(0.14),
        hover_background: wash(0.07),
        pressed_background: wash(0.12),
        focus_ring: tokens.accent,
        ..crate::toolbar_icon::ToolbarIconTokens::dark()
    }
}

/// One icon-only transport control: a quiet circle with a primary-ink
/// glyph, as Apple Music, GNOME's players and Spotify draw them. `toggled`
/// gives a shuffle or repeat its on state.
#[allow(
    clippy::too_many_arguments,
    reason = "one call shape for every transport control"
)]
fn transport_icon<'a>(
    icon: &str,
    label: String,
    message: MediaControlsMessage,
    id: iced::advanced::widget::Id,
    control: f32,
    glyph: f32,
    toggled: Option<bool>,
    pinned: Option<MediaControlsTokens>,
) -> Element<'a, MediaControlsMessage, Theme> {
    let mut button = crate::toolbar_icon::enz_toolbar_icon(icon, message)
        .variant(crate::toolbar_icon::ToolbarIconVariant::Primary)
        .aria_label(label)
        .id(id);
    if let Some(on) = toggled {
        button = button.active(on);
    }
    if let Some(tokens) = pinned {
        button = button.tokens(icon_tokens(tokens));
    }
    button.size(control, glyph).into_element()
}

/// The control and glyph sizes of the transport icons for a button size:
/// `(control, glyph)` for the secondary icons, then for play/pause, the
/// one that leads.
fn transport_sizes(size: ButtonSize) -> ((f32, f32), (f32, f32)) {
    let height = size.tokens().height;
    (
        (height + 6.0, (height * 0.66).round()),
        (height + 12.0, (height * 0.86).round()),
    )
}

/// The bar's inset. The trailing side is wider: the volume knob reaches the
/// end of its track, while the leading play button carries its own inset.
fn bar_padding() -> Padding {
    let (leading, trailing) = (8.0, 16.0);
    let rtl =
        enzui_core::placement::ambient_direction() == enzui_core::placement::Direction::RightToLeft;
    Padding {
        top: 8.0,
        bottom: 8.0,
        left: if rtl { trailing } else { leading },
        right: if rtl { leading } else { trailing },
    }
}

/// The volume slider's width in logical pixels: a level control, not a
/// second scrubber.
pub const VOLUME_WIDTH: f32 = 96.0;

/// Seek and volume track thickness: the thin rail media players use.
pub const MEDIA_TRACK_HEIGHT: f32 = 4.0;

/// Seek and volume knob diameter; it swells while hovered or dragged.
pub const MEDIA_THUMB: f32 = 14.0;

impl EnzMediaControls {
    /// Build the transport element.
    #[must_use]
    pub fn into_element<'a>(self) -> Element<'a, MediaControlsMessage, Theme> {
        Element::from(self)
    }
}

impl EnzMediaControls {
    fn minimum_transport_width(&self) -> f32 {
        use crate::text_measure::{MeasureTypography, enz_text_measure_width};
        let typo = MeasureTypography {
            size_px: Some(12.0),
            ..Default::default()
        };
        let mut extents = Vec::new();
        if self.playlist.is_none() {
            extents.push(transport_sizes(self.transport_button_size).1.0);
        }
        if self.time_display.shows_elapsed(self.snapshot.duration) {
            let duration = self
                .snapshot
                .duration
                .expect("elapsed display requires duration");
            extents.push(enz_text_measure_width(
                &format_duration(self.snapshot.position.min(duration)),
                typo,
            ));
            extents.push(enz_text_measure_width(&format_duration(duration), typo));
        } else if self.time_display.shows_live(self.snapshot.duration) {
            extents.push(enz_text_measure_width(
                &enzui_core::localization::text("enzui-media-live"),
                typo,
            ));
        }
        if should_show_scrubber(&self.snapshot) {
            extents.push(48.0_f32.max(enz_text_measure_width(
                &enzui_core::localization::text("enzui-media-controls-seek"),
                typo,
            )));
        }
        if should_show_audio(&self.snapshot) {
            extents.push(transport_sizes(self.transport_button_size).0.0);
            extents.push(48.0_f32.max(enz_text_measure_width(
                &enzui_core::localization::text("enzui-media-controls-volume"),
                typo,
            )));
        }
        let transport =
            extents.iter().sum::<f32>() + 8.0 * extents.len().saturating_sub(1) as f32 + 16.0;
        let playlist = if self.playlist.is_some() {
            let ((icon, _), (play, _)) = transport_sizes(self.transport_button_size);
            4.0 * icon + play + 4.0 * 12.0 + 16.0
        } else {
            0.0
        };
        transport.max(playlist)
    }
    fn build<'a>(self, compact: bool) -> Element<'a, MediaControlsMessage, Theme> {
        let mc = self;
        let snap = mc.snapshot;
        let pinned = mc.tokens;
        let playlist = mc.playlist;
        let transport_button_size = mc.transport_button_size;
        let time_display = mc.time_display;
        let bordered = mc.bordered;
        let material = mc.material;
        let material_policy = mc.material_policy;
        let show_elapsed = time_display.shows_elapsed(snap.duration);
        let show_live = time_display.shows_live(snap.duration);
        let play_id = derive_id(&mc.id_prefix, "play-pause");
        let seek_id = derive_id(&mc.id_prefix, "seek");
        let mute_id = derive_id(&mc.id_prefix, "mute");
        let volume_id = derive_id(&mc.id_prefix, "volume");

        let mut col = column![].spacing(4).width(Length::Fill);
        if let Some(err) = snap.error.as_ref() {
            let error_text = enz_text(format!("⚠ {err}"))
                .size_px(enzui_theme::text_scale::ui_px(12.0))
                .color_resolved(move |theme: &Theme| resolve_tokens(theme, pinned).error_color)
                .into_element();
            let err_row = container(error_text)
                .padding(Padding::new(4.0))
                .style(move |theme: &Theme| {
                    let tokens = resolve_tokens(theme, pinned);
                    container::Style {
                        background: Some(iced::Background::Color(tokens.error_bg)),
                        border: iced::Border {
                            color: tokens.error_color,
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }
                })
                .width(Length::Fill);
            col = col.push(err_row);
        }
        let (play_name, play_icon) = if snap.playback == PlaybackState::Playing {
            (
                enzui_core::localization::text("enzui-media-controls-pause"),
                "pause.fill",
            )
        } else {
            (
                enzui_core::localization::text("enzui-media-controls-play"),
                "play.fill",
            )
        };
        // Play/pause leads with a larger glyph; every transport control is a
        // quiet icon in primary ink that washes only on hover.
        let ((icon_control, icon_glyph), (play_control, play_glyph)) =
            transport_sizes(transport_button_size);
        let mut transport = row![].spacing(8).align_y(iced::Alignment::Center);
        let play_btn = transport_icon(
            play_icon,
            play_name,
            MediaControlsMessage::PlayPause,
            play_id,
            play_control,
            play_glyph,
            None,
            pinned,
        );
        if let Some(pt) = playlist {
            let secondary = |icon: &str,
                             label: String,
                             message: MediaControlsMessage,
                             key: &str,
                             toggled: Option<bool>| {
                transport_icon(
                    icon,
                    label,
                    message,
                    derive_id(&mc.id_prefix, key),
                    icon_control,
                    icon_glyph,
                    toggled,
                    pinned,
                )
            };
            let playlist_row = row![
                secondary(
                    "shuffle",
                    shuffle_label(pt.shuffle),
                    MediaControlsMessage::ToggleShuffle,
                    "shuffle",
                    Some(pt.shuffle),
                ),
                secondary(
                    "skip-back.fill",
                    enzui_core::localization::text("enzui-media-controls-previous"),
                    MediaControlsMessage::PreviousTrack,
                    "previous",
                    None,
                ),
                play_btn,
                secondary(
                    "skip-forward.fill",
                    enzui_core::localization::text("enzui-media-controls-next"),
                    MediaControlsMessage::NextTrack,
                    "next",
                    None,
                ),
                secondary(
                    repeat_icon_name(pt.repeat_mode),
                    repeat_label(pt.repeat_mode),
                    MediaControlsMessage::CycleRepeat,
                    "repeat",
                    Some(pt.repeat_mode.is_active()),
                ),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center);

            let playlist_row: Element<'a, MediaControlsMessage, Theme> = if compact {
                playlist_row.wrap().into()
            } else {
                playlist_row.into()
            };
            col = col.push(container(playlist_row).center_x(Length::Fill));
        } else {
            transport = transport.push(play_btn);
        }

        if show_elapsed {
            let duration = snap
                .duration
                .expect("guarded by TimeDisplay::shows_elapsed");
            let elapsed = enz_text(format_duration(snap.position.min(duration)))
                .size_px(enzui_theme::text_scale::ui_px(12.0))
                .tabular_figures(true)
                .color_resolved(move |theme: &Theme| resolve_tokens(theme, pinned).text_muted)
                .into_element();
            transport = transport.push(elapsed);
        } else if show_live {
            let live_label = enz_text(enzui_core::localization::text("enzui-media-live"))
                .size_px(enzui_theme::text_scale::ui_px(12.0))
                .color_resolved(move |theme: &Theme| resolve_tokens(theme, pinned).error_color)
                .into_element();
            transport = transport.push(live_label);
        }

        if should_show_scrubber(&snap) {
            let duration = snap.duration.expect("guarded by should_show_scrubber");
            let max_seconds = duration.as_secs_f32();
            let position_seconds = snap.position.min(duration).as_secs_f32();
            let mut seek = enz_slider(0.0..=max_seconds, position_seconds, move |seconds| {
                MediaControlsMessage::Seek(Duration::from_secs_f32(seconds))
            })
            .step(1.0)
            // Named for assistive technology; the times either side say
            // what the bar is, as every player shows it.
            .aria_label(&enzui_core::localization::text("enzui-media-controls-seek"))
            // Spoken as a media time, the way the readout shows it.
            .value_format(|seconds| format_duration(Duration::from_secs_f32(seconds.max(0.0))))
            .track_height(MEDIA_TRACK_HEIGHT)
            .thumb_width(MEDIA_THUMB)
            .thumb_height(MEDIA_THUMB)
            .id(seek_id);
            if let Some(tokens) = pinned {
                seek = seek.tokens(slider_tokens(tokens));
            }
            transport = transport.push(seek.into_element());
        } else if should_show_audio(&snap) && !compact {
            // With no seek track, keep the volume cluster at the trailing edge.
            // If there is no trailing cluster either, a filler would turn an
            // otherwise shrink-wrapped row into a leading-aligned full-width
            // row and defeat the centring below.
            transport = transport.push(space::horizontal());
        }

        if show_elapsed {
            let duration = snap
                .duration
                .expect("guarded by TimeDisplay::shows_elapsed");
            let total = enz_text(format_duration(duration))
                .size_px(enzui_theme::text_scale::ui_px(12.0))
                .tabular_figures(true)
                .color_resolved(move |theme: &Theme| resolve_tokens(theme, pinned).text_muted)
                .into_element();
            transport = transport.push(total);
        }

        if should_show_audio(&snap) {
            let (mute_name, mute_icon) = if snap.muted {
                (
                    enzui_core::localization::text("enzui-media-controls-unmute"),
                    "volume-x",
                )
            } else {
                (
                    enzui_core::localization::text("enzui-media-controls-mute"),
                    volume_icon_name(snap.volume),
                )
            };
            transport = transport.push(transport_icon(
                mute_icon,
                mute_name,
                MediaControlsMessage::ToggleMute,
                mute_id,
                icon_control,
                icon_glyph,
                None,
                pinned,
            ));

            let mut volume = enz_slider(0.0..=1.0, snap.volume as f32, |value| {
                MediaControlsMessage::SetVolume(f64::from(value))
            })
            .step(0.05)
            // Volume reads as continuous: keep the keyboard step, not the
            // twenty stop dots it would paint along the rail.
            .step_marks(false)
            .aria_label(&enzui_core::localization::text(
                "enzui-media-controls-volume",
            ))
            .track_height(MEDIA_TRACK_HEIGHT)
            .thumb_width(MEDIA_THUMB)
            .thumb_height(MEDIA_THUMB)
            .id(volume_id);
            if let Some(tokens) = pinned {
                volume = volume.tokens(slider_tokens(tokens));
            }
            // A short level control beside the speaker; the seek bar takes
            // the width.
            transport =
                transport.push(container(volume.into_element()).width(Length::Fixed(VOLUME_WIDTH)));
        }

        // Empty rows must not add padding below playlist-only controls.
        let row_has_content = playlist.is_none()
            || show_elapsed
            || show_live
            || should_show_scrubber(&snap)
            || should_show_audio(&snap);
        if row_has_content {
            let transport: Element<'a, MediaControlsMessage, Theme> = if compact {
                transport.wrap().into()
            } else {
                transport.into()
            };
            col = col.push(container(transport).center_x(Length::Fill));
        }

        if let Some(style) = material {
            let mut surface = crate::surface::enz_surface(col)
                .material(style)
                .padding(bar_padding())
                .radius(BAR_RADIUS)
                .width(Length::Fill);
            if let Some(policy) = material_policy {
                surface = surface.material_policy(policy);
            }
            return surface.into();
        }

        // Resolve unpinned colors at draw time so retained elements follow theme changes.
        container(col)
            .padding(bar_padding())
            .width(Length::Fill)
            .style(move |theme: &Theme| {
                let tokens = resolve_tokens(theme, pinned);
                container::Style {
                    background: Some(iced::Background::Color(tokens.background)),
                    border: iced::Border {
                        // Border visibility must not change geometry.
                        color: if bordered {
                            tokens.border_color
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        width: 1.0,
                        radius: BAR_RADIUS.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a> From<EnzMediaControls> for Element<'a, MediaControlsMessage, Theme> {
    fn from(mc: EnzMediaControls) -> Self {
        let minimum_width = mc.minimum_transport_width();
        iced::widget::responsive(move |size| mc.clone().build(size.width < minimum_width))
            .height(Length::Shrink)
            .into()
    }
}
