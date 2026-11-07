//! EnzColorSwatchGrid: wrapping palette-inventory grid of coloured
//! swatches with captions.
//!
//! Each cell is a coloured rounded rectangle with a caption label beneath. Cells wrap to fit
//! the width available using iced's `Row::wrap()`.
//!
//! Unlike `EnzColorSwatchButton`, cells here are not clickable —
//! this is an inventory / token-showcase view, not a picker. Compose
//! `EnzColorSwatchButton` in a `Row::wrap()` for clickable
//! selection.
//!
//! # Backgrounds — solid and gradient
//!
//! A token's fill is a [`SwatchFill`]: a solid [`Color`] or a linear
//! `iced::Gradient`. The common solid case stays ergonomic via
//! `Color::into()` / `SwatchFill::from(color)`, so existing call sites read
//! `swatch: some_color.into()`. Gradient tokens use
//! [`SwatchFill::gradient`] or `SwatchFill::from(gradient)`. This is plain iced
//! `Background::Gradient` — no shader / backdrop-blur work (glass fills stay out
//! of scope in this repo).
//!
//! # Accessibility
//!
//! The meaningful datum of a swatch is the colour, not the token caption, and
//! AT can only speak the caption. Each cell therefore publishes a terminal
//! [`Semantics`](enzui_core::semantics) node whose description carries
//! the colour value: an authored [`EnzColorSwatchView::description`] when set,
//! otherwise a deterministic derived string — `#RRGGBB` for a solid, a
//! `linear-gradient(...)` equivalent for a gradient. An *empty* authored
//! description is honoured as "publish no description" rather than falling back
//! to the derived hex (empty is expressible intent).

use crate::enz_text::{TextAlign, TextSize, enz_text};
use enzui_core::semantics::{Role, Semantics};
use iced::widget::row::Wrapping;
use iced::widget::{column, container, row};
use iced::{Background, Border, Color, Element, Gradient, Length};

const LABEL_GAP: f32 = 10.0;

/// The vertical geometry of one cell: `(effective_swatch, label_box_height)`.
///
/// A cell stacks `swatch | LABEL_GAP | label` and must total exactly
/// `cell_height` so the fixed grid wraps and aligns. When
/// `swatch_height` is too tall to leave room for the gap and a 1px label, the
/// swatch is clamped — the grid's fixed-cell contract wins over the
/// caller's swatch size, exactly as in `radius_token_grid`. Without this, e.g.
/// `.cell_height(20)` with the default `swatch_height = 36` gives a column
/// of `36 + 10 + max(1, 20-36-10)` = 47 px inside a 20 px cell, breaking
/// the fixed-cell contract and misaligning every wrapped row.
///
/// Both inputs are already `FinitePositive`-validated by their setters, so this
/// only fits two valid numbers and introduces no order dependence. Pure so the
/// invariant `effective_swatch + LABEL_GAP + label_box_height == cell_height`
/// (when the caller's sizes fit) is directly testable.
fn cell_geometry(cell_height: f32, swatch_height: f32) -> (f32, f32) {
    let swatch_room = (cell_height - LABEL_GAP - 1.0).max(0.0);
    let effective_swatch = swatch_height.min(swatch_room);
    let label_box_height = (cell_height - effective_swatch - LABEL_GAP).max(1.0);
    (effective_swatch, label_box_height)
}

/// How a swatch cell is filled: a solid colour or a linear gradient.
///
/// iced's `Container` background accepts `Background::Color` and
/// `Background::Gradient`, so an inventory item can represent either without a
/// second grid widget. Prefer `Color::into()` for the common solid case.
#[derive(Debug, Clone, PartialEq)]
pub enum SwatchFill {
    /// A flat solid colour.
    Solid(Color),
    /// A linear gradient (angle + colour stops).
    Gradient(Gradient),
}

impl SwatchFill {
    /// Wrap a linear gradient as a swatch fill.
    pub fn gradient(gradient: impl Into<Gradient>) -> Self {
        SwatchFill::Gradient(gradient.into())
    }

    /// The iced [`Background`] this fill paints as.
    pub(crate) fn background(&self) -> Background {
        match self {
            SwatchFill::Solid(c) => Background::Color(*c),
            SwatchFill::Gradient(g) => Background::Gradient(*g),
        }
    }

    /// A deterministic textual equivalent of the fill, used as the derived
    /// accessible description when the view authors none.
    ///
    /// A solid renders as `#RRGGBB`; a gradient renders as
    /// `linear-gradient(<angle>deg, #RRGGBB <pct>%, ...)` over its populated
    /// stops. The angle is rounded to whole degrees and stops to whole percent
    /// so the string is stable and screen-reader friendly.
    pub(crate) fn derived_description(&self) -> String {
        match self {
            SwatchFill::Solid(c) => hex_of(*c),
            SwatchFill::Gradient(Gradient::Linear(linear)) => {
                let deg = linear.angle.0.to_degrees().round() as i64;
                let stops: Vec<String> = linear
                    .stops
                    .iter()
                    .flatten()
                    .map(|s| format!("{} {}%", hex_of(s.color), (s.offset * 100.0).round() as i64))
                    .collect();
                format!("linear-gradient({deg}deg, {})", stops.join(", "))
            }
        }
    }
}

impl From<Color> for SwatchFill {
    fn from(c: Color) -> Self {
        SwatchFill::Solid(c)
    }
}

impl From<Gradient> for SwatchFill {
    fn from(g: Gradient) -> Self {
        SwatchFill::Gradient(g)
    }
}

impl From<iced::gradient::Linear> for SwatchFill {
    fn from(l: iced::gradient::Linear) -> Self {
        SwatchFill::Gradient(Gradient::Linear(l))
    }
}

/// `#RRGGBB` for a colour, clamped to `[0,1]` and rounded per channel.
fn hex_of(c: Color) -> String {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", ch(c.r), ch(c.g), ch(c.b))
}

/// One colour token to preview: a caption, the fill it displays, and an
/// optional authored accessible description of the colour value.
#[derive(Debug, Clone)]
pub struct EnzColorSwatchView {
    /// Caption shown under the swatch (e.g. the token name like `"accent"`).
    pub label: String,
    /// How this swatch is filled — solid colour or gradient. The common
    /// solid case is `some_color.into()`.
    pub swatch: SwatchFill,
    /// Authored accessible description of the colour value. When `None`,
    /// the widget derives one (`#RRGGBB` / `linear-gradient(...)`). An authored
    /// empty string means "publish no description" and is not replaced by
    /// the derived hex.
    pub description: Option<String>,
}

/// Builder for a grid of colour-token swatches (a showcase demo). See
/// [`enz_color_swatch_grid`].
#[must_use]
pub struct EnzColorSwatchGrid<'a, Message> {
    swatches: Vec<EnzColorSwatchView>,
    cell_width: f32,
    cell_height: f32,
    swatch_height: f32,
    spacing_x: f32,
    spacing_y: f32,
    swatch_border: Option<Color>,
    label_color: Option<Color>,
    /// Active runtime typography scale, forwarded by the view owner holding the
    /// live `EnzTheme`. `None` = the local [`TextSize`] ramp. See
    /// [`EnzColorSwatchGrid::typography`].
    typography: Option<enzui_theme::TypographyScale>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Construct the grid from a list of colour swatches. Defaults: 120×78px cells,
/// 36px swatch height, 8/8px spacing, fill-shaded border and theme-resolved caption.
pub fn enz_color_swatch_grid<Message>(
    swatches: Vec<EnzColorSwatchView>,
) -> EnzColorSwatchGrid<'static, Message> {
    EnzColorSwatchGrid {
        swatches,
        cell_width: 120.0,
        cell_height: 78.0,
        swatch_height: 36.0,
        spacing_x: 8.0,
        spacing_y: 8.0,
        swatch_border: None,
        label_color: None,
        typography: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzColorSwatchGrid<'a, Message> {
    /// Width of each grid cell in px. Non-finite or non-positive input falls
    /// back to 120.
    pub fn cell_width(mut self, w: f32) -> Self {
        self.cell_width = enzui_core::metrics::FinitePositive::new_or(w, 120.0).get();
        self
    }
    /// Height of each grid cell in px (swatch + caption). Non-finite or
    /// non-positive input falls back to 78.
    pub fn cell_height(mut self, h: f32) -> Self {
        self.cell_height = enzui_core::metrics::FinitePositive::new_or(h, 78.0).get();
        self
    }
    /// Height of the colour swatch within a cell in px. Non-finite or
    /// non-positive input falls back to 36.
    pub fn swatch_height(mut self, h: f32) -> Self {
        self.swatch_height = enzui_core::metrics::FinitePositive::new_or(h, 36.0).get();
        self
    }
    /// Horizontal gap between cells in px. Non-finite or negative input falls
    /// back to 8.
    pub fn spacing_x(mut self, s: f32) -> Self {
        self.spacing_x = enzui_core::metrics::FiniteNonNeg::new_or(s, 8.0).get();
        self
    }
    /// Vertical gap between cell rows in px. Non-finite or negative input falls
    /// back to 8.
    pub fn spacing_y(mut self, s: f32) -> Self {
        self.spacing_y = enzui_core::metrics::FiniteNonNeg::new_or(s, 8.0).get();
        self
    }
    /// Override the border. Solid fills default to a darker shade; gradients have no border.
    pub fn swatch_border(mut self, c: Color) -> Self {
        self.swatch_border = Some(c);
        self
    }
    /// Override the caption ink. Unset by default (resolved from the theme).
    pub fn label_color(mut self, c: Color) -> Self {
        self.label_color = Some(c);
        self
    }

    /// Forward the theme's runtime [`TypographyScale`](enzui_theme::TypographyScale)
    /// so the caption resizes (and its line box reflows) under a custom scale.
    ///
    /// This is the production seam that makes a theme's typography scale reach
    /// the caption text. iced computes layout before the runtime theme exists
    /// and `Widget::layout` never receives a theme, so a leaf caption cannot
    /// follow a typography token from inside its own layout pass — the view
    /// owner holding the live `EnzTheme` calls `.typography(theme.typography)`,
    /// mirroring [`crate::radius_token_grid::EnzRadiusTokenGrid::typography`].
    /// Unset = the local [`TextSize`] ramp.
    pub fn typography(mut self, scale: enzui_theme::TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// The number of swatches in the grid.
    pub fn swatch_count(&self) -> usize {
        self.swatches.len()
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // Explicit overrides win; unset colours resolve from the runtime
        // `iced::Theme` per render. A frozen `Palette::dark()` would draw
        // dark-palette ink and borders under a light theme.
        let border_pin = self.swatch_border;
        let label_pin = self.label_color;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        // Clamp the swatch to fit the fixed cell, and take
        // the label band from the same split so the two cannot drift. A
        // no-op in the common case (`min` returns the original swatch height).
        let (swatch_height, label_box_height) = cell_geometry(cell_height, self.swatch_height);
        let typography = self.typography;

        let mut wrapped = row![].spacing(self.spacing_x);

        for sw in self.swatches {
            // The fill may be solid or gradient; paint whichever the
            // model carries. `background()` maps `Solid`/`Gradient` onto the
            // matching iced `Background` variant.
            let background = sw.swatch.background();
            let default_border = match sw.swatch {
                SwatchFill::Solid(color) => crate::surface_edge::edge_color(color, 0.12),
                SwatchFill::Gradient(_) => Color::TRANSPARENT,
            };
            let border_color = border_pin.unwrap_or(default_border);
            let swatch_rect = container(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(swatch_height)),
            )
            .width(Length::Fixed(cell_width))
            .height(Length::Fixed(swatch_height))
            .style(move |t: &iced::Theme| iced::widget::container::Style {
                background: Some(background),
                border: Border {
                    color: border_color,
                    width: if border_color.a == 0.0 { 0.0 } else { 1.0 },
                    // Resolve at draw time so runtime radius changes reach every swatch.
                    radius: crate::theme_lookup::radius(t).md.into(),
                },
                ..Default::default()
            });

            // Ellipsis needs a fixed width; a shrink-wrapped caption always fits its own line.
            let mut caption = enz_text(sw.label)
                .size(TextSize::Caption)
                .weight_rung(enzui_theme::WeightRung::Medium)
                .align(TextAlign::Left)
                .single_line_elided()
                .width(Length::Fixed(cell_width))
                .color_resolved(move |t: &iced::Theme| {
                    label_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text)
                });
            if let Some(scale) = typography {
                caption = caption.typography(scale);
            }
            // center_y(Fill) would replace the fixed label-band height; align_y preserves it.
            let label = container(caption)
                .width(Length::Fixed(cell_width))
                .height(Length::Fixed(label_box_height))
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Center);

            let cell: Element<'a, Message> = column![swatch_rect, label].spacing(LABEL_GAP).into();

            // Publish the colour value to AT. The caption already reaches
            // AT through the child `enz_text`'s `Operation::text`; the datum
            // that is otherwise sighted-only is the colour, so it rides on this
            // node's *description*. Authored description wins; when absent, a
            // deterministic `#RRGGBB` / `linear-gradient(...)` is derived. An
            // authored empty string is honoured as "no description" (expressible
            // intent) rather than replaced by the derived hex.
            let description = match &sw.description {
                Some(authored) => authored.clone(),
                None => sw.swatch.derived_description(),
            };
            let mut semantics = Semantics::new(Role::Image);
            if !description.is_empty() {
                semantics = semantics.description(description);
            }
            // A `leaf` suppresses child semantics, but the caption prose must
            // reach AT through `Operation::text`, so use a group whose
            // description carries the colour and whose child publishes the
            // caption `Role::Label`.
            let cell = crate::semantics_container::SemanticsContainer::new(cell, semantics);
            wrapped = wrapped.push(Element::from(cell));
        }

        // Forward `spacing_y` to the wrapping layout's *vertical* gap.
        // Without this call `Wrapping::layout` does
        // `vertical_spacing.unwrap_or(spacing)` (in iced's `Row`), silently reusing
        // the horizontal `spacing_x` for every row gap — making `spacing_y` a
        // dead public setter that never reaches layout.
        let wrapping: Wrapping<'a, Message> = wrapped.wrap().vertical_spacing(self.spacing_y);
        container(wrapping).width(Length::Fill).into()
    }
}

impl<'a, Message: 'a> From<EnzColorSwatchGrid<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzColorSwatchGrid<'a, Message>) -> Self {
        w.into_element()
    }
}
