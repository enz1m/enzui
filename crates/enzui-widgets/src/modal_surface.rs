//! Opaque rounded surface for dialog content.
//!
//! Defaults are a 12 px content inset, the theme's `dialog` corner radius
//! (12 px in the built-in theme) and level-4 elevation.
//! Colors and shadow follow the runtime theme. The surface owns decoration;
//! [`modal_host`](crate::modal_host) owns modality, dismissal and focus.
//!
//! `.padding_bottom(..)` overrides the bottom inset. For scrolling bodies,
//! pair `.padding_bottom(0.0)` with a bottom inset inside the scrollable so
//! content can reach the surface edge. The visible border remains protected.
//! `.on_dismiss(..)` adds a labeled close button inset from the surface edge.

use enzui_theme::{Palette, SpaceToken};
use iced::widget::{container, stack};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

/// Default elevation for a modal dialog surface: a raised dialog sits high
/// above the page. Matches the source `elevation: 0..5` default of 4 called
/// by the source and the `EnzCard` "Elevated" popout tier.
pub const DEFAULT_ELEVATION: u8 = 4;

// ---------------------------------------------------------------------------
// Close-affordance constants
// ---------------------------------------------------------------------------

/// Control diameter of the modal-surface close button (px). Matches
/// [`window_control_button::DEFAULT_CONTROL_SIZE`](crate::window_control_button::DEFAULT_CONTROL_SIZE)
/// — a familiar 28 px circle.
pub const CLOSE_CONTROL_SIZE: f32 = 28.0;

/// Glyph size of the close button (px). `icon-sm` = 16 px.
pub const CLOSE_ICON_SIZE: f32 = 16.0;

/// Inset in px from the painted surface edge to the close button's bounding
/// box at the default corner radius, matching the window close control's: a
/// 28 px control 8 px in from a 22 px corner is concentric with it. It is
/// measured from the surface edge, not from inside its padding. See
/// [`close_inset`] for other radii.
pub const CLOSE_INSET: f32 = 8.0;

/// The close button's inset for a surface with corner `radius`: concentric
/// with the corner, as the window controls are, and never closer to the edge
/// than [`CLOSE_INSET`] nor further than 16 px.
#[must_use]
pub fn close_inset(radius: f32) -> f32 {
    let concentric = radius - CLOSE_CONTROL_SIZE / 2.0;
    if concentric.is_finite() {
        concentric.clamp(CLOSE_INSET, 16.0)
    } else {
        CLOSE_INSET
    }
}

/// Default dialog content inset: 24 logical pixels (`Xl`), the breathing
/// room libadwaita and macOS alerts give their content.
/// Presets with their own content container share this token.
pub const DEFAULT_DIALOG_PADDING: SpaceToken = SpaceToken::Xl;

/// Id-derivation prefix for the close button. One modal surface at a time
/// is the normal case, so a fixed suffix under this prefix is unique enough.
const CLOSE_ID_PREFIX: &str = "enz-modal-surface";

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colour tokens for the modal surface.
#[derive(Debug, Clone, Copy)]
pub struct ModalSurfaceTokens {
    /// Dialog surface fill (default `surface_elevated`).
    pub background: Color,
    /// Dialog border color, derived from the surface fill.
    pub border: Color,
}

impl ModalSurfaceTokens {
    /// Derive tokens from a runtime palette (the normal path).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_elevated,
            border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
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
}

// ---------------------------------------------------------------------------
// Close-affordance tokens
// ---------------------------------------------------------------------------

/// Colour tokens for the modal-surface close button.
///
/// Resolved from the runtime theme palette by default; a caller that pins
/// [`ModalSurfaceTokens`] can also pin these to keep the two in sync.
#[derive(Debug, Clone, Copy)]
pub struct ModalCloseTokens {
    /// Glyph + icon tint. Default: `palette.text_muted` — dismissal is
    /// secondary, not the dialog's primary action, so the X is muted rather
    /// than accent-coloured.
    pub fg: Color,
    /// Resting fill (transparent).
    pub bg: Color,
    /// Hover fill. Default: `palette.overlay_hover()` — the standard
    /// surface-relative wash.
    pub hover_bg: Color,
    /// Pressed fill. Default: `palette.overlay_pressed()`.
    pub pressed_bg: Color,
    /// Focus ring ink.
    pub focus_ring: Color,
}

impl ModalCloseTokens {
    /// Derive from a runtime palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            fg: p.text_muted,
            bg: Color::TRANSPARENT,
            hover_bg: p.overlay_hover(),
            pressed_bg: p.overlay_pressed(),
            focus_ring: p.focus_ring,
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
}

/// The [`ButtonSizeTokens`](crate::tokens::ButtonSizeTokens) geometry the
/// close affordance uses: a square box with `radius = size / 2` (a
/// perfect circle). Exposed for tests and for callers that need to
/// query the geometry without building an element.
pub fn close_button_geometry() -> crate::tokens::ButtonSizeTokens {
    crate::tokens::ButtonSizeTokens {
        height: CLOSE_CONTROL_SIZE,
        padding_x: 0.0,
        text_size: CLOSE_ICON_SIZE,
        icon_size: CLOSE_ICON_SIZE,
        radius: CLOSE_CONTROL_SIZE / 2.0,
        gap: 0.0,
    }
}

/// The [`ButtonTokens`](crate::tokens::ButtonTokens) the close affordance
/// uses under `palette`. Only the Ghost path is exercised, so only the
/// `ghost_*` fills, the ink, and the focus ring matter.
pub fn close_button_tokens(palette: &Palette) -> crate::tokens::ButtonTokens {
    let ct = ModalCloseTokens::from_palette(palette);
    crate::tokens::ButtonTokens {
        ghost_bg: ct.bg,
        ghost_hover_bg: ct.hover_bg,
        ghost_pressed_bg: ct.pressed_bg,
        ghost_fg: ct.fg,
        // The framework ghost kind paints a rest border to fix
        // the light-theme flat-button-vs-page defect. The modal close
        // affordance is a titlebar-slot chrome control whose visual
        // relationship to the modal surface is already carried by
        // `ModalCloseTokens` (its own hover/pressed washes), so this
        // caller opts back into the borderless idiom.
        ghost_border_width: 0.0,
        focus_ring: ct.focus_ring,
        ..crate::tokens::ButtonTokens::from_palette(palette)
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for a modal dialog surface. See [`enz_modal_surface`].
#[must_use]
pub struct EnzModalSurface<'a, Message> {
    /// See [`EnzModalSurface::material`].
    material: Option<enzui_theme::MaterialStyle>,
    /// See [`EnzModalSurface::grain`].
    grain: enzui_theme::GrainSpec,
    content: Element<'a, Message>,
    radius: f32,
    padding: f32,
    /// When `Some`, replaces `padding` on the bottom edge only.
    /// `None` keeps the uniform `padding`.
    bottom_padding: Option<f32>,
    top_padding: Option<f32>,
    right_padding: Option<f32>,
    border_width: f32,
    /// Drop-shadow row `0..=5` (clamped). Level 0 = flat (no shadow).
    elevation: u8,
    tokens: Option<ModalSurfaceTokens>,
    /// Width of the painted container. Defaults to `Length::Shrink` (fit
    /// to content). Applied directly to the surface's visible background /
    /// border / shadow container, not a transparent wrapper, so a requested
    /// width actually reaches the visible surface.
    width: Length,
    /// Height of the painted container. Same semantics as `width`.
    height: Length,
    /// Per-field colour overrides, applied on top of runtime-resolved tokens.
    /// Unlike `tokens`, these compose with runtime resolution: an unpinned
    /// field still follows the live theme. `ShortcutsDialog` uses this to pin
    /// one surface colour without losing runtime resolution of the other.
    background_override: Option<Color>,
    border_override: Option<Color>,
    /// Lazy dismiss factory for the close affordance. When set, the surface
    /// renders a circular X button in the top-right corner. Opt-in: the
    /// surface doesn't own a message type, so callers must provide the
    /// factory. Higher-level dialogs (about, confirm, form) wire this
    /// through automatically.
    close_dismiss: Option<Box<dyn Fn() -> Message + 'a>>,
    /// Stable widget id for the close button. When unset a fixed derived id
    /// is used (`CLOSE_ID_PREFIX` + "close").
    close_button_id: Option<iced::advanced::widget::Id>,
    /// Override for the close affordance's accessible name. When
    /// unset the generic `"Close"` string is published; higher-level
    /// presets that own a title (`enz_settings_dialog`, `about`, ...) thread a
    /// disambiguated string like `"Close settings"` through this so a
    /// composed screen does not publish two identically-named `Role::Button`
    /// nodes.
    close_label: Option<String>,
}

/// Construct a modal surface around `content`.
pub fn enz_modal_surface<'a, Message: 'a>(
    content: Element<'a, Message>,
) -> EnzModalSurface<'a, Message> {
    EnzModalSurface {
        material: None,
        grain: enzui_theme::GrainSpec::None,
        content,
        radius: enzui_theme::text_defaults::shape_radius(|s| s.dialog),
        padding: DEFAULT_DIALOG_PADDING.px(),
        bottom_padding: None,
        top_padding: None,
        right_padding: None,
        border_width: 1.0,
        elevation: DEFAULT_ELEVATION,
        tokens: None,
        width: Length::Shrink,
        height: Length::Shrink,
        background_override: None,
        border_override: None,
        close_dismiss: None,
        close_button_id: None,
        close_label: None,
    }
}

impl<'a, Message: Clone + 'a> EnzModalSurface<'a, Message> {
    /// Corner radius in px, instead of the theme's `dialog` radius.
    /// Non-finite or negative input keeps the theme's.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = enzui_core::metrics::FiniteNonNeg::new_or(r, self.radius).get();
        self
    }
    /// Inner padding in px. Non-finite or negative input falls back to 12 (`Md`).
    pub fn padding(mut self, p: f32) -> Self {
        self.padding =
            enzui_core::metrics::FiniteNonNeg::new_or(p, DEFAULT_DIALOG_PADDING.px()).get();
        self
    }
    /// The bottom padding in px, overriding [`padding`](Self::padding) for that
    /// edge. Use it when a scrollable body should run to the surface's bottom
    /// edge; pad the scrollable's own content instead.
    ///
    /// The value is floored at the border width so the child never paints
    /// over the surface's border. Non-finite or negative values fall back to
    /// the uniform padding.
    pub fn padding_bottom(mut self, p: f32) -> Self {
        self.bottom_padding =
            Some(enzui_core::metrics::FiniteNonNeg::new_or(p, self.padding).get());
        self
    }
    /// The top padding in px, overriding [`padding`](Self::padding) for that
    /// edge: a header that shares the close button's band starts at
    /// [`close_inset`]. Floored at the border width; non-finite or negative
    /// values fall back to the uniform padding.
    pub fn padding_top(mut self, p: f32) -> Self {
        self.top_padding = Some(enzui_core::metrics::FiniteNonNeg::new_or(p, self.padding).get());
        self
    }
    /// The right padding in px, overriding [`padding`](Self::padding) for that
    /// edge: a scrollable body that should keep its scrollbar at the surface's
    /// edge pads its own content instead. Floored at the border width.
    pub fn padding_right(mut self, p: f32) -> Self {
        self.right_padding = Some(enzui_core::metrics::FiniteNonNeg::new_or(p, self.padding).get());
        self
    }
    /// Border width in px. Non-finite or negative input falls back to 1.
    pub fn border_width(mut self, w: f32) -> Self {
        self.border_width = enzui_core::metrics::FiniteNonNeg::new_or(w, 1.0).get();
        self
    }
    /// Drop-shadow elevation row `0..=5` (values above 5 clamp to 5). Level 0
    /// is flat — no shadow. Defaults to [`DEFAULT_ELEVATION`] (4). The blur,
    /// offset and colour all come from the runtime theme's
    /// [`enzui_theme::ElevationScale`]; no shadow literal is introduced.
    pub fn elevation(mut self, level: u8) -> Self {
        self.elevation = level.min(5);
        self
    }
    /// Override the colour tokens. Unset by default (resolved from the runtime
    /// theme).
    pub fn tokens(mut self, t: ModalSurfaceTokens) -> Self {
        self.tokens = Some(t);
        self
    }
    /// The width of the painted surface (default [`Shrink`](Length::Shrink)).
    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }
    /// Height of the painted surface container. Same semantics as
    /// [`width`](Self::width).
    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }
    /// Override the surface background colour, leaving the border resolved
    /// from the runtime theme. Composes with `tokens()`: if both are set,
    /// `tokens()` wins.
    pub fn background(mut self, c: Color) -> Self {
        self.background_override = Some(c);
        self
    }
    /// Override the surface border colour, leaving the background resolved
    /// from the runtime theme. Composes with `tokens()`: if both are set,
    /// `tokens()` wins.
    pub fn border_color(mut self, c: Color) -> Self {
        self.border_override = Some(c);
        self
    }
    /// The shadow this surface paints under `theme`, the same value the
    /// render path puts in `container::Style.shadow`. The flat tier (level 0)
    /// yields [`Shadow::default`]. Exposed so tests can assert the resolved
    /// shadow rather than read back the stored elevation.
    pub fn resolved_shadow(&self, theme: &iced::Theme) -> Shadow {
        shadow_for(theme, self.elevation)
    }
    /// Adds a circular close button in the top-right corner; `f` builds the
    /// message on activation (pointer, Enter, Space or an assistive-technology
    /// action). The higher-level dialogs wire this.
    pub fn on_dismiss(mut self, f: impl Fn() -> Message + 'a) -> Self {
        self.close_dismiss = Some(Box::new(f));
        self
    }
    /// Override the close button's widget id. Without this a fixed derived
    /// id (`CLOSE_ID_PREFIX` + `"close"`) is used. Override when multiple
    /// surfaces may be live at once (e.g. stacked modals).
    pub fn close_button_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.close_button_id = Some(id);
        self
    }

    /// The close button's accessible name (default `"Close"`). Presets with a
    /// title derive a specific one, such as `"Close settings"`.
    pub fn close_label(mut self, label: impl Into<String>) -> Self {
        self.close_label = Some(label.into());
        self
    }

    /// Whether this surface will render a close affordance.
    pub fn has_close_affordance(&self) -> bool {
        self.close_dismiss.is_some()
    }

    /// Concentric radius for a child inset by `padding`.
    pub fn content_radius(&self) -> f32 {
        (self.radius - self.padding).max(0.0)
    }

    /// The four inner padding edges this surface will apply, with the
    /// bottom-edge override resolved (and floored at the border width, so a
    /// child can never paint over the surface's own border).
    fn resolved_padding(&self) -> Padding {
        Padding {
            top: self
                .top_padding
                .map_or(self.padding, |p| p.max(self.border_width)),
            right: self
                .right_padding
                .map_or(self.padding, |p| p.max(self.border_width)),
            bottom: self
                .bottom_padding
                .map_or(self.padding, |p| p.max(self.border_width)),
            left: self.padding,
        }
    }

    /// Put the dialog surface on liquid glass of `style` (see [`crate::glass`]).
    /// Controls inside default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Lay grain on the dialog surface: a [`Grain`](enzui_theme::Grain), or
    /// [`GrainSpec::Theme`](enzui_theme::GrainSpec::Theme) for the theme's.
    /// See [`crate::grain`].
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// [`Self::material`] for a caller that forwards an optional style.
    pub(crate) fn material_opt(mut self, style: Option<enzui_theme::MaterialStyle>) -> Self {
        self.material = style;
        self
    }

    /// Builder-consuming widget-tree producing.
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.material;
        let radius = self.radius;
        let bw = self.border_width;
        let padding = self.resolved_padding();
        let elevation = self.elevation;
        let pinned = self.tokens;
        let bg_override = self.background_override;
        let border_override = self.border_override;
        // On glass the glass paints the panel, its rim and its shadow.
        let glass = material.is_some();

        // Painted-surface-container building: padding/border/background/shadow
        // content.
        // Grain lies on the panel under the content, across the padding,
        // masked to the panel's corners.
        let content: Element<'a, Message> = if self.grain == enzui_theme::GrainSpec::None {
            self.content
        } else {
            crate::grain::GrainLayer::new(self.content, self.grain)
                .outset(padding)
                .radius(radius.into())
                .into()
        };
        let painted: Element<'a, Message> = container(content)
            .width(self.width)
            .height(self.height)
            .padding(padding)
            .style(move |theme: &iced::Theme| {
                if glass {
                    return container::Style::default();
                }
                let mut t = pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, ModalSurfaceTokens::from_palette)
                });
                // Resolved-token-composed per-field overrides: live-theme-
                // following-still unpinned fields.
                if let Some(bg) = bg_override {
                    t.background = bg;
                }
                if let Some(bc) = border_override {
                    t.border = bc;
                }
                // The shadow is a draw-time `container::Style`
                // field, so it follows the runtime theme's elevation ladder
                // (blur/offset from the tier, colour from the mode-dependent
                // `elevation-shadow` token). A flat tier -> `Shadow::default()`,
                // i.e. no shadow — never a zero-blur shadow.
                container::Style {
                    background: Some(Background::Color(t.background)),
                    border: Border {
                        color: t.border,
                        width: bw,
                        radius: radius.into(),
                    },
                    // Same resolver as `resolved_shadow`, deliberately: a
                    // query helper and a paint site that agree only by
                    // convention drift, and every test here reads the helper —
                    // pinning this line to a constant level left all of them
                    // green when I tried it. `shadow_for` is the single
                    // resolution both go through, so they cannot disagree.
                    shadow: shadow_for(theme, elevation),
                    ..Default::default()
                }
            })
            .into();

        // The close button is stacked over the painted surface, outside its
        // padding, so `CLOSE_INSET` is measured from the surface edge.
        let surface: Element<'a, Message> = if let Some(dismiss) = self.close_dismiss {
            let close_btn = build_close_button(dismiss, self.close_button_id, self.close_label);
            // The stack takes the surface's own width and height explicitly;
            // its default `Fit` collapses the surface under a shrinking
            // parent.
            stack![
                painted,
                container(close_btn)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top)
                    .padding(Padding {
                        top: close_inset(radius),
                        right: close_inset(radius),
                        bottom: 0.0,
                        left: 0.0,
                    })
            ]
            .width(self.width)
            .height(self.height)
            .into()
        } else {
            painted
        };
        match material {
            Some(style) => crate::glass::enz_glass(surface)
                .style(style)
                .radius(radius)
                .elevation(elevation)
                .into(),
            None => surface,
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzModalSurface<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzModalSurface<'a, Message>) -> Self {
        w.into_element()
    }
}

/// Pure helper — concentric radius for a child inset by `padding`
/// inside a modal-surface of `radius`.
pub const fn concentric_radius(radius: f32, padding: f32) -> f32 {
    let r = radius - padding;
    if r < 0.0 { 0.0 } else { r }
}

// ---------------------------------------------------------------------------
// Close-affordance builder
// ---------------------------------------------------------------------------

pub(crate) fn build_close_button<'a, Message: Clone + 'a>(
    dismiss: Box<dyn Fn() -> Message + 'a>,
    id: Option<iced::advanced::widget::Id>,
    label_override: Option<String>,
) -> Element<'a, Message> {
    use crate::focus_button::FocusStatus;
    use crate::icon::enz_icon;
    use iced::Alignment;

    let btn_id = id.unwrap_or_else(|| enzui_core::identity::derive_id(CLOSE_ID_PREFIX, "close"));

    let control_size = CLOSE_CONTROL_SIZE;
    let icon_size = CLOSE_ICON_SIZE;
    let radius = control_size / 2.0;

    // Shared status cell: the focus_button's style closure writes the live
    // FocusStatus and the icon reads it, so both resolve the same
    // interaction state every frame.
    let status_cell = std::rc::Rc::new(std::cell::Cell::new(FocusStatus::Active));

    let inner = container(enz_icon("action.close").size(icon_size).tint_with_status(
        std::rc::Rc::clone(&status_cell),
        move |theme: &iced::Theme, status: FocusStatus| {
            let p = crate::theme_lookup::palette(theme);
            match status {
                FocusStatus::Hovered | FocusStatus::Pressed => p.text,
                FocusStatus::Disabled => p.disabled,
                _ => p.text_muted,
            }
        },
    ))
    .width(Length::Fixed(control_size))
    .height(Length::Fixed(control_size))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center);

    // An unset override keeps the plain `"Close"` string so the
    // opt-in modal-surface affordance stays source-compatible; presets that
    // own a title (settings dialog, about) thread a disambiguated name
    // through so a composed screen does not publish two identically-named
    // `Role::Button` nodes.
    let label = label_override
        .unwrap_or_else(|| enzui_core::localization::text("enzui-modal-surface-close"));
    let semantics = enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Button)
        .label(label)
        .default_action();

    // The style closure: Ghost-like background with per-render theme
    // resolution. Hover/pressed washes come from the palette's overlay
    // roles (surface-relative, not scrim-relative).
    let style_fn = {
        move |theme: &iced::Theme, status: FocusStatus| -> iced::widget::button::Style {
            status_cell.set(status);
            let p = crate::theme_lookup::palette(theme);
            let bg = match status {
                FocusStatus::Hovered => p.overlay_hover(),
                FocusStatus::Pressed => p.overlay_pressed(),
                _ => Color::TRANSPARENT,
            };
            let border = if matches!(status, FocusStatus::Focused) {
                crate::focus_button::icon_action_focus_ring(theme, bg, radius)
            } else {
                Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: radius.into(),
                }
            };
            iced::widget::button::Style {
                background: Some(Background::Color(bg)),
                border,
                ..Default::default()
            }
        }
    };

    crate::focus_button::focus_button_maybe_with(inner, Some(dismiss))
        .style_focus(style_fn)
        .semantics(semantics)
        .id(btn_id)
        .width(Length::Fixed(control_size))
        .height(Length::Fixed(control_size))
        .padding(Padding::ZERO)
        .into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The shadow an [`EnzModalSurface`] paints at `level` under `theme`.
///
/// Single resolution point for both the render closure and
/// [`EnzModalSurface::resolved_shadow`]. Two places computing the same thing is
/// a mutation nothing can catch — the same lesson `modal_host::resolve_scrim`
/// records for the scrim.
fn shadow_for(theme: &iced::Theme, level: u8) -> Shadow {
    crate::elevation::iced_shadow_for_theme(theme, level)
}

#[cfg(test)]
mod tests {

    // -- Elevation drop-shadow ----------------------------------------------
    //
    // `container::Style` carries a `shadow: Shadow` field. These assert the *resolved*
    // shadow the render path drops into that field (via `resolved_shadow`, which is
    // exactly `iced_shadow_for_theme(theme, self.elevation)` — the same call the style
    // closure makes), never the stored `elevation` field.

    // -- geometry must be unaffected by the shadow --------------------------
    //
    // A shadow is a `container::Style` field and must not touch layout. These
    // measure the *laid-out* bounds and prove they are identical across
    // elevation levels (including flat), which is the guarantee that adding a
    // shadow did not move anything.

    // -- Width/height reach the painted container ----------------------------

    // -----------------------------------------------------------------------
    // Close affordance
    // -----------------------------------------------------------------------
}
