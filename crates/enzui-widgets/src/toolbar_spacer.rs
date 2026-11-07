//! Fixed or flexible horizontal gaps with an optional centred toolbar divider.
//!
//! Flexible gaps share remaining row width with other Fill children. Fixed
//! gaps preserve zero; negative or non-finite widths fall back to 12 px.
//! Both variants fill the assigned height. Dividers default to 60% of that
//! height and live quiet ink from [`crate::toolbar_divider::default_divider_ink`].
//! Pinning tokens uses their colour instead; palette-derived tokens use border
//! ink. Zero divider thickness or height fraction suppresses the line while
//! preserving the gap allocation.

use crate::hairline::sanitize_thickness;
use crate::surface::LineSize;
use crate::toolbar_divider::vertical_rule;
use enzui_theme::Palette;
use iced::widget::{Space, container, rule::FillMode};
use iced::{Color, Element, Length};

/// Default fixed gap and invalid-width fallback in logical pixels: 12.
pub const DEFAULT_LENGTH: f32 = 12.0;
/// Default visible divider fraction of assigned height: 0.6.
pub const DIVIDER_HEIGHT_FRACTION: f32 = 0.6;
/// Choose a fixed gap or flexible share of remaining row width.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ToolbarSpacerKind {
    /// Use a fixed logical-pixel width. Zero is preserved; invalid values fall back to 12.
    Fixed(f32),
    /// Share remaining width with other Fill children; this is the default.
    #[default]
    Flexible,
}

impl ToolbarSpacerKind {
    /// Use the fixed stock gap of 12 px; this does not resolve live spacing.
    pub fn default_fixed() -> Self {
        ToolbarSpacerKind::Fixed(DEFAULT_LENGTH)
    }
}

/// Pinned divider styling, resolved when building the spacer.
#[derive(Debug, Clone, Copy)]
pub struct ToolbarSpacerTokens {
    /// Pinned divider ink, including alpha; palette constructors use border ink.
    pub divider_color: Color,
    /// Logical-pixel line thickness; negative or non-finite values fall back to 1.
    /// Zero suppresses the line; positive fractions retain native physical-pixel snapping.
    pub divider_width: f32,
    /// Visible fraction of assigned height, clamped to 0..1.
    /// Non-finite values fall back to 0.6; zero suppresses the line.
    pub divider_height_fraction: f32,
}

impl ToolbarSpacerTokens {
    /// Use palette border ink, 1 px thickness and 0.6 height fraction.
    /// An unpinned spacer instead uses live quiet toolbar-divider ink.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            divider_color: p.border,
            divider_width: LineSize::Thin.px(),
            divider_height_fraction: DIVIDER_HEIGHT_FRACTION,
        }
    }
    /// Return tokens derived from the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Return tokens derived from the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Builder for a horizontal gap that fills its assigned height.
#[must_use]
pub struct EnzToolbarSpacer {
    kind: ToolbarSpacerKind,
    divider: bool,
    tokens: Option<ToolbarSpacerTokens>,
}

impl EnzToolbarSpacer {
    /// Choose fixed width or flexible fill; defaults to Flexible.
    pub fn kind(mut self, kind: ToolbarSpacerKind) -> Self {
        self.kind = kind;
        self
    }
    /// Show a centred vertical line; defaults to false.
    pub fn divider(mut self, on: bool) -> Self {
        self.divider = on;
        self
    }
    /// Pin divider ink and geometry; validation occurs when building the element.
    pub fn tokens(mut self, tokens: ToolbarSpacerTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Build the gap and optional line using the parent's assigned height.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let width: Length = match self.kind {
            ToolbarSpacerKind::Fixed(px) => {
                let px = if px.is_finite() && px >= 0.0 {
                    px
                } else {
                    DEFAULT_LENGTH
                };
                Length::Fixed(px)
            }
            ToolbarSpacerKind::Flexible => Length::Fill,
        };

        if !self.divider {
            return Space::new().width(width).height(Length::Fill).into();
        }
        let pinned = self.tokens;
        let geo = pinned.unwrap_or_else(ToolbarSpacerTokens::dark);
        let thickness = sanitize_thickness(geo.divider_width, LineSize::Thin.px());
        let frac = if geo.divider_height_fraction.is_finite() {
            geo.divider_height_fraction.clamp(0.0, 1.0)
        } else {
            DIVIDER_HEIGHT_FRACTION
        };
        // Native Rule snapping can resurrect a zero-length line; omit it but retain the gap.
        if thickness == 0.0 || frac == 0.0 {
            return Space::new().width(width).height(Length::Fill).into();
        }

        let rule = vertical_rule::<Message>(
            thickness,
            FillMode::Percent(frac * 100.0),
            pinned.map(|t| t.divider_color),
        );
        container(rule)
            .width(width)
            .height(Length::Fill)
            .center_x(width)
            .into()
    }
}

impl<'a, Message: 'a> From<EnzToolbarSpacer> for Element<'a, Message> {
    fn from(s: EnzToolbarSpacer) -> Self {
        s.into_element()
    }
}

/// Build a flexible gap without a divider or pinned tokens.
pub fn enz_toolbar_spacer() -> EnzToolbarSpacer {
    EnzToolbarSpacer {
        kind: ToolbarSpacerKind::default(),
        divider: false,
        tokens: None,
    }
}
