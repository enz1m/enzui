//! Place caller-styled bands vertically with no background or padding.
//!
//! Width and height default to Fill, and spacing to zero. Children retain their
//! own width policies; use Fill on each band that must reach the side edges.
//! Fill-height children share the available slack. The surrounding shell owns
//! clipping and pane styling; the stack adds neither.

use enzui_core::metrics::FiniteNonNeg;
use iced::widget::Column;
use iced::{Element, Length, Padding};

/// Stack the supplied bands with Fill width and height and zero spacing.
pub fn enz_band_stack<'a, Message>(bands: Vec<Element<'a, Message>>) -> EnzBandStack<'a, Message>
where
    Message: 'a,
{
    EnzBandStack {
        bands,
        width: Length::Fill,
        height: Length::Fill,
        spacing: 0.0,
    }
}

/// A transparent vertical placement builder; bands supply their own paint.
#[must_use]
pub struct EnzBandStack<'a, Message> {
    bands: Vec<Element<'a, Message>>,
    width: Length,
    height: Length,
    spacing: f32,
}

impl<'a, Message> EnzBandStack<'a, Message>
where
    Message: 'a,
{
    /// Set the outer width policy; defaults to Fill.
    /// Each child retains its own width policy.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the outer height policy; defaults to Fill.
    /// Fill-height bands share remaining space.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Set vertical gaps in logical pixels; defaults to zero.
    /// Negative or non-finite values become zero.
    pub fn spacing(mut self, s: f32) -> Self {
        self.spacing = FiniteNonNeg::new_or(s, 0.0).get();
        self
    }
}

impl<'a, Message> EnzBandStack<'a, Message>
where
    Message: 'a,
{
    /// Build the transparent column with no padding.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzBandStack<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzBandStack<'a, Message>) -> Self {
        Column::with_children(w.bands)
            .width(w.width)
            .height(w.height)
            .spacing(w.spacing)
            .padding(Padding::ZERO)
            .into()
    }
}
