//! Shared trigger chrome, popup placement, and elevation for date-valued fields.
//!
//! [`crate::date_picker`] and [`crate::date_range_picker`] supply selection and
//! dismissal policy. Custom date fields can reuse [`date_field`] to share their
//! sizing, theme resolution, and direction-aware popup alignment.

use enzui_core::calendar_semantics::CalendarLabels;
use enzui_core::placement::{Alignment, Direction, Placement, Side, resolve_direction};
use enzui_core::semantics::Semantics;
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{column, container, row, text};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::calendar::{CalendarTokens, EnzCalendar};
use crate::focus_button::{FocusStatus, focus_button_maybe};
use crate::tokens::FieldSizeTokens;

/// Gap between the trigger and popup in pixels.
pub const POPUP_GAP: f32 = 4.0;

/// Popup corner radius in pixels, shared with the calendar surface.
pub const POPUP_RADIUS: f32 = crate::calendar::SURFACE_RADIUS;

/// Trailing calendar glyph size in pixels.
pub const FIELD_ICON_SIZE: f32 = 14.0;

/// Registry name of the trailing calendar glyph.
pub const FIELD_ICON: &str = "calendar";

/// Places a popup below the trigger with a four-pixel gap and leading-edge alignment.
///
/// Right-to-left direction aligns the popup’s right edge; the placement resolver
/// may flip or shift it to fit the viewport.
#[must_use]
pub fn date_popup_placement(direction: Direction) -> Placement {
    Placement::new(Side::Bottom)
        .gap(POPUP_GAP)
        .alignment(Alignment::Start)
        .direction(direction)
}

/// Resolved colours for a date field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DateFieldTokens {
    /// Field fill.
    pub background: Color,
    /// Field border when idle.
    pub border: Color,
    /// Field border when hovered.
    pub border_hover: Color,
    /// Focus ring.
    pub focus_ring: Color,
    /// Ink for a chosen date.
    pub text: Color,
    /// Ink for the placeholder shown when unset.
    pub placeholder: Color,
    /// Label ink.
    pub label: Color,
    /// Trailing glyph ink.
    pub icon: Color,
    /// Popup surface fill.
    pub popup_background: Color,
    /// Popup surface border.
    pub popup_border: Color,
}

impl DateFieldTokens {
    /// Resolves field chrome and elevated popup colors from a palette.
    #[must_use]
    pub const fn from_palette(palette: &Palette) -> Self {
        Self {
            background: crate::tokens::field_fill(palette),
            border: palette.field_border,
            border_hover: palette.field_border_strong,
            focus_ring: palette.focus_ring,
            text: palette.text,
            placeholder: palette.text_muted,
            label: palette.text_muted,
            icon: palette.text_muted,
            popup_background: palette.surface_elevated,
            popup_border: crate::surface_edge::edge_color(
                palette.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
        }
    }

    /// Dark-theme tokens.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light-theme tokens.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Multiplies field ink and border alpha by the default disabled opacity.
///
/// Field and popup fills, popup border, and focus-ring color are preserved.
#[must_use]
pub fn dim(tokens: DateFieldTokens) -> DateFieldTokens {
    dim_by(tokens, enzui_theme::DEFAULT_DISABLED_OPACITY)
}

/// Multiplies field ink and border alpha by the supplied opacity without clamping it.
///
/// Field and popup fills, popup border, and focus-ring color are preserved.
#[must_use]
pub fn dim_by(mut tokens: DateFieldTokens, opacity: f32) -> DateFieldTokens {
    for ink in [
        &mut tokens.text,
        &mut tokens.placeholder,
        &mut tokens.label,
        &mut tokens.icon,
        &mut tokens.border,
        &mut tokens.border_hover,
    ] {
        ink.a *= opacity;
    }
    tokens
}

/// Shared runtime-theme resolver for date-field colors.
pub type FieldResolver = std::rc::Rc<dyn Fn(&iced::Theme) -> DateFieldTokens>;

/// Builds a runtime-theme resolver with filled field treatment.
///
/// Disabled fields use the theme’s disabled opacity for ink and borders.
#[must_use]
pub fn field_resolver(enabled: bool) -> FieldResolver {
    field_resolver_variant(enabled, crate::tokens::FieldVariant::Filled)
}

/// Builds a runtime-theme resolver with the supplied field fill treatment.
///
/// Disabled fields dim ink and borders while retaining field and popup fills.
#[must_use]
pub fn field_resolver_variant(
    enabled: bool,
    variant: crate::tokens::FieldVariant,
) -> FieldResolver {
    let dimmed = !enabled;
    std::rc::Rc::new(move |theme: &iced::Theme| -> DateFieldTokens {
        let p = crate::theme_lookup::palette(theme);
        let mut base = DateFieldTokens::from_palette(&p);
        base.background = crate::tokens::field_fill_for(&p, variant);
        if dimmed {
            dim_by(base, crate::theme_lookup::opacity(theme).disabled())
        } else {
            base
        }
    })
}

/// Display, sizing, activation, and accessibility inputs for a date-field trigger.
pub struct FieldTrigger<'a, Message> {
    /// The label drawn above the field, if any.
    pub label: Option<&'a str>,
    /// What the field shows. Already resolved — placeholder or value.
    pub display: String,
    /// Selects value ink rather than placeholder ink, including pending range anchors.
    pub shows_value: bool,
    /// The message the trigger emits when activated, or `None` if inert.
    pub on_press: Option<Message>,
    /// Withdraw the trigger from keyboard focus while its popup owns focus.
    /// Pointer activation remains live so clicking the trigger can still
    /// toggle an open popup.
    pub embedded: bool,
    /// What the field publishes to assistive technology.
    pub semantics: Semantics,
    /// Field dimensions; invalid values use medium defaults.
    pub metrics: FieldSizeTokens,
    /// Width offered consistently by the visual body, focus owner, and optional label column.
    pub width: Length,
    /// Writing-direction override for trigger contents and popup alignment.
    ///
    /// None follows the ambient direction; right-to-left places the glyph on the left.
    pub direction: Option<Direction>,
    /// Stable identity for routed accessibility actions and post-popup focus restoration.
    ///
    /// None leaves the trigger unaddressable by id.
    pub id: Option<iced::advanced::widget::Id>,
}

/// Builds date-field chrome with an optional anchored popup.
///
/// No popup overlay is published when `popup` is none; trigger activation and
/// focus participation are configured independently through [`FieldTrigger`].
pub fn date_field<'a, Message: Clone + 'a>(
    trigger: FieldTrigger<'_, Message>,
    popup: Option<Element<'a, Message>>,
    resolve: FieldResolver,
) -> Element<'a, Message> {
    let stock = FieldSizeTokens::MD;
    let metrics = FieldSizeTokens {
        height: enzui_core::metrics::FiniteNonNeg::new_or(trigger.metrics.height, stock.height)
            .get(),
        padding_x: enzui_core::metrics::FiniteNonNeg::new_or(
            trigger.metrics.padding_x,
            stock.padding_x,
        )
        .get(),
        text_size: enzui_core::metrics::FinitePositive::new_or(
            trigger.metrics.text_size,
            stock.text_size,
        )
        .get(),
        radius: enzui_core::metrics::FiniteNonNeg::new_or(trigger.metrics.radius, stock.radius)
            .get(),
    };
    let shows_value = trigger.shows_value;
    let trigger_id = trigger.id.clone();

    let label_row = trigger.label.map(|label| {
        let resolve = std::rc::Rc::clone(&resolve);
        text(label.to_owned())
            .size((metrics.text_size - 1.0).max(1.0))
            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(resolve(theme).label),
            })
    });

    let value_text = {
        let resolve = std::rc::Rc::clone(&resolve);
        text(trigger.display)
            .width(Length::Fill)
            .wrapping(iced::widget::text::Wrapping::None)
            .ellipsis(iced::widget::text::Ellipsis::End)
            .size(metrics.text_size)
            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(if shows_value {
                    resolve(theme).text
                } else {
                    resolve(theme).placeholder
                }),
            })
    };

    let glyph = {
        let resolve = std::rc::Rc::clone(&resolve);
        crate::icon::enz_icon(FIELD_ICON)
            .size(FIELD_ICON_SIZE)
            .tint_resolved(move |theme: &iced::Theme| resolve(theme).icon)
    };

    let rtl = matches!(resolve_direction(trigger.direction), Direction::RightToLeft,);
    let value_container = container(value_text)
        .width(Length::Fill)
        .center_y(metrics.height);
    let icon_container = container(glyph.into_element::<Message>()).center_y(metrics.height);
    let field_body = if rtl {
        row![icon_container, value_container]
    } else {
        row![value_container, icon_container]
    }
    .spacing(8.0)
    .align_y(iced::Alignment::Center);

    let button = focus_button_maybe(
        container(field_body)
            .width(trigger.width)
            .height(metrics.height)
            .padding(Padding::from([0.0, metrics.padding_x]))
            .clip(true),
        trigger.on_press,
    )
    // Forward Fill to the focus owner so parent rows allocate the same share as the body.
    .width(trigger.width);
    let button = match trigger_id {
        Some(id) => button.id(id),
        None => button,
    };
    let button = button
        .embedded(trigger.embedded)
        .semantics(trigger.semantics)
        .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
            let t = resolve(theme);
            ButtonStyle {
                background: Some(Background::Color(t.background)),
                text_color: t.text,
                border: Border {
                    color: match status {
                        FocusStatus::Focused => t.focus_ring,
                        FocusStatus::Hovered | FocusStatus::Pressed => t.border_hover,
                        _ => t.border,
                    },
                    width: if matches!(status, FocusStatus::Focused) {
                        crate::focus_button::dense_focus_ring_width()
                    } else {
                        1.0
                    },
                    radius: metrics.radius.into(),
                },
                ..ButtonStyle::default()
            }
        });

    let anchored = crate::anchored_overlay::anchored_overlay(button, popup)
        .placement(date_popup_placement(resolve_direction(trigger.direction)));

    match label_row {
        // A caption wrapper must also expose the field’s Fill factor to its parent.
        Some(label) => column![label, anchored]
            .spacing(4.0)
            .width(trigger.width)
            .into(),
        None => anchored.into(),
    }
}

/// Applies locale labels and a runtime token resolver with the field’s popup elevation.
///
/// Replaces any existing calendar token override. Day colors follow the ambient
/// palette; only surface fill and border come from the field resolver.
pub fn elevate_popup<'a, Message: Clone + 'a>(
    calendar: EnzCalendar<'a, Message>,
    resolve: FieldResolver,
    labels: &'a dyn CalendarLabels,
) -> EnzCalendar<'a, Message> {
    calendar
        .labels(labels)
        .tokens_resolved(move |theme: &iced::Theme| {
            let base = crate::theme_lookup::resolve(theme, CalendarTokens::from_palette);
            elevated_calendar_tokens(base, &resolve(theme))
        })
}

/// Replaces calendar surface fill and border while preserving all other token values.
#[must_use]
pub fn elevated_calendar_tokens(
    mut base: CalendarTokens,
    field: &DateFieldTokens,
) -> CalendarTokens {
    base.surface = field.popup_background;
    base.border = field.popup_border;
    base
}
