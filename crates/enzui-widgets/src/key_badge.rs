//! Keycap chips for individual keys and shortcut chords.
//!
//! Labels stay on one line and use end ellipsis under narrow constraints.
//! Stock text publishes a passive label and remains discoverable by text selectors.
//! Unpinned colours follow the live palette. Geometry defaults to a 24 px height,
//! 8 px horizontal padding and the theme's `key` corner radius (5 px); the
//! constructor captures UI-scaled 11 px text. Fonts default to the interface face at medium weight;
//! [`EnzKeyBadge::font_from_theme`] selects the theme's monospace.
//!
//! Keys with a widely recognised symbol draw it instead of their name: Shift
//! ⇧, Caps Lock ⇪, Enter/Return ↵, Backspace ⌫, Tab ⇥, Space ␣, the arrow
//! keys, Command ⌘ and Option ⌥ (see [`key_symbol`]). Their accessible name
//! stays the key's name. Ctrl, Alt, Esc and Super keep their names, as Linux
//! desktops print them. [`EnzKeyBadge::symbol`] turns symbols off.
//!
//! [`crate::shortcut_list`] composes badges in shortcut rows.

use enzui_theme::{EnzTheme, FontRole, Palette};
use iced::font::Family;
use iced::widget::text::{Ellipsis, Wrapping};
use iced::widget::{container, text};
use iced::{Border, Color, Element, Font, Length, Padding};
/// Default chip height in logical pixels: 24.
pub const DEFAULT_HEIGHT: f32 = 24.0;
/// Default horizontal padding in logical pixels: 8.
pub const DEFAULT_PADDING_X: f32 = 8.0;
/// Base glyph size before UI scaling in logical pixels: 11.
pub const DEFAULT_TEXT_SIZE: f32 = 11.0;
// Rejected values must not latch explicit-width precedence.
fn checked(v: f32) -> Option<f32> {
    (v.is_finite() && v >= 0.0).then_some(v)
}

fn sane(v: f32, fallback: f32) -> f32 {
    checked(v).unwrap_or(fallback)
}
// The interface face at medium weight, as keycaps read in macOS menus,
// GNOME's shortcuts window and Windows key captions; a typewriter face made
// "Ctrl" look like code.
const DEFAULT_GLYPH_FONT: Font = Font {
    weight: iced::font::Weight::Medium,
    ..Font::DEFAULT
};

fn glyph_text<'a>(key: &str, size: f32, font: Font) -> text::Text<'a, iced::Theme> {
    // Generic monospace needs an upright-face fallback registered before shaping.
    if font.family == Family::Monospace {
        crate::monospace_fallback::ensure_monospace_fallback();
    }
    text(key.to_owned())
        .size(size)
        .font(font)
        .wrapping(Wrapping::None)
        .ellipsis(Ellipsis::End)
        .center()
}
/// The bundled icon that stands for `key` on a keycap, if the key has a
/// widely recognised symbol. Matches names case-insensitively ("shift",
/// "Return", "⌘").
pub fn key_symbol(key: &str) -> Option<&'static str> {
    let key = key.trim().to_lowercase();
    Some(match key.as_str() {
        "shift" | "⇧" => "arrow-big-up",
        "caps lock" | "capslock" | "caps" | "⇪" => "arrow-big-up-dash",
        "enter" | "return" | "⏎" | "↵" => "corner-down-left",
        "backspace" | "⌫" => "delete",
        "tab" | "⇥" => "arrow-right-to-line",
        "space" | "spacebar" | "␣" => "space",
        "up" | "arrow up" | "arrowup" | "↑" => "arrow-up",
        "down" | "arrow down" | "arrowdown" | "↓" => "arrow-down",
        "left" | "arrow left" | "arrowleft" | "←" => "arrow-left",
        "right" | "arrow right" | "arrowright" | "→" => "arrow-right",
        "cmd" | "command" | "⌘" => "command",
        "option" | "opt" | "⌥" => "option",
        _ => return None,
    })
}

/// Chip fill, outline and glyph ink overrides.
#[derive(Debug, Clone, Copy)]
pub struct KeyBadgeTokens {
    /// Chip fill.
    pub background: Color,
    /// Outline ink.
    pub border: Color,
    /// Glyph ink.
    pub text_color: Color,
}

impl KeyBadgeTokens {
    /// Use the alternate surface with an outline derived from that fill.
    /// Glyph ink uses [`Palette::text_muted_on`] to resolve contrast against the fill.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_alt,
            border: crate::surface_edge::edge_color(
                p.surface_alt,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            text_color: p.text_muted_on(p.surface_alt),
        }
    }
    /// Use the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Use the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
    /// Use primary text ink with the alternate surface and its derived outline.
    /// Unlike [`Self::from_palette`], this does not resolve glyph contrast against the fill.
    pub fn runtime_text_from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_alt,
            border: crate::surface_edge::edge_color(
                p.surface_alt,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            text_color: p.text,
        }
    }
}

type ClosureResolver<'a> = std::rc::Rc<dyn Fn(&Palette) -> KeyBadgeTokens + 'a>;
/// Builder for a passive, single-line key or chord chip.
#[must_use]
pub struct EnzKeyBadge<'a, Message> {
    key: String,
    height: f32,
    padding_x: f32,
    radius: f32,
    text_size: f32,
    min_width: f32,
    min_width_explicit: bool,
    tokens: Option<KeyBadgeTokens>,
    token_resolver: fn(&Palette) -> KeyBadgeTokens,
    closure_resolver: Option<ClosureResolver<'a>>,
    font: Font,
    symbol: bool,
    _phantom: std::marker::PhantomData<&'a Message>,
}
/// Create a chip with default geometry, live palette colours and UI-scaled text.
pub fn enz_key_badge<'a, Message>(key: impl Into<String>) -> EnzKeyBadge<'a, Message> {
    EnzKeyBadge {
        key: key.into(),
        height: DEFAULT_HEIGHT,
        padding_x: DEFAULT_PADDING_X,
        radius: enzui_theme::text_defaults::shape_radius(|s| s.key),
        text_size: enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
        min_width: DEFAULT_HEIGHT,
        min_width_explicit: false,
        tokens: None,
        token_resolver: KeyBadgeTokens::from_palette,
        closure_resolver: None,
        font: DEFAULT_GLYPH_FONT,
        symbol: true,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzKeyBadge<'a, Message> {
    /// Draw a key's symbol (⇧ for Shift) instead of its name, when it has
    /// one; see [`key_symbol`]. Defaults to true.
    pub fn symbol(mut self, symbol: bool) -> Self {
        self.symbol = symbol;
        self
    }

    /// Set the chip height in logical pixels; the minimum width follows until pinned.
    /// Non-finite or negative values retain the previous height.
    pub fn height(mut self, h: f32) -> Self {
        self.height = sane(h, self.height);
        if !self.min_width_explicit {
            self.min_width = self.height;
        }
        self
    }
    /// Set horizontal padding in logical pixels.
    /// Non-finite or negative values retain the previous padding.
    pub fn padding(mut self, px: f32) -> Self {
        self.padding_x = sane(px, self.padding_x);
        self
    }
    /// Set corner radius in logical pixels.
    /// Non-finite or negative values retain the previous radius.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = sane(r, self.radius);
        self
    }
    /// Set glyph size in logical pixels without UI scaling.
    /// Non-finite or negative values retain the previous size; zero is accepted.
    pub fn text_size(mut self, s: f32) -> Self {
        self.text_size = sane(s, self.text_size);
        self
    }
    /// Pin the minimum chip width in logical pixels, subject to parent constraints.
    /// Valid values may be smaller than the height, including zero. Later height
    /// changes retain this pin. Non-finite or negative values leave both the width
    /// and its precedence unchanged.
    pub fn min_width(mut self, w: f32) -> Self {
        if let Some(v) = checked(w) {
            self.min_width = v;
            self.min_width_explicit = true;
        }
        self
    }
    /// Pin colours, overriding both live palette resolvers.
    pub fn tokens(mut self, tokens: KeyBadgeTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Choose how unpinned colours derive from the live palette at paint time.
    /// Defaults to [`KeyBadgeTokens::from_palette`]; a closure resolver takes precedence.
    pub fn token_resolver(mut self, resolver: fn(&Palette) -> KeyBadgeTokens) -> Self {
        self.token_resolver = resolver;
        self
    }
    /// Capture the theme's Monospace family with normal weight, style and stretch.
    /// Later theme changes do not update the captured font.
    pub fn font_from_theme(self, theme: &EnzTheme) -> Self {
        self.font_from_theme_with(
            theme,
            iced::font::Weight::Normal,
            iced::font::Style::Normal,
            iced::font::Stretch::Normal,
        )
    }
    /// Capture the theme's Monospace family with the supplied font attributes.
    /// Later theme changes do not update the captured font.
    pub fn font_from_theme_with(
        mut self,
        theme: &EnzTheme,
        weight: iced::font::Weight,
        style: iced::font::Style,
        stretch: iced::font::Stretch,
    ) -> Self {
        self.font = theme.resolve_font(FontRole::Monospace, weight, style, stretch);
        self
    }
    /// Resolve unpinned colours from the live palette using captured instance state.
    /// Overrides [`Self::token_resolver`]; an explicit [`Self::tokens`] pin still wins.
    pub fn closure_resolver(
        mut self,
        resolver: std::rc::Rc<dyn Fn(&Palette) -> KeyBadgeTokens + 'a>,
    ) -> Self {
        self.closure_resolver = Some(resolver);
        self
    }
    /// Return the font used to construct the glyph.
    #[doc(hidden)]
    pub fn glyph_font_committed(&self) -> Font {
        self.font
    }
    /// Build a single-line chip with a passive stock-text label.
    pub fn into_element(self) -> Element<'a, Message> {
        let text_size = self.text_size;
        let padding_x = self.padding_x;
        let radius = self.radius;
        let height = self.height;
        let min_width = self.min_width;
        let pinned = self.tokens;
        let resolver = self.token_resolver;
        let closure = self.closure_resolver.clone();
        let font = self.glyph_font_committed();

        let resolve_tokens = move |theme: &iced::Theme| -> KeyBadgeTokens {
            if let Some(t) = pinned {
                return t;
            }
            let palette = crate::theme_lookup::palette(theme);
            match &closure {
                Some(f) => f(&palette),
                None => resolver(&palette),
            }
        };
        let resolve_glyph = resolve_tokens.clone();

        // A borrowed closure resolver cannot tint an icon, which needs a
        // `'static` ink; such a badge keeps the key's name.
        let symbol = (self.symbol && self.closure_resolver.is_none())
            .then(|| key_symbol(&self.key))
            .flatten();
        let glyph: Element<'a, Message> = match symbol {
            // The symbol, named for assistive technology by the key's name.
            Some(icon) => crate::semantics_container::SemanticsContainer::new(
                crate::icon::enz_icon(icon)
                    .size((text_size * 1.3).round())
                    .tint_resolved(move |theme: &iced::Theme| {
                        pinned.map_or_else(
                            || resolver(&crate::theme_lookup::palette(theme)).text_color,
                            |tokens| tokens.text_color,
                        )
                    })
                    .into_element(),
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Label)
                    .label(self.key.clone()),
            )
            .into(),
            None => glyph_text(&self.key, text_size, font)
                .style(move |theme: &iced::Theme| {
                    let tokens = resolve_glyph(theme);
                    iced::widget::text::Style {
                        color: Some(tokens.text_color),
                    }
                })
                .into(),
        };
        // A terminal semantics wrapper would hide stock text from selectors and collectors.
        container(glyph)
            .padding(Padding {
                top: 0.0,
                right: padding_x,
                bottom: 0.0,
                left: padding_x,
            })
            .height(Length::Fixed(height))
            .width(Length::Shrink.min(min_width))
            .center_x(Length::Shrink.min(min_width))
            .center_y(Length::Fixed(height))
            .style(move |theme: &iced::Theme| {
                let tokens = resolve_tokens(theme);
                container::Style {
                    background: Some(tokens.background.into()),
                    border: Border {
                        color: tokens.border,
                        width: 1.0,
                        radius: radius.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a, Message: 'a> From<EnzKeyBadge<'a, Message>> for Element<'a, Message> {
    fn from(k: EnzKeyBadge<'a, Message>) -> Self {
        k.into_element()
    }
}
