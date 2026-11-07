//! What the frame of a window that draws its own frame is made of.
//!
//! An application chooses its own frame (the app shell's `frame_material`).
//! The theme file's `window.frame` can decide for every application instead:
//! [`WindowFrame::App`], the default, leaves the choice to each one;
//! [`WindowFrame::Opaque`] turns glass frames off everywhere; a glass style
//! turns them on everywhere. A glass frame shows the desktop and the windows
//! behind it, filtered by the compositor, around and between the window's
//! panes; where the compositor paints no glass, the frame stays opaque.

use crate::{MaterialStyle, MaterialTokens};

/// What a theme asks of every window frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowFrame {
    /// Each application decides.
    #[default]
    App,
    /// Opaque, whatever the application asks.
    Opaque,
    /// Glass of this style, whatever the application asks.
    Glass(MaterialStyle),
}

impl WindowFrame {
    /// The theme-file identifiers.
    pub const NAMES: &[&str] = &[
        "app",
        "opaque",
        "frosted",
        "flat_frosted",
        "subtle",
        "glass",
        "clear",
    ];

    /// The frame a theme-file identifier names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "app" => Self::App,
            "opaque" => Self::Opaque,
            "frosted" => Self::Glass(MaterialStyle::Frosted),
            "flat_frosted" => Self::Glass(MaterialStyle::FlatFrosted),
            "subtle" => Self::Glass(MaterialStyle::Subtle),
            "glass" => Self::Glass(MaterialStyle::Regular),
            "clear" => Self::Glass(MaterialStyle::Clear),
            _ => return None,
        })
    }

    /// The theme-file identifier; a style no identifier names reads as `app`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Opaque => "opaque",
            Self::Glass(MaterialStyle::Frosted) => "frosted",
            Self::Glass(MaterialStyle::FlatFrosted) => "flat_frosted",
            Self::Glass(MaterialStyle::Subtle) => "subtle",
            Self::Glass(MaterialStyle::Regular) => "glass",
            Self::Glass(MaterialStyle::Clear) => "clear",
            Self::Glass(_) => "app",
        }
    }

    /// The frame material of a window whose application asked for `app`.
    #[must_use]
    pub fn resolve(self, app: Option<MaterialStyle>) -> Option<MaterialStyle> {
        match self {
            Self::App => app,
            Self::Opaque => None,
            Self::Glass(style) => Some(style),
        }
    }
}

/// The tokens of `style` as the glass behind a whole window. The presets
/// are tuned for panels inside a window; behind a window the backdrop is the
/// desktop and other windows, seen across the frame's narrow gaps, so a
/// frosted frame blurs it at window scale, lifts its colour back after the
/// blur and tints it with the frame's own tone.
pub fn frame_tokens(style: MaterialStyle) -> MaterialTokens {
    let tokens = style.tokens();
    if tokens.blur_sigma <= 0.0 {
        return tokens;
    }
    MaterialTokens {
        blur_sigma: tokens.blur_sigma * 5.0,
        saturation: tokens.saturation.max(1.8),
        tint_opacity: tokens.tint_opacity.max(0.55),
        ..tokens
    }
}

/// Frame `tokens` under a dark theme: the backdrop a dark window frame shows
/// through is held further down, so bright windows and wallpapers behind it
/// read as a dim, coloured frost rather than a pale band around dark panes.
pub fn in_dark(tokens: MaterialTokens) -> MaterialTokens {
    MaterialTokens {
        tint_opacity: tokens.tint_opacity.max(0.9),
        brightness: tokens.brightness.min(-0.35),
        saturation: tokens.saturation.min(1.4),
        ..tokens
    }
}
