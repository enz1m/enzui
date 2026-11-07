//! Series labels with rounded colour swatches and optional row wrapping.
//!
//! Entries wrap onto additional rows by default, and labels wrap at words or
//! graphemes. Disabling wrapping permits long labels to overflow. Unpinned
//! label ink and transparent-entry fallback follow the live palette; geometry
//! uses the constants below. Labels use ambient Body typography.

use enzui_core::metrics::FinitePositive;
use enzui_theme::Palette;
use iced::border::Radius;
use iced::widget::{Row, container, row};
use iced::{Background, Border, Color, Element, Length};

use crate::enz_text::{TextSize, enz_text};

/// An owned series label paired with its swatch ink.
#[derive(Debug, Clone)]
pub struct ChartLegendEntry {
    /// Series text shown beside the swatch.
    pub label: String,
    /// Swatch ink; non-positive or NaN alpha uses the accent fallback.
    pub color: Color,
}

impl ChartLegendEntry {
    /// Own a series label and swatch colour.
    pub fn new(label: impl Into<String>, color: Color) -> Self {
        Self {
            label: label.into(),
            color,
        }
    }
}

/// Default swatch edge length in logical pixels: 12.
pub const SWATCH_SIZE: f32 = 12.0;
/// Default swatch corner radius in logical pixels: 3.
pub const SWATCH_RADIUS: f32 = 3.0;
/// Default gap between entries and wrapped rows in logical pixels: 8.
pub const ITEM_GAP: f32 = 8.0;
/// Default swatch-to-label gap in logical pixels: 4.
pub const LABEL_GAP: f32 = 4.0;

fn sane_dim(v: f32, default: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        default
    }
}

fn sane_size(v: f32, default: f32) -> f32 {
    FinitePositive::new_or(v, default).get()
}

/// Swatch geometry, label ink and accent fallback overrides.
#[derive(Debug, Clone, Copy)]
pub struct ChartLegendTokens {
    /// Swatch edge length in logical pixels; defaults to 12 and must be positive.
    pub swatch_size: f32,
    /// Swatch corner radius in logical pixels; defaults to 3.
    pub swatch_radius: f32,
    /// Gap between entries and wrapped rows in logical pixels; defaults to 8.
    pub item_gap: f32,
    /// Swatch-to-label gap in logical pixels; defaults to 4.
    pub label_gap: f32,
    /// Label ink; defaults to the palette primary text colour.
    pub label_color: Color,
    /// Fallback swatch ink when entry alpha is not positive; defaults to palette accent.
    pub accent: Color,
}

impl ChartLegendTokens {
    /// Resolve label and accent ink from this palette with fixed default geometry.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            swatch_size: SWATCH_SIZE,
            swatch_radius: SWATCH_RADIUS,
            item_gap: ITEM_GAP,
            label_gap: LABEL_GAP,
            label_color: p.text,
            accent: p.accent,
        }
    }

    /// Use the built-in dark palette and default geometry.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Use the built-in light palette and default geometry.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Keep entry ink when alpha is positive, including partial alpha; otherwise use accent.
    pub fn resolve_fill(&self, entry_color: Color) -> Color {
        if entry_color.a > 0.0 {
            entry_color
        } else {
            self.accent
        }
    }
}

/// Builder for series labels and swatches without event callbacks.
#[must_use]
pub struct EnzChartLegend {
    entries: Vec<ChartLegendEntry>,
    tokens: Option<ChartLegendTokens>,
    wrap: bool,
}

impl EnzChartLegend {
    /// Pin geometry, label ink and fallback accent instead of resolving live palette colours.
    /// Negative or non-finite dimensions fall back to their defaults. Zero is allowed
    /// except for swatch size, which falls back to 12 px unless finite and positive.
    pub fn tokens(mut self, tokens: ChartLegendTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Allow entries and labels to wrap within the available width; defaults to true.
    /// False keeps one row and permits long labels to overflow.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Build the legend using ambient Body typography and the configured wrapping policy.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let pinned = self.tokens;
        let geo = pinned.unwrap_or_else(ChartLegendTokens::dark);
        let swatch_size = sane_size(geo.swatch_size, SWATCH_SIZE);
        let swatch_radius = sane_dim(geo.swatch_radius, SWATCH_RADIUS);
        let item_gap = sane_dim(geo.item_gap, ITEM_GAP);
        let label_gap = sane_dim(geo.label_gap, LABEL_GAP);

        let mut items: Row<'a, Message> = row![].spacing(item_gap);
        for entry in self.entries {
            let entry_color = entry.color;
            let swatch = container(iced::widget::Space::new())
                .width(Length::Fixed(swatch_size))
                .height(Length::Fixed(swatch_size))
                .style(move |theme: &iced::Theme| {
                    let accent = match pinned {
                        Some(t) => t.accent,
                        None => crate::theme_lookup::palette(theme).accent,
                    };
                    let fill = if entry_color.a > 0.0 {
                        entry_color
                    } else {
                        accent
                    };
                    container::Style {
                        background: Some(Background::Color(fill)),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(swatch_radius),
                        },
                        ..Default::default()
                    }
                });

            let mut label = enz_text(entry.label).size(TextSize::Body);
            if self.wrap {
                label = label.wrapping(iced::widget::text::Wrapping::WordOrGlyph);
            }
            if let Some(t) = pinned {
                label = label.tokens(fixed_text_tokens(t.label_color));
            }

            let item = row![swatch, label]
                .spacing(label_gap)
                .align_y(iced::Alignment::Center);
            items = items.push(item);
        }

        let items = items.align_y(iced::Alignment::Center);
        if self.wrap {
            items.wrap().vertical_spacing(item_gap).into()
        } else {
            items.into()
        }
    }
}

fn fixed_text_tokens(color: Color) -> crate::enz_text::TextTokens {
    crate::enz_text::TextTokens {
        default_color: color,
        muted_color: color,
        subtle_color: color,
        accent_color: color,
        success_color: color,
        warning_color: color,
        danger_color: color,
        disabled_color: color,
    }
}

impl<'a, Message: 'a> From<EnzChartLegend> for Element<'a, Message> {
    fn from(l: EnzChartLegend) -> Self {
        l.into_element()
    }
}

/// Own the supplied entries with wrapping enabled, default geometry and live palette ink.
pub fn enz_chart_legend(entries: impl IntoIterator<Item = ChartLegendEntry>) -> EnzChartLegend {
    EnzChartLegend {
        entries: entries.into_iter().collect(),
        tokens: None,
        wrap: true,
    }
}
