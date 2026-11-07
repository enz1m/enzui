//! Rigid single-axis gaps sized by semantic spacing tokens.
//!
//! [`enz_spacer_gap`] uses fixed extent on the chosen axis and Shrink on the
//! cross axis, so a gap does not stretch its parent sideways. Resolution happens
//! at view construction: an explicit pixel pin wins, then a forwarded spacing
//! scale, then [`SpacingScale::default`]. Rebuild with the active scale to follow
//! spacing changes. [`crate::toolbar_spacer`] adds flexible toolbar spacing.

use enzui_theme::{SpaceToken, SpacingScale};
use iced::widget::Space;
use iced::{Element, Length};

/// Which axis a rigid [`enz_spacer_gap`] occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// A horizontal gap: fixed width, `Shrink` height.
    Horizontal,
    /// A vertical gap: fixed height, `Shrink` width.
    Vertical,
}

/// Build a fixed-extent token gap with a Shrink cross axis.
/// The stock spacing ladder applies unless a scale or explicit extent is supplied.
#[derive(Debug, Clone, Copy)]
#[must_use]
pub struct EnzSpacerGap {
    axis: Axis,
    size: SpaceToken,
    spacing_scale: Option<SpacingScale>,
    extent_px_explicit: Option<f32>,
}

impl EnzSpacerGap {
    /// Resolve the token using a forwarded scale at construction, unless a pixel pin wins.
    /// Scale values are not validated here: negative and NaN extents resolve to zero,
    /// while positive infinity remains in the resolved extent.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Pin the main-axis extent in logical pixels, overriding spacing-scale resolution.
    /// Negative or non-finite input becomes zero; zero remains a literal pin.
    pub fn extent_px(mut self, px: f32) -> Self {
        let clamped = if px.is_finite() { px.max(0.0) } else { 0.0 };
        self.extent_px_explicit = Some(clamped);
        self
    }

    fn resolve_px(&self) -> f32 {
        if let Some(px) = self.extent_px_explicit {
            return px;
        }
        match self.spacing_scale.as_ref() {
            Some(scale) => self.size.resolve(scale).max(0.0),
            None => self.size.resolve(&SpacingScale::default()).max(0.0),
        }
    }
}

impl EnzSpacerGap {
    /// Build the gap element with fixed main-axis extent and a Shrink cross axis.
    #[must_use]
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzSpacerGap> for Element<'a, Message> {
    fn from(g: EnzSpacerGap) -> Self {
        let px = g.resolve_px();
        match g.axis {
            Axis::Vertical => Space::new().width(Length::Shrink).height(Length::Fixed(px)),
            Axis::Horizontal => Space::new().width(Length::Fixed(px)).height(Length::Shrink),
        }
        .into()
    }
}

/// Build a gap for the chosen axis and token.
/// [`SpaceToken::None`] is zero on the stock ladder; a forwarded space0 or
/// explicit extent can change it.
pub fn enz_spacer_gap(axis: Axis, size: SpaceToken) -> EnzSpacerGap {
    EnzSpacerGap {
        axis,
        size,
        spacing_scale: None,
        extent_px_explicit: None,
    }
}
