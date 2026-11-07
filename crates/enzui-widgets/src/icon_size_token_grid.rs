//! EnzIconSizeTokenGrid: wrapping preview grid of icon-size tokens.
//!
//! Each cell shows a fixed slot with an icon at the token's size,
//! plus a `label` line and a `"{n}px"` value line below.
use iced::widget::row::Wrapping;
use iced::widget::{column, container, row, text};
use iced::{Border, Color, Element, Length};

/// Icon previewed at each size when the caller does not pick one.
const DEFAULT_ICON_NAME: &str = "action.settings";
/// Gap between the icon slot and the label line.
const SLOT_LABEL_GAP: f32 = 6.0;
/// Gap between the label line and the `NNpx` value line: a 14 px label line
/// at `slot-height + 6` leaves the value line at `slot-height + 24`.
const TEXT_LINE_GAP: f32 = 4.0;

/// The effective icon size actually rendered (and labelled) for a token.
///
/// # Validation
///
/// [`EnzIconSizeTokenView::size`] is a bare public `f32`, so a caller can set
/// it to `NaN`, `inf` or a negative number via a struct literal. This grid has
/// the same bounded-input contract as `radius_token_grid`:
///
/// * the drawn path was `tok.size.min(slot_height)`, and `(-5.0).min(28.0)`
///   is `-5.0`, so a negative size reached `enz_icon(..).size(-5.0)`; and
/// * the value line was `format!("{}px", tok.size as i32)`, and `as i32` on a
///   negative *masks* the failure rendering a self-consistent-looking
///   `-5 px` next to the broken glyph.
///
/// So the raw field is validated here, at the single choke point where it
/// flows into both the drawn glyph and the label, and the same sanitized value
/// feeds both so they cannot disagree. An invalid size degrades to `0.0` (an
/// empty slot -- a valid, expressible, self-consistent state), matching the
/// `FiniteNonNeg` floor `radius_token_grid` uses. `0.0` is honoured literally:
/// a zero-size token draws nothing and prints `0 px`, not a fallback.
fn effective_icon_size(size: f32) -> f32 {
    enzui_core::metrics::FiniteNonNeg::new_or(size, 0.0).get()
}

/// One token to preview: a caption and the icon size it renders at.
#[derive(Debug, Clone)]
pub struct EnzIconSizeTokenView {
    /// Caption shown under the slot (e.g. the token name like `"icon-sm"`).
    pub label: String,
    /// Icon size in px this slot renders at; also shown as the `NNpx` value line.
    pub size: f32,
}

/// Builder for a grid previewing an icon at each of several size tokens. See
/// [`enz_icon_size_token_grid`].
#[must_use]
pub struct EnzIconSizeTokenGrid<'a, Message> {
    tokens: Vec<EnzIconSizeTokenView>,
    icon_name: String,
    cell_width: f32,
    cell_height: f32,
    slot_width: f32,
    slot_height: f32,
    spacing_x: f32,
    spacing_y: f32,
    slot_background: Option<Color>,
    slot_border: Option<Color>,
    icon_color: Option<Color>,
    label_color: Option<Color>,
    value_color: Option<Color>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Construct the grid from a list of size tokens. Defaults: `"action.settings"`
/// icon, 64×86px cells, 64×48px slots, 16/8px spacing, theme-resolved colours.
pub fn enz_icon_size_token_grid<Message>(
    tokens: Vec<EnzIconSizeTokenView>,
) -> EnzIconSizeTokenGrid<'static, Message> {
    EnzIconSizeTokenGrid {
        tokens,
        icon_name: DEFAULT_ICON_NAME.to_owned(),
        cell_width: 64.0,
        cell_height: 86.0,
        slot_width: 64.0,
        slot_height: 48.0,
        spacing_x: 16.0,
        spacing_y: 8.0,
        slot_background: None,
        slot_border: None,
        icon_color: None,
        label_color: None,
        value_color: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzIconSizeTokenGrid<'a, Message> {
    /// Which icon is previewed at every size (the `icon-name`, default
    /// `"action.settings"`).
    pub fn icon_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !name.is_empty() {
            self.icon_name = name;
        }
        self
    }
    /// Width of each grid cell in px. Non-finite or non-positive input falls
    /// back to 64.
    pub fn cell_width(mut self, w: f32) -> Self {
        self.cell_width = enzui_core::metrics::FinitePositive::new_or(w, 64.0).get();
        self
    }
    /// Height of each grid cell in px (icon slot + two text lines). `0.0` is
    /// expressible intent (a zero-height cell) and is honoured literally;
    /// non-finite or negative input falls back to 86. If the cell height is
    /// smaller than the composed slot + text bands, the cell is clipped to this
    /// height rather than silently grown — the caller's explicit number wins.
    pub fn cell_height(mut self, h: f32) -> Self {
        self.cell_height = enzui_core::metrics::FiniteNonNeg::new_or(h, 86.0).get();
        self
    }
    /// Width of the icon slot inside a cell in px. Non-finite or non-positive
    /// input falls back to 64.
    pub fn slot_width(mut self, w: f32) -> Self {
        self.slot_width = enzui_core::metrics::FinitePositive::new_or(w, 64.0).get();
        self
    }
    /// Height of the icon slot inside a cell in px. Non-finite or non-positive
    /// input falls back to 48.
    pub fn slot_height(mut self, h: f32) -> Self {
        self.slot_height = enzui_core::metrics::FinitePositive::new_or(h, 48.0).get();
        self
    }
    /// Horizontal gap between cells in px. Non-finite or negative input falls
    /// back to 16.
    pub fn spacing_x(mut self, s: f32) -> Self {
        self.spacing_x = enzui_core::metrics::FiniteNonNeg::new_or(s, 16.0).get();
        self
    }
    /// Vertical gap between cell rows in px. Non-finite or negative input falls
    /// back to 8.
    pub fn spacing_y(mut self, s: f32) -> Self {
        self.spacing_y = enzui_core::metrics::FiniteNonNeg::new_or(s, 8.0).get();
        self
    }
    /// Override the icon-slot fill. Unset by default (resolved from the theme).
    pub fn slot_background(mut self, c: Color) -> Self {
        self.slot_background = Some(c);
        self
    }
    /// Override the icon-slot border colour. Unset by default (from the theme).
    pub fn slot_border(mut self, c: Color) -> Self {
        self.slot_border = Some(c);
        self
    }
    /// Override the previewed icon tint. Unset by default (from the theme).
    pub fn icon_color(mut self, c: Color) -> Self {
        self.icon_color = Some(c);
        self
    }
    /// Override the caption ink. Unset by default (resolved from the theme).
    pub fn label_color(mut self, c: Color) -> Self {
        self.label_color = Some(c);
        self
    }
    /// Override the `NNpx` value-line ink. Unset by default (from the theme).
    pub fn value_color(mut self, c: Color) -> Self {
        self.value_color = Some(c);
        self
    }

    /// The number of size tokens in the grid.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // explicit overrides win; unset colours resolve from the runtime
        // `iced::Theme` per render (was a frozen `Palette::dark()`).
        let slot_bg_pin = self.slot_background;
        let slot_border_pin = self.slot_border;
        let icon_pin = self.icon_color;
        let label_pin = self.label_color;
        let value_pin = self.value_color;
        let icon_name = self.icon_name;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let slot_width = self.slot_width.min(self.cell_width);
        let slot_height = self.slot_height;

        let mut wrapped = row![].spacing(self.spacing_x);

        for tok in self.tokens {
            let icon_size = effective_icon_size(tok.size).min(slot_height);
            // `.center_x(Length::Fill)` overrides `.width(Length::Fixed(..))`
            // and would grow the slot to the window; centre via Fixed sizes so
            // the slot stays 64×48.
            let glyph = {
                let mut i = crate::icon::enz_icon(&icon_name).size(icon_size);
                if let Some(c) = icon_pin {
                    i = i.tint(c);
                } else {
                    i = i.tint_theme_tone(crate::icon::IconTone::Text);
                }
                i.into_element::<Message>()
            };
            let slot = container(glyph)
                .width(Length::Fixed(slot_width))
                .height(Length::Fixed(slot_height))
                .center_x(Length::Fixed(slot_width))
                .center_y(Length::Fixed(slot_height))
                .style(move |t: &iced::Theme| {
                    let p = crate::theme_lookup::palette(t);
                    iced::widget::container::Style {
                        background: Some(slot_bg_pin.unwrap_or(p.surface_alt).into()),
                        border: Border {
                            color: slot_border_pin.unwrap_or(p.border),
                            width: 1.0,
                            // `radius-md`, read from the *runtime* theme's ladder:
                            // draw-time closure, so a custom theme's radius
                            // reaches the border instead of the stock scale.
                            radius: crate::theme_lookup::radius(t).md.into(),
                        },
                        ..Default::default()
                    }
                });

            let label_line = container(
                text(tok.label)
                    .size(crate::enz_text::TextSize::Caption2.px())
                    .font(enzui_theme::text_defaults::font(
                        enzui_theme::FontRole::Ui,
                        enzui_theme::WeightRung::Semibold,
                    ))
                    .style(move |t: &iced::Theme| iced::widget::text::Style {
                        color: Some(
                            label_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text),
                        ),
                    }),
            )
            .width(Length::Fixed(cell_width))
            .align_x(iced::alignment::Horizontal::Center);
            // The value line is monospace caption2 so the px digits align
            // column-to-column.
            let value_line = container(
                text(format!("{} px", effective_icon_size(tok.size) as i32))
                    .size(crate::enz_text::TextSize::Caption2.px())
                    // monospace_font() installs the upright-face fallback
                    // at first use, no app opt-in needed.
                    .font(crate::monospace_fallback::monospace_font())
                    .style(move |t: &iced::Theme| iced::widget::text::Style {
                        color: Some(
                            value_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text_muted),
                        ),
                    }),
            )
            .width(Length::Fixed(cell_width))
            .align_x(iced::alignment::Horizontal::Center);

            // stacks these at `y: slot-height + 6px` (label, 14 px tall)
            // and `y: slot-height + 24px` (value) — i.e. a 6 px gap under the
            // slot and a 4 px gap between the two text lines. A single
            // `.spacing(4)` column flattened both to 4.
            let cell_body = column![
                container(slot)
                    .width(Length::Fixed(cell_width))
                    .align_x(iced::alignment::Horizontal::Center),
                column![label_line, value_line].spacing(TEXT_LINE_GAP),
            ]
            .spacing(SLOT_LABEL_GAP);

            // `cell_height` governs the laid-out height of the whole grid
            // entry (icon slot + caption/value bands), the way `slot_height`
            // governs just the icon frame. Before this it was validated on the
            // way in and read nowhere, so `.cell_height(120.0)` changed
            // nothing (a dead-token failure). Pin the cell to a fixed
            // height via `Length::Fixed`; if that is shorter than the composed
            // content the cell is clipped to `cell_height` rather than grown
            // — the caller's explicit number wins (see doc on `cell_height`).
            let cell: Element<'a, Message> = container(cell_body)
                .width(Length::Fixed(cell_width))
                .height(Length::Fixed(cell_height))
                .clip(true)
                .into();

            wrapped = wrapped.push(cell);
        }

        // `spacing_x` sets the horizontal gap on the row; `spacing_y`
        // must be forwarded to the wrapped-line gap via `vertical_spacing`.
        // Before this, `Wrapping` fell back to the row's `spacing` for the
        // vertical gap too, so every `spacing_y` override was discarded.
        let wrapping: Wrapping<'a, Message> = wrapped.wrap().vertical_spacing(self.spacing_y);
        container(wrapping).width(Length::Fill).into()
    }
}

impl<'a, Message: 'a> From<EnzIconSizeTokenGrid<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzIconSizeTokenGrid<'a, Message>) -> Self {
        w.into_element()
    }
}
