#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]
//! enzUI theme tokens: palette, spacing, sizing, radius, typography, motion,
//! scrollbar, stroke, breakpoints, and font roles.
//!
//! One built-in theme family, enzUI Nebula ([`FAMILY`]), in two colour presets
//! (dark and light). [`EnzTheme::iced_theme`] embeds the full token set in an
//! iced theme and projects its palette for stock widgets.
//!
//! # Example
//!
//! ```
//! use enzui_theme::EnzTheme;
//!
//! let theme = EnzTheme::dark();
//! let bg = theme.palette.background;
//! let gap = theme.spacing.space4;
//! ```
//!
//! Colour-scheme feeds require `system-theme`. [`current_system_color_scheme`]
//! and [`system_color_scheme_subscription`] preserve an unanswered preference;
//! the `EnzTheme` feeds instead map it to light. Returned tasks require dispatch
//! through the host runtime. Font, material and motion feeds have their own
//! feature switches.
//!
pub mod breakpoints;
pub mod find_match;
pub mod font_preferences;
pub mod font_role;
#[cfg(feature = "desktop-fonts")]
pub mod font_source;
pub mod grain;
pub mod material;
#[cfg(feature = "desktop-materials")]
pub mod material_source;
pub mod motion;
pub mod opacity;
pub mod palette;
pub mod palette_fingerprint;
pub mod radius;
pub mod scrollbar;
pub mod shape;
pub mod sizing;
pub mod spacing;
pub mod stroke;
pub mod text_defaults;
pub mod text_scale;
pub mod typography;
pub mod window_buttons;
pub mod window_frame;
pub mod window_shadow;
pub mod workspace_accent;

pub use breakpoints::{BreakpointScale, EnzSizeClass};
pub use find_match::{FindMatchTokens, INACTIVE_TINT, MatchEmphasis};
pub use font_preferences::{
    DesktopFontPreferences, FontLayer, FontPreferencePolicy, ResolvedFontRole,
};
pub use font_role::{
    FontCatalog, FontFamily, FontResolver, FontRole, FontSize, GenericFamily,
    ParsedFontDescription, RawFontDescription,
};
#[cfg(feature = "desktop-fonts")]
pub use font_source::{
    DesktopFontBackend, DesktopFontFeed, FontFeedStatus, FontPreferenceEvent, FontPreferenceRead,
};

pub use grain::{
    DEFAULT_GRAIN_DENSITY, DEFAULT_GRAIN_FREQUENCY, DEFAULT_GRAIN_INTENSITY, DEFAULT_GRAIN_TILE,
    Grain, GrainMapping, GrainPlacement, GrainSource, GrainSpec, GrainSvg, GrainTiling, GrainTone,
};
pub use material::{
    GlassInk, MaterialInk, MaterialPolicy, MaterialPolicyOwner, MaterialPolicySource,
    MaterialStyle, MaterialTokens, PreferenceSet, current_policy, install_policy_owner,
    policy_with_theme_motion, published_layers,
};
#[cfg(feature = "desktop-materials")]
pub use material_source::{
    MaterialPreferenceBackend, MaterialPreferenceEvent, MaterialPreferenceFeed,
    MaterialPreferenceRead, MaterialPreferenceSignals, PreferenceFeedStatus, PreferenceSignal,
};
pub use motion::{
    CubicBezier, ElevationScale, ElevationTier, ExpressiveMotion, MAX_MOTION_MS, MotionChannel,
    MotionEasing, MotionScale, MotionToken, Spring, sane_motion_ms,
};
pub use opacity::{
    DEFAULT_DISABLED_OPACITY, DEFAULT_DISABLED_SURFACE_OPACITY, DEFAULT_DROP_TARGET_OPACITY,
    OpacityScale,
};
pub use palette::{
    ContrastFloor, Palette, RaisedInk, SemanticTone, SurfaceRole, perceptual_distance,
    raise_to_floor, raise_to_target,
};
pub use palette_fingerprint::{COMMITTED_FINGERPRINT, palette_fingerprint};
pub use radius::RadiusScale;
pub use scrollbar::ScrollbarTokens;
pub use shape::{RadiusStep, ShapeScale};
pub use sizing::SizingTokens;
pub use spacing::{SpaceToken, SpacingScale};
pub use stroke::{StrokeScale, StrokeToken};
pub use text_scale::{
    TextScale, TextScaleAdmission, TextScaleDelivery, TextScalePolicy, TextScaleSource,
};
pub use typography::{LETTER_SPACING_UNSUPPORTED, TypographyScale, WeightRung, css_weight_to_iced};
pub use window_buttons::{DEFAULT_WINDOW_RADIUS, WindowButtons};
pub use window_frame::WindowFrame;
pub use window_shadow::{WindowShadow, WindowShadowLayer};
pub use workspace_accent::{
    DEFAULT_WASH_ALPHA_REST, DEFAULT_WASH_ALPHA_SELECTED, WorkspaceAccentKey,
    WorkspaceAccentPalette, tab_wash_over, tab_wash_passes_body_text,
};

extern crate iced_core as iced;

use iced_core::Color;
use iced_core::Theme;
use iced_core::theme::Mode;
use iced_core::theme::palette::{
    Background as IcedBackground, Pair, Palette as IcedPalette, Seed, Swatch,
};

#[cfg(feature = "system-theme")]
use iced_futures::Subscription;
#[cfg(feature = "system-theme")]
use iced_runtime::Task;

/// Display family shared by the built-in light and dark presets.
/// [`EnzThemeMode::theme_name`] supplies their mode-specific iced names.
pub const FAMILY: &str = "enzUI Nebula";

/// The two colour modes of the one built-in enzUI family ([`FAMILY`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzThemeMode {
    /// The dark mode of [`FAMILY`].
    #[default]
    Dark,
    /// The light mode of [`FAMILY`].
    Light,
}

impl EnzThemeMode {
    /// Returns this mode's theme family, always [`FAMILY`].
    #[must_use]
    pub const fn family(self) -> &'static str {
        FAMILY
    }

    /// Returns the display name used by [`EnzTheme::iced_theme`].
    #[must_use]
    pub const fn theme_name(self) -> &'static str {
        match self {
            Self::Dark => "enzUI Nebula Dark",
            Self::Light => "enzUI Nebula Light",
        }
    }

    /// The mode word alone (`"Dark"` / `"Light"`) — for a UI that offers the
    /// two modes of the one family rather than a list of theme products.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }
}

/// Complete enzUI theme tokens: palette, typography, spacing, radius, sizing,
/// stroke, breakpoints, motion, elevation, opacity, scrollbars, and font roles.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzTheme {
    /// Color palette.
    pub palette: Palette,
    /// Typography scale.
    pub typography: TypographyScale,
    /// Spacing scale.
    pub spacing: SpacingScale,
    /// Border radius scale.
    pub radius: RadiusScale,
    /// Which radius step each component family uses.
    pub shape: ShapeScale,
    /// Sizing tokens.
    pub sizing: SizingTokens,
    /// Logical thicknesses for drawn rules and grid gaps, resolved through the
    /// theme's stroke scale.
    pub stroke: StrokeScale,
    /// Responsive breakpoint catalog.
    pub breakpoints: BreakpointScale,
    /// Motion policy: named durations + reduced-motion switch.
    pub motion: MotionScale,
    /// Elevation scale: the six semantic shadow tiers.
    pub elevation: ElevationScale,
    /// Whole-control state opacities (disabled controls and surfaces).
    pub opacity: OpacityScale,
    /// Overlay scrollbar tokens.
    ///
    /// Widths, opacities and content gutter for the auto-hiding overlay
    /// scrollbar chrome.  All scroll primitives consult these so a custom
    /// theme can retune the bar without touching widget code.
    pub scrollbar: ScrollbarTokens,
    /// Semantic font-role catalog.
    pub font_catalog: FontCatalog,
    /// The grain a surface draws when it asks for the theme's
    /// ([`GrainSpec::Theme`]).
    pub grain: Grain,
    /// The window controls a self-drawn frame shows; see [`WindowButtons`].
    pub window_buttons: WindowButtons,
    /// The corner radius of a self-drawn window frame, in logical px.
    pub window_radius: f32,
    /// The shadow and edge lines of a self-drawn window frame.
    pub window_shadow: WindowShadow,
    /// What every self-drawn window frame is made of; see [`WindowFrame`].
    pub window_frame: WindowFrame,
    // A material-context marker prevents repeated palette adaptation.
    backdrop: Option<MaterialStyle>,
}

impl EnzTheme {
    /// Creates the dark mode of [`FAMILY`]. Non-colour scales match
    /// [`Self::light`].
    pub fn dark() -> Self {
        Self {
            palette: Palette::dark(),
            typography: TypographyScale::default(),
            spacing: SpacingScale::default(),
            radius: RadiusScale::default(),
            shape: ShapeScale::default(),
            sizing: SizingTokens::default(),
            stroke: StrokeScale::default(),
            breakpoints: BreakpointScale::default(),
            motion: MotionScale::default(),
            elevation: ElevationScale::default(),
            opacity: OpacityScale::default(),
            scrollbar: ScrollbarTokens::default(),
            font_catalog: FontCatalog::default(),
            grain: Grain::default(),
            window_buttons: WindowButtons::default(),
            window_frame: WindowFrame::default(),
            window_radius: DEFAULT_WINDOW_RADIUS,
            window_shadow: WindowShadow::default(),
            backdrop: None,
        }
    }

    /// Creates the light mode of [`FAMILY`]. Non-colour scales match
    /// [`Self::dark`].
    pub fn light() -> Self {
        Self {
            palette: Palette::light(),
            typography: TypographyScale::default(),
            spacing: SpacingScale::default(),
            radius: RadiusScale::default(),
            shape: ShapeScale::default(),
            sizing: SizingTokens::default(),
            stroke: StrokeScale::default(),
            breakpoints: BreakpointScale::default(),
            motion: MotionScale::default(),
            elevation: ElevationScale::default(),
            opacity: OpacityScale::default(),
            scrollbar: ScrollbarTokens::default(),
            font_catalog: FontCatalog::default(),
            grain: Grain::default(),
            window_buttons: WindowButtons::default(),
            window_frame: WindowFrame::default(),
            window_radius: DEFAULT_WINDOW_RADIUS,
            window_shadow: WindowShadow::default(),
            backdrop: None,
        }
    }

    /// Builds a custom theme from an explicit palette and token set.
    ///
    /// Mode is derived through [`Palette::is_dark`], so palette and mode cannot
    /// contradict each other.
    pub fn custom(
        palette: Palette,
        typography: TypographyScale,
        spacing: SpacingScale,
        radius: RadiusScale,
        sizing: SizingTokens,
    ) -> Self {
        Self {
            palette,
            typography,
            spacing,
            radius,
            shape: ShapeScale::default(),
            sizing,
            stroke: StrokeScale::default(),
            breakpoints: BreakpointScale::default(),
            motion: MotionScale::default(),
            elevation: ElevationScale::default(),
            opacity: OpacityScale::default(),
            scrollbar: ScrollbarTokens::default(),
            font_catalog: FontCatalog::default(),
            grain: Grain::default(),
            window_buttons: WindowButtons::default(),
            window_frame: WindowFrame::default(),
            window_radius: DEFAULT_WINDOW_RADIUS,
            window_shadow: WindowShadow::default(),
            backdrop: None,
        }
    }

    /// Adapt the palette for content on a sampled glass surface.
    ///
    /// [`Palette::on_glass`] changes fills and borders. An already adapted theme
    /// or style whose current tokens are opaque returns an unchanged clone.
    /// This does not enforce accessibility policy or request a material pass.
    pub fn on_material(&self, style: MaterialStyle) -> Self {
        if style.tokens().is_opaque() || self.backdrop.is_some() {
            return self.clone();
        }
        Self {
            palette: self.palette.on_glass(),
            backdrop: Some(style),
            ..self.clone()
        }
    }

    /// Adapt glass content for light ink at `1.0`, dark ink at `0.0`, or a blend.
    /// Uses [`Palette::on_glass_ink`] and the opaque/nested exclusions of
    /// [`Self::on_material`].
    pub fn on_material_ink(&self, style: MaterialStyle, light_ink: f32) -> Self {
        if style.tokens().is_opaque() || self.backdrop.is_some() {
            return self.clone();
        }
        Self {
            palette: self.palette.on_glass_ink(light_ink),
            backdrop: Some(style),
            ..self.clone()
        }
    }

    /// The glass this theme's widgets sit on, if any.
    #[must_use]
    pub fn backdrop(&self) -> Option<MaterialStyle> {
        self.backdrop
    }

    /// Override the responsive breakpoint catalog. Chainable on a
    /// constructed theme.
    ///
    /// The supplied [`BreakpointScale`] is already normalised by construction —
    /// its fields are private and every constructor enforces the non-descending
    /// invariant — so there is nothing to validate or reject here.
    pub fn with_breakpoints(mut self, breakpoints: BreakpointScale) -> Self {
        self.breakpoints = breakpoints;
        self
    }

    /// Override the motion policy (durations / reduced-motion). Chainable on a
    /// constructed theme.
    pub fn with_motion(mut self, motion: MotionScale) -> Self {
        self.motion = motion;
        self
    }

    /// Override the elevation scale. Chainable on a constructed theme.
    pub fn with_elevation(mut self, elevation: ElevationScale) -> Self {
        self.elevation = elevation;
        self
    }

    /// Override the whole-control state opacities. Chainable on a constructed
    /// theme.
    pub fn with_opacity(mut self, opacity: OpacityScale) -> Self {
        self.opacity = opacity;
        self
    }

    /// Override the stroke / hairline scale. Chainable on a constructed
    /// theme, so a denser or airier theme can retune its grid hairlines.
    pub fn with_stroke(mut self, stroke: StrokeScale) -> Self {
        self.stroke = stroke;
        self
    }

    /// Replaces the complete font-role catalog; widgets read it on the next
    /// view build.
    pub fn with_font_catalog(mut self, catalog: FontCatalog) -> Self {
        self.font_catalog = catalog;
        self
    }

    /// Resolve `(role, weight, style, stretch)` against this theme's catalog into
    /// an `iced::Font`.
    pub fn resolve_font(
        &self,
        role: FontRole,
        weight: iced_core::font::Weight,
        style: iced_core::font::Style,
        stretch: iced_core::font::Stretch,
    ) -> iced_core::Font {
        FontResolver::new(self.font_catalog.clone()).resolve(role, weight, style, stretch)
    }

    /// Returns the colour mode from [`Palette::is_dark`]'s background-channel
    /// heuristic, so palette and mode cannot contradict each other.
    pub fn mode(&self) -> EnzThemeMode {
        if p_is_dark(&self.palette) {
            EnzThemeMode::Dark
        } else {
            EnzThemeMode::Light
        }
    }

    /// Maps [`Mode::Dark`] to the dark theme and both [`Mode::Light`] and
    /// [`Mode::None`] to the light theme.
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::dark(),
            Mode::Light | Mode::None => Self::light(),
        }
    }

    /// Embed these tokens in a custom iced theme and project its colour palette.
    ///
    /// Background and primary tiers use enzUI roles; status tiers retain
    /// generated colours with additional enzUI roles carried for recovery.
    /// The extension preserves the full theme for [`Self::from_iced`]. This
    /// creates a snapshot; later edits to this value do not update that snapshot.
    pub fn iced_theme(&self) -> Theme {
        let p = self.palette.clone();
        let name = self.mode().theme_name();
        let seed = Seed {
            background: p.background,
            text: p.text,
            primary: p.accent,
            success: p.success,
            warning: p.warning,
            danger: p.danger,
        };
        // The projected palette serves stock widgets; the extension preserves all enzUI roles.
        let theme = Theme::custom_with_fn(name, seed, move |seed| enzui_extended_palette(&p, seed));
        theme.with_extension(std::sync::Arc::new(self.clone()))
    }

    /// The desktop text-size factor this theme's type ramp carries.
    #[must_use]
    pub fn text_scale(&self) -> TextScale {
        self.typography.scale_factor()
    }

    /// Replace the typography's baked text factor while preserving other tokens.
    ///
    /// Rescaling removes the prior factor before applying the new one, subject
    /// to floating-point rounding. Publish the same factor through
    /// [`text_scale::publish`] for ambient text; this method does not publish it.
    /// Theme and ambient ramps can still have different authored sizes.
    pub fn with_text_scale(&self, scale: TextScale) -> Self {
        let mut scaled = self.clone();
        scaled.typography = self.typography.with_text_scale(scale);
        scaled
    }

    /// Returns the embedded enzUI theme, or `None` for stock and other themes
    /// without the enzUI extension payload.
    pub fn from_iced(theme: &Theme) -> Option<Self> {
        theme.extension::<EnzTheme>().map(|arc| (*arc).clone())
    }

    /// Returns the embedded enzUI palette without cloning the other tokens, or
    /// `None` when the theme has no enzUI extension payload.
    pub fn palette_from_iced(theme: &Theme) -> Option<Palette> {
        theme.extension::<EnzTheme>().map(|arc| arc.palette.clone())
    }
}

fn enzui_extended_palette(p: &Palette, seed: Seed) -> IcedPalette {
    let generated = IcedPalette::generate(seed);
    // Extra foreground roles share palette text slots for extension-free recovery.
    let carry = |color, text| Pair { color, text };
    let pair = |color| Pair {
        color,
        text: p.text,
    };

    let background = IcedBackground {
        base: carry(p.background, p.text),

        weakest: carry(p.surface_elevated, p.text_muted),

        weaker: carry(p.surface, p.text_subtle),

        weak: carry(p.surface_alt, p.disabled),

        neutral: pair(p.surface_hover),

        strong: pair(p.surface_selected),

        stronger: pair(p.border),

        strongest: pair(p.border_strong),
    };
    // The soft fill and marked foreground preserve enzUI status roles.
    let status = |soft: Color, fg: Color, from_seed: Swatch| Swatch {
        base: from_seed.base,
        weak: carry(soft, from_seed.weak.text),
        strong: carry(from_seed.strong.color, ferried(fg)),
    };
    IcedPalette {
        background,

        primary: Swatch {
            base: pair(p.accent),
            weak: Pair::new(p.accent_soft, p.text),
            strong: Pair::new(p.accent_pressed, p.text),
        },
        secondary: generated.secondary,
        success: status(p.success_soft, p.tone_success_fg, generated.success),
        warning: status(p.warning_soft, p.tone_warning_fg, generated.warning),
        danger: status(p.danger_soft, p.tone_error_fg, generated.danger),
        is_dark: p_is_dark(p),
    }
}

// Mark carried foregrounds so extension-free recovery can distinguish stock colours.
const FERRY_ALPHA: f32 = 254.0 / 255.0;

fn ferried(c: Color) -> Color {
    Color {
        a: FERRY_ALPHA,
        ..c
    }
}

fn unferried(c: Color) -> Option<Color> {
    ((c.a - FERRY_ALPHA).abs() < 1.0 / 512.0).then_some(Color { a: 1.0, ..c })
}

fn p_is_dark(p: &Palette) -> bool {
    p.is_dark()
}

impl Default for EnzTheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Request the runtime's current colour preference as a theme, mapping none to light.
///
/// Dispatch the task through the host runtime; this does not query the desktop
/// directly. The runtime cache can contain startup fallback or a live answer.
/// Consumers using both feeds should retain live-message precedence if the
/// request's result arrives after a subscription update.
#[cfg(feature = "system-theme")]
pub fn current_system_theme() -> Task<EnzTheme> {
    iced_runtime::system::theme().map(EnzTheme::for_mode)
}

/// A runtime colour preference that distinguishes no answer from light.
/// [`EnzTheme::for_mode`] folds no preference into light; this type lets a
/// caller retain an existing appearance instead.
///
/// ```
/// use enzui_theme::{EnzThemeMode, SystemColorScheme};
/// use iced_core::theme::Mode;
///
/// assert_eq!(SystemColorScheme::from_mode(Mode::Dark).mode(), Some(EnzThemeMode::Dark));
/// assert_eq!(SystemColorScheme::from_mode(Mode::Light).mode(), Some(EnzThemeMode::Light));
/// assert_eq!(SystemColorScheme::from_mode(Mode::None).mode(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SystemColorScheme {
    /// The desktop expressed no preference, or the query failed: the absence of
    /// an answer, distinct from a light preference.
    #[default]
    NoPreference,
    /// The desktop asked for a light appearance.
    Light,
    /// The desktop asked for a dark appearance.
    Dark,
}

impl SystemColorScheme {
    /// Read an iced [`Mode`] without folding `Mode::None` into light.
    #[must_use]
    pub fn from_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::Dark,
            Mode::Light => Self::Light,
            Mode::None => Self::NoPreference,
        }
    }

    /// The mode the desktop asked for, or `None` when it asked for nothing.
    #[must_use]
    pub fn mode(self) -> Option<EnzThemeMode> {
        match self {
            Self::NoPreference => None,
            Self::Light => Some(EnzThemeMode::Light),
            Self::Dark => Some(EnzThemeMode::Dark),
        }
    }

    /// The theme the desktop asked for, or `None` when it asked for nothing.
    ///
    /// Use this to decide whether to change the theme at all; use
    /// [`EnzTheme::for_mode`] when a value is required regardless (a boot
    /// default with nothing else to fall back on).
    #[must_use]
    pub fn theme(self) -> Option<EnzTheme> {
        match self {
            Self::NoPreference => None,
            Self::Light => Some(EnzTheme::light()),
            Self::Dark => Some(EnzTheme::dark()),
        }
    }
}

/// Request the runtime's current colour preference without mapping none to light.
/// Dispatch is required; snapshot and live-message ordering follows
/// [`current_system_theme`].
#[cfg(feature = "system-theme")]
pub fn current_system_color_scheme() -> Task<SystemColorScheme> {
    iced_runtime::system::theme().map(SystemColorScheme::from_mode)
}

/// Subscribe to runtime colour-preference notifications, preserving no preference.
/// No preference lets the caller retain its current mode; platform detection
/// determines which changes produce notifications.
#[cfg(feature = "system-theme")]
pub fn system_color_scheme_subscription() -> Subscription<SystemColorScheme> {
    iced_runtime::system::theme_changes().map(SystemColorScheme::from_mode)
}

/// Subscribe to runtime colour-preference notifications as theme values.
/// No preference maps to light. Use [`system_color_scheme_subscription`] when
/// that distinction must survive; prioritize live messages over late snapshots.
#[cfg(feature = "system-theme")]
pub fn system_theme_subscription() -> Subscription<EnzTheme> {
    iced_runtime::system::theme_changes().map(EnzTheme::for_mode)
}

/// Request the runtime's current reduced-motion preference.
/// Dispatch the task to read the runtime cache, which defaults to false when
/// detection is unavailable. [`ReducedMotionPrecedence`] gives live messages
/// precedence over a result delivered later.
#[cfg(feature = "system-reduced-motion")]
pub fn current_reduced_motion() -> Task<bool> {
    iced_runtime::system::reduced_motion()
}

/// Streams OS reduced-motion changes.
///
/// Platform detection determines notifications. Feed values through
/// [`ReducedMotionPrecedence::on_live`] to prioritize them over late snapshots.
#[cfg(feature = "system-reduced-motion")]
pub fn reduced_motion_subscription() -> Subscription<bool> {
    iced_runtime::system::reduced_motion_changes()
}

/// Ensures live reduced-motion updates take precedence over the startup
/// snapshot.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReducedMotionPrecedence {
    /// The current reduced-motion value the application should honour.
    /// Defaults to `false` until either feed supplies a value.
    pub reduced_motion: bool,
    /// Latched on the first `on_live` call; guards `on_snapshot` from
    /// overwriting a live value with a stale bootstrap answer.
    pub saw_live: bool,
}

impl ReducedMotionPrecedence {
    /// Start with motion enabled and no live update observed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a startup snapshot unless a live update has already arrived.
    /// Returns `true` only when an accepted snapshot changes the effective
    /// value; stale and unchanged snapshots return `false`.
    pub fn on_snapshot(&mut self, value: bool) -> bool {
        if self.saw_live {
            return false;
        }
        let changed = self.reduced_motion != value;
        self.reduced_motion = value;
        changed
    }

    /// Accepts a live value and latches precedence over later snapshots.
    /// Returns `true` when the value changes or this is the first live update;
    /// later identical live values return `false`.
    pub fn on_live(&mut self, value: bool) -> bool {
        let changed = self.reduced_motion != value || !self.saw_live;
        self.saw_live = true;
        self.reduced_motion = value;
        changed
    }

    /// The current reduced-motion value.
    pub fn value(&self) -> bool {
        self.reduced_motion
    }
}
