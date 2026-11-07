//! Place content on a sampled-backdrop material without adding padding.
//!
//! Defaults are [`MaterialStyle::Regular`], a 12 px radius and elevation 1.
//! [`MaterialStyle::FlatFrosted`] selects flat tinted blur with elevation 0;
//! [`EnzGlass::tint_opacity`] adjusts its tint locally.
//! The surface follows the child's sizing intent. Match its radius to the
//! content, or use [`EnzGlass::radius_resolved`] for theme-dependent corners.
//! The surface clips content to its rounded outline.
//! `ink(GlassInk::Theme)` preserves theme ink with the chosen optical style.
//! Unmeasured surfaces paint with theme ink and a neutral wash.
//! Adaptive ink remains visible while fresh backdrop
//! measurements are pending. Replacement views need distinct parent keys to
//! discard retained measurement history.
//! Content receives a material-adapted theme when optical composition succeeds;
//! explicit colours and pinned tokens are unaffected. Stock iced themes carry
//! no enzUI palette payload to adapt. Child overlays are forwarded separately.
//! Unsupported rendering or accessibility policy produces an opaque fallback
//! with the ordinary child theme. Nested materials also fall back instead of
//! filtering recursively. No protected content inset is added; the caller
//! owns clearance from the refracting edge. See [`crate::surface::EnzSurface`]
//! for material policy and backdrop options.

use crate::surface::enz_surface;
use enzui_theme::{GlassInk, MaterialInk, MaterialStyle, MaterialTokens};
use iced::Element;

type RadiusResolver = Box<dyn Fn(&iced::Theme) -> f32>;

/// Builder returned by [`enz_glass`].
pub struct EnzGlass<'a, Message> {
    content: Element<'a, Message>,
    style: MaterialStyle,
    radius: f32,
    radius_resolver: Option<RadiusResolver>,
    elevation: Option<u8>,
    material_tokens: Option<MaterialTokens>,
    tint_opacity: Option<f32>,
    ink: Option<GlassInk>,
}

/// Place content on glass with Regular material, a 12 px radius and elevation 1.
/// No padding or protected content inset is added; the caller owns edge clearance.
pub fn enz_glass<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> EnzGlass<'a, Message> {
    EnzGlass {
        content: content.into(),
        style: MaterialStyle::Regular,
        radius: 12.0,
        radius_resolver: None,
        elevation: None,
        material_tokens: None,
        tint_opacity: None,
        ink: None,
    }
}

impl<'a, Message: 'a> EnzGlass<'a, Message> {
    /// The material style.
    #[must_use]
    pub fn style(mut self, style: MaterialStyle) -> Self {
        self.style = style;
        self
    }

    /// Override foreground adaptation while preserving the selected material's optics.
    /// `GlassInk::Theme` keeps application ink and adds a neutral theme wash for
    /// legibility over changing backdrops. Optical tokens remain unchanged.
    /// The style's ink policy is used by default; accessibility refusals still apply.
    #[must_use]
    pub fn ink(mut self, ink: GlassInk) -> Self {
        self.ink = Some(ink);
        self
    }

    /// Overrides this surface's material tokens; process-wide tuning is unchanged.
    #[must_use]
    pub fn material_tokens(mut self, tokens: MaterialTokens) -> Self {
        self.material_tokens = Some(tokens);
        self
    }

    /// Sets local tint strength, `0..=1`, overriding the token field.
    ///
    /// Finite values clamp; non-finite values preserve the selected tokens.
    /// At one the existing opaque fallback is used.
    #[must_use]
    pub fn tint_opacity(mut self, opacity: f32) -> Self {
        self.tint_opacity = Some(opacity);
        self
    }

    /// The corner radius, in logical pixels. Match the content's own radius.
    #[must_use]
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self.radius_resolver = None;
        self
    }

    /// Resolve the corner radius from the theme on every draw.
    /// This replaces the fixed radius; a later [`Self::radius`] clears the resolver.
    /// The surface repairs negative or non-finite resolved radii to 6 px.
    #[must_use]
    pub fn radius_resolved(mut self, resolver: impl Fn(&iced::Theme) -> f32 + 'static) -> Self {
        self.radius_resolver = Some(Box::new(resolver));
        self
    }

    /// Sets the explicit shadow tier, `0..=5`.
    /// Defaults to zero for FlatFrosted and one for other styles.
    /// Positive bevels with adaptive dark ink can add a silhouette shadow.
    #[must_use]
    pub fn elevation(mut self, level: u8) -> Self {
        self.elevation = Some(level);
        self
    }

    /// Build the element.
    pub fn into_element(self) -> Element<'a, Message> {
        let size = self.content.as_widget().size();
        let mut surface = enz_surface(self.content)
            .material(self.style)
            .material_ink(MaterialInk::Strong)
            .padding(0)
            .border_width(0.0)
            .radius(self.radius)
            .elevation(
                self.elevation
                    .unwrap_or(if self.style == MaterialStyle::FlatFrosted {
                        0
                    } else {
                        1
                    }),
            )
            .width(size.width)
            .height(size.height);
        if let Some(tokens) = self.material_tokens {
            surface = surface.material_tokens(tokens);
        }
        if let Some(opacity) = self.tint_opacity {
            surface = surface.material_tint_opacity(opacity);
        }
        if let Some(ink) = self.ink {
            surface = surface.glass_ink(ink);
        }
        match self.radius_resolver {
            Some(resolver) => surface
                .radius_resolved(move |theme| iced::border::Radius::new(resolver(theme)))
                .into(),
            None => surface.into(),
        }
    }
}

impl<'a, Message: 'a> From<EnzGlass<'a, Message>> for Element<'a, Message> {
    fn from(glass: EnzGlass<'a, Message>) -> Self {
        glass.into_element()
    }
}

pub(crate) fn wrap<'a, Message: 'a>(
    element: Element<'a, Message>,
    style: Option<MaterialStyle>,
    radius: f32,
) -> Element<'a, Message> {
    match style {
        Some(style) => enz_glass(element).style(style).radius(radius).into(),
        None => element,
    }
}

pub(crate) fn wrap_resolved<'a, Message: 'a>(
    element: Element<'a, Message>,
    style: Option<MaterialStyle>,
    radius: impl Fn(&iced::Theme) -> f32 + 'static,
) -> Element<'a, Message> {
    match style {
        Some(style) => enz_glass(element)
            .style(style)
            .radius_resolved(radius)
            .into(),
        None => element,
    }
}
