//! Adaptive liquid glass in any shape: numerals, a pill, an outline.
//!
//! [`enz_adaptive_glass`] fills a [`GlassSilhouette`] with a flat glass lens
//! over whatever the window drew behind it. Each connected component of the
//! shape (each numeral of a clock) bends the backdrop towards its own centre,
//! with colour separating at the rim, a soft contour highlight, and a tone
//! that adapts to the backdrop: over dark content the glass lifts towards
//! white to stay legible, over bright content it stays translucent. One tone
//! covers the whole shape, so separate components never band.
//!
//! Two presets, [`AdaptiveGlass::Clear`] (sharp) and
//! [`AdaptiveGlass::Frosted`] (diffused), follow the approved login-clock
//! recipe; [`AdaptiveGlass::material`] returns it for a given width so a
//! custom primitive can reuse it.
//!
//! The lens is flat: no bevel or raised-face shading, and no ink contrast
//! floor, since the shape is the content. Text drawn on top needs its own
//! contrast layer.
//!
//! The user's material policy settles it like every other material: reduced
//! motion removes refraction and dispersion, and reduced transparency, or a
//! renderer without optical materials (the software renderer), draws the
//! shape filled with a plain colour instead.
//!
//! ```no_run
//! use std::sync::Arc;
//! use enzui_widgets::adaptive_glass::{AdaptiveGlass, GlassSilhouette, enz_adaptive_glass};
//!
//! let pill = Arc::new(
//!     GlassSilhouette::from_svg(
//!         "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 360 128'>\
//!          <rect width='360' height='128' rx='64'/></svg>",
//!         360.0,
//!         128.0,
//!     )
//!     .expect("a valid document"),
//! );
//! let view: iced::Element<'_, ()> = enz_adaptive_glass(pill, AdaptiveGlass::Clear)
//!     .width(360)
//!     .into();
//! ```

mod primitive;
mod silhouette;

use std::sync::Arc;

use enzui_theme::{MaterialPolicy, MaterialStyle};
use iced::advanced::image::{self, Renderer as _};
use iced::advanced::material::Material;
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse, renderer};
use iced::{Color, Element, Length, Rectangle, Size};
use iced_wgpu::primitive::Renderer as _;

pub use silhouette::{GlassSilhouette, SilhouetteError};

use primitive::GlassPrimitive;

/// The two adaptive glass recipes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdaptiveGlass {
    /// Sharp: the backdrop shows through unblurred.
    Clear,
    /// Diffused: the backdrop is blurred behind the lens.
    Frosted,
}

impl AdaptiveGlass {
    /// The material for a shape `width` logical pixels wide, settled by
    /// `policy`.
    ///
    /// Bevel width and optical depth carry half lengths, scaled in the shader
    /// by each component's share of the whole width; dispersion is likewise
    /// relative. Per 375 px component that is a 29 px rim reach, a 47 px
    /// displacement and 9.1 px of colour separation.
    #[must_use]
    pub fn material(self, width: f32, policy: &MaterialPolicy) -> Material {
        let frosted = self == Self::Frosted;
        let mut tokens = if frosted {
            MaterialStyle::Frosted
        } else {
            MaterialStyle::Clear
        }
        .tokens();
        tokens.contrast_protection = 0.0;
        tokens.brightness = 0.0;
        tokens.tint_opacity = 0.003;
        tokens.bevel_width = (width * 29.0 / 750.0).clamp(0.25, 96.0);
        tokens.optical_depth = (width * 47.0 / 750.0).clamp(0.25, 96.0);
        tokens.refraction = 1.0;
        tokens.dispersion = (width * 9.1 / 750.0 / 36.0).clamp(0.0, 1.0);
        tokens.index_of_refraction = 1.5;
        tokens.saturation = 1.0;
        tokens.color_diffusion = 0.0;
        tokens.color_reach = 24.0;
        tokens.light_angle = 250.0;
        tokens.profile_curvature = 2.0;
        tokens.highlight = if frosted { 0.16 } else { 0.18 };
        tokens.blur_sigma = if frosted { 3.5 } else { 0.0 };
        policy.settle(tokens).to_material([0.0; 4], Color::WHITE)
    }
}

/// Fills `silhouette` with adaptive glass of the `preset` recipe.
#[must_use]
pub fn enz_adaptive_glass(
    silhouette: Arc<GlassSilhouette>,
    preset: AdaptiveGlass,
) -> EnzAdaptiveGlass {
    EnzAdaptiveGlass {
        silhouette,
        preset,
        width: Length::Fill,
        alpha: 1.0,
        fallback: None,
    }
}

/// A shape of adaptive glass; see the [module docs](self).
///
/// Its height follows the silhouette's aspect ratio. It publishes no
/// semantics: wrap it in a labelled container when it carries meaning, as a
/// clock does.
pub struct EnzAdaptiveGlass {
    silhouette: Arc<GlassSilhouette>,
    preset: AdaptiveGlass,
    width: Length,
    alpha: f32,
    fallback: Option<Color>,
}

impl EnzAdaptiveGlass {
    /// The width; fills by default.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Overall opacity, for fading the shape in or out.
    #[must_use]
    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// The plain fill used where glass cannot render; the theme's text colour
    /// by default.
    #[must_use]
    pub fn fallback_color(mut self, color: Color) -> Self {
        self.fallback = Some(color);
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzAdaptiveGlass {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let width = limits.resolve(self.width, Length::Shrink, Size::ZERO).width;
        let height = width / self.silhouette.aspect_ratio();
        layout::Node::new(limits.resolve(self.width, height, Size::new(width, height))).apply(tree);
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 || self.alpha <= 0.0 {
            return;
        }
        let policy = crate::theme_lookup::material_policy(theme);
        if renderer.supports_primitives()
            && renderer.material_support().is_optical()
            && !policy.forbids_material()
        {
            let material = self.preset.material(bounds.width, &policy);
            // A measure-only boundary makes the frame behind a sampleable
            // scene without painting a panel; the primitive does the optics.
            let mut boundary = material;
            boundary.measure_only = true;
            boundary.blur_sigma = 0.0;
            boundary.refraction = 0.0;
            renderer.with_layer_material(bounds, boundary, |renderer| {
                renderer.draw_primitive(
                    bounds,
                    GlassPrimitive {
                        silhouette: Arc::clone(&self.silhouette),
                        material,
                        alpha: self.alpha,
                        key: GlassPrimitive::key_for(
                            &self.silhouette,
                            bounds,
                            &material,
                            self.alpha,
                        ),
                    },
                );
            });
            return;
        }
        let color = self
            .fallback
            .unwrap_or_else(|| crate::theme_lookup::palette(theme).text);
        let handle = self.silhouette.fallback_image(color);
        renderer.draw_image(
            image::Image {
                opacity: self.alpha,
                ..image::Image::new(handle)
            },
            bounds,
            bounds,
        );
    }
}

impl<'a, Message: 'a> From<EnzAdaptiveGlass> for Element<'a, Message> {
    fn from(glass: EnzAdaptiveGlass) -> Self {
        Element::new(glass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_optical_materials_the_shape_is_filled_plainly_at_its_aspect() {
        use iced::advanced::renderer::Headless as _;
        use iced_test::runtime::user_interface::{Cache, UserInterface};

        // A 4:1 bar with a transparent margin.
        let (w, h) = (64u32, 16u32);
        let alpha: Vec<u8> = (0..w * h)
            .map(|i| {
                if (2..14).contains(&(i / w)) && (4..60).contains(&(i % w)) {
                    255
                } else {
                    0
                }
            })
            .collect();
        let bar = Arc::new(GlassSilhouette::from_alpha(w, h, &alpha));
        let mut renderer = iced_test::futures::futures::executor::block_on(iced::Renderer::new(
            iced::advanced::renderer::Settings::default(),
            Some("tiny-skia"),
        ))
        .expect("the software renderer");
        let red = Color::from_rgb8(255, 0, 0);
        let view: Element<'_, ()> = enz_adaptive_glass(bar, AdaptiveGlass::Frosted)
            .width(200)
            .fallback_color(red)
            .into();
        let size = iced::Size::new(200u32, 100u32);
        let mut ui = UserInterface::build(
            iced::widget::column![view],
            iced::Size::new(size.width as f32, size.height as f32),
            Cache::default(),
            &mut renderer,
        );
        ui.draw(
            &mut renderer,
            &iced::Theme::Dark,
            &renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        let pixels = renderer.screenshot(size, 1.0, Color::BLACK);
        let px = |x: u32, y: u32| {
            let i = ((y * size.width + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        assert_eq!(
            px(100, 25),
            [255, 0, 0],
            "the bar, 50 px tall at 200 px wide"
        );
        assert_eq!(px(2, 25), [0, 0, 0], "its transparent margin");
        assert_eq!(px(100, 60), [0, 0, 0], "below its height");
    }
}

#[cfg(test)]
mod gpu_tests;
