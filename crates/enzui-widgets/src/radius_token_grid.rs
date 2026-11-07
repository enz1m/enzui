//! EnzRadiusTokenGrid: wrapping preview grid of border-radius tokens.
//!
//! Wrapping preview grid of border-radius tokens. Each cell renders a
//! small rounded rectangle at the token's radius, labelled with the pixel
//! value inside and the token name below.
//!
//! # Deferred
//!
//! - Preview background gradient — token showcase intentionally
//!   uses a flat `accent_soft` background so the corner curvature
//!   reads cleanly.

use crate::enz_text::{TextAlign, TextSize, enz_text};
use iced::widget::row::Wrapping;
use iced::widget::{column, container, row};
use iced::{Border, Color, Element, Length};

/// Gap between the radius preview and its caption.
const LABEL_GAP: f32 = 10.0;

/// Effective corner radius actually rendered for a token: a radius larger than
/// half the preview edge becomes a full circle, so it is clamped to
/// `preview_size / 2` . Exposed as a pure function so
/// the clamp is *observable* in tests rather than only happening inline.
///
/// # Validation
///
/// [`EnzRadiusTokenView::radius`] is a bare public `f32`, so a caller can set
/// it to `NaN`, `inf` or a negative number. This is the single choke point
/// where a token's radius flows into `Border.radius`, so validation happens
/// here: an invalid radius degrades to `0.0` (a square corner — a valid,
/// expressible radius) before the `.min(preview_size / 2.0)` clamp.
///
/// Order matters. `(-5.0).min(28.0) == -5.0`, so validating *after* the clamp
/// would let a negative radius through untouched — the label
/// `format!("{}px", radius as i32)` masks it, so it renders a `-5px` corner
/// while displaying `-5px` and looking self-consistent. `preview_size` is
/// already `FinitePositive`-validated by its setter, so `preview_size / 2.0`
/// is always a finite positive bound and cannot re-introduce a bad value.
/// `0.0` is honoured (a real "none" radius token exists), not floored away.
fn effective_radius(radius: f32, preview_size: f32) -> f32 {
    let radius = enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get();
    radius.min(preview_size / 2.0)
}

/// The vertical geometry of one cell: `(effective_preview, label_box_height)`.
///
/// A cell stacks `preview | LABEL_GAP | label` and must total exactly
/// `cell_height` so the fixed grid wraps and aligns. This is
/// the single source of truth for that split, pure so the fit invariant
/// `effective_preview + LABEL_GAP + label_box_height == cell_height` (when the
/// caller's sizes fit) is directly testable.
///
/// When `preview_size` is too tall to leave room for the gap and a 1px label,
/// the preview is clamped (the grid's fixed-cell contract wins over the
/// caller's preview size), and the label keeps its 1px floor. Both inputs are
/// already `FinitePositive`-validated by their setters, so this only fits two
/// valid numbers; it introduces no order dependence.
fn cell_geometry(cell_height: f32, preview_size: f32) -> (f32, f32) {
    // Reserve the gap and a 1px minimum label band; clamp the preview to fit.
    let preview_room = (cell_height - LABEL_GAP - 1.0).max(0.0);
    let effective_preview = preview_size.min(preview_room);
    let label_box_height = (cell_height - effective_preview - LABEL_GAP).max(1.0);
    (effective_preview, label_box_height)
}

/// One radius token to preview: a caption and the corner radius it renders at.
#[derive(Debug, Clone)]
pub struct EnzRadiusTokenView {
    /// Caption shown under the preview (e.g. the token name like `"radius-md"`).
    pub label: String,
    /// Corner radius in px this preview swatch is rounded to.
    pub radius: f32,
}

/// Builder for a grid previewing a set of radius tokens. See
/// [`enz_radius_token_grid`].
#[must_use]
pub struct EnzRadiusTokenGrid<'a, Message> {
    tokens: Vec<EnzRadiusTokenView>,
    cell_width: f32,
    cell_height: f32,
    preview_size: f32,
    spacing_x: f32,
    spacing_y: f32,
    preview_background: Option<Color>,
    preview_border: Option<Color>,
    preview_text: Option<Color>,
    label_color: Option<Color>,
    /// Active runtime typography scale, forwarded by the view owner holding the
    /// live `EnzTheme`. `None` = the local [`TextSize`] ramp. See
    /// [`EnzRadiusTokenGrid::typography`].
    typography: Option<enzui_theme::TypographyScale>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Construct the grid from a list of radius tokens. Defaults: 72×88px cells,
/// 56px preview swatch, 16/8px spacing, theme-resolved colours.
pub fn enz_radius_token_grid<Message>(
    tokens: Vec<EnzRadiusTokenView>,
) -> EnzRadiusTokenGrid<'static, Message> {
    EnzRadiusTokenGrid {
        tokens,
        cell_width: 72.0,
        cell_height: 88.0,
        preview_size: 56.0,
        spacing_x: 16.0,
        spacing_y: 8.0,
        preview_background: None,
        preview_border: None,
        preview_text: None,
        label_color: None,
        typography: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzRadiusTokenGrid<'a, Message> {
    /// Width of each grid cell in px. Non-finite or non-positive input falls
    /// back to 72.
    pub fn cell_width(mut self, w: f32) -> Self {
        self.cell_width = enzui_core::metrics::FinitePositive::new_or(w, 72.0).get();
        self
    }
    /// Height of each grid cell in px (preview + caption). Non-finite or
    /// non-positive input falls back to 88.
    pub fn cell_height(mut self, h: f32) -> Self {
        self.cell_height = enzui_core::metrics::FinitePositive::new_or(h, 88.0).get();
        self
    }
    /// Edge length of the rounded preview swatch in px. Non-finite or
    /// non-positive input falls back to 56.
    pub fn preview_size(mut self, s: f32) -> Self {
        self.preview_size = enzui_core::metrics::FinitePositive::new_or(s, 56.0).get();
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
    /// Override the preview-swatch fill. Unset by default (resolved from the theme).
    pub fn preview_background(mut self, c: Color) -> Self {
        self.preview_background = Some(c);
        self
    }
    /// Override the preview-swatch border colour. Unset by default (from the theme).
    pub fn preview_border(mut self, c: Color) -> Self {
        self.preview_border = Some(c);
        self
    }
    /// Override the `NNpx` value-line ink inside the preview. Unset by default
    /// (theme ink resolved against the preview's actual background).
    pub fn preview_text(mut self, c: Color) -> Self {
        self.preview_text = Some(c);
        self
    }
    /// Override the caption ink. Unset by default (resolved from the theme).
    pub fn label_color(mut self, c: Color) -> Self {
        self.label_color = Some(c);
        self
    }

    /// Forward the theme's runtime [`TypographyScale`](enzui_theme::TypographyScale) so both captions resize
    /// (and their line boxes reflow) under a custom scale.
    ///
    /// This is the production seam that makes a theme's typography scale reach
    /// the caption text. iced computes layout before the runtime theme exists
    /// and `Widget::layout` never receives a theme, so a leaf caption cannot
    /// follow a typography token from inside its own layout pass — the view
    /// owner holding the live `EnzTheme` calls `.typography(theme.typography)`,
    /// mirroring [`crate::text_pair::EnzTextPair::typography`] and
    /// [`crate::enz_text::EnzText::typography`]. Unset = the local
    /// [`TextSize`] ramp.
    pub fn typography(mut self, scale: enzui_theme::TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// The number of radius tokens in the grid.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // Explicit overrides win; unset colours resolve from the runtime
        // `iced::Theme` per render, never from a frozen `Palette::dark()`.
        let bg_pin = self.preview_background;
        let border_pin = self.preview_border;
        let text_pin = self.preview_text;
        let label_pin = self.label_color;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let typography = self.typography;

        // The cell stacks `preview | gap | label` and must total
        // `cell_height`. When the caller's `preview_size` is so large that
        // `preview_size + LABEL_GAP >= cell_height`, pinning the label band to
        // its `1.0` floor would grow the column to
        // `preview_size + LABEL_GAP + 1.0`, overflowing the fixed cell and
        // misaligning every wrapped row below it.
        //
        // Decision: clamp the preview to fit the cell, rather than letting
        // the cell grow. A radius-token grid's contract is a *fixed grid* — the
        // wrap and the alignment of later rows depend on every cell being
        // exactly `cell_height` — so the dependent field to give way is the
        // preview, not the cell. Reserve the gap plus a 1px minimum label band
        // and clamp the preview to what remains (never below 0). When the
        // caller's sizes fit (the common case) this is a no-op: `min` returns
        // the original `preview_size` unchanged.
        //
        // The clamp is `preview_size.min(room)`, not a sanitize of one field
        // against another's raw value: both `cell_height` and `preview_size`
        // are already each `FinitePositive`-validated by their own setters, so
        // this is a pure geometric fit of two *valid* numbers, and it falls
        // back to nothing order-dependent. `cell_geometry` owns the split.
        let (preview_size, label_box_height) = cell_geometry(cell_height, self.preview_size);

        let mut wrapped = row![].spacing(self.spacing_x);

        for tok in self.tokens {
            let r = effective_radius(tok.radius, preview_size);
            // Do not write
            // .width(Fixed(preview_size)).center_x(Length::Fill) — the
            // Length::Fill on center_x() overrides the fixed width in
            // iced 0.15, so the preview grows to fill the window and the tiny
            // radius (4/8/12 px) becomes invisible relative to the huge
            // box. Center the inner text at the preview's Fixed size.
            // Compose the semantic typography primitive rather than raw
            // `text`, so the caption resizes/reflows under a runtime
            // `TypographyScale` and stays single-line + end-elided within the
            // fixed preview band (raw `text` defaults permit word-wrap with no
            // ellipsis, overflowing the band on a long value). `.width(Fixed)`
            // gives the `Wrapping::None` run a bound to elide against — a
            // shrink-wrapped run always fits its own line, so the width is what
            // makes the ellipsis engage (enz_text.rs `single_line_elided`).
            // Label the *validated* token radius, not the raw field. The
            // label intentionally shows the declared value (e.g. `999px` for a
            // "full" token even though the swatch clamps to `preview/2`), so a
            // finite non-negative radius passes through unchanged. But an
            // invalid one (`NaN` / negative) must not print `-5px` next to a
            // square corner: `radius as i32` on a negative masks the defect by
            // rendering a self-consistent-looking `-5px`. Floor to the same
            // `0.0` the render path uses so caption and swatch agree.
            let labelled_radius = enzui_core::metrics::FiniteNonNeg::new_or(tok.radius, 0.0).get();
            let mut value = enz_text(format!("{}px", labelled_radius as i32))
                // Provenance: `size: "caption2"` (10) + `weight: "semibold"`.
                .size(TextSize::Caption2)
                .weight_rung(enzui_theme::WeightRung::Semibold)
                .align(TextAlign::Center)
                .single_line_elided()
                .width(Length::Fixed(preview_size))
                .color_resolved(move |t: &iced::Theme| {
                    text_pin.unwrap_or_else(|| {
                        let p = crate::theme_lookup::palette(t);
                        enzui_theme::Palette::brand_text_on(
                            bg_pin.unwrap_or(p.accent_soft),
                            p.text_muted,
                        )
                    })
                });
            if let Some(scale) = typography {
                value = value.typography(scale);
            }
            let preview = container(value)
                .width(Length::Fixed(preview_size))
                .height(Length::Fixed(preview_size))
                .center_x(Length::Fixed(preview_size))
                .center_y(Length::Fixed(preview_size))
                .style(move |t: &iced::Theme| {
                    let p = crate::theme_lookup::palette(t);
                    iced::widget::container::Style {
                        background: Some(bg_pin.unwrap_or(p.accent_soft).into()),
                        border: Border {
                            color: border_pin.unwrap_or(p.accent),
                            width: 1.0,
                            radius: r.into(),
                        },
                        ..Default::default()
                    }
                });

            // The caption sits at `y: preview-size + LABEL_GAP` and fills the
            // rest of the cell. `label_box_height`
            // is computed once by `cell_geometry` above so the preview clamp and
            // the label band cannot drift apart.
            // Same semantic transport for the token-name caption. The
            // width bound is the whole cell so a long/localized public token
            // name elides at the cell edge instead of wrapping and overflowing
            // the fixed `label_box_height` band.
            let mut name = enz_text(tok.label)
                // Provenance: `size: "caption"` (11) + `weight: "medium"`.
                .size(TextSize::Caption)
                .weight_rung(enzui_theme::WeightRung::Medium)
                .align(TextAlign::Center)
                .single_line_elided()
                .width(Length::Fixed(cell_width))
                .color_resolved(move |t: &iced::Theme| {
                    label_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text_muted)
                });
            if let Some(scale) = typography {
                name = name.typography(scale);
            }
            let label = container(name)
                .width(Length::Fixed(cell_width))
                .height(Length::Fixed(label_box_height))
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center);

            let cell: Element<'a, Message> = column![
                container(preview)
                    .width(Length::Fixed(cell_width))
                    .align_x(iced::alignment::Horizontal::Center),
                label,
            ]
            .spacing(LABEL_GAP)
            .into();

            wrapped = wrapped.push(cell);
        }

        // `spacing_y` is the vertical gap between wrapped rows. Without
        // this call, `Wrapping::layout` does `vertical_spacing.unwrap_or(spacing)`
        // (iced/widget/src/row.rs), silently inheriting the
        // *horizontal* `spacing_x` — which makes `.spacing_y(..)` a dead
        // public API.
        let wrapping: Wrapping<'a, Message> = wrapped.wrap().vertical_spacing(self.spacing_y);
        // The first fixed-width cell cannot wrap
        // when the allocation is narrower than one cell. A renderer layer is
        // therefore required; merely handing the child a narrow viewport does
        // not clip every renderer primitive.
        crate::inert_clip::InertClip::paint_only(container(wrapping).width(Length::Fill)).into()
    }
}

impl<'a, Message: 'a> From<EnzRadiusTokenGrid<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzRadiusTokenGrid<'a, Message>) -> Self {
        w.into_element()
    }
}
