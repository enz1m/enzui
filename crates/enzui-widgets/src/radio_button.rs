//! Standalone radio selection with checked-state semantics and live-theme paint.
//!
//! Each enabled option is its own Tab stop; Enter and Space select its value.
//! Sharing `selected` gives mutual exclusion, but adds no group semantics or
//! arrow navigation. Use [`crate::radio_group::EnzRadioGroup`] for related choices.
//! Custom composites own roving focus and keyboard handling and may withdraw
//! each option's focus registration with [`EnzRadioButton::embedded`].
//!
//! Defaults are enabled, medium-sized and unpinned. Colours follow the live
//! palette; [`RadioTokens`] pin colours, opacity and focus geometry. A stable
//! [`EnzRadioButton::id`] is required for targeted focus and routed activation;
//! no ID is derived from the label. Selection remains caller-owned, and the
//! callback runs lazily on activation.
//!
//! # Independent options

use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use crate::hover_control::{ControlPaint, HoverControl};
use crate::radio_group::{RadioGroupSize, RadioTokens, ring_border_width, ring_fill_border};
use enzui_core::semantics::{Role, Semantics, Toggle};
use iced::widget::{Space, text};
use iced::{Border, Color, Element, Length};

/// Build one independent radio; equality with `selected` determines checked state.
/// The callback receives this option's value on activation; the caller updates selection.
pub fn enz_radio_button<'a, Message, V>(
    label: impl Into<String>,
    value: V,
    selected: Option<V>,
    on_select: impl Fn(V) -> Message + 'a,
) -> EnzRadioButton<'a, Message, V>
where
    Message: Clone + 'a,
    V: Copy + Eq + 'a,
{
    EnzRadioButton {
        label: label.into(),
        value,
        selected,
        on_select: Box::new(on_select),
        size: RadioGroupSize::default(),
        enabled: true,
        tokens: None,
        id: None,
        embedded: false,
    }
}

/// Builder returned by [`enz_radio_button`].
#[must_use]
pub struct EnzRadioButton<'a, Message, V> {
    label: String,
    value: V,
    selected: Option<V>,
    on_select: Box<dyn Fn(V) -> Message + 'a>,
    size: RadioGroupSize,
    enabled: bool,
    tokens: Option<RadioTokens>,
    id: Option<iced::advanced::widget::Id>,
    embedded: bool,
}

impl<'a, Message, V> EnzRadioButton<'a, Message, V>
where
    Message: Clone + 'a,
    V: Copy + Eq + 'a,
{
    /// Choose ring, dot and label dimensions; defaults to [`RadioGroupSize::Md`].
    pub fn size(mut self, size: RadioGroupSize) -> Self {
        self.size = size;
        self
    }

    /// Allow activation and focus when true; defaults to true.
    /// False also announces disabled state and dims the control.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Pin colours, disabled opacity and focus geometry to a snapshot.
    /// `border_width: None` still follows the live hairline stroke.
    pub fn tokens(mut self, tokens: RadioTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set the stable identity for targeted focus and accessibility actions.
    /// Defaults to no ID; the label is not used to derive one.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Delegate focus and keyboard activation to a caller-owned composite.
    /// Defaults to false. True withdraws this option's Tab stop, Enter/Space handling
    /// and pointer self-focus; pointer activation, hover and semantics remain.
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    pub(crate) fn semantics(&self) -> Semantics {
        let is_selected = self.selected == Some(self.value);
        Semantics::new(Role::RadioButton)
            .label(self.label.clone())
            .toggled(if is_selected {
                Toggle::True
            } else {
                Toggle::False
            })
            .disabled(!self.enabled)
            .default_action()
    }
}

impl<'a, Message, V> EnzRadioButton<'a, Message, V>
where
    Message: Clone + 'a,
    V: Copy + Eq + 'a,
{
    /// Finish the builder with an explicit element type for macro call sites.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message, V> From<EnzRadioButton<'a, Message, V>> for Element<'a, Message>
where
    Message: Clone + 'a,
    V: Copy + Eq + 'a,
{
    fn from(b: EnzRadioButton<'a, Message, V>) -> Self {
        let semantics = b.semantics();
        let widget_id = b.id;
        let embedded = b.embedded;
        let pinned = b.tokens;
        let sz = b.size;
        let enabled = b.enabled;
        let on_select = b.on_select;
        let value = b.value;
        let is_selected = b.selected == Some(b.value);
        let label = b.label;
        let ring = sz.ring_size();
        let dot = sz.dot_size();

        let resolve = move |theme: &iced::Theme| -> RadioTokens {
            crate::theme_lookup::resolve_pinned_live(
                crate::theme_lookup::PIN_SITE_RADIO_BUTTON,
                pinned,
                theme,
                RadioTokens::from_palette,
            )
        };

        let label_size = sz.label_size();
        let label_text = text(label)
            .size(label_size)
            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some({
                    let t = resolve(theme);
                    if enabled {
                        t.text_color
                    } else {
                        dim(t.text_color, &t)
                    }
                }),
            });
        // Whole-row hover changes the ring, including when the pointer is over its label.
        let inner = HoverControl {
            glyph: Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into(),
            text_col: label_text.into(),
            box_size: ring,
            on: is_selected,
            paint: Box::new(move |theme: &iced::Theme, hovered: bool| {
                let t = resolve(theme);
                let (dot_color, border_color) = ring_fill_border(&t, is_selected, hovered, enabled);
                ControlPaint {
                    background: if is_selected {
                        border_color
                    } else {
                        crate::radio_group::ring_background(&t, enabled)
                    },
                    border_color,
                    border_width: ring_border_width(is_selected, &t, theme),
                    radius: ring / 2.0,
                    dot: if is_selected {
                        Some((dot, dot_color))
                    } else {
                        None
                    },
                }
            }),
        };
        let on_press: Option<Box<dyn Fn() -> Message + 'a>> = if enabled {
            Some(Box::new(move || (on_select)(value)))
        } else {
            None
        };

        let mut fb = focus_button_maybe_with(inner, on_press)
            .embedded(embedded)
            .semantics(semantics)
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                let t = resolve(theme);
                let mut style = iced::widget::button::Style {
                    background: None,
                    text_color: if enabled {
                        t.text_color
                    } else {
                        dim(t.text_color, &t)
                    },
                    border: Border::default(),
                    ..Default::default()
                };
                if matches!(status, FocusStatus::Focused) && enabled {
                    style.border = Border {
                        color: t.focus_ring,
                        width: enzui_core::metrics::FiniteNonNeg::new_or(t.focus_ring_width, 2.0)
                            .get(),
                        radius: enzui_core::metrics::FiniteNonNeg::new_or(t.focus_ring_radius, 4.0)
                            .get()
                            .into(),
                    };
                }
                style
            });
        if let Some(id) = widget_id {
            fb = fb.id(id);
        }
        fb.into()
    }
}

fn dim(c: Color, tokens: &RadioTokens) -> Color {
    crate::color::with_opacity(c, crate::color::disabled_alpha(tokens.disabled_alpha))
}
