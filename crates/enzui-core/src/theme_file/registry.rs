//! Registry of editable [`EnzTheme`] tokens.
//!
//! The registry is the canonical source for token lookup, validation, application, serialization, and editor enumeration. Tokens are either shared or independently scoped to light and dark modes.

use enzui_theme::EnzTheme;
use enzui_theme::font_role::{FontFamily, FontRole, GenericFamily, RawFontDescription};
use enzui_theme::motion::{ElevationTier, MotionEasing};
use enzui_theme::{GrainPlacement, GrainTone};
use iced_core::Color;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

/// Unit a numeric token is expressed in — shown by an editor, never parsed
/// from the file (the file always carries a bare number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Logical pixels.
    Px,
    /// Milliseconds.
    Ms,
    /// A 0..=1 fraction.
    Ratio,
    /// A dimensionless multiplier.
    Scalar,
}

impl Unit {
    /// Short suffix for display (`"px"`, `"ms"`, `""`).
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Unit::Px => "px",
            Unit::Ms => "ms",
            Unit::Ratio | Unit::Scalar => "",
        }
    }
}

/// What kind of value a token accepts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenKind {
    /// An RGBA colour, written `"#rrggbb"` or `"#rrggbbaa"`.
    Color,
    /// A finite number inside `[min, max]`.
    Number {
        /// Smallest accepted value, inclusive.
        min: f32,
        /// Largest accepted value, inclusive.
        max: f32,
        /// Display unit.
        unit: Unit,
    },
    /// A boolean.
    Bool,
    /// One of a closed set of lowercase identifiers.
    Enum(&'static [&'static str]),
    /// A font family description (validated through [`RawFontDescription`]).
    FontFamily,
}

/// A concrete token value that can be validated against a [`TokenKind`].
#[derive(Debug, Clone, PartialEq)]
pub enum TokenValue {
    /// Colour value.
    Color(Color),
    /// Numeric value.
    Number(f32),
    /// Boolean value.
    Bool(bool),
    /// Enum variant, one of the kind's accepted identifiers.
    Enum(String),
    /// Font family description.
    FontFamily(String),
}

impl TokenValue {
    /// The TOML text this value serialises to (right-hand side only).
    #[must_use]
    pub fn to_toml_value(&self) -> String {
        match self {
            TokenValue::Color(c) => format!("\"{}\"", format_color(*c)),
            TokenValue::Number(n) => format_number(*n),
            TokenValue::Bool(b) => b.to_string(),
            TokenValue::Enum(s) | TokenValue::FontFamily(s) => escape_toml_string(s),
        }
    }
}

impl TokenKind {
    /// Returns whether `value` has this kind’s variant and satisfies its domain constraints.
    #[must_use]
    pub fn accepts(self, value: &TokenValue) -> bool {
        match (self, value) {
            (TokenKind::Color, TokenValue::Color(_)) => true,
            (TokenKind::Number { min, max, .. }, TokenValue::Number(n)) => {
                n.is_finite() && *n >= min && *n <= max
            }
            (TokenKind::Bool, TokenValue::Bool(_)) => true,
            (TokenKind::Enum(names), TokenValue::Enum(word)) => names.contains(&word.as_str()),
            (TokenKind::FontFamily, TokenValue::FontFamily(desc)) => {
                parse_font_family(desc).is_some()
            }
            _ => false,
        }
    }
}

/// Whether a token's value reaches a renderer, or is registered but inert.
///
/// An unsupported token is readable and documented but not writable: the loader
/// reports the key instead of storing a value nothing can honour, and an editor
/// presents it non-editable. Keeping it registered (rather than deleting it) is
/// what lets a file that mentions the key receive a diagnostic naming the
/// limitation instead of the generic
/// [`DiagnosticKind::UnknownKey`](super::document::DiagnosticKind::UnknownKey).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSupport {
    /// At least one supported layout or paint consumer honors this value.
    /// This does not make every widget family theme-driven or override explicit pins.
    Rendered,
    /// Registered and readable, without a supported consumer for theme-file overrides.
    /// Carries the user-facing reason for the editor and loader diagnostic.
    Unsupported(&'static str),
}

impl TokenSupport {
    /// Whether an editor may offer this token as writable.
    #[must_use]
    pub const fn is_editable(self) -> bool {
        matches!(self, TokenSupport::Rendered)
    }

    /// The reason this token is inert, when it is.
    #[must_use]
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            TokenSupport::Rendered => None,
            TokenSupport::Unsupported(why) => Some(why),
        }
    }
}

/// Whether a token is per-mode or shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    /// Shared by both colour modes; written at the top level of the file.
    Common,
    /// Written under `[light]` and `[dark]` independently.
    Mode,
}

impl TokenScope {
    /// True for [`TokenScope::Mode`].
    #[must_use]
    pub const fn is_mode(self) -> bool {
        matches!(self, TokenScope::Mode)
    }
}

/// One editable theme token.
#[derive(Clone)]
pub struct TokenDescriptor {
    path: &'static str,
    group: &'static str,
    scope: TokenScope,
    kind: TokenKind,
    support: TokenSupport,
    get: fn(&EnzTheme) -> TokenValue,
    set: fn(&mut EnzTheme, &TokenValue),
}

impl std::fmt::Debug for TokenDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenDescriptor")
            .field("path", &self.path)
            .field("scope", &self.scope)
            .field("kind", &self.kind)
            .field("support", &self.support)
            .finish()
    }
}

impl TokenDescriptor {
    /// Dotted token path, e.g. `palette.accent` or `spacing.space6`.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        self.path
    }
    /// The navigator group this token belongs to (first path segment).
    #[must_use]
    pub const fn group(&self) -> &'static str {
        self.group
    }
    /// Mode-scoped or common.
    #[must_use]
    pub const fn scope(&self) -> TokenScope {
        self.scope
    }
    /// The accepted value kind.
    #[must_use]
    pub const fn kind(&self) -> TokenKind {
        self.kind
    }
    /// Whether this token's value reaches a renderer.
    ///
    /// [`TokenSupport::Unsupported`] tokens are readable (so an editor can show
    /// the inherited value and explain the gap) but not writable through
    /// [`Self::write`] or
    /// [`ThemeDocument::set`](super::document::ThemeDocument::set).
    #[must_use]
    pub const fn support(&self) -> TokenSupport {
        self.support
    }
    /// Read the token out of a theme.
    #[must_use]
    pub fn read(&self, theme: &EnzTheme) -> TokenValue {
        (self.get)(theme)
    }
    /// Writes a value only when this token is [editable](TokenSupport::is_editable)
    /// and the value satisfies its [`kind`](Self::kind); otherwise leaves the
    /// theme unchanged.
    ///
    /// The support check is here, at the single write seam, rather than in each
    /// caller: an inert token must not be installable into a live theme by any
    /// route, or the "nothing consumes it" claim becomes route-dependent.
    pub fn write(&self, theme: &mut EnzTheme, value: &TokenValue) {
        if self.support.is_editable() && self.kind.accepts(value) {
            (self.set)(theme, value);
        }
    }
}

/// Parse `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` into a [`Color`].
///
/// Returns `None` for anything else — no named colours, no `rgb()` functions,
/// no expressions: the file is data, not a language.
#[must_use]
pub fn parse_color(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nib = |c: char| c.to_digit(16).map(|v| v as u8);
    let (r, g, b, a) = match hex.len() {
        3 | 4 => {
            let mut it = hex.chars();
            let r = nib(it.next()?)?;
            let g = nib(it.next()?)?;
            let b = nib(it.next()?)?;
            let a = match it.next() {
                Some(c) => nib(c)?,
                None => 0xF,
            };
            (r * 17, g * 17, b * 17, a * 17)
        }
        6 | 8 => {
            let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            let r = byte(0)?;
            let g = byte(2)?;
            let b = byte(4)?;
            let a = if hex.len() == 8 { byte(6)? } else { 0xFF };
            (r, g, b, a)
        }
        _ => return None,
    };
    Some(Color::from_rgba8(r, g, b, f32::from(a) / 255.0))
}

/// Format a colour as `#rrggbb`, or `#rrggbbaa` when it is not fully opaque.
#[must_use]
pub fn format_color(c: Color) -> String {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b, a) = (q(c.r), q(c.g), q(c.b), q(c.a));
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// The text form of a font family: a generic keyword (`sans-serif`,
/// `serif`, `monospace`, `cursive`, `fantasy`) or the named family verbatim.
#[must_use]
pub fn format_font_family(f: &FontFamily) -> String {
    match f {
        FontFamily::Generic(g) => match g {
            GenericFamily::Serif => "serif",
            GenericFamily::SansSerif => "sans-serif",
            GenericFamily::Cursive => "cursive",
            GenericFamily::Fantasy => "fantasy",
            GenericFamily::Monospace => "monospace",
        }
        .to_owned(),
        FontFamily::Named(n) => n.clone(),
    }
}

/// Parse a font family description.
///
/// The five CSS generic keywords map to [`FontFamily::Generic`] so a default
/// catalog round-trips exactly; anything else goes through the Pango-shaped
/// [`RawFontDescription::validate`], which rejects an empty description.
#[must_use]
pub fn parse_font_family(text: &str) -> Option<FontFamily> {
    let generic = match text.trim() {
        "serif" => Some(GenericFamily::Serif),
        "sans-serif" => Some(GenericFamily::SansSerif),
        "cursive" => Some(GenericFamily::Cursive),
        "fantasy" => Some(GenericFamily::Fantasy),
        "monospace" => Some(GenericFamily::Monospace),
        _ => None,
    };
    match generic {
        Some(g) => Some(FontFamily::Generic(g)),
        None => RawFontDescription::new(text.to_owned()).validate(),
    }
}

/// Quotes `text` as a TOML basic string.
///
/// Quotes, backslashes, and controls are escaped; other Unicode scalars are emitted literally.
#[must_use]
pub fn escape_toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Format a number the way the canonical file writes it: integral values
/// without a fractional part, everything else with up to three decimals.
#[must_use]
pub fn format_number(n: f32) -> String {
    if n.fract() == 0.0 && n.abs() < 1.0e9 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// Accepted identifiers for the `shape.*` tokens: the radius steps.
pub const RADIUS_STEP_NAMES: &[&str] = &[
    "none", "xs", "sm", "md", "lg", "xl", "xxl", "full", "window",
];

macro_rules! shape_token {
    ($v:expr, $field:ident) => {
        $v.push(TokenDescriptor {
            path: concat!("shape.", stringify!($field)),
            group: "shape",
            scope: TokenScope::Common,
            kind: TokenKind::Enum(RADIUS_STEP_NAMES),
            support: TokenSupport::Rendered,
            get: |t| TokenValue::Enum(t.shape.$field.name().to_owned()),
            set: |t, v| {
                if let TokenValue::Enum(name) = v
                    && let Some(step) = enzui_theme::RadiusStep::from_name(name)
                {
                    t.shape.$field = step;
                }
            },
        })
    };
}

/// Accepted identifiers for `motion.easing`.
pub const EASING_NAMES: &[&str] = &["linear", "ease_in", "ease_out", "ease_in_out"];
/// The accepted `grain.tone` identifiers.
pub const GRAIN_TONES: &[&str] = &["both", "dark", "light"];
/// The accepted `grain.placement` identifiers.
pub const GRAIN_PLACEMENTS: &[&str] = &["behind", "over"];

macro_rules! color_token {
    ($v:expr, $field:ident) => {
        $v.push(TokenDescriptor {
            path: concat!("palette.", stringify!($field)),
            group: "palette",
            scope: TokenScope::Mode,
            kind: TokenKind::Color,
            support: TokenSupport::Rendered,
            get: |t| TokenValue::Color(t.palette.$field),
            set: |t, v| {
                if let TokenValue::Color(c) = v {
                    t.palette.$field = *c;
                }
            },
        })
    };
}

macro_rules! num_token {
    ($v:expr, $group:ident, $field:ident, $min:expr, $max:expr, $unit:expr) => {
        num_token!(
            $v,
            $group,
            $field,
            $min,
            $max,
            $unit,
            TokenSupport::Rendered
        )
    };
    ($v:expr, $group:ident, $field:ident, $min:expr, $max:expr, $unit:expr, $support:expr) => {
        $v.push(TokenDescriptor {
            path: concat!(stringify!($group), ".", stringify!($field)),
            group: stringify!($group),
            scope: TokenScope::Common,
            kind: TokenKind::Number {
                min: $min,
                max: $max,
                unit: $unit,
            },
            support: $support,
            get: |t| TokenValue::Number(t.$group.$field),
            set: |t, v| {
                if let TokenValue::Number(n) = v {
                    t.$group.$field = *n;
                }
            },
        })
    };
}

/// Writes an accepted numeric font weight by rounding it to the underlying `u16` field.
macro_rules! int_token {
    ($v:expr, $group:ident, $field:ident, $min:expr, $max:expr) => {
        $v.push(TokenDescriptor {
            path: concat!(stringify!($group), ".", stringify!($field)),
            group: stringify!($group),
            scope: TokenScope::Common,
            kind: TokenKind::Number {
                min: $min,
                max: $max,
                unit: Unit::Scalar,
            },
            support: TokenSupport::Rendered,
            get: |t| TokenValue::Number(f32::from(t.$group.$field)),
            set: |t, v| {
                if let TokenValue::Number(n) = v {
                    t.$group.$field = n.round().clamp($min, $max) as u16;
                }
            },
        })
    };
}

macro_rules! num_group {
    ($v:expr, $group:ident, $min:expr, $max:expr, $unit:expr, [$($field:ident),* $(,)?]) => {
        $( num_token!($v, $group, $field, $min, $max, $unit); )*
    };
    ($v:expr, $group:ident, $min:expr, $max:expr, $unit:expr, $support:expr, [$($field:ident),* $(,)?]) => {
        $( num_token!($v, $group, $field, $min, $max, $unit, $support); )*
    };
}

fn build() -> Vec<TokenDescriptor> {
    let mut v: Vec<TokenDescriptor> = Vec::new();

    color_token!(v, background);
    color_token!(v, surface);
    color_token!(v, surface_alt);
    color_token!(v, surface_elevated);
    color_token!(v, surface_hover);
    color_token!(v, surface_selected);
    color_token!(v, text);
    color_token!(v, text_muted);
    color_token!(v, text_subtle);
    color_token!(v, image_canvas);
    color_token!(v, video_canvas);
    color_token!(v, border);
    color_token!(v, border_strong);
    color_token!(v, control_border);
    color_token!(v, control_border_strong);
    color_token!(v, field);
    color_token!(v, field_border);
    color_token!(v, field_border_strong);
    color_token!(v, accent);
    color_token!(v, accent_hover);
    color_token!(v, accent_pressed);
    color_token!(v, accent_soft);
    color_token!(v, danger);
    color_token!(v, danger_soft);
    color_token!(v, destructive);
    color_token!(v, success);
    color_token!(v, success_soft);
    color_token!(v, warning);
    color_token!(v, warning_soft);
    color_token!(v, rating);
    color_token!(v, info);
    color_token!(v, info_soft);
    color_token!(v, tone_success_fg);
    color_token!(v, tone_warning_fg);
    color_token!(v, tone_error_fg);
    color_token!(v, overlay);
    color_token!(v, shadow);
    color_token!(v, focus_ring);
    color_token!(v, disabled);
    color_token!(v, selected);
    color_token!(v, error);
    color_token!(v, button_primary_bg);
    color_token!(v, button_primary_hover_bg);
    color_token!(v, button_primary_pressed_bg);

    num_group!(
        v,
        typography,
        1.0,
        256.0,
        Unit::Px,
        [
            caption_size,
            label_size,
            monospace_size,
            statusbar_text_size,
            footnote_size,
            body_size,
            section_size,
            title3_size,
            title_size,
            title1_size,
            large_title_size,
            hero_size,
        ]
    );
    // Registered, not writable: a file mentioning tracking earns a "not
    // rendered" diagnostic rather than "unknown key", but iced's text
    // descriptor has no tracking field, so an accepted edit would move no glyph.
    num_group!(
        v,
        typography,
        -2.0,
        4.0,
        Unit::Px,
        TokenSupport::Unsupported(enzui_theme::LETTER_SPACING_UNSUPPORTED),
        [
            letter_spacing_tight,
            letter_spacing_normal,
            letter_spacing_wide,
        ]
    );

    int_token!(v, typography, weight_regular, 1.0, 1000.0);
    int_token!(v, typography, weight_medium, 1.0, 1000.0);
    int_token!(v, typography, weight_semibold, 1.0, 1000.0);
    int_token!(v, typography, weight_bold, 1.0, 1000.0);
    int_token!(v, typography, weight_extrabold, 1.0, 1000.0);

    // Registered after every size rung so that document application scales the
    // rungs the file pinned, whatever order the file wrote them in.
    v.push(TokenDescriptor {
        path: "typography.text_scale",
        group: "typography",
        scope: TokenScope::Common,
        kind: TokenKind::Number {
            min: enzui_theme::text_scale::MIN_FACTOR,
            max: enzui_theme::text_scale::MAX_FACTOR,
            unit: Unit::Scalar,
        },
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Number(t.typography.text_scale),
        set: |t, v| {
            if let TokenValue::Number(n) = v {
                t.typography = t
                    .typography
                    .with_text_scale(enzui_theme::TextScale::from_backend(*n));
            }
        },
    });

    num_group!(
        v,
        spacing,
        0.0,
        512.0,
        Unit::Px,
        [
            space0, space1, space2, space3, space4, space5, space6, space7, space8, space9,
            space10, space11, space12, space13, space14, space15,
        ]
    );

    num_group!(
        v,
        radius,
        0.0,
        4096.0,
        Unit::Px,
        [none, xs, sm, md, lg, xl, xxl, full]
    );

    shape_token!(v, button);
    shape_token!(v, button_group);
    shape_token!(v, search_field);
    shape_token!(v, badge);
    shape_token!(v, card);
    shape_token!(v, checkbox);
    shape_token!(v, field);
    shape_token!(v, dialog);
    shape_token!(v, popover);
    shape_token!(v, popup);
    shape_token!(v, row);
    shape_token!(v, tooltip);
    shape_token!(v, tab);
    shape_token!(v, key);
    shape_token!(v, media);

    num_group!(
        v,
        sizing,
        0.0,
        8192.0,
        Unit::Px,
        [control_height, min_hit_target, control_min_hit,]
    );
    num_token!(v, sizing, row_height, 0.0, 8192.0, Unit::Px);
    num_group!(
        v,
        sizing,
        0.0,
        8192.0,
        Unit::Px,
        [
            toolbar_height,
            titlebar_height,
            field_height,
            search_field_width,
            sidebar_width,
            sidebar_row_height,
            sidebar_min_width,
            sidebar_max_width,
            window_min_width,
            window_min_height,
            window_default_width,
            window_default_height,
            dialog_width_sm,
            dialog_width_md,
            dialog_width_lg,
            touch_target,
            statusbar_height,
            icon_xxs,
            icon_xs,
            icon_sm,
            icon_md,
            icon_lg,
            icon_xl,
            icon_xxl,
            status_dot,
            container_narrow,
            container_default,
            container_wide,
            container_xwide,
            layout_switch_narrow,
            layout_switch_stack,
            layout_switch_compact,
            layout_switch_wide,
        ]
    );

    num_group!(v, stroke, 0.0, 64.0, Unit::Px, [hairline, focus_ring]);

    num_group!(
        v,
        opacity,
        0.0,
        1.0,
        Unit::Ratio,
        [disabled, disabled_surface]
    );

    num_token!(v, opacity, drop_target, 0.0, 0.2, Unit::Ratio);

    num_group!(
        v,
        scrollbar,
        0.0,
        256.0,
        Unit::Px,
        [
            width,
            hit_width,
            min_thumb_length,
            content_gutter,
            border_radius
        ]
    );
    num_group!(v, scrollbar, 0.0, 1.0, Unit::Ratio, [alpha, alpha_hover]);

    num_group!(
        v,
        motion,
        0.0,
        10_000.0,
        Unit::Ms,
        [fast_ms, normal_ms, slow_ms]
    );
    v.push(TokenDescriptor {
        path: "motion.easing",
        group: "motion",
        scope: TokenScope::Common,
        kind: TokenKind::Enum(EASING_NAMES),
        support: TokenSupport::Rendered,
        get: |t| {
            TokenValue::Enum(
                match t.motion.easing {
                    MotionEasing::Linear => "linear",
                    MotionEasing::EaseIn => "ease_in",
                    MotionEasing::EaseOut => "ease_out",
                    MotionEasing::EaseInOut => "ease_in_out",
                }
                .to_owned(),
            )
        },
        set: |t, v| {
            if let TokenValue::Enum(name) = v {
                t.motion.easing = match name.as_str() {
                    "linear" => MotionEasing::Linear,
                    "ease_in" => MotionEasing::EaseIn,
                    "ease_in_out" => MotionEasing::EaseInOut,
                    _ => MotionEasing::EaseOut,
                };
            }
        },
    });
    v.push(TokenDescriptor {
        path: "motion.reduced_motion",
        group: "motion",
        scope: TokenScope::Common,
        kind: TokenKind::Bool,
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Bool(t.motion.reduced_motion),
        set: |t, v| {
            if let TokenValue::Bool(b) = v {
                t.motion.reduced_motion = *b;
            }
        },
    });

    // The theme's grain: what a surface asking for `GrainSpec::Theme` draws.
    macro_rules! grain_number {
        ($path:literal, $field:ident, $min:expr, $max:expr, $unit:expr) => {
            v.push(TokenDescriptor {
                path: $path,
                group: "grain",
                scope: TokenScope::Common,
                kind: TokenKind::Number {
                    min: $min,
                    max: $max,
                    unit: $unit,
                },
                support: TokenSupport::Rendered,
                get: |t| TokenValue::Number(t.grain.$field),
                set: |t, v| {
                    if let TokenValue::Number(n) = v {
                        t.grain.$field = (*n).clamp($min, $max);
                    }
                },
            });
        };
    }
    grain_number!("grain.intensity", intensity, 0.0, 1.0, Unit::Ratio);
    grain_number!("grain.density", density, 0.0, 1.0, Unit::Ratio);
    grain_number!("grain.scale", scale, 0.25, 8.0, Unit::Scalar);
    grain_number!("grain.opacity", opacity, 0.0, 1.0, Unit::Ratio);
    grain_number!("grain.frequency", frequency, 0.05, 1.5, Unit::Scalar);
    v.push(TokenDescriptor {
        path: "grain.octaves",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Number {
            min: 1.0,
            max: 6.0,
            unit: Unit::Scalar,
        },
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Number(f32::from(t.grain.octaves)),
        set: |t, v| {
            if let TokenValue::Number(n) = v {
                // Clamped to 1..=6 first, so the cast cannot truncate.
                t.grain.octaves = n.round().clamp(1.0, 6.0) as u8;
            }
        },
    });
    v.push(TokenDescriptor {
        path: "grain.seed",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Number {
            min: 0.0,
            max: 65_535.0,
            unit: Unit::Scalar,
        },
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Number(f32::from(u16::try_from(t.grain.seed).unwrap_or(u16::MAX))),
        set: |t, v| {
            if let TokenValue::Number(n) = v {
                // Clamped to the u16 range first, so the cast cannot truncate.
                t.grain.seed = u32::from(n.round().clamp(0.0, 65_535.0) as u16);
            }
        },
    });
    grain_number!("grain.tile", tile, 32.0, 1024.0, Unit::Px);
    v.push(TokenDescriptor {
        path: "grain.motion_fps",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Number {
            min: 0.0,
            max: 60.0,
            unit: Unit::Scalar,
        },
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Number(t.grain.motion.unwrap_or(0.0)),
        set: |t, v| {
            if let TokenValue::Number(n) = v {
                t.grain.motion = (*n > 0.0).then(|| n.clamp(1.0, 60.0));
            }
        },
    });
    v.push(TokenDescriptor {
        path: "grain.dark_ink",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Color,
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Color(t.grain.dark_ink),
        set: |t, v| {
            if let TokenValue::Color(c) = v {
                t.grain.dark_ink = *c;
            }
        },
    });
    v.push(TokenDescriptor {
        path: "grain.light_ink",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Color,
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Color(t.grain.light_ink),
        set: |t, v| {
            if let TokenValue::Color(c) = v {
                t.grain.light_ink = *c;
            }
        },
    });
    v.push(TokenDescriptor {
        path: "grain.tone",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Enum(GRAIN_TONES),
        support: TokenSupport::Rendered,
        get: |t| {
            TokenValue::Enum(
                match t.grain.tone {
                    GrainTone::Both => "both",
                    GrainTone::Dark => "dark",
                    GrainTone::Light => "light",
                }
                .to_owned(),
            )
        },
        set: |t, v| {
            if let TokenValue::Enum(name) = v {
                t.grain.tone = match name.as_str() {
                    "dark" => GrainTone::Dark,
                    "light" => GrainTone::Light,
                    _ => GrainTone::Both,
                };
            }
        },
    });
    v.push(TokenDescriptor {
        path: "grain.placement",
        group: "grain",
        scope: TokenScope::Common,
        kind: TokenKind::Enum(GRAIN_PLACEMENTS),
        support: TokenSupport::Rendered,
        get: |t| {
            TokenValue::Enum(
                match t.grain.placement {
                    GrainPlacement::Behind => "behind",
                    GrainPlacement::Over => "over",
                }
                .to_owned(),
            )
        },
        set: |t, v| {
            if let TokenValue::Enum(name) = v {
                t.grain.placement = if name == "over" {
                    GrainPlacement::Over
                } else {
                    GrainPlacement::Behind
                };
            }
        },
    });

    // The corner radius of a self-drawn window frame; its panes follow, 2 px
    // tighter.
    v.push(TokenDescriptor {
        path: "window.radius",
        group: "window",
        scope: TokenScope::Common,
        kind: TokenKind::Number {
            min: 0.0,
            max: 48.0,
            unit: Unit::Px,
        },
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Number(t.window_radius),
        set: |t, v| {
            if let TokenValue::Number(n) = v {
                t.window_radius = n.clamp(0.0, 48.0);
            }
        },
    });

    // The shadow and edge lines of a self-drawn window frame.
    num_group!(v, window_shadow, 0.0, 64.0, Unit::Px, [size]);
    num_group!(v, window_shadow, -32.0, 32.0, Unit::Px, [offset]);
    num_group!(
        v,
        window_shadow,
        0.0,
        1.0,
        Unit::Ratio,
        [opacity, inactive_opacity, outline, highlight]
    );

    // Which controls a self-drawn window frame shows. The default follows the
    // desktop's button layout.
    v.push(TokenDescriptor {
        path: "window.buttons",
        group: "window",
        scope: TokenScope::Common,
        kind: TokenKind::Enum(enzui_theme::WindowButtons::NAMES),
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Enum(t.window_buttons.name().to_owned()),
        set: |t, v| {
            if let TokenValue::Enum(name) = v
                && let Some(buttons) = enzui_theme::WindowButtons::from_name(name)
            {
                t.window_buttons = buttons;
            }
        },
    });

    // What every self-drawn window frame is made of. The default leaves it
    // to each application.
    v.push(TokenDescriptor {
        path: "window.frame",
        group: "window",
        scope: TokenScope::Common,
        kind: TokenKind::Enum(enzui_theme::WindowFrame::NAMES),
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Enum(t.window_frame.name().to_owned()),
        set: |t, v| {
            if let TokenValue::Enum(name) = v
                && let Some(frame) = enzui_theme::WindowFrame::from_name(name)
            {
                t.window_frame = frame;
            }
        },
    });

    macro_rules! bp_token {
        ($field:ident, $idx:expr) => {
            v.push(TokenDescriptor {
                path: concat!("breakpoints.", stringify!($field)),
                group: "breakpoints",
                scope: TokenScope::Common,
                kind: TokenKind::Number {
                    min: 0.0,
                    max: 16_384.0,
                    unit: Unit::Px,
                },
                support: TokenSupport::Rendered,
                get: |t| TokenValue::Number(t.breakpoints.$field()),
                set: |t, v| {
                    if let TokenValue::Number(n) = v {
                        let b = t.breakpoints;
                        let mut edges = [b.xs(), b.sm(), b.md(), b.lg(), b.xl()];
                        edges[$idx] = *n;
                        t.breakpoints = enzui_theme::breakpoints::BreakpointScale::new(
                            edges[0], edges[1], edges[2], edges[3], edges[4],
                        );
                    }
                },
            })
        };
    }
    bp_token!(xs, 0);
    bp_token!(sm, 1);
    bp_token!(md, 2);
    bp_token!(lg, 3);
    bp_token!(xl, 4);

    macro_rules! elev_token {
        ($level:expr, $part:ident, $min:expr) => {
            v.push(TokenDescriptor {
                path: concat!("elevation.level", $level, ".", stringify!($part)),
                group: "elevation",
                scope: TokenScope::Common,
                kind: TokenKind::Number {
                    min: $min,
                    max: 256.0,
                    unit: Unit::Px,
                },
                support: TokenSupport::Rendered,
                get: |t| TokenValue::Number(t.elevation.tier($level).$part),
                set: |t, v| {
                    if let TokenValue::Number(n) = v {
                        let old = t.elevation.tier($level);
                        let mut tier = ElevationTier::new(old.blur, old.y_offset);
                        tier.$part = *n;
                        t.elevation = t.elevation.with_tier($level, tier);
                    }
                },
            })
        };
    }
    v.push(TokenDescriptor {
        path: "elevation.button",
        group: "elevation",
        scope: TokenScope::Common,
        kind: TokenKind::Number {
            min: 0.0,
            max: 5.0,
            unit: Unit::Scalar,
        },
        support: TokenSupport::Rendered,
        get: |t| TokenValue::Number(f32::from(t.elevation.button())),
        set: |t, v| {
            if let TokenValue::Number(n) = v {
                t.elevation = t.elevation.with_button(n.round().clamp(0.0, 5.0) as u8);
            }
        },
    });
    elev_token!(0, blur, 0.0);
    elev_token!(0, y_offset, -256.0);
    elev_token!(1, blur, 0.0);
    elev_token!(1, y_offset, -256.0);
    elev_token!(2, blur, 0.0);
    elev_token!(2, y_offset, -256.0);
    elev_token!(3, blur, 0.0);
    elev_token!(3, y_offset, -256.0);
    elev_token!(4, blur, 0.0);
    elev_token!(4, y_offset, -256.0);
    elev_token!(5, blur, 0.0);
    elev_token!(5, y_offset, -256.0);

    macro_rules! font_token {
        ($name:ident, $role:expr) => {
            v.push(TokenDescriptor {
                path: concat!("fonts.", stringify!($name)),
                group: "fonts",
                scope: TokenScope::Common,
                kind: TokenKind::FontFamily,
                support: TokenSupport::Rendered,
                get: |t| TokenValue::FontFamily(format_font_family(t.font_catalog.family($role))),
                set: |t, v| {
                    if let TokenValue::FontFamily(desc) = v
                        && let Some(family) = parse_font_family(desc)
                    {
                        t.font_catalog.set_family($role, family);
                    }
                },
            })
        };
    }
    font_token!(ui, FontRole::Ui);
    font_token!(document, FontRole::Document);
    font_token!(monospace, FontRole::Monospace);

    v
}

static TOKENS: OnceLock<Vec<TokenDescriptor>> = OnceLock::new();
static BY_PATH: OnceLock<BTreeMap<&'static str, usize>> = OnceLock::new();

/// Every editable token, in canonical file order.
#[must_use]
pub fn tokens() -> &'static [TokenDescriptor] {
    TOKENS.get_or_init(build)
}

/// Look a token up by dotted path.
#[must_use]
pub fn token(path: &str) -> Option<&'static TokenDescriptor> {
    let index = BY_PATH.get_or_init(|| {
        tokens()
            .iter()
            .enumerate()
            .map(|(i, d)| (d.path, i))
            .collect()
    });
    index.get(path).map(|&i| &tokens()[i])
}

/// Every distinct group name, in canonical order (`palette` first).
#[must_use]
pub fn groups() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for d in tokens() {
        if !seen.contains(&d.group) {
            seen.push(d.group);
        }
    }
    seen
}
