//! Grain: a printed, paper-like speckle laid over a surface.
//!
//! A [`Grain`] describes the texture; `enzui-widgets` rasterises it once per
//! device scale into a seamless tile and repeats that tile over any surface
//! (`enz_surface(..).grain(..)`, a shell pane's `PaneStyle::grain`, or the
//! `enz_grain` wrapper). The theme carries a default in [`EnzTheme::grain`]
//! that `GrainSpec::Theme` follows.
//!
//! The built-in [`GrainSource::Noise`] is fractal gradient noise on a lattice
//! that wraps at the tile's edge, so its tile repeats without a seam by
//! construction. [`GrainSource::Svg`] takes any SVG document instead. Under
//! [`GrainMapping::Specks`] the source's brightness becomes light and dark
//! specks of the grain's inks; under [`GrainMapping::Artwork`] it is drawn in
//! its own colours.
//!
//! Most noise SVGs found online are an `feTurbulence` filter. The renderer
//! does not stitch `feTurbulence` tiles (`stitchTiles` is ignored), so such a
//! document would show its tile edges; [`Grain::svg`] gives it
//! [`GrainTiling::Blend`], which cross-fades the edges into a shifted copy.
//! Patterned art that repeats by itself keeps [`GrainTiling::Repeat`], and
//! art that does not can use [`GrainTiling::Mirror`].
//!
//! The widely shared "grainy gradients" noise works as it is:
//!
//! ```
//! use enzui_theme::{Grain, GrainTiling};
//!
//! let noise = Grain::svg(
//!     "<svg viewBox='0 0 250 250' width='250' height='250' xmlns='http://www.w3.org/2000/svg'>\
//!      <filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.65' \
//!      numOctaves='3' stitchTiles='stitch'/></filter>\
//!      <rect width='100%' height='100%' filter='url(#n)'/></svg>",
//! )
//! .intensity(0.14);
//! assert_eq!(noise.tiling, GrainTiling::Blend);
//! ```
//!
//! | knob | what it changes | rasterised |
//! |---|---|---|
//! | [`intensity`](Grain::intensity) | how strong a full speck is, at draw time | no |
//! | [`density`](Grain::density) | how much of the surface carries specks | remapped |
//! | [`scale`](Grain::scale) | speck size (1 = one device pixel per noise texel) | yes |
//! | [`tone`](Grain::tone) | light specks, dark specks, or both | remapped |
//! | [`inks`](Grain::inks) / [`tint`](Grain::tint) | speck colours | remapped |
//! | [`frequency`](Grain::frequency), [`octaves`](Grain::octaves), [`seed`](Grain::seed) | the built-in noise | yes |
//! | [`tile`](Grain::tile) | the repeating tile's logical size | yes |
//! | [`tiling`](Grain::tiling) | repeat, or mirror a source that does not tile | yes |
//! | [`opacity`](Grain::opacity) | the whole layer, at draw time | no |
//! | [`placement`](Grain::placement) | behind the surface's content, or over it | no |
//! | [`motion`](Grain::motion) | film-grain shimmer, frames per second | no |
//!
//! Knobs marked "no" change nothing but how the cached tile is drawn, so
//! animating them is free. "Remapped" knobs recolour the cached noise field
//! into a new tile (about 1.5 ms for a 2x screen's tile); the others
//! compute the field again too. Tiles and fields are cached by value. The
//! built-in noise tile spans [`tile`](Grain::tile) at device scale whatever
//! the [`scale`](Grain::scale), so no knob makes it larger to compute.
//!
//! [`EnzTheme::grain`]: crate::EnzTheme::grain

use iced_core::Color;
use std::sync::{Arc, Mutex};

/// An SVG document interned for grain, by handle: keeps [`Grain`] `Copy`, so
/// it fits in copied token sets like a shell pane's style.
///
/// Documents are interned once per distinct text for the process's life;
/// grain sources are few and long-lived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GrainSvg(u32);

static SVGS: Mutex<Vec<Arc<str>>> = Mutex::new(Vec::new());

impl GrainSvg {
    /// Intern `document`, returning the existing handle for a document
    /// already interned.
    pub fn intern(document: &str) -> Self {
        let mut svgs = SVGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = svgs.iter().position(|svg| &**svg == document) {
            return Self(index as u32);
        }
        svgs.push(Arc::from(document));
        Self((svgs.len() - 1) as u32)
    }

    /// The interned document.
    pub fn document(self) -> Arc<str> {
        let svgs = SVGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        svgs.get(self.0 as usize)
            .cloned()
            .unwrap_or_else(|| Arc::from(""))
    }
}

/// Where a grain's texture comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GrainSource {
    /// The built-in stitched fractal noise, shaped by
    /// [`Grain::frequency`], [`Grain::octaves`] and [`Grain::seed`].
    Noise,
    /// An SVG document. Its width and height (or view box) are one tile in
    /// logical pixels. A document that does not repeat seamlessly by itself
    /// can use [`GrainTiling::Mirror`].
    Svg(GrainSvg),
}

/// How a grain's source becomes specks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrainMapping {
    /// Brightness above the tile's mean becomes light specks and below it
    /// dark specks, in the grain's inks; [`Grain::density`] keeps only the
    /// strongest. Transparent source pixels read as the mean.
    #[default]
    Specks,
    /// The source is drawn in its own colours and alpha, scaled by
    /// [`Grain::intensity`]. For hand-made speckle art.
    Artwork,
}

/// Which specks a [`GrainMapping::Specks`] grain keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrainTone {
    /// Light and dark specks: reads on any surface colour.
    #[default]
    Both,
    /// Only dark specks: ink on paper.
    Dark,
    /// Only light specks: dust on a dark surface.
    Light,
}

/// How the tile repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrainTiling {
    /// The tile repeats as is; its source must be seamless (the built-in
    /// noise is).
    #[default]
    Repeat,
    /// The tile is mirrored into a 2x2 block that repeats seamlessly whatever
    /// the source, at twice the period. Suits patterned art; on noise the
    /// mirror symmetry can show.
    Mirror,
    /// Near its edges the tile cross-fades into a copy shifted by half a
    /// tile, which continues across the wrap: noise from any source repeats
    /// without a seam. Speck values keep their contrast through the fade.
    Blend,
}

/// Whether grain lies under a surface's content or over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrainPlacement {
    /// On the surface's fill, under its content: text stays clean.
    #[default]
    Behind,
    /// Over the content too, like a printed page.
    Over,
}

/// A grain texture. See the [module documentation](self) for what each knob
/// does and which ones re-rasterise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grain {
    /// Where the texture comes from.
    pub source: GrainSource,
    /// How the source becomes specks.
    pub mapping: GrainMapping,
    /// The alpha of a full-strength speck, `0..=1`.
    pub intensity: f32,
    /// The share of the surface that carries specks, `0..=1`.
    pub density: f32,
    /// Speck size: `1.0` draws one noise texel per device pixel.
    pub scale: f32,
    /// Which specks to keep.
    pub tone: GrainTone,
    /// The colour of dark specks.
    pub dark_ink: Color,
    /// The colour of light specks.
    pub light_ink: Color,
    /// The opacity of the whole layer, applied at draw time.
    pub opacity: f32,
    /// The built-in noise's base frequency: higher is finer, up to `1.5`.
    pub frequency: f32,
    /// The built-in noise's octaves, `1..=6`: each adds finer detail at half
    /// the strength.
    pub octaves: u8,
    /// The built-in noise's seed.
    pub seed: u32,
    /// The built-in noise's tile, in logical pixels.
    pub tile: f32,
    /// How the tile repeats.
    pub tiling: GrainTiling,
    /// Under or over the surface's content.
    pub placement: GrainPlacement,
    /// Film-grain shimmer: the tile jumps to a new offset this many times a
    /// second. `None` keeps it still; reduced motion stills it.
    pub motion: Option<f32>,
}

/// The default speck alpha.
pub const DEFAULT_GRAIN_INTENSITY: f32 = 0.14;
/// The default share of the surface that carries specks.
pub const DEFAULT_GRAIN_DENSITY: f32 = 0.95;
/// The default built-in noise frequency.
pub const DEFAULT_GRAIN_FREQUENCY: f32 = 1.3;
/// The default built-in noise tile, in logical pixels.
pub const DEFAULT_GRAIN_TILE: f32 = 256.0;

impl Default for Grain {
    /// A fine, even paper grain in light and dark specks.
    fn default() -> Self {
        Self {
            source: GrainSource::Noise,
            mapping: GrainMapping::Specks,
            intensity: DEFAULT_GRAIN_INTENSITY,
            density: DEFAULT_GRAIN_DENSITY,
            scale: 1.0,
            tone: GrainTone::Both,
            dark_ink: Color::BLACK,
            light_ink: Color::WHITE,
            opacity: 1.0,
            frequency: DEFAULT_GRAIN_FREQUENCY,
            octaves: 2,
            seed: 7,
            tile: DEFAULT_GRAIN_TILE,
            tiling: GrainTiling::Repeat,
            placement: GrainPlacement::Behind,
            motion: None,
        }
    }
}

impl Grain {
    /// The default grain; see [`Grain::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A barely-there texture for large panels.
    pub fn subtle() -> Self {
        Self {
            intensity: 0.08,
            ..Self::default()
        }
    }

    /// A strong printed-paper grain over the content, after the enzwall
    /// bauhaus scene.
    pub fn paper() -> Self {
        Self {
            intensity: 0.3,
            density: 1.0,
            placement: GrainPlacement::Over,
            ..Self::default()
        }
    }

    /// Coarser shimmering film grain.
    pub fn film() -> Self {
        Self {
            intensity: 0.22,
            scale: 1.5,
            motion: Some(24.0),
            ..Self::default()
        }
    }

    /// Texture from an SVG document, one tile at its own size.
    ///
    /// An `feTurbulence` document gets [`GrainTiling::Blend`], since its
    /// tile does not repeat seamlessly by itself; anything else repeats as is.
    pub fn svg(document: &str) -> Self {
        let tiling = if document.contains("feTurbulence") {
            GrainTiling::Blend
        } else {
            GrainTiling::Repeat
        };
        Self {
            source: GrainSource::Svg(GrainSvg::intern(document)),
            tiling,
            ..Self::default()
        }
    }

    /// How the source becomes specks.
    #[must_use]
    pub fn mapping(mut self, mapping: GrainMapping) -> Self {
        self.mapping = mapping;
        self
    }

    /// The alpha of a full-strength speck, clamped to `0..=1`.
    #[must_use]
    pub fn intensity(mut self, intensity: f32) -> Self {
        self.intensity = unit(intensity, DEFAULT_GRAIN_INTENSITY);
        self
    }

    /// The share of the surface that carries specks, clamped to `0..=1`.
    #[must_use]
    pub fn density(mut self, density: f32) -> Self {
        self.density = unit(density, DEFAULT_GRAIN_DENSITY);
        self
    }

    /// Speck size, clamped to `0.25..=8`.
    #[must_use]
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = if scale.is_finite() {
            scale.clamp(0.25, 8.0)
        } else {
            1.0
        };
        self
    }

    /// Which specks to keep.
    #[must_use]
    pub fn tone(mut self, tone: GrainTone) -> Self {
        self.tone = tone;
        self
    }

    /// The colours of dark and light specks.
    #[must_use]
    pub fn inks(mut self, dark: Color, light: Color) -> Self {
        self.dark_ink = dark;
        self.light_ink = light;
        self
    }

    /// One colour for every speck: a tinted grain.
    #[must_use]
    pub fn tint(self, color: Color) -> Self {
        self.inks(color, color)
    }

    /// The opacity of the whole layer, clamped to `0..=1`.
    #[must_use]
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = unit(opacity, 1.0);
        self
    }

    /// The built-in noise's base frequency, clamped to `0.05..=1.5`: finer
    /// noise than one feature per pixel and a half aliases into a hatch.
    /// Frequencies within 0.15 of a whole number are steered clear of it,
    /// where the noise would beat against the pixel grid.
    #[must_use]
    pub fn frequency(mut self, frequency: f32) -> Self {
        self.frequency = if frequency.is_finite() {
            frequency.clamp(0.05, 1.5)
        } else {
            DEFAULT_GRAIN_FREQUENCY
        };
        self
    }

    /// The built-in noise's octaves, clamped to `1..=6`.
    #[must_use]
    pub fn octaves(mut self, octaves: u8) -> Self {
        self.octaves = octaves.clamp(1, 6);
        self
    }

    /// The built-in noise's seed.
    #[must_use]
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// The built-in noise's tile in logical pixels, clamped to `32..=1024`.
    #[must_use]
    pub fn tile(mut self, tile: f32) -> Self {
        self.tile = if tile.is_finite() {
            tile.clamp(32.0, 1024.0)
        } else {
            DEFAULT_GRAIN_TILE
        };
        self
    }

    /// How the tile repeats.
    #[must_use]
    pub fn tiling(mut self, tiling: GrainTiling) -> Self {
        self.tiling = tiling;
        self
    }

    /// Under or over the surface's content.
    #[must_use]
    pub fn placement(mut self, placement: GrainPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Film-grain shimmer at `fps` frames a second (clamped to `1..=60`), or
    /// still with `None`.
    #[must_use]
    pub fn motion(mut self, fps: Option<f32>) -> Self {
        self.motion = fps
            .filter(|f| f.is_finite() && *f > 0.0)
            .map(|f| f.clamp(1.0, 60.0));
        self
    }

    /// Whether this grain draws anything at all.
    pub fn is_visible(&self) -> bool {
        self.intensity > 0.0 && self.opacity > 0.0 && self.density > 0.0
    }
}

fn unit(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        fallback
    }
}

/// Which grain a surface draws: the theme's, or its own.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GrainSpec {
    /// No grain.
    #[default]
    None,
    /// The theme's [`EnzTheme::grain`](crate::EnzTheme::grain), resolved at draw time.
    Theme,
    /// This grain, whatever the theme.
    Custom(Grain),
}

impl From<Grain> for GrainSpec {
    fn from(grain: Grain) -> Self {
        Self::Custom(grain)
    }
}
