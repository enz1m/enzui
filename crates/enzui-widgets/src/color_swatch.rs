//! Clickable colour chip with selection borders, keyboard focus and accessible identity.
//!
//! Defaults to a 28 px square with a 6 px radius. Fixed colours keep their
//! fill and use progressively darker fill shades for rest, hover and selection.
//! The no-colour option follows the theme's surface and border colours.
//!
//! [`EnzColorSwatch::size`] and [`EnzColorSwatch::radius`] override geometry
//! independently of theme chrome. They take logical pixels; size determines
//! both the painted box and pointer target, including a literal zero.
//! [`EnzColorSwatch::tokens`] pins border/focus colours and geometry, while
//! the no-colour fill still follows the runtime theme. Explicit size/radius
//! overrides take precedence over token metrics regardless of setter order.
//!
//! Standalone chips publish Button semantics, selected/disabled state and
//! enabled activation. Keyboard focus adds an inset ring beside selection.
//! Names default to `#RRGGBB`, or [`NONE_LABEL`] for the no-colour option.
//! [`EnzColorSwatch::aria_label`] changes the name without changing identity.
//!
//! Default IDs derive from a namespace and rounded RGB hex, ignoring alpha.
//! Duplicate RGB keys, including colours that round to the same bytes, need
//! distinct [`EnzColorSwatch::id`] values or [`EnzColorSwatch::id_prefix`] namespaces.
//! The no-colour option uses [`NONE_ID_KEY`] within that namespace.

use enzui_theme::Palette;
use iced::widget::Space;
use iced::widget::button::Style as ButtonStyle;
use iced::{Background, Border, Color, Length, border::Radius};

use crate::focus_button::{FocusStatus, focus_button_maybe};
use enzui_core::semantics::{Role, Semantics};

/// Colour + geometry knobs for a swatch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSwatchTokens {
    /// Pinned rest border, or the theme border for the no-color option.
    pub normal_border: Color,
    /// Pinned selected border, or the theme accent for the no-color option.
    pub selected_border: Color,
    /// Pinned hover border, or the strong theme border for the no-color option.
    pub hover_border: Color,
    /// Keyboard focus-ring colour (drawn when the swatch is Tab-focused).
    pub focus_ring: Color,
    /// Swatch edge length in px (default 28).
    pub swatch_size: f32,
    /// Swatch corner radius in px (default 6).
    pub swatch_radius: f32,
    /// Border width in the normal/hover state in px (default 1).
    pub normal_border_width: f32,
    /// Border width when selected in px (default 2).
    pub selected_border_width: f32,
    /// Multiplier applied to fill and border alpha when disabled.
    pub disabled_alpha: f32,
}

impl ColorSwatchTokens {
    /// Use palette border/focus colours with stock geometry and 0.55 disabled alpha.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            normal_border: p.control_border,
            selected_border: p.accent,
            hover_border: p.control_border_strong,
            focus_ring: p.focus_ring,
            swatch_size: 28.0,
            swatch_radius: 6.0,
            normal_border_width: 1.0,
            selected_border_width: 2.0,
            disabled_alpha: 0.55,
        }
    }
}

const DEFAULT_GEOMETRY_TOKENS: ColorSwatchTokens = ColorSwatchTokens {
    normal_border: Color::TRANSPARENT,
    selected_border: Color::TRANSPARENT,
    hover_border: Color::TRANSPARENT,
    focus_ring: Color::TRANSPARENT,
    swatch_size: 28.0,
    swatch_radius: 6.0,
    normal_border_width: 1.0,
    selected_border_width: 2.0,
    disabled_alpha: 0.55,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Fill {
    Fixed(Color),
    None,
}

/// Convenience constructor: `enz_color_swatch(color, on_click)`.
pub fn enz_color_swatch<Message>(color: Color, on_click: Message) -> EnzColorSwatch<Message>
where
    Message: Clone,
{
    EnzColorSwatch::new(color, on_click)
}

/// Build a no-colour option whose fill follows the runtime `surface_alt`.
/// Its accessible name defaults to [`NONE_LABEL`], and its derived ID uses
/// [`NONE_ID_KEY`] instead of a colour hex.
pub fn enz_color_swatch_none<Message>(on_click: Message) -> EnzColorSwatch<Message>
where
    Message: Clone,
{
    EnzColorSwatch {
        fill: Fill::None,
        ..EnzColorSwatch::new(Color::TRANSPARENT, on_click)
    }
}

/// Default accessible name for the no-colour option.
pub const NONE_LABEL: &str = "No colour";

/// The id key [`enz_color_swatch_none`] derives its widget [`Id`] from, in
/// place of the `#RRGGBB` a coloured swatch uses.
///
/// [`Id`]: iced::advanced::widget::Id
pub const NONE_ID_KEY: &str = "none";

/// The namespace swatch ids are derived under when no [`EnzColorSwatch::id`] or
/// [`EnzColorSwatch::id_prefix`] is supplied.
pub const DEFAULT_ID_PREFIX: &str = "enz-color-swatch";

/// Builder for [`enz_color_swatch`].
#[must_use]
pub struct EnzColorSwatch<Message> {
    fill: Fill,
    on_click: Message,
    selected: bool,
    enabled: bool,
    aria_label: Option<String>,
    semantic_role: Role,
    embedded: bool,
    tokens: Option<ColorSwatchTokens>,
    size: Option<f32>,
    radius: Option<f32>,
    id: Option<iced::advanced::widget::Id>,
    id_prefix: String,
}

impl<Message> EnzColorSwatch<Message>
where
    Message: Clone,
{
    /// Build a swatch for `color` that emits `on_click` when activated.
    /// Defaults: not selected, enabled, hex-derived accessible name,
    /// theme-resolved tokens.
    pub fn new(color: Color, on_click: Message) -> Self {
        Self {
            fill: Fill::Fixed(color),
            on_click,
            selected: false,
            enabled: true,
            aria_label: None,
            semantic_role: Role::Button,
            embedded: false,
            tokens: None,
            size: None,
            radius: None,
            id: None,
            id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        }
    }

    /// Override the accessible name (defaults to the colour's `#RRGGBB` hex).
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Mark the chip selected; defaults to false.
    /// Selection thickens the border; unpinned fixed colours use a deeper fill shade,
    /// and unpinned no-colour chips use the theme accent.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    // Radio options share the chooser's roving focus owner and withdraw individual Tab stops.
    pub(crate) fn radio_option(mut self) -> Self {
        self.semantic_role = Role::RadioButton;
        self.embedded = true;
        self
    }

    /// Enable activation; defaults to true.
    /// Disabled chips suppress activation, publish disabled state and multiply fill
    /// and border alpha by the resolved disabled opacity (0.55 on the stock scale).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the square edge length in logical pixels; defaults to 28.
    /// The override wins over token size regardless of setter order and changes both
    /// paint and pointer bounds. Zero is honored; negative or non-finite input leaves
    /// the previous override unchanged. Border/focus colours keep their current policy.
    pub fn size(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.size = Some(px);
        }
        self
    }

    /// Set the corner radius in logical pixels; defaults to 6.
    /// Overrides token radius regardless of setter order. Zero makes square corners;
    /// negative or non-finite input leaves the previous override unchanged.
    pub fn radius(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.radius = Some(px);
        }
        self
    }

    /// Pin border/focus colours, border widths, geometry and disabled opacity.
    /// Without tokens, these values resolve from the runtime theme, with fixed-colour
    /// borders derived from the fill. The no-colour fill always follows `surface_alt`.
    /// Explicit size/radius overrides still win over the pinned metrics. Invalid
    /// token size falls back to 28; token radius and border widths are passed through.
    pub fn tokens(mut self, tokens: ColorSwatchTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Assign a stable widget identity, forwarded to the composed
    /// [`crate::focus_button::FocusButton`] so programmatic `focus(id)` and
    /// AccessKit action routing can target this swatch.
    ///
    /// Wins over the colour-derived default (see the [module docs](self)). Set
    /// this when two swatches in one view carry the *same* colour, which would
    /// otherwise derive one id twice.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the colour-derived default id.
    ///
    /// Two grids over overlapping palettes on one screen would derive the same
    /// `{DEFAULT_ID_PREFIX}-#RRGGBB` id for the same colour; a distinct prefix
    /// per grid keeps every swatch individually routable. Ignored when
    /// [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }
}

fn with_alpha(mut c: Color, a: f32) -> Color {
    c.a *= a;
    c
}

fn finite_size(v: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        DEFAULT_GEOMETRY_TOKENS.swatch_size
    }
}
fn hex_name(c: Color) -> String {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", to8(c.r), to8(c.g), to8(c.b))
}

impl<Message> EnzColorSwatch<Message> {
    /// Build the chip element; useful where `.into()` has no inferred target type.
    #[must_use]
    pub fn into_element<'a>(self) -> iced::Element<'a, Message>
    where
        Message: 'a + Clone,
    {
        iced::Element::from(self)
    }
}

impl<'a, Message> From<EnzColorSwatch<Message>> for iced::Element<'a, Message>
where
    Message: 'a + Clone,
{
    fn from(w: EnzColorSwatch<Message>) -> Self {
        let geom = w.tokens.unwrap_or(DEFAULT_GEOMETRY_TOKENS);

        let pinned = w.tokens;
        let size_override = w.size;
        let radius_override = w.radius;
        let fill = w.fill;
        let selected = w.selected;
        let enabled = w.enabled;
        let semantic_role = w.semantic_role;
        let embedded = w.embedded;
        let aria = w.aria_label.unwrap_or_else(|| match fill {
            Fill::None => enzui_core::localization::text("enzui-color-swatch-none"),
            Fill::Fixed(c) => hex_name(c),
        });
        let outer = finite_size(size_override.unwrap_or(geom.swatch_size));
        let inner = (outer - 2.0).max(0.0);
        let content = Space::new()
            .width(Length::Fixed(inner))
            .height(Length::Fixed(inner));

        let on_press = if enabled { Some(w.on_click) } else { None };
        let id_key = match fill {
            Fill::None => NONE_ID_KEY.to_owned(),
            Fill::Fixed(c) => hex_name(c),
        };
        let widget_id =
            Some(w.id.unwrap_or_else(|| enzui_core::identity::derive_id(&w.id_prefix, &id_key)));

        let mut fb = focus_button_maybe(content, on_press)
            .width(Length::Fixed(outer))
            .height(Length::Fixed(outer))
            .padding(0.0)
            // Native inset-ring drawing preserves the separate selection border.
            .inset_focus_ring(true)
            .embedded(embedded)
            .semantics(
                Semantics::new(semantic_role)
                    .label(aria)
                    .selected(selected)
                    .disabled(!enabled),
            )
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                let palette = crate::theme_lookup::palette(theme);
                let mut t = pinned.unwrap_or_else(|| {
                    use crate::theme_lookup::FollowsThemeOpacity as _;
                    ColorSwatchTokens::from_palette(&palette)
                        .with_theme_opacity(&crate::theme_lookup::opacity(theme))
                });
                if let Some(px) = size_override {
                    t.swatch_size = px;
                }
                if let Some(px) = radius_override {
                    t.swatch_radius = px;
                }
                let alpha = if enabled { 1.0 } else { t.disabled_alpha };
                let base = match fill {
                    Fill::Fixed(c) => c,
                    Fill::None => palette.surface_alt,
                };
                if pinned.is_none() && matches!(fill, Fill::Fixed(_)) {
                    t.normal_border = crate::surface_edge::edge_color(base, 0.12);
                    t.hover_border = crate::surface_edge::edge_color(base, 0.25);
                    t.selected_border = crate::surface_edge::edge_color(base, 0.40);
                }
                let bg = with_alpha(base, alpha);
                let (border_color, border_width) = match status {
                    FocusStatus::Focused => (t.focus_ring, t.selected_border_width),
                    _ if selected => (t.selected_border, t.selected_border_width),
                    FocusStatus::Hovered | FocusStatus::Pressed => {
                        (t.hover_border, t.normal_border_width)
                    }
                    _ => (t.normal_border, t.normal_border_width),
                };
                ButtonStyle {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        color: with_alpha(border_color, alpha),
                        width: border_width,
                        radius: Radius::from(t.swatch_radius),
                    },
                    ..Default::default()
                }
            });
        if let Some(id) = widget_id {
            fb = fb.id(id);
        }
        fb.into()
    }
}

impl crate::theme_lookup::FollowsThemeOpacity for ColorSwatchTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled_surface(),
            ..self
        }
    }
}
