//! Button interaction and semantics for arbitrary content.
//!
//! [`EnzClickable`] delegates focus, pointer/touch activation, Enter/Space and
//! routed accessibility actions to [`crate::focus_button::FocusButton`]. Chrome
//! follows the live palette unless tokens are pinned. Disabled controls suppress
//! activation and dim chrome and inherited text color; explicit child colors
//! remain unchanged. Fixed-size children retain their requested dimensions;
//! fluid children fill the padded control slot. Content is aligned and clipped
//! inside the button's focus ring.
//!
//! The default requested minimum is [`DEFAULT_MIN_HIT_TARGET`] logical pixels
//! per axis. Fixed sizes bypass it, and parent constraints can limit layout.
//! [`EnzClickable::min_hit`] accepts zero to remove the minimum. Provide a label
//! when content has no usable text name. [`crate::pressable::EnzPressable`] also
//! supports persistent selection styling.

use crate::accessibility_metrics::DEFAULT_MIN_HIT_TARGET;
use crate::focus_button::{
    FocusStatus, focus_button_maybe, focus_button_maybe_with, focus_button_maybe_with_point,
};
use crate::surface_interaction::{DISABLED_ALPHA, DimContent, composite};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::{Background, Border, Color, Element, Length, Padding, Theme};

/// Chrome colors and geometry for [`EnzClickable`].
///
/// Defaults resolve from the runtime palette during drawing. Hover and pressed
/// overlays are composited over the token background; resulting contrast still
/// depends on the content and surrounding surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClickableTokens {
    /// Resting fill, transparent in palette-derived tokens.
    pub background: Color,
    /// Translucent hover tint composited over [`background`](Self::background)
    /// (`Palette::overlay_hover`, `text` at 0.07 alpha).
    pub hover_overlay: Color,
    /// Translucent pressed tint composited over
    /// [`background`](Self::background) (`Palette::overlay_pressed`, `text` at
    /// 0.12 alpha) — one step heavier than hover.
    pub pressed_overlay: Color,
    /// Resting border colour, painted only when
    /// [`border_width`](EnzClickable::border_width) is non-zero
    /// (`palette.border`).
    pub border: Color,
    /// Inherited foreground ink for the wrapped content (`palette.text`). Also
    /// the value the disabled dim is applied to.
    pub text: Color,
    /// Focus-visible ring colour (`palette.focus_ring`).
    pub focus_ring: Color,
    /// Focus-visible ring width. Zero is honoured literally (a caller may
    /// legitimately suppress the ring).
    pub focus_ring_width: f32,
    /// Disabled opacity factor for chrome and inherited text color.
    pub disabled_alpha: f32,
}

impl ClickableTokens {
    /// Derive the tokens from a runtime palette — the normal path.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: Color::TRANSPARENT,
            hover_overlay: p.overlay_hover(),
            pressed_overlay: p.overlay_pressed(),
            border: p.border,
            text: p.text,
            focus_ring: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            disabled_alpha: DISABLED_ALPHA,
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

    /// Replace invalid ring width and disabled alpha with 2.0 and the shared default.
    ///
    /// Zero is preserved. Disabled alpha has no upper bound here, and colors
    /// are not validated.
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

#[derive(Clone)]
enum TokenSource {
    Fixed(ClickableTokens),
    Resolved(std::rc::Rc<dyn Fn(&Theme) -> ClickableTokens>),
}

impl TokenSource {
    fn resolve(&self, theme: &Theme) -> ClickableTokens {
        match self {
            Self::Fixed(t) => *t,
            Self::Resolved(f) => f(theme),
        }
    }
}

enum Activation<'a, Message> {
    Eager(Message),
    Lazy(Box<dyn Fn() -> Message + 'a>),
    Positioned(Box<dyn Fn(Option<iced::Point>) -> Message + 'a>),
}

/// Builder for a clickable wrapper around arbitrary content. See
/// [`enz_clickable`].
pub struct EnzClickable<'a, Message> {
    content: Element<'a, Message>,
    content_status: Option<ClickableStatus>,
    on_press: Option<Activation<'a, Message>>,
    enabled: bool,
    aria_label: Option<String>,
    description: Option<String>,
    id: Option<iced::advanced::widget::Id>,
    tokens: Option<TokenSource>,
    /// Set by [`EnzClickable::width`]; unset, the content's sizing decides.
    width: Option<Length>,
    height: Option<Length>,
    padding: Padding,
    radius: f32,
    border_width: f32,
    min_hit: f32,
    align_x: Option<iced::Alignment>,
    align_y: Option<iced::Alignment>,
}

/// Wrap `content` so it behaves as a button activated by `on_press` — pointer
/// click, Enter, Space, or a routed AccessKit `Click`.
pub fn enz_clickable<'a, Message: Clone + 'a>(
    content: impl Into<Element<'a, Message>>,
    on_press: Message,
) -> EnzClickable<'a, Message> {
    enz_clickable_maybe(content, Some(on_press))
}

/// Live clickable state read by the content's style or draw callback.
pub type ClickableStatus = crate::focus_button::ContentStatus;

/// Builds content whose paint follows the clickable's own interaction state.
///
/// The factory runs once when the view is built. Move its status handle into
/// the child's style/draw callback and read [`ClickableStatus::get`] there.
/// Hover, pointer/touch press, keyboard Space, focus and disabled state remain
/// owned by the clickable; no application messages or content rebuilds are
/// needed for a state transition. Geometry remains the child's responsibility.
pub fn enz_clickable_with_status<'a, Message: Clone + 'a>(
    content: impl FnOnce(ClickableStatus) -> Element<'a, Message>,
    on_press: Message,
) -> EnzClickable<'a, Message> {
    let status = ClickableStatus::default();
    let mut clickable = enz_clickable(content(status.clone()), on_press);
    clickable.content_status = Some(status);
    clickable
}

/// Wrap `content` as a button whose activation message may be absent.
///
/// `None` disables activation, Tab participation and the accessible `Click`
/// action, and applies disabled chrome, as does `.enabled(false)`.
pub fn enz_clickable_maybe<'a, Message: Clone + 'a>(
    content: impl Into<Element<'a, Message>>,
    on_press: Option<Message>,
) -> EnzClickable<'a, Message> {
    EnzClickable {
        content: content.into(),
        content_status: None,
        on_press: on_press.map(Activation::Eager),
        enabled: true,
        aria_label: None,
        description: None,
        id: None,
        tokens: None,
        width: None,
        height: None,
        padding: Padding::new(0.0),
        radius: 0.0,
        border_width: 0.0,
        min_hit: DEFAULT_MIN_HIT_TARGET,
        align_x: None,
        align_y: None,
    }
}

impl<'a, Message: Clone + 'a> EnzClickable<'a, Message> {
    /// Set a factory invoked on activation, without constructing messages during view building.
    ///
    /// Disabled controls do not invoke the factory.
    #[must_use]
    pub fn on_press_with(mut self, on_press: impl Fn() -> Message + 'a) -> Self {
        self.on_press = Some(Activation::Lazy(Box::new(on_press)));
        self
    }

    /// Set an activation factory receiving the window-space point latched at press.
    ///
    /// Keyboard and routed accessibility activation receive `None`. The host
    /// can choose a layout-derived anchor when no pointer point is available.
    #[must_use]
    pub fn on_press_with_point(
        mut self,
        on_press: impl Fn(Option<iced::Point>) -> Message + 'a,
    ) -> Self {
        self.on_press = Some(Activation::Positioned(Box::new(on_press)));
        self
    }

    /// Enable interaction; defaults to `true`.
    ///
    /// Disabling suppresses activation and dims chrome and inherited text
    /// color by [`ClickableTokens::disabled_alpha`]. Explicit child colors remain unchanged.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Accessible name for the AT tree.
    ///
    /// Strongly recommended. The semantics collector can synthesize a name
    /// from a wrapped text node, but an icon-only or graphic composition has
    /// nothing to synthesize from and would reach AT-SPI as a nameless
    /// `Role::Button` — a screen-reader user hears "button" and nothing else.
    #[must_use]
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Supplementary accessible description (AT-SPI *description*), for a hint
    /// that should not be part of the name.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Assign a stable widget id, so programmatic `focus(id)` and a routed
    /// AccessKit action can target this control.
    #[must_use]
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Pin all chrome tokens, sanitizing geometry and disabled alpha.
    ///
    /// Pinned colors do not follow theme changes; use [`Self::tokens_resolved`]
    /// for overrides that should continue tracking the runtime palette.
    #[must_use]
    pub fn tokens(mut self, tokens: ClickableTokens) -> Self {
        self.tokens = Some(TokenSource::Fixed(tokens.sanitized()));
        self
    }

    /// Resolve and sanitize chrome tokens from the runtime theme during drawing.
    ///
    /// The resolver may run more than once per draw for content and chrome.
    #[must_use]
    pub fn tokens_resolved(
        mut self,
        resolver: impl Fn(&Theme) -> ClickableTokens + 'static,
    ) -> Self {
        self.tokens = Some(TokenSource::Resolved(std::rc::Rc::new(move |theme| {
            resolver(theme).sanitized()
        })));
        self
    }

    /// Outer width. Unset, it follows the content: filling where the content
    /// fills (`Fill` or `FillPortion`), otherwise [`Length::Shrink`] (hug it).
    ///
    /// Whatever is passed is still floored by [`min_hit`](Self::min_hit): a
    /// `Shrink` or `Fill` width gains a minimum bound, while an explicit
    /// [`Length::Fixed`] is honoured literally (a caller stating an exact size
    /// means it).
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    /// Outer height. Defaults like [`width`](Self::width). Floored by
    /// [`min_hit`](Self::min_hit) on the same terms as [`width`](Self::width).
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = Some(height.into());
        self
    }

    /// Inner padding around the content. Defaults to zero — an arbitrary
    /// composition normally carries its own insets.
    #[must_use]
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Corner radius of the chrome quad and of the content mask.
    ///
    /// The content is masked to this rounded outline via
    /// [`crate::rounded_clip`], not merely clipped rectangularly, so a
    /// full-bleed child (a thumbnail, a coloured tile) cannot paint through
    /// the corner arcs. Non-finite / negative values are ignored.
    #[must_use]
    pub fn radius(mut self, radius: f32) -> Self {
        if radius.is_finite() && radius >= 0.0 {
            self.radius = radius;
        }
        self
    }

    /// Resting border width, painted in [`ClickableTokens::border`]. Defaults
    /// to zero (no border). Non-finite / negative values are ignored.
    #[must_use]
    pub fn border_width(mut self, width: f32) -> Self {
        if width.is_finite() && width >= 0.0 {
            self.border_width = width;
        }
        self
    }

    /// Set a requested minimum per axis in logical pixels, defaulting to [`DEFAULT_MIN_HIT_TARGET`].
    ///
    /// Zero removes the minimum. Invalid values leave the previous value
    /// unchanged. Fixed lengths bypass the minimum, and parent constraints may
    /// limit the resulting bounds.
    #[must_use]
    pub fn min_hit(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.min_hit = px;
        }
        self
    }

    /// Set horizontal content alignment.
    ///
    /// When unset, an effective `Shrink` or `Fit` length aligns to the leading
    /// edge; other lengths center. The default minimum turns a `Shrink` length
    /// into a bounded length, so default content is centered.
    #[must_use]
    pub fn align_x(mut self, align: impl Into<iced::Alignment>) -> Self {
        self.align_x = Some(align.into());
        self
    }

    /// Vertical alignment of the content inside the box. Unset derives it from
    /// the height, as [`align_x`](Self::align_x) does from the width.
    #[must_use]
    pub fn align_y(mut self, align: impl Into<iced::Alignment>) -> Self {
        self.align_y = Some(align.into());
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let captured = self.tokens;
        let radius = self.radius;
        let border_width = self.border_width;
        let padding = self.padding;

        let inert = !self.enabled || self.on_press.is_none();

        // FocusButton derives disabled semantics from the activation it receives.
        let mut semantics = Semantics::new(Role::Button);
        if let Some(label) = self.aria_label {
            semantics = semantics.label(label);
        }
        if let Some(description) = self.description {
            semantics = semantics.description(description);
        }

        let tokens_for = {
            let captured = captured;
            move |theme: &Theme| {
                captured.as_ref().map_or_else(
                    || crate::theme_lookup::resolve_live(theme, ClickableTokens::from_palette),
                    |source| source.resolve(theme),
                )
            }
        };
        let dim_for = move |t: &ClickableTokens| if inert { t.disabled_alpha } else { 1.0 };
        let content_tokens = tokens_for.clone();
        // Unset, the clickable fills where its content fills, so wrapping
        // content to make it pressable leaves its sizing alone.
        let content_size = self.content.as_widget().size();
        let adopt = |own: Option<Length>, content: Length| {
            own.unwrap_or_else(|| {
                if content.is_fill() {
                    content
                } else {
                    Length::Shrink
                }
            })
        };
        let own_width = adopt(self.width, content_size.width);
        let own_height = adopt(self.height, content_size.height);
        let dimmed_content =
            DimContent::themed(self.content, move |theme| dim_for(&content_tokens(theme)));

        let width = crate::accessibility_metrics::hit_floor(own_width, self.min_hit);
        let height = crate::accessibility_metrics::hit_floor(own_height, self.min_hit);
        let default_alignment = |length| {
            if matches!(length, Length::Shrink | Length::Fit) {
                iced::Alignment::Start
            } else {
                iced::Alignment::Center
            }
        };
        // The mask owns the control slot while its aligned child keeps intrinsic sizing.
        let clipped: Element<'a, Message> =
            crate::rounded_clip::rounded_clip(dimmed_content, iced::border::Radius::new(radius))
                .align_control_content(
                    iced::Size::new(content_length(own_width), content_length(own_height)),
                    [
                        self.align_x.unwrap_or_else(|| default_alignment(width)),
                        self.align_y.unwrap_or_else(|| default_alignment(height)),
                    ],
                )
                .outset(padding)
                .into();

        let mut btn = match self.on_press {
            _ if inert => focus_button_maybe::<Message, Theme, iced::Renderer>(clipped, None),
            Some(Activation::Eager(m)) => focus_button_maybe(clipped, Some(m)),
            Some(Activation::Lazy(f)) => focus_button_maybe_with(clipped, Some(f)),
            Some(Activation::Positioned(f)) => focus_button_maybe_with_point(clipped, Some(f)),
            None => focus_button_maybe(clipped, None),
        }
        .padding(padding)
        .width(width)
        .height(height)
        .clip(true)
        .semantics(semantics)
        .style_focus(move |theme: &Theme, status: FocusStatus| {
            let t = tokens_for(theme);
            clickable_style(status, t, radius, border_width, dim_for(&t))
        });

        if let Some(status) = self.content_status {
            btn = btn.content_status(status);
        }
        if let Some(align) = self.align_x {
            btn = btn.content_align_x(align);
        }
        if let Some(align) = self.align_y {
            btn = btn.content_align_y(align);
        }
        if let Some(id) = self.id {
            btn = btn.id(id);
        }

        btn.into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzClickable<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzClickable<'a, Message>) -> Self {
        w.into_element()
    }
}

fn content_length(length: Length) -> Length {
    use iced::advanced::graphics::core::length::{Constraint, Sizing};
    match length {
        Length::Fixed(_) | Length::Fluid(Constraint::Min(_)) => Length::Fill,
        Length::Fluid(Constraint::Max) => Length::Fit,
        Length::Bounded { sizing, .. } => match sizing {
            Sizing::Fit => Length::Fit,
            Sizing::Shrink => Length::Shrink,
            Sizing::Fill(portion) => Length::FillPortion(portion),
        },
        other => other,
    }
}

fn dimmed(c: Color, dim: f32) -> Color {
    if dim >= 1.0 {
        return c;
    }
    Color { a: c.a * dim, ..c }
}

fn clickable_style(
    status: FocusStatus,
    t: ClickableTokens,
    radius: f32,
    border_width: f32,
    dim: f32,
) -> ButtonStyle {
    let bg = match status {
        FocusStatus::Pressed => composite(t.background, t.pressed_overlay),
        FocusStatus::Hovered => composite(t.background, t.hover_overlay),
        FocusStatus::Active | FocusStatus::Focused | FocusStatus::Disabled => t.background,
    };

    let border = if matches!(status, FocusStatus::Focused) {
        Border {
            color: t.focus_ring,
            width: t.focus_ring_width,
            radius: radius.into(),
        }
    } else {
        Border {
            color: if border_width > 0.0 {
                t.border
            } else {
                Color::TRANSPARENT
            },
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

impl crate::theme_lookup::FollowsThemeOpacity for ClickableTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled_surface(),
            ..self
        }
    }
}
