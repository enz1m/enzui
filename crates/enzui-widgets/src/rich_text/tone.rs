//! Per-span theme keys: mapping a semantic [`TextTone`] to and from the
//! opaque `u64` `Span::color_key`, and resolving that key against a runtime or
//! pinned token set.
//!
//! Real behaviour lives here: [`tone_key`] tags this module's keys so [`tone_from_key`]
//! can reject foreign keys, and [`ThemedTokens::resolve`] / [`resolve_tone_key`]
//! turn a key back into a themed [`Color`] (or `None`, "no opinion").

use crate::enz_text::{TextTokens, TextTone};
use iced::Color;

// ---------------------------------------------------------------------------
// per-span theme keys
// ---------------------------------------------------------------------------

/// Namespace tag for every `Span::color_key` this module emits.
///
/// `color_key` is a single opaque `u64` shared by the whole application, so a
/// bare `0..=7` tone discriminant would collide with any other widget that
/// keys its own spans from zero. Tagging the high bits makes these keys
/// self-identifying: [`tone_from_key`] returns `None` for anything that is not
/// ours, and a resolver returning `None` means "no opinion", which iced treats
/// as "fall back to the baked `Span::color`"
/// (`iced/widget/src/text/rich.rs`). So a foreign key is inert here
/// rather than being silently mis-resolved as a tone.
const TONE_KEY_TAG: u64 = 0x656E_7A75_0000_0000; // b"enzu" << 32

/// The `Span::color_key` naming `tone`, for the theme-reactive path.
///
/// The inverse is [`tone_from_key`]; `tone_key_roundtrip` pins that every
/// variant survives the trip, so adding a `TextTone` variant cannot silently
/// lose its key.
#[must_use]
pub fn tone_key(tone: TextTone) -> u64 {
    let discriminant: u64 = match tone {
        TextTone::Default => 0,
        TextTone::Muted => 1,
        TextTone::Subtle => 2,
        TextTone::Accent => 3,
        TextTone::Success => 4,
        TextTone::Warning => 5,
        TextTone::Danger => 6,
        TextTone::Disabled => 7,
    };
    TONE_KEY_TAG | discriminant
}

/// The [`TextTone`] a `Span::color_key` names, or `None` if the key is not one
/// of ours (see `TONE_KEY_TAG`).
#[must_use]
pub fn tone_from_key(key: u64) -> Option<TextTone> {
    if key & 0xFFFF_FFFF_0000_0000 != TONE_KEY_TAG {
        return None;
    }
    Some(match key & 0xFFFF_FFFF {
        0 => TextTone::Default,
        1 => TextTone::Muted,
        2 => TextTone::Subtle,
        3 => TextTone::Accent,
        4 => TextTone::Success,
        5 => TextTone::Warning,
        6 => TextTone::Danger,
        7 => TextTone::Disabled,
        _ => return None,
    })
}

/// The key of an inline `code` span's fill, resolved by [`ThemedTokens`]
/// alongside the tone keys (see [`code_fill`]).
pub(super) const CODE_FILL_KEY: u64 = TONE_KEY_TAG | 0x100;

/// How much of the body ink an inline `code` span's fill carries. Body ink
/// keeps about 10:1 on the chip over the light `background` (measured 10.4:1).
const CODE_FILL_ALPHA: f32 = 0.08;

/// The fill behind an inline `code` span: the body ink at low alpha, so it
/// reads as a chip on any backdrop and follows the same tokens as the text.
pub(super) fn code_fill(tokens: &TextTokens) -> Color {
    Color {
        a: CODE_FILL_ALPHA,
        ..tokens.color_for(TextTone::Default)
    }
}

/// How a widget derives its [`TextTokens`] from the live theme.
///
/// Toned spans are re-resolved through this on every draw, so it is the single
/// place that decides *which palette a tone means*. Two very different callers
/// need two different answers, and conflating them would collapse
/// the distinction below:
///
/// * `block_list` wants the runtime palette — that is the primary case,
///   and [`ThemedTokens::from_runtime_palette`] is the answer.
/// * `reading_pane`'s paper stocks (`Sepia`/`Light`/`Dark`) are *reader-chosen
///   paper*, deliberately fixed and explicitly not following app chrome
///   (see `reading_colors` in `reading_pane`). It builds its own [`TextTokens`] from the
///   paper's ink, and a resolver that ignored them would repaint sepia body
///   text in the app's grey. [`ThemedTokens::fixed`] preserves that.
pub enum ThemedTokens<'a> {
    /// Derive tokens from the runtime theme's enzUI palette, so every tone
    /// follows a light/dark/custom switch with no rebuild.
    FromRuntimePalette,
    /// Always answer with these tokens, whatever the theme is. For a caller
    /// whose ink is deliberately theme-independent.
    Fixed(TextTokens),
    /// Derive tokens with a caller-supplied closure over the live theme.
    Custom(Box<dyn Fn(&iced::Theme) -> TextTokens + 'a>),
}

impl<'a> ThemedTokens<'a> {
    /// Tokens derived from the runtime theme's palette.
    #[must_use]
    pub fn from_runtime_palette() -> Self {
        Self::FromRuntimePalette
    }

    /// Tokens that never change with the theme.
    #[must_use]
    pub fn fixed(tokens: TextTokens) -> Self {
        Self::Fixed(tokens)
    }

    /// The [`TextTokens`] for `theme`.
    #[must_use]
    pub fn tokens(&self, theme: &iced::Theme) -> TextTokens {
        match self {
            Self::FromRuntimePalette => {
                TextTokens::from_palette(&crate::theme_lookup::palette(theme))
            }
            Self::Fixed(t) => *t,
            Self::Custom(f) => f(theme),
        }
    }

    /// Resolve one `Span::color_key` against `theme`.
    ///
    /// Returns `None` for a foreign key, which iced reads as "no opinion" and
    /// answers with the span's baked `Span::color`
    /// (`iced/widget/src/text/rich.rs`).
    #[must_use]
    pub fn resolve(&self, theme: &iced::Theme, key: u64) -> Option<Color> {
        if key == CODE_FILL_KEY {
            return Some(code_fill(&self.tokens(theme)));
        }
        Some(self.tokens(theme).color_for(tone_from_key(key)?))
    }
}

/// Resolve a `Span::color_key` against the live `theme`, using the
/// runtime palette as the token source.
///
/// This is the default closure for `rich_text(..).resolve_color(..)`: it turns
/// one of these tone keys into the ink that tone names *in the runtime theme's
/// palette*, via [`crate::theme_lookup::palette`] (so a custom/Dracula/Nord
/// theme drives real hues rather than snapping to a dark/light preset).
///
/// Callers whose tones must not follow the runtime theme want
/// [`ThemedTokens::fixed`] instead.
///
/// Returns `None` for a foreign key, which iced reads as "no opinion" and
/// answers with the span's baked `Span::color`.
#[must_use]
pub fn resolve_tone_key(theme: &iced::Theme, key: u64) -> Option<Color> {
    ThemedTokens::FromRuntimePalette.resolve(theme, key)
}
