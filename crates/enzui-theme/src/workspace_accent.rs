//! Preset-aware workspace accent keys, swatches, and tab-wash contrast helpers.
//!
//! [`WorkspaceAccentKey`] is stable across theme changes;
//! [`WorkspaceAccentPalette`] resolves each key to a dark- or light-preset
//! colour.

use crate::{ContrastFloor, Palette};
use iced_core::Color;

/// Stable workspace-accent identities that resolve to preset-specific colours.
/// Persist the key rather than its sRGB value so theme changes retune the hue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspaceAccentKey {
    /// A restrained cool blue — the default recommendation.
    Blue,
    /// A muted teal, distinct from Blue and Green.
    Teal,
    /// A calm mid-green.
    Green,
    /// A warm amber (the light preset dulls this away from a pure yellow so
    /// it stays a workspace tag, not a highlighter).
    Amber,
    /// A softened orange, distinct from Amber and Rose.
    Orange,
    /// A dusty rose (rather than a saturated red, which would read as an
    /// alert rather than a tag).
    Rose,
    /// A quiet violet.
    Purple,
}

impl WorkspaceAccentKey {
    /// All seven keys in chooser order.
    pub const ALL: &'static [WorkspaceAccentKey] = &[
        WorkspaceAccentKey::Blue,
        WorkspaceAccentKey::Teal,
        WorkspaceAccentKey::Green,
        WorkspaceAccentKey::Amber,
        WorkspaceAccentKey::Orange,
        WorkspaceAccentKey::Rose,
        WorkspaceAccentKey::Purple,
    ];

    /// Returns the accessible swatch label, stable across preset changes and
    /// distinct from the lowercase persistence key returned by [`Self::id_str`].
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            WorkspaceAccentKey::Blue => "Blue",
            WorkspaceAccentKey::Teal => "Teal",
            WorkspaceAccentKey::Green => "Green",
            WorkspaceAccentKey::Amber => "Amber",
            WorkspaceAccentKey::Orange => "Orange",
            WorkspaceAccentKey::Rose => "Rose",
            WorkspaceAccentKey::Purple => "Purple",
        }
    }

    /// Returns the stable lowercase identifier used for persistence and derived
    /// widget ids.
    #[must_use]
    pub const fn id_str(self) -> &'static str {
        match self {
            WorkspaceAccentKey::Blue => "blue",
            WorkspaceAccentKey::Teal => "teal",
            WorkspaceAccentKey::Green => "green",
            WorkspaceAccentKey::Amber => "amber",
            WorkspaceAccentKey::Orange => "orange",
            WorkspaceAccentKey::Rose => "rose",
            WorkspaceAccentKey::Purple => "purple",
        }
    }

    /// Parses an [`Self::id_str`] value, returning `None` for any unrecognized
    /// or differently cased string.
    #[must_use]
    pub fn from_id_str(s: &str) -> Option<WorkspaceAccentKey> {
        Self::ALL.iter().copied().find(|k| k.id_str() == s)
    }
}

/// Preset-specific colours for every [`WorkspaceAccentKey`].
/// [`Self::for_mode`] selects the set through [`Palette::is_dark`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceAccentPalette {
    /// The Blue pigment for this preset.
    pub blue: Color,
    /// The Teal pigment for this preset.
    pub teal: Color,
    /// The Green pigment for this preset.
    pub green: Color,
    /// The Amber pigment for this preset.
    pub amber: Color,
    /// The Orange pigment for this preset.
    pub orange: Color,
    /// The Rose pigment for this preset.
    pub rose: Color,
    /// The Purple pigment for this preset.
    pub purple: Color,
}

/// Default alpha for compositing a resting workspace accent over its tab base.
///
/// Built-in light and dark accent/base combinations retain body-text contrast
/// at this value.
pub const DEFAULT_WASH_ALPHA_REST: f32 = 0.14;

/// Default selected-tab wash alpha.
///
/// It is stronger than [`DEFAULT_WASH_ALPHA_REST`]. Built-in light and dark
/// accent/base combinations retain body-text contrast at this value.
pub const DEFAULT_WASH_ALPHA_SELECTED: f32 = 0.28;

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.03928 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: Color) -> f32 {
    let (r, g, b) = (
        srgb_to_linear(c.r),
        srgb_to_linear(c.g),
        srgb_to_linear(c.b),
    );
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn wcag_contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Composites an RGBA `overlay` over an opaque `base`.
pub fn composite_over(base: Color, overlay: Color) -> Color {
    let a = overlay.a;
    Color {
        r: base.r * (1.0 - a) + overlay.r * a,
        g: base.g * (1.0 - a) + overlay.g * a,
        b: base.b * (1.0 - a) + overlay.b * a,
        a: 1.0,
    }
}

/// Returns the opaque tab fill produced by compositing `accent` at `alpha`
/// over `base`.
pub fn tab_wash_over(base: Color, accent: Color, alpha: f32) -> Color {
    let overlay = Color {
        a: alpha.clamp(0.0, 1.0),
        ..accent
    };
    composite_over(base, overlay)
}

/// Whether painting `text` on the tab fill produced by
/// [`tab_wash_over`] with `alpha` still clears WCAG 1.4.3 body
/// text (`4.5:1`).
#[must_use]
pub fn tab_wash_passes_body_text(text: Color, base: Color, accent: Color, alpha: f32) -> bool {
    wcag_contrast(text, tab_wash_over(base, accent, alpha)) >= ContrastFloor::Text.minimum_ratio()
}

impl WorkspaceAccentPalette {
    /// Dark-preset accents tuned to remain distinguishable in low-alpha tab
    /// washes.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            // Slightly desaturated hues chosen to sit apart from
            // `Palette::dark().accent` (which is a saturated iOS blue) so a
            // workspace tint does not read as the selection accent.
            blue: Color::from_rgb(0.34, 0.55, 0.92),
            teal: Color::from_rgb(0.20, 0.70, 0.78),
            green: Color::from_rgb(0.46, 0.78, 0.42),
            amber: Color::from_rgb(0.95, 0.78, 0.22),
            orange: Color::from_rgb(0.94, 0.52, 0.28),
            rose: Color::from_rgb(0.92, 0.51, 0.55),
            purple: Color::from_rgb(0.68, 0.55, 0.92),
        }
    }

    /// Light-preset versions of the same seven accents, with deeper values for
    /// low-alpha washes over near-white tabs.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            blue: Color::from_rgb(0.19, 0.42, 0.83),
            teal: Color::from_rgb(0.06, 0.53, 0.62),
            green: Color::from_rgb(0.24, 0.58, 0.24),
            amber: Color::from_rgb(0.78, 0.60, 0.08),
            orange: Color::from_rgb(0.80, 0.36, 0.08),
            rose: Color::from_rgb(0.79, 0.28, 0.36),
            purple: Color::from_rgb(0.44, 0.30, 0.75),
        }
    }

    /// Selects the dark or light accent set from [`Palette::is_dark`].
    #[must_use]
    pub fn for_mode(palette: &Palette) -> Self {
        if palette.is_dark() {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// Resolve one accent key to its preset-tuned [`Color`].
    pub const fn resolve(&self, key: WorkspaceAccentKey) -> Color {
        match key {
            WorkspaceAccentKey::Blue => self.blue,
            WorkspaceAccentKey::Teal => self.teal,
            WorkspaceAccentKey::Green => self.green,
            WorkspaceAccentKey::Amber => self.amber,
            WorkspaceAccentKey::Orange => self.orange,
            WorkspaceAccentKey::Rose => self.rose,
            WorkspaceAccentKey::Purple => self.purple,
        }
    }
}
