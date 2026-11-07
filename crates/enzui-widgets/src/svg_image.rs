//! An SVG picture that follows the theme and tells assistive technology what
//! it shows.
//!
//! [`enz_svg`] draws an SVG at a size the app picks. A
//! [`symbolic`](EnzSvg::symbolic) SVG (a single-colour drawing such as a
//! pictogram) is painted in the theme's text colour, so it follows dark and
//! light; others keep their own colours. With a [`label`](EnzSvg::label) it
//! is an image to screen readers; without one it is decorative and skipped.
//! For icons from the icon set use [`icon`](crate::icon).

use iced::widget::svg;
use iced::{ContentFit, Element, Length};

use crate::decorative::decorative;
use crate::semantics_container::SemanticsContainer;
use enzui_core::semantics::{Role, Semantics};

/// An SVG picture. See the [module](self) documentation.
#[must_use]
pub struct EnzSvg {
    handle: svg::Handle,
    width: Length,
    height: Length,
    fit: ContentFit,
    symbolic: bool,
    label: Option<String>,
}

/// An SVG picture of `handle`.
pub fn enz_svg(handle: impl Into<svg::Handle>) -> EnzSvg {
    EnzSvg {
        handle: handle.into(),
        width: Length::Shrink,
        height: Length::Shrink,
        fit: ContentFit::Contain,
        symbolic: false,
        label: None,
    }
}

impl EnzSvg {
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

    /// How the drawing fits its box; [`ContentFit::Contain`] by default.
    pub fn content_fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }

    /// Paints the drawing in the theme's text colour.
    pub fn symbolic(mut self, symbolic: bool) -> Self {
        self.symbolic = symbolic;
        self
    }

    /// What the picture shows, read by screen readers.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl<'a, Message: 'a> From<EnzSvg> for Element<'a, Message> {
    fn from(picture: EnzSvg) -> Self {
        let symbolic = picture.symbolic;
        let drawing = svg(picture.handle)
            .width(picture.width)
            .height(picture.height)
            .content_fit(picture.fit)
            .style(move |theme: &iced::Theme, _status| svg::Style {
                color: symbolic.then(|| crate::theme_lookup::palette(theme).text),
            });
        match picture.label {
            Some(label) => SemanticsContainer::leaf(
                drawing,
                Semantics::new(Role::Image).label_opt(Some(label)),
            )
            .into(),
            None => Element::new(decorative(drawing)),
        }
    }
}
