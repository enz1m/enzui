//! Avatars with initials, optional photos, loading sheen, failure markers and presence dots.
//!
//! The host owns image loading and supplies the media state. Named avatars
//! publish one Image node with presence and loading/failure annotations;
//! blank names are decorative. Tokens follow the live palette unless pinned.
//!
//! The host replaces the builder's media state as loading completes.

use crate::enz_text::enz_text;
use enzui_theme::Palette;
use iced::widget::{container, image, stack, text};
use iced::{Border, Color, ContentFit, Element, Length, Padding};

const DEFAULT_AVATAR_SIZE: f32 = 40.0;

const MAX_AVATAR_SIZE: f32 = 1024.0;

const DEFAULT_ERROR_ICON: &str = "image-off";

const ANNOTATION_LOADING: &str = "loading";
const ANNOTATION_UNAVAILABLE: &str = "image unavailable";

const SHIMMER_SHEEN_IDLE: f32 = 0.0;
const SHIMMER_SHEEN_CREST: f32 = 0.7;
const SHIMMER_SHEEN_STATIC: f32 = 0.35;
const SHIMMER_PERIOD_SECS: f32 = 1.8;

fn snap(v: f32) -> f32 {
    v.round()
}

fn normalize_size(size: f32) -> f32 {
    let s = if size.is_finite() && size > 0.0 {
        size.min(MAX_AVATAR_SIZE)
    } else {
        DEFAULT_AVATAR_SIZE
    };
    snap(s).max(1.0)
}

/// Avatar silhouette shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AvatarShape {
    /// Fully rounded circle (radius = size / 2).
    #[default]
    Circle,
    /// Rounded square with a 10 logical-pixel radius.
    RoundedSquare,
    /// Sharp square with no rounding.
    Square,
}

impl AvatarShape {
    /// Return the radius in logical pixels without normalizing `avatar_size`.
    pub fn radius(self, avatar_size: f32) -> f32 {
        match self {
            Self::Circle => avatar_size / 2.0,
            Self::RoundedSquare => 10.0,
            Self::Square => 0.0,
        }
    }
}

/// Presence/status indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    /// No status dot shown.
    #[default]
    None,
    /// Online status; default dot ink is green.
    Online,
    /// Away status; default dot ink is amber.
    Away,
    /// Busy or do-not-disturb status; default dot ink is red.
    Busy,
    /// Offline status; default dot ink uses muted text.
    Offline,
}

impl Presence {
    /// Lowercase status word for the accessible label, e.g.
    /// `"online"`. `None` maps to `""`.
    pub fn as_str(self) -> &'static str {
        match self {
            Presence::None => "",
            Presence::Online => "online",
            Presence::Away => "away",
            Presence::Busy => "busy",
            Presence::Offline => "offline",
        }
    }
}

/// Host-supplied photo state.
#[derive(Debug, Clone, Default)]
pub enum AvatarMedia {
    /// No photo — initials fallback.
    #[default]
    None,
    /// A decoded image handle to draw over the initials.
    Image(image::Handle),
    /// Loading in progress — render the shimmer skeleton.
    Loading,
    /// Load failed — initials fallback with an upper-right failure marker.
    Failed,
}

impl AvatarMedia {
    fn is_loading(&self) -> bool {
        matches!(self, AvatarMedia::Loading)
    }

    fn is_failed(&self) -> bool {
        matches!(self, AvatarMedia::Failed)
    }

    fn image_handle(&self) -> Option<&image::Handle> {
        match self {
            AvatarMedia::Image(h) => Some(h),
            _ => None,
        }
    }
}

/// Color tokens for avatars, derived from the palette.
#[derive(Debug, Clone, Copy)]
pub struct AvatarTokens {
    /// Background fill (behind initials).
    pub background: Color,
    /// Foreground (initials text color).
    pub foreground: Color,
    /// Online presence dot color.
    pub presence_online: Color,
    /// Away presence dot color.
    pub presence_away: Color,
    /// Busy presence dot color.
    pub presence_busy: Color,
    /// Offline presence dot color.
    pub presence_offline: Color,
    /// Ring color separating the presence dot from the silhouette. Defaults
    /// to the parent surface so the dot reads as sitting *on* the surface the
    /// avatar occupies. Override
    /// per-instance via `EnzAvatar::presence_ring_color` when the avatar sits on
    /// a non-default surface.
    pub presence_ring: Color,
    /// Loading-shimmer base fill.
    pub shimmer_base: Color,
    /// Loading sheen ink; its alpha is replaced by the resolved pulse opacity.
    pub shimmer_sheen: Color,
}

impl AvatarTokens {
    /// Resolve silhouette, initials and presence from the live semantic palette.
    pub fn from_palette(p: &Palette) -> Self {
        let fill = p.filled_color(p.accent);
        Self {
            background: fill.color,
            foreground: fill.text,
            presence_online: p.success,
            presence_away: p.warning,
            presence_busy: p.destructive,
            presence_offline: p.text_muted,
            presence_ring: p.surface,
            shimmer_base: p.surface_alt,
            shimmer_sheen: p.border,
        }
    }

    /// Convenience: dark-theme tokens.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: light-theme tokens.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Returns the dot color for a presence value.
    pub fn presence_color(&self, presence: Presence) -> Option<Color> {
        match presence {
            Presence::None => None,
            Presence::Online => Some(self.presence_online),
            Presence::Away => Some(self.presence_away),
            Presence::Busy => Some(self.presence_busy),
            Presence::Offline => Some(self.presence_offline),
        }
    }
}

/// Builder for an EnzAvatar element.
#[must_use]
pub struct EnzAvatar {
    label: String,
    initials: Option<String>,
    avatar_size: f32,
    shape: AvatarShape,
    presence: Presence,
    presence_ring_color: Option<Color>,
    tokens: Option<AvatarTokens>,
    media: AvatarMedia,
    shimmer_phase: Option<f32>,
    reduced_motion: Option<bool>,
    error_icon: String,
}

/// Create a 40 px circular avatar with derived initials, no photo and no presence.
/// The label supplies its accessible name; blank or whitespace-only labels
/// are decorative even if explicit initials or media are supplied.
pub fn enz_avatar(label: &str) -> EnzAvatar {
    EnzAvatar {
        label: label.to_owned(),
        initials: None,
        avatar_size: DEFAULT_AVATAR_SIZE,
        shape: AvatarShape::default(),
        presence: Presence::default(),
        presence_ring_color: None,
        tokens: None,
        media: AvatarMedia::default(),
        shimmer_phase: None,
        reduced_motion: None,
        error_icon: DEFAULT_ERROR_ICON.to_owned(),
    }
}

impl EnzAvatar {
    /// Override initials; an empty string restores label-derived initials.
    /// Derivation uppercases the first character of each of the first two words.
    /// Unicode uppercasing can yield more than two characters.
    pub fn initials(mut self, initials: &str) -> Self {
        if initials.is_empty() {
            self.initials = None;
        } else {
            self.initials = Some(initials.to_owned());
        }
        self
    }

    /// Set square dimensions in logical pixels.
    /// When building the element, invalid or nonpositive values fall back to 40.
    /// Positive values cap at 1024 and round to whole logical pixels, with a minimum of 1.
    pub fn avatar_size(mut self, size: f32) -> Self {
        self.avatar_size = size;
        self
    }

    /// Set the shape.
    pub fn shape(mut self, shape: AvatarShape) -> Self {
        self.shape = shape;
        self
    }

    /// Set the presence indicator.
    pub fn presence(mut self, presence: Presence) -> Self {
        self.presence = presence;
        self
    }

    /// Override the presence ring and failure-marker border ink.
    /// Defaults to the palette surface; use the surrounding surface for a matching ring.
    pub fn presence_ring_color(mut self, color: Color) -> Self {
        self.presence_ring_color = Some(color);
        self
    }

    /// Pin all avatar colours instead of resolving the live palette.
    /// Sheen alpha still follows motion policy; the ring override takes precedence.
    pub fn tokens(mut self, tokens: AvatarTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set the full photo lifecycle state directly.
    pub fn media(mut self, media: AvatarMedia) -> Self {
        self.media = media;
        self
    }

    /// Show a decoded photo over the initials (`ContentFit::Cover`, clipped to
    /// the silhouette). The app owns the async load and passes the resulting
    /// [`image::Handle`] here; see [`AvatarMedia`].
    pub fn image(mut self, handle: impl Into<image::Handle>) -> Self {
        self.media = AvatarMedia::Image(handle.into());
        self
    }

    /// Supply elapsed seconds for a deterministic sheen sample and disable its frame clock.
    /// Each rise or fall takes 0.9 seconds. Without this override, loading avatars
    /// animate through an internal clock; reduced motion pins the sheen at its crest.
    pub fn shimmer_phase(mut self, elapsed_secs: f32) -> Self {
        self.shimmer_phase = Some(elapsed_secs);
        self
    }

    /// Override the theme motion policy: `true` pins the sheen at its crest,
    /// while `false` permits animation. An unset override follows the draw-time theme.
    pub fn reduced_motion(mut self, on: bool) -> Self {
        self.reduced_motion = Some(on);
        self
    }

    /// Override the failure glyph (`error-icon`, default `image-off`).
    /// An empty name resets to the default.
    pub fn error_icon(mut self, name: &str) -> Self {
        self.error_icon = if name.is_empty() {
            DEFAULT_ERROR_ICON.to_owned()
        } else {
            name.to_owned()
        };
        self
    }

    fn resolved_reduced_motion(&self, theme: &iced::Theme) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme))
    }

    fn shimmer_sheen_opacity_with(&self, reduced_motion: bool) -> f32 {
        match self.shimmer_phase {
            Some(p) => crate::skeleton::pulse_alpha_with_period(
                p,
                SHIMMER_PERIOD_SECS,
                SHIMMER_SHEEN_IDLE,
                SHIMMER_SHEEN_CREST,
                reduced_motion,
            ),
            None => {
                if reduced_motion {
                    SHIMMER_SHEEN_CREST
                } else {
                    SHIMMER_SHEEN_STATIC
                }
            }
        }
    }

    /// Inspect the explicit shimmer sample using the override or supplied theme policy.
    /// Without a supplied phase, returns 0.35 when motion is allowed and 0.7
    /// when reduced. This does not inspect the built widget's frame-clock phase.
    pub fn shimmer_sheen_opacity_for_theme(&self, theme: &iced::Theme) -> f32 {
        self.shimmer_sheen_opacity_with(self.resolved_reduced_motion(theme))
    }

    /// Inspect shimmer opacity against the stock dark theme.
    /// See [`Self::shimmer_sheen_opacity_for_theme`] for the no-phase fallback.
    pub fn shimmer_sheen_opacity(&self) -> f32 {
        self.shimmer_sheen_opacity_for_theme(&iced::Theme::Dark)
    }

    /// Render the loading shimmer skeleton (photo + initials withheld). The
    /// a11y node stays a named `Image` with `", loading"` appended to its name.
    pub fn loading(mut self) -> Self {
        self.media = AvatarMedia::Loading;
        self
    }

    /// Show initials with an upper-right failure marker (`image-off` by default).
    pub fn failed(mut self) -> Self {
        self.media = AvatarMedia::Failed;
        self
    }

    fn derive_initials(label: &str) -> String {
        label
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .flat_map(char::to_uppercase)
            .collect()
    }

    fn accessible_name(&self) -> Option<String> {
        let person = self.label.trim();
        if person.is_empty() {
            return None;
        }
        let mut name = person.to_owned();
        if self.presence != Presence::None {
            name.push_str(", ");
            name.push_str(self.presence.as_str());
        }
        if self.media.is_loading() {
            name.push_str(", ");
            name.push_str(ANNOTATION_LOADING);
        } else if self.media.is_failed() {
            name.push_str(", ");
            name.push_str(ANNOTATION_UNAVAILABLE);
        }
        Some(name)
    }

    /// Return the trimmed name with presence and lifecycle annotations, or an empty string.
    /// Initials are excluded; blank names are decorative.
    pub fn accessible_label(&self) -> String {
        self.accessible_name().unwrap_or_default()
    }

    /// Whether the avatar is currently in its loading (skeleton) state.
    pub fn is_loading(&self) -> bool {
        self.media.is_loading()
    }

    /// Return named Image semantics in every media state, or `None` for a blank name.
    /// The built element publishes this as one leaf and suppresses initials semantics.
    pub fn semantics(&self) -> Option<enzui_core::semantics::Semantics> {
        use enzui_core::semantics::{Role, Semantics};
        Some(Semantics::new(Role::Image).label(self.accessible_name()?))
    }

    /// Build a square avatar with live palette colours unless tokens are pinned.
    /// Photos use Cover with the selected silhouette radius. Failure markers sit
    /// upper-right and presence dots lower-right within the avatar bounds.
    /// Loading without an explicit phase pulses the sheen on the render lane;
    /// theme-reduced motion requests delayed probes, while explicit reduced
    /// motion requests none.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let shimmer_phase = self.shimmer_phase;
        let reduced_override = self.reduced_motion;
        let error_icon = self.error_icon.clone();
        let shimmer_self_animate = shimmer_phase.is_none() && self.media.is_loading();
        let a11y_semantics = self.semantics();
        let pinned_tokens = self.tokens;
        let size = normalize_size(self.avatar_size);
        let radius = self.shape.radius(size);
        let presence = self.presence;
        let ring_override = self.presence_ring_color;
        let media = self.media;
        let loading = media.is_loading();
        let failed = media.is_failed();

        let display_text = self
            .initials
            .unwrap_or_else(|| Self::derive_initials(&self.label));

        let font_size = (size * 0.30).max(11.0);

        let resolve = move |theme: &iced::Theme| -> AvatarTokens {
            crate::theme_lookup::resolve_pinned(
                crate::theme_lookup::PIN_SITE_AVATAR,
                pinned_tokens,
                theme,
                AvatarTokens::from_palette,
            )
        };

        let mut layers: Vec<Element<'a, Message>> = Vec::new();
        if !loading {
            // Fixed silhouette centring requires this line height instead of the ambient ladder.
            let initials_run = initials_enz_text(display_text, font_size)
                .color_resolved(move |theme: &iced::Theme| resolve(theme).foreground);
            let initials_el = crate::decorative::decorative(initials_run.into_element::<Message>());
            layers.push(
                container(initials_el)
                    .width(Length::Fixed(size))
                    .height(Length::Fixed(size))
                    .center_x(Length::Fixed(size))
                    .center_y(Length::Fixed(size))
                    .into(),
            );
        }
        if !loading
            && !failed
            && let Some(handle) = media.image_handle()
        {
            // Container clipping is rectangular; the image must own its rounded mask.
            layers.push(
                image(handle.clone())
                    .width(Length::Fixed(size))
                    .height(Length::Fixed(size))
                    .content_fit(ContentFit::Cover)
                    .border_radius(radius)
                    .into(),
            );
        }
        if loading {
            let sheen = container(text(""))
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .style(move |theme: &iced::Theme| {
                    let reduced = reduced_override
                        .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme));
                    // A self-animating sheen paints at full strength; the
                    // pulse fades it as a whole.
                    let shimmer_sheen_opacity = match shimmer_phase {
                        None if shimmer_self_animate => 1.0,
                        Some(p) => crate::skeleton::pulse_alpha_with_period(
                            p,
                            SHIMMER_PERIOD_SECS,
                            SHIMMER_SHEEN_IDLE,
                            SHIMMER_SHEEN_CREST,
                            reduced,
                        ),
                        None => {
                            if reduced {
                                SHIMMER_SHEEN_CREST
                            } else {
                                SHIMMER_SHEEN_STATIC
                            }
                        }
                    };
                    let mut c = resolve(theme).shimmer_sheen;
                    c.a = shimmer_sheen_opacity;
                    container::Style {
                        background: Some(c.into()),
                        border: Border {
                            radius: radius.into(),
                            width: 0.0,
                            color: Color::TRANSPARENT,
                        },
                        ..Default::default()
                    }
                });
            let sheen: Element<'a, Message> = if shimmer_self_animate {
                crate::opacity_pulse::OpacityPulse::new(
                    sheen.into(),
                    SHIMMER_PERIOD_SECS,
                    SHIMMER_SHEEN_IDLE,
                    SHIMMER_SHEEN_CREST,
                    reduced_override,
                )
                .into()
            } else {
                sheen.into()
            };
            layers.push(
                container(sheen)
                    .width(Length::Fixed(size))
                    .height(Length::Fixed(size))
                    .style(move |theme: &iced::Theme| container::Style {
                        background: Some(resolve(theme).shimmer_base.into()),
                        border: Border {
                            radius: radius.into(),
                            width: 0.0,
                            color: Color::TRANSPARENT,
                        },
                        ..Default::default()
                    })
                    .into(),
            );
        }
        let silhouette = container(stack(layers))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .padding(Padding::ZERO)
            .clip(true)
            .center_x(Length::Fixed(size))
            .center_y(Length::Fixed(size))
            .style(move |theme: &iced::Theme| container::Style {
                // Loading, the silhouette is the shimmer's own base: an accent
                // under it would fringe the anti-aliased rim.
                background: Some(
                    if loading {
                        resolve(theme).shimmer_base
                    } else {
                        resolve(theme).background
                    }
                    .into(),
                ),
                border: Border {
                    radius: radius.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            });
        let silhouette: Element<'a, Message> = if failed && !loading {
            let badge_size = size * 0.25;
            let glyph = crate::icon::enz_icon(&error_icon)
                .size(badge_size * 0.8)
                .tint_resolved(move |theme: &iced::Theme| resolve(theme).foreground)
                .into_element::<Message>();
            let marker = container(glyph)
                .width(Length::Fixed(badge_size))
                .height(Length::Fixed(badge_size))
                .center_x(Length::Fixed(badge_size))
                .center_y(Length::Fixed(badge_size))
                .style(move |theme: &iced::Theme| container::Style {
                    background: Some(resolve(theme).background.into()),
                    border: Border {
                        radius: (badge_size / 2.0).into(),
                        width: (badge_size * 0.05).min(1.0),
                        color: ring_override.unwrap_or_else(|| resolve(theme).presence_ring),
                    },
                    ..Default::default()
                });
            // The half-pixel inset contains border antialiasing within avatar bounds.
            let marker_layer = container(marker)
                .padding(Padding::ZERO.top(0.5).right(0.5))
                .clip(true)
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .align_right(Length::Fixed(size));
            stack![silhouette, marker_layer]
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .into()
        } else {
            silhouette.into()
        };
        let wrap = move |el: Element<'a, Message>| -> Element<'a, Message> {
            match a11y_semantics {
                Some(sem) => crate::semantics_container::SemanticsContainer::leaf(el, sem).into(),
                None => el,
            }
        };

        if presence == Presence::None {
            return wrap(silhouette);
        }
        // Small presence rings leave room for the 11 px initials floor.
        let preferred_dot = (snap(size * 0.28)).max(8.0);
        let preferred_ring = preferred_dot + 4.0;
        let ring_size = preferred_ring.min((size * 0.5 - 5.0).max(size * 0.25));
        let dot_size = preferred_dot * (ring_size / preferred_ring);

        let ring = container(
            container(text(""))
                .width(Length::Fixed(dot_size))
                .height(Length::Fixed(dot_size))
                .style(move |theme: &iced::Theme| container::Style {
                    background: Some(presence_fill(&resolve(theme), presence).into()),
                    border: Border {
                        radius: (dot_size / 2.0).into(),
                        width: 0.0,
                        color: Color::TRANSPARENT,
                    },
                    ..Default::default()
                }),
        )
        .width(Length::Fixed(ring_size))
        .height(Length::Fixed(ring_size))
        .center_x(Length::Fixed(ring_size))
        .center_y(Length::Fixed(ring_size))
        .style(move |theme: &iced::Theme| {
            let t = resolve(theme);
            container::Style {
                background: Some(ring_override.unwrap_or(t.presence_ring).into()),
                border: Border {
                    radius: (ring_size / 2.0).into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            }
        });
        let badge_layer = container(ring)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .align_right(Length::Fixed(size))
            .align_bottom(Length::Fixed(size));

        wrap(
            stack![silhouette, badge_layer]
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .into(),
        )
    }
}

fn presence_fill(tokens: &AvatarTokens, presence: Presence) -> Color {
    tokens
        .presence_color(presence)
        .unwrap_or(tokens.presence_offline)
}

/// Build initials typography with an ExtraBold weight and relative 1.3 line height.
/// Colour is supplied separately by the avatar.
#[doc(hidden)]
pub fn initials_enz_text(display_text: String, font_size: f32) -> crate::enz_text::EnzText {
    enz_text(display_text)
        .size_px(font_size)
        .weight_rung(enzui_theme::WeightRung::ExtraBold)
        .line_height(iced::widget::text::LineHeight::Relative(1.3))
}

impl<'a, Message: 'a> From<EnzAvatar> for Element<'a, Message> {
    fn from(avatar: EnzAvatar) -> Self {
        avatar.into_element()
    }
}
