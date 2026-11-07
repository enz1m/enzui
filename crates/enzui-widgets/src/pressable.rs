//! Focusable surface for selectable cards, rows, chips, and plain hit targets.
//!
//! Defaults to card styling, fill width, zero padding, a 24-pixel hit floor,
//! and enabled activation. Content is clipped to the rounded outline; selected
//! fill takes precedence over pressed and hover states. Chrome and inherited
//! text dim when inert; explicit child colors and images retain their opacity.
//!
//! [`EnzPressable::embedded`] leaves keyboard focus and activation to a
//! composite owner while preserving pointer and routed accessibility actions.
//! Compose right-click behavior with [`crate::context_target`].
//!
//! # Context menu

use crate::focus_button::{FocusStatus, focus_button_maybe};
use crate::surface_interaction::{DISABLED_ALPHA, DimContent, composite};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::{Background, Border, Color, Element, Length, Theme};

/// Selects corner radius, resting fill, and default border width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PressableStyle {
    /// Card styling: 6-pixel radius, 1-pixel border, and elevated fill; the default.
    #[default]
    Card,
    /// Row styling: 10-pixel radius, transparent resting fill, and no default border.
    Row,
    /// Chip styling: 6-pixel radius, hover-surface resting fill, and no default border.
    Chip,
    /// Plain styling: 6-pixel radius, transparent resting fill, and no default border.
    Plain,
}

impl PressableStyle {
    /// Returns the style’s corner radius in logical pixels: 10 for rows, 6 otherwise.
    pub fn radius(self) -> f32 {
        match self {
            PressableStyle::Row => 10.0,
            PressableStyle::Card | PressableStyle::Chip | PressableStyle::Plain => 6.0,
        }
    }
    /// Returns default border width in logical pixels: 1 for cards, 0 otherwise.
    pub fn border_width(self) -> f32 {
        match self {
            PressableStyle::Card => 1.0,
            _ => 0.0,
        }
    }

    /// Resolves selected fill for custom child ink: cards/plain use `card_selected_bg`,
    /// while rows/chips use `row_chip_selected_bg`.
    pub fn selected_fill(self, tokens: &PressableTokens) -> Color {
        selected_fill(self, tokens)
    }
}

/// Surface colors and interaction metrics, normally resolved from the runtime palette.
#[derive(Debug, Clone, Copy)]
pub struct PressableTokens {
    /// Resting card fill; defaults to the palette’s elevated surface.
    pub card_bg: Color,
    /// Resting chip fill; defaults to the palette’s hover surface.
    pub chip_bg: Color,
    /// Resting card outline, derived by shading its elevated fill.
    pub card_border: Color,
    /// Selected card outline; defaults to palette accent.
    pub card_selected_border: Color,
    /// Selected card/plain fill; defaults to palette accent-soft.
    pub card_selected_bg: Color,
    /// Selected row/chip fill; defaults to palette selected surface.
    pub row_chip_selected_bg: Color,
    /// Hover tint composited over the resting fill; defaults to [`Palette::overlay_hover`].
    pub hover_overlay: Color,
    /// Pressed tint composited over the fill; defaults to [`Palette::overlay_pressed`].
    pub pressed_overlay: Color,
    /// Foreground text colour (used to dim disabled content).
    pub text: Color,
    /// Inert-state alpha multiplier; defaults to 0.55 in palette-derived tokens.
    ///
    /// Below one, chrome applies this once and inherited text twice. Above one,
    /// chrome stays unchanged while the inherited-text wrapper amplifies alpha.
    pub disabled_alpha: f32,
    /// Keyboard focus-ring colour.
    pub focus_ring: Color,
    /// Focus-ring width in logical pixels; palette-derived tokens default to 2.
    pub focus_ring_width: f32,
}

impl PressableTokens {
    /// Derives surface tokens from `p`, with a 2-pixel focus ring and 0.55 disabled multiplier.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            card_bg: p.surface_elevated,
            chip_bg: p.surface_hover,
            card_border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            card_selected_border: p.accent,
            card_selected_bg: p.accent_soft,
            row_chip_selected_bg: p.surface_selected,
            hover_overlay: p.overlay_hover(),
            pressed_overlay: p.overlay_pressed(),
            text: p.text,
            disabled_alpha: DISABLED_ALPHA,
            focus_ring: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
        }
    }
    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replaces negative or nonfinite ring width and disabled multiplier with 2 and 0.55.
    ///
    /// Zero and finite positive values survive, including multipliers above one.
    /// Colors are unchanged; no opacity upper bound is imposed.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        Self {
            focus_ring_width: FiniteNonNeg::new_or(self.focus_ring_width, 2.0).get(),
            disabled_alpha: FiniteNonNeg::new_or(self.disabled_alpha, DISABLED_ALPHA).get(),
            ..*self
        }
    }
}

/// Per-instance color replacements; absent fields retain style/token defaults.
///
/// Color alpha is honored. Hover replacements bypass the hover composite, while
/// pressed state still composites its overlay over the replacement.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SurfaceColors {
    /// Replaces resting fill; selected fill still wins.
    pub background: Option<Color>,
    /// Replaces hovered fill and supplies the base for the pressed overlay.
    pub hover_background: Option<Color>,
    /// Replaces resting and selected border color; focus-ring color still wins.
    pub border: Option<Color>,
}

/// Builder for a pressable surface. See [`enz_pressable`].
#[must_use]
pub struct EnzPressable<'a, Message> {
    content: Element<'a, Message>,
    on_activate: Option<Message>,
    on_activate_with_modifiers: Option<Box<dyn Fn(iced::keyboard::Modifiers) -> Message + 'a>>,
    style: PressableStyle,
    selected: bool,
    selectable: bool,
    enabled: bool,
    aria_label: Option<String>,
    semantics_override: Option<Semantics>,
    id: Option<iced::advanced::widget::Id>,
    tokens: Option<PressableTokens>,
    width: Length,
    colors: Option<SurfaceColorsResolver>,
    radius: Option<f32>,
    border_width: Option<f32>,
    focus_ring_width: Option<f32>,
    focus_ring_color: Option<Color>,
    min_height: Option<f32>,
    min_height_center: bool,
    content_align_x: Option<iced::Alignment>,
    min_hit: f32,
    padding: Option<iced::Padding>,
    embedded: bool,
    on_enter: Option<Box<dyn Fn() -> Message + 'a>>,
    on_space: Option<Box<dyn Fn() -> Message + 'a>>,
    focus_visible: bool,
}

type SurfaceColorsResolver = std::rc::Rc<dyn Fn(&Theme) -> SurfaceColors>;

/// Builds an enabled card surface with fill width, zero padding, and a 24-pixel hit floor.
pub fn enz_pressable<'a, Message: Clone + 'a>(
    content: Element<'a, Message>,
    on_activate: Message,
) -> EnzPressable<'a, Message> {
    enz_pressable_maybe(content, Some(on_activate))
}

/// Builds a surface with optional activation.
///
/// Without activation it is inert, unless [`EnzPressable::on_activate_with_modifiers`]
/// is supplied. Typed Enter/Space overrides alone do not enable it.
pub fn enz_pressable_maybe<'a, Message: Clone + 'a>(
    content: Element<'a, Message>,
    on_activate: Option<Message>,
) -> EnzPressable<'a, Message> {
    EnzPressable {
        content,
        on_activate,
        on_activate_with_modifiers: None,
        style: PressableStyle::default(),
        selected: false,
        selectable: false,
        enabled: true,
        aria_label: None,
        semantics_override: None,
        id: None,
        tokens: None,
        width: Length::Fill,
        colors: None,
        radius: None,
        border_width: None,
        focus_ring_width: None,
        focus_ring_color: None,
        min_height: None,
        min_height_center: false,
        content_align_x: None,
        min_hit: crate::accessibility_metrics::DEFAULT_MIN_HIT_TARGET,
        padding: None,
        embedded: false,
        on_enter: None,
        on_space: None,
        focus_visible: false,
    }
}

impl<'a, Message: Clone + 'a> EnzPressable<'a, Message> {
    /// Set the visual style. Defaults to [`PressableStyle::Card`].
    pub fn style(mut self, s: PressableStyle) -> Self {
        self.style = s;
        self
    }
    /// Mark the surface as currently selected. Selected paint takes precedence
    /// over pressed/hover. Defaults to `false`.
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    /// Reports selection membership even when unselected; defaults to false.
    ///
    /// A selected surface reports selection regardless. A semantics override owns
    /// these flags instead.
    pub fn selectable(mut self, s: bool) -> Self {
        self.selectable = s;
        self
    }
    /// Enables interaction; defaults to true.
    ///
    /// Disabling silences activation and dims chrome plus inherited text using the
    /// resolved disabled multiplier; explicit child colors and images are unaffected.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Sets the accessible name, unless [`Self::semantics`] supplies a descriptor.
    ///
    /// Without a name, button semantics derive it from drawn text. Icon-only content
    /// needs an explicit name to avoid the collector’s nameless-actionable record.
    pub fn aria_label(mut self, l: impl Into<String>) -> Self {
        self.aria_label = Some(l.into());
        self
    }

    /// Routes activation through a modifier-aware callback, overriding the plain message.
    ///
    /// Pointer modifiers latch at press. Accepted keyboard and routed accessibility
    /// activations pass empty modifiers; typed Enter/Space overrides still apply.
    /// The callback alone enables an otherwise activation-less surface.
    pub fn on_activate_with_modifiers(
        mut self,
        f: impl Fn(iced::keyboard::Modifiers) -> Message + 'a,
    ) -> Self {
        self.on_activate_with_modifiers = Some(Box::new(f));
        self
    }

    /// Replaces the surface’s default button descriptor.
    ///
    /// This owns role, label, and selection flags, overriding the corresponding
    /// builder settings. Live focus, disabled state, and default action still come
    /// from the shared focus button.
    pub fn semantics(mut self, semantics: Semantics) -> Self {
        self.semantics_override = Some(semantics);
        self
    }
    /// Pins widget identity for programmatic focus and routed activation.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }
    /// Pins sanitized color and interaction tokens.
    ///
    /// Without a pin, palette and disabled opacity follow the runtime theme.
    pub fn tokens(mut self, t: PressableTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }
    /// Override the outer width (default `Length::Fill`). Use `Length::Shrink`
    /// for hug-content chips.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Resolves per-instance color replacements against the runtime theme during drawing.
    ///
    /// Absent fields retain tokens. Hover replacement alpha is honored; pressed
    /// state composites its overlay over the hover replacement.
    pub fn surface_colors_resolved(
        mut self,
        resolver: impl Fn(&Theme) -> SurfaceColors + 'static,
    ) -> Self {
        self.colors = Some(std::rc::Rc::new(resolver));
        self
    }

    /// Sets ring width in logical pixels; defaults to the resolved token (normally 2).
    ///
    /// Nonfinite or negative values leave the previous setting unchanged; zero is valid.
    pub fn focus_ring_width(mut self, width: f32) -> Self {
        if width.is_finite() && width >= 0.0 {
            self.focus_ring_width = Some(width);
        }
        self
    }

    /// Sets focus-ring color; defaults to the resolved token.
    pub fn focus_ring_color(mut self, color: Color) -> Self {
        self.focus_ring_color = Some(color);
        self
    }

    /// Insets content; defaults to zero padding.
    pub fn padding(mut self, padding: impl Into<iced::Padding>) -> Self {
        self.padding = Some(padding.into());
        self
    }

    /// Sets corner radius in logical pixels; defaults to the style’s radius.
    ///
    /// Nonfinite or negative values leave the previous setting unchanged; zero is valid.
    pub fn radius(mut self, radius: f32) -> Self {
        if radius.is_finite() && radius >= 0.0 {
            self.radius = Some(radius);
        }
        self
    }

    /// Sets resting border width in logical pixels; defaults to 1 for cards, 0 otherwise.
    ///
    /// Nonfinite or negative values leave the previous setting unchanged; zero is valid.
    pub fn border_width(mut self, width: f32) -> Self {
        if width.is_finite() && width >= 0.0 {
            self.border_width = Some(width);
        }
        self
    }

    /// Sets a growable minimum height in logical pixels, bounded by parent constraints.
    ///
    /// By default this raises the button’s own bounds. With [`Self::min_height_center`],
    /// the button keeps its natural height inside a taller wrapper. Nonfinite or
    /// nonpositive values leave the previous setting unchanged.
    pub fn min_height(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.min_height = Some(px);
        }
        self
    }

    /// Leaves Tab registration, keyboard activation, and press-time focus to a composite owner.
    ///
    /// Defaults to false. Pointer activation and accessibility semantics remain;
    /// [`Self::focus_visible`] supplies the embedded node’s focus flag and ring.
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Lazily overrides Enter and routed accessibility default action with a distinct message.
    ///
    /// The surface must already have activation. Embedded cells leave keyboard Enter
    /// to their owner, but retain the routed default-action override.
    pub fn on_enter(mut self, on_enter: impl Fn() -> Message + 'a) -> Self {
        self.on_enter = Some(Box::new(on_enter));
        self
    }

    /// Sets or clears the Enter/default-action override; none falls back to ordinary activation.
    pub fn on_enter_maybe(mut self, on_enter: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_enter = on_enter;
        self
    }

    /// Lazily overrides keyboard Space with a distinct message.
    ///
    /// The surface must already have activation; embedded cells leave Space to their owner.
    pub fn on_space(mut self, on_space: impl Fn() -> Message + 'a) -> Self {
        self.on_space = Some(Box::new(on_space));
        self
    }

    /// Sets or clears the Space override; none falls back to ordinary activation.
    pub fn on_space_maybe(mut self, on_space: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_space = on_space;
        self
    }

    /// Supplies an embedded cell’s focused semantics and keyboard-ring presentation.
    ///
    /// Defaults to false. Standalone surfaces use their own focus and input modality.
    pub fn focus_visible(mut self, focus_visible: bool) -> Self {
        self.focus_visible = focus_visible;
        self
    }

    /// Aligns the content horizontally inside a wider surface.
    ///
    /// Defaults to centred for a fill or fixed width; a row or list item whose
    /// content is laid out from its leading edge sets `Start`.
    pub fn content_align_x(mut self, align: iced::Alignment) -> Self {
        self.content_align_x = Some(align);
        self
    }

    /// Centers the natural-height button inside the minimum-height wrapper.
    ///
    /// The wrapper’s extra space does not extend the button’s hit or semantic bounds.
    pub fn min_height_center(mut self) -> Self {
        self.min_height_center = true;
        self
    }

    /// Sets the minimum hit-target floor in logical pixels; defaults to 24.
    ///
    /// Zero removes the floor. Nonfinite or negative values leave the previous
    /// setting unchanged. Fixed lengths retain their exact size; shrink/fill lengths
    /// receive the floor on the button’s own bounds.
    pub fn min_hit(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.min_hit = px;
        }
        self
    }

    /// Builds the surface, clipped content, and shared focus/activation owner.
    pub fn into_element(self) -> Element<'a, Message> {
        let captured = self.tokens;
        let radius = self.radius.unwrap_or_else(|| self.style.radius());
        let border_width = self
            .border_width
            .unwrap_or_else(|| self.style.border_width());
        let colors = self.colors.clone();
        let min_height_floor = self.min_height;
        let ring_width = self.focus_ring_width;
        let ring_color = self.focus_ring_color;
        let style = self.style;
        let selected = self.selected;
        let enabled = self.enabled;
        let width = self.width;

        // Typed key overrides do not count as the base activation that enables this surface.
        let inert =
            !enabled || (self.on_activate.is_none() && self.on_activate_with_modifiers.is_none());
        let semantics = match self.semantics_override {
            Some(s) => s.disabled(inert),
            None => {
                let mut semantics = Semantics::new(Role::Button).disabled(inert);
                if self.selectable || selected {
                    semantics = semantics.selected(selected);
                }
                if let Some(label) = self.aria_label {
                    semantics = semantics.label(label);
                }
                semantics
            }
        };

        let tokens_for = move |theme: &Theme| {
            captured.unwrap_or_else(|| {
                crate::theme_lookup::resolve_live(theme, PressableTokens::from_palette)
            })
        };
        let dim_for = move |t: &PressableTokens| if inert { t.disabled_alpha } else { 1.0 };
        let content = DimContent::themed(self.content, move |theme| dim_for(&tokens_for(theme)));

        let padding = self.padding.unwrap_or_else(|| iced::Padding::from(0));
        // Clip inside the button so its focus ring remains outside the child mask.
        let clipped_content: Element<'a, Message> =
            crate::rounded_clip::rounded_clip(content, iced::border::Radius::new(radius))
                .outset(padding)
                .into();

        let (on_activate, on_activate_with_modifiers) = if enabled {
            (self.on_activate, self.on_activate_with_modifiers)
        } else {
            (None, None)
        };

        let width = crate::accessibility_metrics::hit_floor(width, self.min_hit);
        let height = crate::accessibility_metrics::hit_floor(Length::Shrink, self.min_hit);

        let mut btn = match on_activate_with_modifiers {
            Some(f) => {
                focus_button_maybe(clipped_content, None).on_press_with_modifiers_maybe(Some(f))
            }
            None => focus_button_maybe(clipped_content, on_activate),
        }
        .padding(padding)
        .width(width)
        .height(height)
        .clip(true)
        .embedded(self.embedded)
        .on_enter_with_maybe(self.on_enter)
        .on_space_with_maybe(self.on_space)
        .focus_visible(self.focus_visible)
        .semantics(semantics)
        .style_focus(move |theme: &Theme, status: FocusStatus| {
            let mut t = tokens_for(theme);
            let dim = dim_for(&t);
            if let Some(w) = ring_width {
                t.focus_ring_width = w;
            }
            if let Some(c) = ring_color {
                t.focus_ring = c;
            }
            let overrides = colors.as_ref().map(|f| f(theme)).unwrap_or_default();
            pressable_style(
                status,
                style,
                selected,
                radius,
                border_width,
                t,
                overrides,
                dim,
            )
        });

        if let Some(id) = self.id {
            btn = btn.id(id);
        }
        if let Some(align) = self.content_align_x {
            btn = btn.content_align_x(align);
        }

        match min_height_floor {
            Some(floor) => {
                let mut mh = crate::min_height::min_height(btn, floor);
                if self.min_height_center {
                    mh = mh.align_y(iced::Alignment::Center);
                }
                mh.into()
            }
            None => btn.into(),
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzPressable<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzPressable<'a, Message>) -> Self {
        w.into_element()
    }
}

fn base_fill(style: PressableStyle, t: &PressableTokens) -> Color {
    match style {
        PressableStyle::Card => t.card_bg,
        PressableStyle::Chip => t.chip_bg,
        PressableStyle::Row | PressableStyle::Plain => Color::TRANSPARENT,
    }
}

fn selected_fill(style: PressableStyle, t: &PressableTokens) -> Color {
    match style {
        PressableStyle::Card | PressableStyle::Plain => t.card_selected_bg,
        PressableStyle::Row | PressableStyle::Chip => t.row_chip_selected_bg,
    }
}

fn dimmed(c: Color, dim: f32) -> Color {
    if dim >= 1.0 {
        return c;
    }
    Color { a: c.a * dim, ..c }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the style inputs: status, style, palette and pins"
)]
fn pressable_style(
    status: FocusStatus,
    style: PressableStyle,
    selected: bool,
    radius: f32,
    border_width: f32,
    t: PressableTokens,
    colors: SurfaceColors,
    dim: f32,
) -> ButtonStyle {
    let base = colors.background.unwrap_or_else(|| base_fill(style, &t));
    let bg = if selected {
        selected_fill(style, &t)
    } else {
        match status {
            FocusStatus::Pressed => match colors.hover_background {
                Some(h) => composite(h, t.pressed_overlay),
                None => composite(base, t.pressed_overlay),
            },
            FocusStatus::Hovered => match colors.hover_background {
                Some(h) => h,
                None => composite(base, t.hover_overlay),
            },
            _ => base,
        }
    };

    let resting_border = if let Some(c) = colors.border {
        c
    } else if matches!(style, PressableStyle::Card) {
        if selected {
            t.card_selected_border
        } else {
            t.card_border
        }
    } else {
        Color::TRANSPARENT
    };

    let border = if matches!(status, FocusStatus::Focused) {
        Border {
            color: t.focus_ring,
            width: t.focus_ring_width,
            radius: radius.into(),
        }
    } else {
        Border {
            color: resting_border,
            width: border_width,
            radius: radius.into(),
        }
    };

    ButtonStyle {
        background: Some(Background::Color(dimmed(bg, dim))),
        text_color: dimmed(t.text, dim),
        border: Border {
            color: dimmed(border.color, dim),
            ..border
        },
        ..ButtonStyle::default()
    }
}

impl crate::theme_lookup::FollowsThemeOpacity for PressableTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled_surface(),
            ..self
        }
    }
}
