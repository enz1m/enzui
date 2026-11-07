//! Centred icons in sized slots for aligned icon columns.
//!
//! Defaults to an unscaled 16 px square and live muted ink. Width and height
//! overrides reserve a different box, subject to parent constraints. Explicit
//! colour takes precedence over a semantic tone for bundled icons; unpinned ink
//! follows the theme. SVG/raster handles supplied through `icon_source` retain
//! their original colors.

use iced::widget::container;
use iced::{Alignment, Color, Element, Length};

/// Builder for a centred icon inside a sized box.
#[must_use]
pub struct EnzIconSlot<'a> {
    source: crate::icon::IconSource,
    icon_size: f32,
    slot_width: Option<f32>,
    slot_height: Option<f32>,
    icon_color: Option<Color>,
    icon_tone: Option<crate::icon::IconTone>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Create a 16 px icon slot from a Lucide or semantic icon name.
pub fn enz_icon_slot<'a>(name: impl Into<String>) -> EnzIconSlot<'a> {
    EnzIconSlot {
        source: crate::icon::IconSource::Named(name.into()),
        icon_size: DEFAULT_ICON_SIZE,
        slot_width: None,
        slot_height: None,
        icon_color: None,
        icon_tone: None,
        _lifetime: std::marker::PhantomData,
    }
}

const DEFAULT_ICON_SIZE: f32 = 16.0;

impl<'a> EnzIconSlot<'a> {
    /// Use SVG or raster artwork without recoloring it; bundled names retain slot ink.
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.source = source.into();
        self
    }

    /// Set unscaled glyph size in logical pixels; defaults to 16.
    /// Non-finite or non-positive input resets the size to 16 px.
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = enzui_core::metrics::FinitePositive::new_or(s, DEFAULT_ICON_SIZE).get();
        self
    }
    /// Pin slot width in logical pixels; otherwise it follows the current glyph size.
    /// Non-finite or non-positive input clears any prior width override.
    pub fn slot_width(mut self, w: f32) -> Self {
        self.slot_width = enzui_core::metrics::FinitePositive::new(w)
            .map(enzui_core::metrics::FinitePositive::get);
        self
    }
    /// Pin slot height in logical pixels; otherwise it follows the current glyph size.
    /// Non-finite or non-positive input clears any prior height override.
    pub fn slot_height(mut self, h: f32) -> Self {
        self.slot_height = enzui_core::metrics::FinitePositive::new(h)
            .map(enzui_core::metrics::FinitePositive::get);
        self
    }
    /// Pin glyph ink, overriding the live semantic tone.
    pub fn icon_color(mut self, c: Color) -> Self {
        self.icon_color = Some(c);
        self
    }

    /// Set glyph ink from a live palette role.
    /// An explicit [`Self::icon_color`] pin takes precedence.
    pub fn icon_tone(mut self, tone: crate::icon::IconTone) -> Self {
        self.icon_tone = Some(tone);
        self
    }

    /// The resolved slot width in px (the override, else the icon size).
    pub fn slot_width_value(&self) -> f32 {
        self.slot_width.unwrap_or(self.icon_size)
    }
    /// The resolved slot height in px (the override, else the icon size).
    pub fn slot_height_value(&self) -> f32 {
        self.slot_height.unwrap_or(self.icon_size)
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element<Message: 'a>(self) -> Element<'a, Message> {
        let w = self.slot_width_value();
        let h = self.slot_height_value();
        let (icon_color, icon_tone) = (self.icon_color, self.icon_tone);
        let icon: Element<'a, Message> =
            self.source
                .into_slot_element(self.icon_size, |icon| match (icon_color, icon_tone) {
                    (Some(color), _) => icon.tint(color),
                    (None, Some(tone)) => icon.tint_theme_tone(tone),
                    (None, None) => icon,
                });

        container(icon)
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    }
}

impl<'a, Message: 'a> From<EnzIconSlot<'a>> for Element<'a, Message> {
    fn from(w: EnzIconSlot<'a>) -> Self {
        w.into_element()
    }
}
