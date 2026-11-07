//! Shared ambient-text color precedence: pin, resolver, tone, then surface fallback.
//!
//! Palette-only resolution uses a palette callback. The theme-aware path prefers
//! a theme callback and otherwise falls back to palette resolution. Installing a
//! theme callback with [`AmbientInk::with_theme_resolver`] creates both handles;
//! its palette-only bridge reconstructs default non-color scales.
//!
//! [`BoundInk`] carries the surface fallback. At [`crate::rich_text`] entry
//! points, an absent ladder inherits ancestor ink, while a present empty ladder
//! resolves its own fallback. [`AmbientInk::is_set`] does not distinguish them.

use crate::enz_text::{TextTokens, TextTone};
use enzui_theme::Palette;
use iced::Color;

/// Shared callback resolving ink from a palette whenever resolution requests it.
pub type InkResolver = std::rc::Rc<dyn Fn(&Palette) -> Color>;

/// Shared callback resolving ink from a full theme without reconstructing its scales.
pub type InkThemeResolver = std::rc::Rc<dyn Fn(&iced::Theme) -> Color>;

fn theme_carrying(palette: &Palette) -> iced::Theme {
    // The reconstructed theme’s mode must agree with its palette for debug validation.
    let mut enz = if palette.is_dark() {
        enzui_theme::EnzTheme::dark()
    } else {
        enzui_theme::EnzTheme::light()
    };
    enz.palette = palette.clone();
    enz.iced_theme()
}

/// Surface-owned default ink used when caller overrides are absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Full-strength palette body text.
    Text,
    /// Muted palette text.
    Muted,
}

impl Fallback {
    /// Resolve this fallback against `palette`.
    pub fn color(self, palette: &Palette) -> Color {
        match self {
            Self::Text => palette.text,
            Self::Muted => palette.text_muted,
        }
    }
}

/// Caller color overrides resolved in pin, resolver, tone, fallback order.
///
/// Defaults to all fields unset. Cloning shares callbacks without copying their captures.
#[derive(Clone, Default)]
#[must_use]
pub struct AmbientInk {
    /// Absolute color override, preferred by both resolution paths.
    pub pin: Option<Color>,
    /// Palette callback, preferred over tone and fallback by palette-only resolution.
    pub resolver: Option<InkResolver>,
    /// Theme callback, preferred over the palette callback by theme-aware resolution.
    ///
    /// Direct assignment does not install a palette bridge or affect [`Self::is_set`].
    /// [`Self::with_theme_resolver`] installs both callback handles.
    pub theme_resolver: Option<InkThemeResolver>,
    /// Semantic palette role, used when neither pin nor applicable resolver is present.
    pub tone: Option<TextTone>,
}

impl std::fmt::Debug for AmbientInk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmbientInk")
            .field("pin", &self.pin)
            .field("resolver", &self.resolver.is_some())
            .field("theme_resolver", &self.theme_resolver.is_some())
            .field("tone", &self.tone)
            .finish()
    }
}

impl AmbientInk {
    /// Creates an empty ladder that resolves the supplied surface fallback.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs shared theme and palette callback handles, replacing both resolver fields.
    ///
    /// Theme-aware resolution passes the actual theme. Palette-only resolution
    /// reconstructs a theme with that palette and default non-color scales; use
    /// [`Self::resolve_with_theme`] to retain caller typography, spacing, and motion.
    /// Existing pin and tone remain unchanged.
    pub fn with_theme_resolver(mut self, f: impl Fn(&iced::Theme) -> Color + 'static) -> Self {
        let f = std::rc::Rc::new(f);
        let for_palette = std::rc::Rc::clone(&f);
        self.resolver = Some(std::rc::Rc::new(move |p: &Palette| -> Color {
            for_palette(&theme_carrying(p))
        }));
        self.theme_resolver = Some(std::rc::Rc::new(move |t: &iced::Theme| -> Color { f(t) }));
        self
    }

    /// Resolves palette ink in pin, palette callback, tone, fallback order.
    ///
    /// A theme callback assigned directly is ignored by this path.
    pub fn resolve(&self, palette: &Palette, fallback: Fallback) -> Color {
        match (self.pin, self.resolver.as_ref(), self.tone) {
            (Some(c), _, _) => c,
            (None, Some(f), _) => f(palette),
            (None, None, Some(tone)) => TextTokens::from_palette(palette).color_for(tone),
            (None, None, None) => fallback.color(palette),
        }
    }

    /// Resolves ink in pin, theme callback, then palette-resolution order.
    ///
    /// The theme callback receives `theme` unchanged, preserving non-color scales.
    pub fn resolve_with_theme(&self, theme: &iced::Theme, fallback: Fallback) -> Color {
        match (self.pin, self.theme_resolver.as_ref(), self.tone) {
            (Some(c), _, _) => c,
            (None, Some(f), _) => f(theme),
            (None, None, _) => self.resolve(&crate::theme_lookup::palette(theme), fallback),
        }
    }

    /// Reports whether a pin, palette resolver, or tone is set.
    ///
    /// A directly assigned theme resolver alone is not counted. An empty ladder
    /// still resolves its surface fallback; use `Option<BoundInk>` when absence
    /// must mean inherited ancestor ink.
    pub fn is_set(&self) -> bool {
        self.pin.is_some() || self.resolver.is_some() || self.tone.is_some()
    }

    /// Pairs this ladder with the surface’s fallback for transport across widget APIs.
    pub fn bound(self, fallback: Fallback) -> BoundInk {
        BoundInk {
            ink: self,
            fallback,
        }
    }
}

/// A color ladder paired with its surface-owned fallback.
#[derive(Clone, Debug)]
pub struct BoundInk {
    ink: AmbientInk,
    fallback: Fallback,
}

impl BoundInk {
    /// Creates an empty ladder bound to `fallback`.
    pub fn fallback_only(fallback: Fallback) -> Self {
        AmbientInk::new().bound(fallback)
    }

    /// Returns the caller override ladder.
    pub fn ink(&self) -> &AmbientInk {
        &self.ink
    }

    /// Returns the bound surface fallback.
    pub fn fallback(&self) -> Fallback {
        self.fallback
    }

    /// Resolves palette ink using this ladder and its bound fallback.
    pub fn resolve(&self, palette: &Palette) -> Color {
        self.ink.resolve(palette, self.fallback)
    }

    /// Resolves ink from the full theme using this ladder and its bound fallback.
    pub fn resolve_with_theme(&self, theme: &iced::Theme) -> Color {
        self.ink.resolve_with_theme(theme, self.fallback)
    }
}
