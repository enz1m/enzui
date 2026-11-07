//! A canvas drawing that tells assistive technology what it shows.
//!
//! A raw `iced::widget::canvas` is invisible to screen readers. [`enz_canvas`]
//! draws the same [`Program`] and publishes
//! an image with a [`label`](EnzCanvas::label) and an optional
//! [`description`](EnzCanvas::description) (the numbers a chart shows, say),
//! or nothing when it is decorative. Its colours come from the theme the
//! program is drawn with; read them through `theme_lookup`-style palettes in
//! `draw`. Charts have their own widgets ([`bar_chart`](crate::bar_chart)
//! and friends).

use iced::widget::canvas::{self, Program};
use iced::{Element, Length, Renderer, Theme};

use crate::decorative::decorative;
use crate::semantics_container::SemanticsContainer;
use enzui_core::semantics::{Role, Semantics};

/// A canvas drawing. See the [module](self) documentation.
#[must_use]
pub struct EnzCanvas<P> {
    program: P,
    width: Length,
    height: Length,
    label: Option<String>,
    description: Option<String>,
}

/// A drawing of `program`.
pub fn enz_canvas<P>(program: P) -> EnzCanvas<P> {
    EnzCanvas {
        program,
        width: Length::Fill,
        height: Length::Fill,
        label: None,
        description: None,
    }
}

impl<P> EnzCanvas<P> {
    /// The width.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// The height.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// What the drawing shows, read by screen readers.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The detail a sighted user reads off the drawing.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl<'a, Message: 'a, P: Program<Message, Theme, Renderer> + 'a> From<EnzCanvas<P>>
    for Element<'a, Message>
{
    fn from(view: EnzCanvas<P>) -> Self {
        let drawing = canvas::Canvas::new(view.program)
            .width(view.width)
            .height(view.height);
        match view.label {
            Some(label) => {
                let mut semantics = Semantics::new(Role::Image).label_opt(Some(label));
                semantics.description = view.description;
                SemanticsContainer::new(drawing, semantics).into()
            }
            None => Element::new(decorative(drawing)),
        }
    }
}
