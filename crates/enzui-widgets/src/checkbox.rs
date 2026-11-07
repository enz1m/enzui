//! EnzCheckbox: a styled checkbox component for enzUI-ICED.
//!
//! Rounded-square checkboxes with three sizes, optional descriptions,
//! theme-resolved checked fills, a mixed state, and a token override for the
//! corner radius.
//!
//! A checkbox is unchecked, checked, or mixed ([`EnzCheckbox::mixed`]): the
//! mixed state stands for a group whose members disagree, such as a "Select
//! all" box over a partly selected list. It paints a dash on the checked fill
//! and publishes `aria-checked=mixed`. Toggle order: activating a mixed box
//! asks for checked, then checked asks for unchecked and unchecked for
//! checked. The box never returns to mixed by itself; the application shows
//! it again when its members disagree again.

use crate::color::{darken, with_opacity};
use crate::enz_text::enz_text;
use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use crate::hover_control::{ControlPaint, HoverControl};
use enzui_core::semantics::{Role, Semantics, Toggle};
use enzui_theme::{Palette, TypographyScale};
use iced::widget::column;
use iced::{Alignment, Border, Color, Element};
use std::borrow::Cow;

// ---------------------------------------------------------------------------
// Size enum
// ---------------------------------------------------------------------------

/// Checkbox size presets matching the enzUI 3-step scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckboxSize {
    /// 14px box, 12px label.
    Sm,
    /// 16px box, 13px label (default).
    #[default]
    Md,
    /// 20px box, 15px label.
    Lg,
}

impl CheckboxSize {
    /// Returns the box side length for iced's `checkbox.size()`.
    pub fn box_size(self) -> f32 {
        match self {
            Self::Sm => 14.0,
            Self::Md => 16.0,
            Self::Lg => 20.0,
        }
    }

    /// Returns the box corner radius when the theme does not set one: a
    /// rounded square (a quarter of the side). The theme's `shape.checkbox`
    /// step overrides it (round by default); the check mark tells a round
    /// checkbox from a radio button's dot.
    pub fn radius(self) -> f32 {
        self.box_size() / 4.0
    }

    /// Returns the label text size.
    ///
    /// `Md` is derived from [`TypographyScale::body_size`] (13 px) — the
    /// default rung a checkbox label sits on. `Sm` (12) and `Lg` (15) are left
    /// as literals on purpose: 12 has no *text* rung in
    /// [`TypographyScale`] (`monospace_size` is 12 but is a monospace metric,
    /// not a UI-label one) and 15 is `section_size`, a heading rung that a
    /// checkbox label is not. Mapping either would be a wrong token, which is
    /// worse than a literal.
    pub fn label_size(self) -> f32 {
        match self {
            Self::Sm => 12.0,
            Self::Md => TypographyScale::ambient().body_size,
            Self::Lg => 15.0,
        }
    }

    /// Returns the description (subtitle) text size, derived from named
    /// [`TypographyScale`] rungs rather than bare literals.
    ///
    /// The three subtitle sizes each duplicate an existing token exactly, so
    /// they track the ramp instead of drifting the moment a token moves:
    ///
    /// | size | px | token |
    /// |------|----|-------|
    /// | `Sm` | 10 | [`TypographyScale::caption_size`] |
    /// | `Md` | 11 | [`TypographyScale::label_size`] |
    /// | `Lg` | 13 | [`TypographyScale::body_size`] |
    ///
    /// None is skipped — every rung has a clean semantic match, and the
    /// rendered sizes are 10/11/13 either way. This deliberately keeps the
    /// three-step subtitle scale rather than collapsing it to a single caption
    /// token.
    ///
    /// Equal-value trap: 10/11/13 each collide with *other* tokens, so
    /// the tests pin these against the named fields, never the numbers.
    ///
    /// [`TypographyScale`]: enzui_theme::TypographyScale
    /// [`TypographyScale::caption_size`]: enzui_theme::TypographyScale::caption_size
    /// [`TypographyScale::label_size`]: enzui_theme::TypographyScale::label_size
    /// [`TypographyScale::body_size`]: enzui_theme::TypographyScale::body_size
    pub fn description_size(self) -> f32 {
        let ty = enzui_theme::TypographyScale::ambient();
        match self {
            Self::Sm => ty.caption_size,
            Self::Md => ty.label_size,
            Self::Lg => ty.body_size,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Checkbox colours and shape, derived from the palette.
#[derive(Debug, Clone, Copy)]
pub struct CheckboxTokens {
    /// Corner radius in logical pixels; `None` follows the size's rounded-square
    /// default. Zero makes square corners; half the box side makes a circle.
    /// Invalid values use the default.
    pub radius: Option<f32>,
    /// Box border width in logical pixels; `None` follows the live hairline stroke.
    /// Invalid values use the live hairline. Zero hides the border.
    pub border_width: Option<f32>,
    /// Keyboard focus-ring width in logical pixels; defaults to `2.0`.
    /// Invalid values use the default. Zero hides the ring.
    pub focus_ring_width: f32,
    /// Disabled opacity in `[0, 1]`; defaults to `0.62` and multiplies colour alpha.
    /// Out-of-range values are clamped; non-finite values use the default.
    pub disabled_alpha: f32,
    /// Unchecked box border color.
    pub border: Color,
    /// Unchecked box background color.
    pub background: Color,
    /// Hover border color (unchecked).
    pub hover_border: Color,
    /// Checked fill + border color (accent).
    pub accent: Color,
    /// Checkmark and mixed-dash icon color.
    pub check_color: Color,
    /// Label text color.
    pub text_color: Color,
    /// Description/subtitle text color.
    pub description_color: Color,
    /// Focus-ring colour (keyboard focus-visible).
    pub focus_ring: Color,
}

impl CheckboxTokens {
    /// Derive checkbox tokens from a theme palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            radius: None,
            border_width: None,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            disabled_alpha: 0.62,
            // An unchecked box's border is the only indicator
            // the control is present (2.29 dark / 1.78 light on `surface`).
            border: p.control_border_strong,
            background: empty_control_fill(p),
            hover_border: p.accent,
            accent: p.filled_color(p.accent).color,
            check_color: p.filled_color(p.accent).text,
            text_color: p.text,
            description_color: p.text_muted,
            focus_ring: p.focus_ring,
        }
    }

    /// Convenience: tokens for dark theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: tokens for light theme.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for an EnzCheckbox element.
#[must_use]
pub struct EnzCheckbox<'a, Message> {
    is_checked: bool,
    mixed: bool,
    label: Cow<'a, str>,
    on_toggle: Box<dyn Fn(bool) -> Message + 'a>,
    size: CheckboxSize,
    description: Cow<'a, str>,
    description_spacing: f32,
    enabled: bool,
    tokens: Option<CheckboxTokens>,
    id: Option<iced::advanced::widget::Id>,
    aria_label: Option<String>,
}

/// Create an EnzCheckbox element.
///
/// # Arguments
///
/// * `is_checked` — whether the checkbox is currently checked.
/// * `label` — primary text label.
/// * `on_toggle` — callback producing a message when toggled.
pub fn enz_checkbox<'a, Message: 'a>(
    is_checked: bool,
    label: impl Into<Cow<'a, str>>,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> EnzCheckbox<'a, Message> {
    EnzCheckbox {
        is_checked,
        mixed: false,
        label: label.into(),
        on_toggle: Box::new(on_toggle),
        size: CheckboxSize::default(),
        description: Cow::Borrowed(""),
        description_spacing: DESCRIPTION_SPACING,
        enabled: true,
        tokens: None,
        id: None,
        aria_label: None,
    }
}

impl<'a, Message: 'a> EnzCheckbox<'a, Message> {
    /// Name the checkbox for assistive technology without showing a label,
    /// for a box whose subject is drawn beside it by someone else (a task
    /// list item's text, a table row). It wins over the label and the
    /// description as the accessible name.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Show the mixed state: a dash on the checked fill, published as
    /// `aria-checked=mixed`, for a box whose group is partly checked. It wins
    /// over `is_checked`; activating it asks `on_toggle` for `true`.
    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }

    /// Set the size variant.
    pub fn size(mut self, size: CheckboxSize) -> Self {
        self.size = size;
        self
    }

    /// Set the description (subtitle) text.
    pub fn description(mut self, desc: impl Into<Cow<'a, str>>) -> Self {
        self.description = desc.into();
        self
    }

    /// Space between the label and the description, in logical pixels;
    /// default 0, the two lines set close as one block.
    pub fn description_spacing(mut self, spacing: f32) -> Self {
        self.description_spacing = enzui_core::metrics::FiniteNonNeg::new_or(spacing, 0.0).get();
        self
    }

    /// Set enabled/disabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Pin colours and paint dimensions to a per-instance snapshot.
    /// `border_width: None` still follows the live hairline stroke.
    pub fn tokens(mut self, tokens: CheckboxTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Assign a stable widget `Id` so the
    /// checkbox can be targeted by programmatic focus
    /// (`iced::advanced::widget::operation::focusable::focus(id)`) and given a
    /// stable semantic identity in the AT tree. Forwarded to the
    /// composed [`crate::focus_button::FocusButton`], which owns the focus
    /// state and the `focusable` registration.
    ///
    /// This is a *setter only*: enzUI never auto-derives an index-based id,
    /// because an id that shifts when the surrounding list is reordered is
    /// worse than no id at all — the caller supplies a stable one.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// The accessible name published on the `Role::CheckBox` node: the primary
    /// label, or the description subtitle when the label is empty, or `None`
    /// when both are empty and no `aria_label` is set.
    fn a11y_name(&self) -> Option<&str> {
        if let Some(name) = self.aria_label.as_deref().filter(|name| !name.is_empty()) {
            Some(name)
        } else if !self.label.is_empty() {
            Some(self.label.as_ref())
        } else if !self.description.is_empty() {
            Some(self.description.as_ref())
        } else {
            None
        }
    }

    /// Build the element. If `.tokens()` was not called, chrome tokens
    /// are derived from the runtime `iced::Theme`.
    ///
    /// Composition: the box, label, and description are all children of
    /// a single [`crate::focus_button::FocusButton`] activation target, so the
    /// whole control is Tab-reachable, activates on Enter/Space and on a click
    /// anywhere (including the subtitle), and publishes CheckBox semantics.
    /// The subtitle sits beside the label in a column, not beneath the box.
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: Clone,
    {
        let sz = self.size;
        let pinned_tokens = self.tokens;
        let enabled = self.enabled;
        let mixed = self.mixed;
        let is_checked = self.is_checked && !mixed;
        let fallback_radius = sz.radius();
        let box_size = sz.box_size();
        // The theme's checkbox step, at most half the box (round).
        let radius = move |theme: &iced::Theme| {
            let themed = crate::theme_lookup::shape_radius(theme, |shape| shape.checkbox);
            enzui_core::metrics::FiniteNonNeg::new_or(themed, fallback_radius)
                .get()
                .min(box_size / 2.0)
        };

        // Resolve tokens (pin > runtime dark/light) inside style closures.
        let resolve = move |theme: &iced::Theme| -> CheckboxTokens {
            pinned_tokens.unwrap_or_else(|| {
                crate::theme_lookup::resolve_live(theme, CheckboxTokens::from_palette)
            })
        };

        // Built unchecked too, so the tick (a dash when mixed) can shrink away as it
        // unchecks.
        let mark = if mixed {
            crate::check_mark::Mark::Dash
        } else {
            crate::check_mark::Mark::Tick
        };
        let check_glyph: Element<'a, Message> =
            crate::check_mark::CheckMark::new(mark, box_size, move |theme: &iced::Theme| {
                let t = resolve(theme);
                if enabled {
                    t.check_color
                } else {
                    with_opacity(
                        t.check_color,
                        crate::color::disabled_alpha(t.disabled_alpha),
                    )
                }
            })
            .into();

        // Label + description column.
        // With a description, both lines set a little tighter than body
        // text, so title and description read as one block.
        let line_height = if self.description.is_empty() {
            None
        } else {
            Some(iced::widget::text::LineHeight::Relative(
                DESCRIBED_LINE_HEIGHT,
            ))
        };
        let mut text_col = column![]
            .spacing(self.description_spacing)
            .align_x(Alignment::Start);
        if !self.label.is_empty() {
            let label_size = sz.label_size();
            // `enz_text` rather than raw `text()`, so the run picks up the
            // typography scale and single-line + end-elision. The
            // `color_resolved` closure keeps the per-render token lookup,
            // including the 0.62 disabled alpha.
            text_col = text_col.push(
                tight(
                    enz_text(self.label.to_string()).size_px(label_size),
                    line_height,
                )
                .single_line_elided()
                .color_resolved(move |theme: &iced::Theme| {
                    let t = resolve(theme);
                    if enabled {
                        t.text_color
                    } else {
                        with_opacity(t.text_color, crate::color::disabled_alpha(t.disabled_alpha))
                    }
                })
                .into_element(),
            );
        }
        if !self.description.is_empty() {
            let desc_size = sz.description_size();
            text_col = text_col.push(
                tight(
                    enz_text(self.description.to_string()).size_px(desc_size),
                    line_height,
                )
                .single_line_elided()
                .color_resolved(move |theme: &iced::Theme| {
                    let t = resolve(theme);
                    if enabled {
                        t.description_color
                    } else {
                        with_opacity(
                            t.description_color,
                            crate::color::disabled_alpha(t.disabled_alpha),
                        )
                    }
                })
                .into_element(),
            );
        }

        // The whole control (box + label + description) is one custom widget so
        // the box border can react to hover over *any* of it: the touch area is
        // the entire control, not just the 14/16/20px box. A plain `container`
        // style closure only ever sees `&Theme` and never the hover state,
        // which is why `hover_border` cannot work there — the box never darkens
        // or highlights on hover. `HoverControl` computes hover from its own
        // (whole-row) bounds in `draw` and paints the box quad accordingly.
        let inner = HoverControl {
            glyph: check_glyph,
            text_col: text_col.into(),
            box_size,
            on: is_checked || mixed,
            paint: Box::new(move |theme: &iced::Theme, hovered: bool| {
                let t = resolve(theme);
                let (background, border_color) =
                    box_fill_border(&t, is_checked || mixed, hovered, enabled);
                ControlPaint {
                    background,
                    border_color,
                    border_width: t.border_width.map_or_else(
                        || hairline(theme),
                        |width| {
                            enzui_core::metrics::FiniteNonNeg::new_or(width, hairline(theme)).get()
                        },
                    ),
                    radius: {
                        let fallback = radius(theme);
                        t.radius.map_or(fallback, |value| {
                            enzui_core::metrics::FiniteNonNeg::new_or(value, fallback).get()
                        })
                    },
                    dot: None,
                }
            }),
        };

        // Accessible name: prefer the primary label; when it is empty (e.g. a
        // checkbox whose text is provided externally) fall back to the
        // description subtitle so the CheckBox node is never nameless. With
        // both empty the node is nameless unless `aria_label` names it.
        // Computed before `self.on_toggle` is moved into the lazy factory.
        let a11y_name = self.a11y_name().map(std::borrow::ToOwned::to_owned);
        let mut semantics = Semantics::new(Role::CheckBox)
            .label_opt(a11y_name)
            .toggled(if mixed {
                Toggle::Mixed
            } else if is_checked {
                Toggle::True
            } else {
                Toggle::False
            })
            .disabled(!enabled)
            .default_action();
        // Publish the visual subtitle as the node's *description* (help text),
        // distinct from the accessible name. A description that only ever names
        // a label-less checkbox and is otherwise dropped leaves a screen reader
        // never announcing the subtitle of a normal labelled checkbox. Only
        // publish it when a real label is also present — when the
        // label is empty the description has already been promoted to the name
        // above, and repeating it as the description would double-announce.
        if !self.description.is_empty() && !self.label.is_empty() {
            semantics = semantics.description(self.description.to_string());
        }

        // Lazy activation: the on_toggle factory runs once per real
        // activation, not at view-build time; a disabled checkbox constructs no
        // message and a rebuild never fires the callback. `is_checked` is Copy;
        // a mixed box is unchecked here, so it asks for checked.
        let on_toggle = self.on_toggle;
        let on_press: Option<Box<dyn Fn() -> Message + 'a>> = if enabled {
            Some(Box::new(move || (on_toggle)(!is_checked)))
        } else {
            None
        };

        let mut fb = focus_button_maybe_with(inner, on_press).semantics(semantics);
        // Forward a caller-supplied stable id so `focus(id)` reaches the
        // FocusButton that owns focus registration.
        if let Some(id) = self.id {
            fb = fb.id(id);
        }
        fb.style_focus(move |theme: &iced::Theme, status: FocusStatus| {
            let t = resolve(theme);
            let mut style = iced::widget::button::Style {
                background: None,
                text_color: t.text_color,
                border: Border::default(),
                ..Default::default()
            };
            if matches!(status, FocusStatus::Focused) && enabled {
                style.border = Border {
                    color: t.focus_ring,
                    width: enzui_core::metrics::FiniteNonNeg::new_or(t.focus_ring_width, 2.0).get(),
                    radius: ({
                        let fallback = radius(theme);
                        t.radius.map_or(fallback, |value| {
                            enzui_core::metrics::FiniteNonNeg::new_or(value, fallback).get()
                        })
                    } + enzui_core::metrics::FiniteNonNeg::new_or(
                        t.focus_ring_width,
                        2.0,
                    )
                    .get())
                    .into(),
                };
            }
            style
        })
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzCheckbox<'a, Message>> for Element<'a, Message> {
    fn from(checkbox: EnzCheckbox<'a, Message>) -> Self {
        checkbox.into_element()
    }
}

// ---------------------------------------------------------------------------
// Box chrome — painted by the shared `HoverControl` widget
// ---------------------------------------------------------------------------

/// `text` at `line_height` when one is given.
fn tight(
    text: crate::text::EnzText,
    line_height: Option<iced::widget::text::LineHeight>,
) -> crate::text::EnzText {
    match line_height {
        Some(line_height) => text.line_height(line_height),
        None => text,
    }
}

/// Space between a label and its description by default.
const DESCRIPTION_SPACING: f32 = 0.0;
/// The line height of a label and its description set together.
const DESCRIBED_LINE_HEIGHT: f32 = 1.2;

/// The fill of an empty checkbox or radio circle: white in light mode, as
/// macOS draws them, and a light wash of the text colour in dark mode, so an
/// empty control reads as a solid control on any surface, not a bare ring.
pub(crate) fn empty_control_fill(palette: &Palette) -> Color {
    if palette.is_dark() {
        Color {
            a: 0.12,
            ..palette.text
        }
    } else {
        palette.surface
    }
}

fn hairline(theme: &iced::Theme) -> f32 {
    enzui_theme::StrokeToken::Hairline.resolve(&crate::theme_lookup::stroke(theme))
}

pub(crate) fn box_fill_border(
    t: &CheckboxTokens,
    checked: bool,
    hovered: bool,
    enabled: bool,
) -> (Color, Color) {
    let a = if enabled {
        1.0
    } else {
        crate::color::disabled_alpha(t.disabled_alpha)
    };
    let (bg, border) = if checked {
        // Checked hover darkens the accent fill (and border) 8%.
        let fill = if hovered && enabled {
            darken(t.accent, 0.08)
        } else {
            t.accent
        };
        (fill, fill)
    } else {
        // Unchecked hover raises the border to `hover_border` (accent).
        let border = if hovered && enabled {
            t.hover_border
        } else {
            t.border
        };
        (t.background, border)
    };
    (with_opacity(bg, a), with_opacity(border, a))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

impl crate::theme_lookup::FollowsThemeOpacity for CheckboxTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled(),
            ..self
        }
    }
}
