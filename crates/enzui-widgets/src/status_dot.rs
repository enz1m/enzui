//! EnzStatusDot: compact semantic status indicator.
//!
//! Renders a small filled circle whose color communicates status:
//! availability, ok/error, running, etc. Fixed 8 px default size, fully
//! customizable via the builder.
//!
//! # Two-layer status model
//!
//! The `status` string carries ten semantic values that share five
//! paint tones. Callers keep the full meaning (a `Running` dot is not the same
//! *state* as a `Limited` dot even though both paint `warning`), and can
//! announce/recover it for accessibility:
//!
//! - [`StatusKind`] — the ten typed semantic states.
//! - [`StatusTone`] — the five paint tones each kind resolves to via
//!   [`StatusKind::tone`].
//!
//! | Tone       | Kinds                              | Token        |
//! |------------|------------------------------------|--------------|
//! | `Neutral`  | `Neutral`                          | `text_muted` |
//! | `Info`     | `Info`                             | `accent`     |
//! | `Success`  | `Success`, `Ok`                    | `success`    |
//! | `Warning`  | `Warning`, `Limited`, `Running`    | `warning`    |
//! | `Danger`   | `Error`, `Danger`, `Offline`       | `destructive`|

use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{Palette, SizingTokens};
use iced::widget::{Space, container};
use iced::{Background, Border, Color, Element, Length};

// ---------------------------------------------------------------------------
// Semantic kinds & paint tones
// ---------------------------------------------------------------------------

/// The ten typed semantic status states (the `status` string domain). Each maps to a paint [`StatusTone`] via
/// [`StatusKind::tone`], but the semantic identity is preserved for
/// accessibility announcement and state recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Unavailable / not-applicable (neutral tone).
    Neutral,
    /// Informational (info tone).
    Info,
    /// Success (success tone).
    Success,
    /// Healthy / ok (success tone).
    Ok,
    /// Warning (warning tone).
    Warning,
    /// Limited capability / degraded (warning tone).
    Limited,
    /// In-progress / running (warning tone).
    Running,
    /// Error (danger tone).
    Error,
    /// Danger (danger tone).
    Danger,
    /// Offline / unreachable (danger tone).
    Offline,
}

impl StatusKind {
    /// The paint tone this semantic kind resolves to.
    pub fn tone(self) -> StatusTone {
        match self {
            StatusKind::Neutral => StatusTone::Neutral,
            StatusKind::Info => StatusTone::Info,
            StatusKind::Success | StatusKind::Ok => StatusTone::Success,
            StatusKind::Warning | StatusKind::Limited | StatusKind::Running => StatusTone::Warning,
            StatusKind::Error | StatusKind::Danger | StatusKind::Offline => StatusTone::Danger,
        }
    }

    /// A stable lowercase label for this kind (`status`
    /// string), suitable for accessibility announcement.
    pub fn label(self) -> &'static str {
        match self {
            StatusKind::Neutral => "neutral",
            StatusKind::Info => "info",
            StatusKind::Success => "success",
            StatusKind::Ok => "ok",
            StatusKind::Warning => "warning",
            StatusKind::Limited => "limited",
            StatusKind::Running => "running",
            StatusKind::Error => "error",
            StatusKind::Danger => "danger",
            StatusKind::Offline => "offline",
        }
    }
}

/// The five paint tones a [`StatusKind`] resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// Grey / muted.
    Neutral,
    /// Accent (informational).
    Info,
    /// Success / healthy.
    Success,
    /// Warning / limited / running.
    Warning,
    /// Error / danger / offline.
    Danger,
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colour tokens for the status dot. Each field is the fill for the
/// corresponding [`StatusKind`].
#[derive(Debug, Clone, Copy)]
pub struct StatusDotTokens {
    /// Neutral fill (muted grey).
    pub neutral_color: Color,
    /// Info fill (accent).
    pub info_color: Color,
    /// Success fill.
    pub success_color: Color,
    /// Warning fill.
    pub warning_color: Color,
    /// Danger fill.
    pub danger_color: Color,
}

impl StatusDotTokens {
    /// Derive tokens from a palette (`EnzStatusDot` defaults).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            neutral_color: p.text_muted,
            info_color: p.accent,
            success_color: p.success,
            warning_color: p.warning,
            danger_color: p.destructive,
        }
    }

    /// Dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Light preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Look up the fill colour for a paint [`StatusTone`].
    pub fn color_for_tone(&self, tone: StatusTone) -> Color {
        match tone {
            StatusTone::Neutral => self.neutral_color,
            StatusTone::Info => self.info_color,
            StatusTone::Success => self.success_color,
            StatusTone::Warning => self.warning_color,
            StatusTone::Danger => self.danger_color,
        }
    }

    /// Look up the fill colour for a semantic [`StatusKind`] (via its tone).
    pub fn color_for(&self, kind: StatusKind) -> Color {
        self.color_for_tone(kind.tone())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Construction-time geometry for [`EnzStatusDot`], derived from a concrete
/// theme [`SizingTokens`] ladder.
///
/// # Construction-time seam
///
/// The dot's diameter is consumed as a `Length::Fixed` in `layout()`, and
/// `Widget::layout` receives no `&Theme` (`iced/core/src/widget.rs`,
/// the *no theme in layout* class). So the active
/// [`SizingTokens::status_dot`](enzui_theme::SizingTokens) can never be
/// read at draw time — it has to be supplied when the widget is *built*.
///
/// [`StatusDotGeometry::from_scales`] is that entry point: a view owner holding
/// the live `EnzTheme` calls `.geometry(StatusDotGeometry::from_scales(&theme.sizing))`
/// (or the shorthand [`EnzStatusDot::sizing`]) and the dot's rendered diameter
/// genuinely tracks the theme. `.size()` remains a per-instance override with
/// higher precedence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusDotGeometry {
    /// Dot diameter in px (`EnzStatusDot.dot-size`, 8 by default).
    ///
    /// Private: all constructors (`from_scales`, `new`, `defaults`) sanitize
    /// the value. Direct `StatusDotGeometry { diameter: f32::NAN }` is
    /// prevented so NaN / infinity / negatives can never bypass the check
    /// and reach `Length::Fixed` / border radius.
    diameter: f32,
}

impl StatusDotGeometry {
    /// Derive the dot diameter from the supplied sizing ladder.
    /// Nonfinite or negative values use the stock diameter; zero hides the dot.
    pub fn from_scales(sizing: &SizingTokens) -> Self {
        Self {
            diameter: sane_size(sizing.status_dot, SizingTokens::default().status_dot),
        }
    }

    /// Construct from a raw diameter value. `0` is accepted (an invisible
    /// dot is valid intent); non-finite and negative values
    /// fall back to the stock default (8px).
    pub fn new(diameter: f32) -> Self {
        Self {
            diameter: sane_size(diameter, SizingTokens::default().status_dot),
        }
    }

    /// The sanitized diameter in px.
    pub fn diameter(&self) -> f32 {
        self.diameter
    }

    /// Derive from the default sizing ladder: an 8px diameter.
    ///
    /// `diameter` becomes the container's `Length::Fixed`
    /// width/height, consumed in `layout()`, where iced supplies no `&Theme`.
    /// Callers with a custom ladder use [`StatusDotGeometry::from_scales`] and
    /// pin the result via [`EnzStatusDot::geometry`].
    pub fn defaults() -> Self {
        Self::from_scales(&SizingTokens::default())
    }
}

impl Default for StatusDotGeometry {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Returns `v` if finite and non-negative, else `fallback`.
///
/// `0` is accepted: a zero-diameter dot is valid intent ("no dot"),
/// not an error, and is honoured literally. NaN, infinity, and negatives are
/// rejected.
#[inline]
fn sane_size(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        fallback
    }
}

/// A themed status-dot indicator.
///
/// Constructed via [`enz_status_dot`]. Use the builder chain to override
/// size and tokens, then convert into an [`Element`].
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzStatusDot {
    kind: StatusKind,
    /// Explicit per-instance size override. `None` means "use the geometry
    /// record" — so the theme-derived token is the authority for the default
    /// diameter.
    size: Option<f32>,
    /// Construction-time geometry derived from the active sizing ladder
    /// ([`StatusDotGeometry::from_scales`]). Defaults to the stock ladder.
    geometry: StatusDotGeometry,
    tokens: Option<StatusDotTokens>,
    opacity: f32,
    /// Whether this dot publishes its own accessibility node.
    /// `true` by default for a standalone dot; composed consumers whose parent
    /// already speaks the status set [`EnzStatusDot::decorative`].
    announce: bool,
    motion: StatusDotMotion,
    reduced_motion: Option<bool>,
}

/// Whether a status dot moves, to show a live or ongoing state (recording,
/// a call in progress, a sync running).
///
/// The motion is played by the compositor: the dot is painted once and the
/// lane loops its opacity or scale, so a moving dot wakes the UI thread once
/// every few seconds rather than every frame. It holds still under reduced
/// motion and while scrolled out of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusDotMotion {
    /// No motion (default).
    #[default]
    Still,
    /// The dot fades gently down and back up every 2.4 s.
    Breathe,
    /// A soft halo of the dot's colour grows out of it and fades every 2 s.
    Ripple,
}

/// How long one breath takes.
const BREATHE_PERIOD: iced::time::Duration = iced::time::Duration::from_millis(2400);

/// How long one ripple takes.
const RIPPLE_PERIOD: iced::time::Duration = iced::time::Duration::from_millis(2000);

/// How far a ripple's halo grows, as a multiple of the dot's diameter.
const RIPPLE_REACH: f32 = 2.6;

/// A breath: full at the start of the period, down to 40% half way.
fn breathe(t: f32) -> crate::lane_loop::LoopSample {
    let wave = 0.5 + 0.5 * (t * std::f32::consts::TAU).cos();
    crate::lane_loop::LoopSample {
        scale: 1.0,
        opacity: 0.4 + 0.6 * wave,
    }
}

/// A ripple: the halo eases out from the dot's size to [`RIPPLE_REACH`],
/// fading out on the way, then rests unseen for the last third.
fn ripple(t: f32) -> crate::lane_loop::LoopSample {
    let travel = (t / 0.66).min(1.0);
    let eased = 1.0 - (1.0 - travel).powi(3);
    crate::lane_loop::LoopSample {
        scale: (1.0 + (RIPPLE_REACH - 1.0) * eased) / RIPPLE_REACH,
        opacity: 0.45 * (1.0 - travel).powi(2),
    }
}

impl EnzStatusDot {
    /// Override the dot size, ignoring the sizing token. `0` is accepted
    /// (an invisible dot is valid intent); non-finite and negative values
    /// are rejected (fall back to the token diameter).
    pub fn size(mut self, size: f32) -> Self {
        self.size = if size.is_finite() && size >= 0.0 {
            Some(size)
        } else {
            None
        };
        self
    }

    /// Pin the construction-time [`StatusDotGeometry`] — the theme-driven
    /// seam. A view owner holding the live `EnzTheme` passes
    /// `StatusDotGeometry::from_scales(&theme.sizing)`.
    ///
    /// `geometry` is already sanitized by its constructors (`from_scales`,
    /// `new`, `defaults`), so this setter does not re-check. The `diameter`
    /// field is private, preventing unsanitized construction.
    pub fn geometry(mut self, geometry: StatusDotGeometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// Shorthand for [`Self::geometry`]`(StatusDotGeometry::from_scales(&sizing))`:
    /// pin the sizing ladder the default diameter is derived from.
    ///
    /// Changing `sizing.status_dot` genuinely changes the rendered diameter of
    /// an un-`.size()`-overridden dot.
    pub fn sizing(mut self, sizing: SizingTokens) -> Self {
        self.geometry = StatusDotGeometry::from_scales(&sizing);
        self
    }

    /// Suppress this dot's own accessibility node.
    ///
    /// Use inside a composed control whose **parent already speaks the
    /// status** — e.g. a list row that folds the status into its
    /// `ListItem` node's description. Without this the row would announce the
    /// status twice (once on the row, once on the dot), which is exactly the
    /// double-publication class `collect.rs` exists to prevent.
    pub fn decorative(mut self) -> Self {
        self.announce = false;
        self
    }

    /// The diameter this dot resolves to (`.size()` override, else the
    /// theme-derived [`StatusDotGeometry`]). Exposed for layout callers and
    /// tests.
    pub fn resolved_size(&self) -> f32 {
        self.size.unwrap_or_else(|| self.geometry.diameter())
    }

    /// Let the dot move to show a live state; see [`StatusDotMotion`].
    pub fn motion(mut self, motion: StatusDotMotion) -> Self {
        self.motion = motion;
        self
    }

    /// Pin the motion policy instead of following the theme's reduced-motion
    /// preference.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Override the colour tokens.
    pub fn tokens(mut self, tokens: StatusDotTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set the dot opacity. Non-finite values fall back to fully opaque;
    /// finite values are clamped to `[0, 1]`.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self
    }

    /// Sanitized opacity (finite, clamped to `[0, 1]`).
    #[inline]
    fn resolved_opacity(&self) -> f32 {
        if self.opacity.is_finite() {
            self.opacity.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Resolve the fully-composed `container::Style` for a given runtime
    /// `theme`. This is exactly what the render closure emits, factored out so
    /// tests can assert the painted geometry (radius) and colour (fill after
    /// the alpha multiply) for both the runtime and pinned-token paths.
    fn resolved_style(&self, theme: &iced::Theme) -> container::Style {
        let radius = self.resolved_size() / 2.0;
        let opacity = self.resolved_opacity();
        let tokens = crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_STATUS_DOT,
            self.tokens,
            theme,
            StatusDotTokens::from_palette,
        );
        let mut fill = tokens.color_for(self.kind);
        fill.a *= opacity;
        container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: radius.into(),
            },
            ..Default::default()
        }
    }

    /// Convert into an iced [`Element`].
    ///
    /// Unless [`decorative`](Self::decorative) was called, the dot publishes a
    /// passive `Role::Label` node carrying [`StatusKind::label`], so the
    /// status reaches the accessibility tree instead of being colour-only.
    /// It is a [`SemanticsContainer::leaf`](crate::semantics_container::SemanticsContainer::leaf) because the dot is a
    /// single `Container<Space>` whose semantic node fully represents it.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let size = self.resolved_size();
        let announce = self.announce;
        let kind = self.kind;
        let motion = self.motion;
        let reduced_motion = self.reduced_motion;
        let disc = |diameter: f32, styler: EnzStatusDot| {
            container(
                Space::new()
                    .width(Length::Fixed(diameter))
                    .height(Length::Fixed(diameter)),
            )
            .width(Length::Fixed(diameter))
            .height(Length::Fixed(diameter))
            .style(move |theme: &iced::Theme| {
                let mut style = styler.resolved_style(theme);
                style.border.radius = (diameter / 2.0).into();
                style
            })
        };

        let still = crate::lane_loop::LoopSample {
            scale: 1.0,
            opacity: 1.0,
        };
        let dot: Element<'a, Message> = match motion {
            StatusDotMotion::Still => disc(size, self).into(),
            StatusDotMotion::Breathe => {
                crate::lane_loop::LaneLoop::new(disc(size, self), BREATHE_PERIOD, breathe, still)
                    .reduced_motion(reduced_motion)
                    .into()
            }
            StatusDotMotion::Ripple => {
                let reach = size * RIPPLE_REACH;
                let halo = crate::lane_loop::LaneLoop::new(
                    disc(reach, self.clone()),
                    RIPPLE_PERIOD,
                    ripple,
                    crate::lane_loop::LoopSample {
                        scale: 1.0,
                        opacity: 0.0,
                    },
                )
                .reduced_motion(reduced_motion)
                .footprint(iced::Size::new(size, size));
                iced::widget::stack![halo, disc(size, self)]
                    .width(Length::Fixed(size))
                    .height(Length::Fixed(size))
                    .into()
            }
        };

        if !announce {
            return dot;
        }

        crate::semantics_container::SemanticsContainer::leaf(
            dot,
            Semantics::new(Role::Label).label(kind.label()),
        )
        .into()
    }
}

impl<'a, Message: 'a> From<EnzStatusDot> for Element<'a, Message> {
    fn from(dot: EnzStatusDot) -> Self {
        dot.into_element()
    }
}

/// Construct a new [`EnzStatusDot`] with the given semantic kind.
pub fn enz_status_dot(kind: StatusKind) -> EnzStatusDot {
    EnzStatusDot {
        kind,
        size: None,
        geometry: StatusDotGeometry::defaults(),
        tokens: None,
        opacity: 1.0,
        announce: true,
        motion: StatusDotMotion::Still,
        reduced_motion: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    // -- Shared-token authority ------------------------------------------
    //
    // These tests must fail if the dot reverts to a hard-coded default
    // literal. Each pins a non-8 sizing set
    // and asserts the *rendered geometry* follows.

    // -- Style resolution: runtime vs pinned -----------------------------
}
