//! EnzSurface: tokenized visual surface with no imposed inner layout.
//!
//! Themed container primitive — background tone, radius, optional border,
//! per-corner override, clipping, elevation shadow — caller-owned inner
//! layout. [`EnzCard`](crate::card::EnzCard) adds a vertical layout +
//! title/description; `EnzSurface` is the bare themed rectangle.
//!
//! # Material (liquid glass)
//!
//! [`material`](EnzSurface::material) swaps the flat fill for a sampled,
//! filtered backdrop ([`MaterialStyle`] names it, the theme owns the numbers).
//! Additive: without it the surface is byte-identical; no focus stop, message
//! or a11y node (a transparent paint boundary around the same subtree).
//!
//! * Fallback is opaque and explicit. No sampled-backdrop path, reduced
//!   transparency, increased contrast, material-in-material, or
//!   [`MaterialStyle::Solid`] all paint
//!   [`surface_color`](MaterialStyle::surface_color) — never a faked blur. An
//!   explicit [`fill`](EnzSurface::fill) still wins as the fallback fill.
//! * Padding carries the protected inset. Contrast protection reaches full
//!   strength [`protected_inset`](MaterialTokens::protected_inset) px inside
//!   the silhouette: smaller paddings grow to it. Legible content belongs
//!   inside the inset.
//!
//! [`Regular`](MaterialStyle::Regular) for text, [`Lens`](MaterialStyle::Lens)
//! for compact app/shell chrome; documents and tables stay opaque.
//!
//! # Tones
//!
//! Each [`SurfaceTone`] is one semantic palette entry: `Surface` (default),
//! `SurfaceAlt`/`Sunken` (alt), `Elevated`/`Card` (elevated), `Selected`,
//! `Hover`, `Background` (app bg), `AccentSoft`, `ImageCanvas` (letterbox),
//! `VideoCanvas` (pure black), `Transparent`/`Plain` (no fill).
//!
//! # Radius / border sizes
//!
//! Radius defaults 6 px ([`DEFAULT_RADIUS`]); semantic sizes via [`RadiusSize`]
//! (none/sm/md/lg/xl) plus per-corner overrides. Border widths via [`LineSize`]
//! (thin 1 / regular 1.5 / thick 2 / heavy 3).
//!
//! # Clipping follows the radius
//!
//! [`clip`](EnzSurface::clip) masks children to the ROUNDED outline —
//! full-bleed children carve to the corner arc, never the cutouts. See
//! [`clip`](EnzSurface::clip) for the measurements and the one residual gap
//! (text on wgpu).

use enzui_theme::{MaterialInk, MaterialPolicy, MaterialStyle, MaterialTokens, Palette};
use iced::border::Radius;
use iced::widget::container;
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
use std::cell::Cell;
use std::rc::Rc;

/// A closure that resolves a value from the runtime `iced::Theme` on every
/// render. The shared shape behind [`EnzSurface::fill_resolved`],
/// [`EnzSurface::border_color_resolved`] and [`EnzSurface::shadow_resolved`].
type ThemeResolver<T> = Rc<dyn Fn(&iced::Theme) -> T>;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Semantic surface tone (background fill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceTone {
    /// Base `surface` (default).
    #[default]
    Surface,
    /// `surface_alt` — sunken/secondary.
    SurfaceAlt,
    /// `surface_elevated` — a card-like elevated surface.
    Elevated,
    /// `surface_selected` — highlighted row/panel.
    Selected,
    /// `surface_hover` — hover-tinted surface.
    Hover,
    /// The app background.
    Background,
    /// `accent_soft` — a subtle accent wash.
    AccentSoft,
    /// `image_canvas` — media/image letterbox surface.
    ImageCanvas,
    /// `video_canvas` — pure-black video surface.
    VideoCanvas,
    /// No fill.
    Transparent,
}

/// Border style for the surface outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// No border (default).
    #[default]
    None,
    /// A subtle edge: a shade of the surface's own fill when it is opaque,
    /// else the `border` palette entry.
    Thin,
    /// Strong border (`border_strong`).
    Strong,
    /// Accent border (`accent`).
    Accent,
}

/// Semantic corner-radius size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadiusSize {
    /// 0 px.
    None,
    /// `radius-sm = 5 px`.
    Sm,
    /// `radius-md = 6 px` (default).
    #[default]
    Md,
    /// `radius-lg = 10 px`.
    Lg,
    /// `radius-xl = 12 px`.
    Xl,
}

impl RadiusSize {
    /// The compiled-default size in logical pixels.
    ///
    /// [`EnzSurface::radius_size`] resolves this rung from the live theme instead.
    pub fn px(self) -> f32 {
        match self {
            RadiusSize::None => 0.0,
            RadiusSize::Sm => 5.0,
            RadiusSize::Md => 6.0,
            RadiusSize::Lg => 10.0,
            RadiusSize::Xl => 12.0,
        }
    }

    /// Resolve against a runtime [`RadiusScale`](enzui_theme::RadiusScale)
    /// (the theme-driven path).
    ///
    /// `None -> 0`, `Sm -> sm`, `Md -> md`, `Lg -> lg`, `Xl -> xl`. Under the
    /// default scale `Lg` is 10 px, matching [`px`](Self::px).
    pub fn from_scale(self, scale: &enzui_theme::RadiusScale) -> f32 {
        match self {
            RadiusSize::None => scale.none,
            RadiusSize::Sm => scale.sm,
            RadiusSize::Md => scale.md,
            RadiusSize::Lg => scale.lg,
            RadiusSize::Xl => scale.xl,
        }
    }
}

/// Semantic border/line thickness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineSize {
    /// `line-thin = 1 px` (default when a border style is set).
    #[default]
    Thin,
    /// `line-regular = 1.5 px`.
    Regular,
    /// `line-thick = 2 px`.
    Thick,
    /// `line-heavy = 3 px`.
    Heavy,
}

impl LineSize {
    /// The px value for this line size.
    pub fn px(self) -> f32 {
        match self {
            LineSize::Thin => 1.0,
            LineSize::Regular => 1.5,
            LineSize::Thick => 2.0,
            LineSize::Heavy => 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colour tokens for the surface. Wired from `Palette`.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceTokens {
    /// Fill for [`SurfaceTone::Surface`] — the standard card surface.
    pub surface: Color,
    /// Fill for [`SurfaceTone::SurfaceAlt`] — the alternate/sunken surface.
    pub surface_alt: Color,
    /// Fill for [`SurfaceTone::Elevated`] — the raised surface.
    pub surface_elevated: Color,
    /// Fill for [`SurfaceTone::Selected`] — the selected-item surface.
    pub surface_selected: Color,
    /// Fill for [`SurfaceTone::Hover`] — the hover surface.
    pub surface_hover: Color,
    /// Fill for [`SurfaceTone::Background`] — the app background.
    pub background: Color,
    /// Fill for [`SurfaceTone::AccentSoft`] — the soft accent tint.
    pub accent_soft: Color,
    /// Fill for [`SurfaceTone::ImageCanvas`] — the image placeholder canvas.
    pub image_canvas: Color,
    /// Fill for [`SurfaceTone::VideoCanvas`] — the video placeholder canvas.
    pub video_canvas: Color,
    /// Default border colour (drawn when the surface is bordered).
    pub border: Color,
    /// Strong border colour (the heavier border variant).
    pub border_strong: Color,
    /// Accent colour used for accent-toned borders.
    pub accent: Color,
}

impl SurfaceTokens {
    /// Derive tokens from a runtime palette (the normal path).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            surface: p.surface,
            surface_alt: p.surface_alt,
            surface_elevated: p.surface_elevated,
            surface_selected: p.surface_selected,
            surface_hover: p.surface_hover,
            background: p.background,
            accent_soft: p.accent_soft,
            image_canvas: p.image_canvas,
            video_canvas: p.video_canvas,
            border: p.border,
            border_strong: p.border_strong,
            accent: p.accent,
        }
    }
    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
    /// Look up the fill for a tone. `Transparent` maps to a fully-transparent
    /// `Color::TRANSPARENT`.
    pub fn fill_for(&self, tone: SurfaceTone) -> Color {
        match tone {
            SurfaceTone::Surface => self.surface,
            SurfaceTone::SurfaceAlt => self.surface_alt,
            SurfaceTone::Elevated => self.surface_elevated,
            SurfaceTone::Selected => self.surface_selected,
            SurfaceTone::Hover => self.surface_hover,
            SurfaceTone::Background => self.background,
            SurfaceTone::AccentSoft => self.accent_soft,
            SurfaceTone::ImageCanvas => self.image_canvas,
            SurfaceTone::VideoCanvas => self.video_canvas,
            SurfaceTone::Transparent => Color::TRANSPARENT,
        }
    }
    /// Look up the border colour for a style.
    pub fn border_for(&self, style: BorderStyle) -> Color {
        match style {
            BorderStyle::None => Color::TRANSPARENT,
            BorderStyle::Thin => self.border,
            BorderStyle::Strong => self.border_strong,
            BorderStyle::Accent => self.accent,
        }
    }
}

// ---------------------------------------------------------------------------
// Defaults / sanitize (radius-md = 6)
// ---------------------------------------------------------------------------

/// Default border radius (`radius-md = 6 px`).
pub const DEFAULT_RADIUS: f32 = 6.0;

/// Clamp a geometry value (radius / border width) to a finite, non-negative
/// number, falling back to `default` for NaN / infinity / negatives.
fn sane_dim(v: f32, default: f32) -> f32 {
    // The policy lives in enzui_core::metrics as FiniteNonNeg so every
    // widget shares one implementation instead of re-deriving it.
    enzui_core::metrics::FiniteNonNeg::new_or(v, default).get()
}

/// Raise every side of `padding` to at least `minimum`, ignoring a non-finite
/// or negative minimum.
fn pad_at_least(padding: Padding, minimum: f32) -> Padding {
    let floor = if minimum.is_finite() && minimum > 0.0 {
        minimum
    } else {
        0.0
    };
    Padding {
        top: padding.top.max(floor),
        right: padding.right.max(floor),
        bottom: padding.bottom.max(floor),
        left: padding.left.max(floor),
    }
}

/// The length a material surface's child takes on one axis so that its box is
/// the surface's own: a declared length is filled, and only a surface that
/// hugs its content lets the content decide.
fn stretched(length: Length) -> Length {
    match length {
        Length::Shrink | Length::Fit => length,
        _ => Length::Fill,
    }
}

/// Sanitize a per-corner radius array (each corner independently).
fn sane_corners(c: [f32; 4]) -> [f32; 4] {
    [
        sane_dim(c[0], DEFAULT_RADIUS),
        sane_dim(c[1], DEFAULT_RADIUS),
        sane_dim(c[2], DEFAULT_RADIUS),
        sane_dim(c[3], DEFAULT_RADIUS),
    ]
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RadiusInput {
    Corners([f32; 4]),
    Size(RadiusSize),
}

impl RadiusInput {
    fn resolve(self, scale: impl FnOnce() -> enzui_theme::RadiusScale) -> [f32; 4] {
        match self {
            Self::Corners(c) => c,
            Self::Size(size) => [size.from_scale(&scale()); 4],
        }
    }
}

/// A themed surface primitive. Owns the tone/radius/border/elevation chrome;
/// the content laid out inside is entirely the caller's responsibility.
#[must_use]
pub struct EnzSurface<'a, Message> {
    content: Element<'a, Message>,
    tone: SurfaceTone,
    border_style: BorderStyle,
    radius: RadiusInput,
    radius_scale: Option<enzui_theme::RadiusScale>,
    radius_resolver: Option<ThemeResolver<Radius>>,
    border_width: f32,
    /// Whether the caller set the border width explicitly.
    ///
    /// Kept separate from `border_width` because the value `0` is a legitimate
    /// caller intent ("no border"), not a sentinel for "unset". Inferring
    /// explicitness from `border_width == 0.0` makes the builder
    /// order-dependent — see [`EnzSurface::border_style`].
    border_width_explicit: bool,
    padding: Padding,
    elevation: u8,
    clip: bool,
    width: Length,
    height: Length,
    tokens: Option<SurfaceTokens>,
    material: Option<MaterialStyle>,
    material_ink: MaterialInk,
    glass_ink: Option<enzui_theme::GlassInk>,
    material_policy: Option<MaterialPolicy>,
    material_tokens: Option<MaterialTokens>,
    material_tint_opacity: Option<f32>,
    grain: enzui_theme::GrainSpec,
    backdrop: crate::desktop_glass::Backdrop,
    window_frame: bool,
    fill_override: Option<Color>,
    border_color_override: Option<Color>,
    // Per-render resolvers: each wins over the static override and the token
    // lookup, re-evaluated against the runtime theme every frame — the escape
    // hatch for chrome that is a FUNCTION of the live theme (EnzCard's
    // tone-derived fill/border, its animated elevation shadow the
    // `elevation(level)` knob cannot express). Modelled on
    // `enz_text::EnzText::color_resolved`.
    fill_resolver: Option<ThemeResolver<Color>>,
    border_color_resolver: Option<ThemeResolver<Color>>,
    shadow_resolver: Option<ThemeResolver<Shadow>>,
}

impl<'a, Message: 'a> EnzSurface<'a, Message> {
    /// Override the semantic tone (default `Surface`).
    pub fn tone(mut self, tone: SurfaceTone) -> Self {
        self.tone = tone;
        self
    }

    /// Override the border style (default `None`). Setting a style with no
    /// explicit width supplies 1 px (`line-thin`).
    ///
    /// # Order independence
    ///
    /// Explicitness lives in its own field, never inferred from the value:
    /// `0` is expressible intent (no border), honoured literally — only
    /// non-finite/negative degrades. Value-inference made the builder
    /// order-dependent (`.border_width(0).border_style(Thin)` = 1 px,
    /// reversed = 0 px). Both orders now yield 0 px: the caller said `0`.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        if !matches!(style, BorderStyle::None) && !self.border_width_explicit {
            self.border_width = LineSize::Thin.px();
        }
        self
    }

    /// Pin the uniform corner radius in logical pixels.
    /// Non-finite or negative values fall back to the compiled default of 6 px.
    ///
    /// This is an explicit pixel pin: it wins over any
    /// [`Self::radius_scale`] forwarded later, since the caller has committed
    /// to a specific px.
    pub fn radius(mut self, radius: f32) -> Self {
        let r = sane_dim(radius, DEFAULT_RADIUS);
        self.radius = RadiusInput::Corners([r; 4]);
        self
    }

    /// Set the uniform corner radius from a semantic [`RadiusSize`].
    ///
    /// Resolves on each draw against the live theme. An explicit
    /// [`Self::radius_scale`] overrides the theme's ladder.
    pub fn radius_size(mut self, size: RadiusSize) -> Self {
        self.radius = RadiusInput::Size(size);
        self
    }

    /// Per-corner radius override `[top_left, top_right, bottom_left, bottom_right]`.
    /// Each corner is sanitized independently.
    ///
    /// This is an explicit pixel pin: it wins over any
    /// [`Self::radius_scale`] forwarded later.
    pub fn radius_corners(mut self, corners: [f32; 4]) -> Self {
        self.radius = RadiusInput::Corners(sane_corners(corners));
        self
    }

    /// Pin the ladder used by [`Self::radius_size`] instead of the live theme.
    ///
    /// Explicit pixel radii remain unchanged regardless of this ladder.
    pub fn radius_scale(mut self, scale: enzui_theme::RadiusScale) -> Self {
        self.radius_scale = Some(scale);
        self
    }

    /// Resolve corner radii once per draw, overriding the other radius setters.
    ///
    /// Chrome, material geometry, clipping, and pointer hits share the resolved
    /// shape. Non-finite or negative corners fall back to the compiled 6 px radius.
    pub fn radius_resolved(mut self, resolver: impl Fn(&iced::Theme) -> Radius + 'static) -> Self {
        self.radius_resolver = Some(Rc::new(resolver));
        self
    }

    fn resolved_radii(&self) -> [f32; 4] {
        self.radius
            .resolve(|| self.radius_scale.unwrap_or_default())
    }

    fn resolved_material(&self) -> Option<(MaterialStyle, MaterialTokens)> {
        self.material.map(|style| {
            let mut tokens = self.material_tokens.unwrap_or_else(|| style.tokens());
            if let Some(opacity) = self.material_tint_opacity.filter(|value| value.is_finite()) {
                tokens.tint_opacity = opacity.clamp(0.0, 1.0);
            }
            (style, tokens.validated())
        })
    }

    fn resolved_padding(&self) -> Padding {
        match self.resolved_material() {
            Some((_, tokens)) => {
                pad_at_least(self.padding, self.material_ink.required_inset(&tokens))
            }
            None => self.padding,
        }
    }

    /// The protected inset this surface guarantees its own content (logical
    /// px), or `None` with no material in force.
    ///
    /// For consumers laying out their own chrome (a compact toolbar checking
    /// bezel clearance) — never assume the default. [`Strong`](MaterialInk::Strong)
    /// answers `0.0`: the caller took responsibility; the surface stops
    /// guaranteeing an inset it no longer enforces.
    pub fn material_protected_inset(&self) -> Option<f32> {
        self.resolved_material()
            .map(|(_, tokens)| self.material_ink.required_inset(&tokens))
    }

    /// Override the border width (default 1 px when a style is set, 0 otherwise).
    ///
    /// Non-finite/negative values fall back to the field's own previous
    /// value's default (0). A width of `0` is honoured literally — it means
    /// "no border" — and marks the width explicit, so a later
    /// [`Self::border_style`] will not silently replace it.
    pub fn border_width(mut self, width: f32) -> Self {
        self.border_width = sane_dim(width, 0.0);
        self.border_width_explicit = true;
        self
    }

    /// Set the border width from a semantic [`LineSize`].
    ///
    /// Like [`Self::border_width`], this marks the width explicit.
    pub fn line_size(mut self, size: LineSize) -> Self {
        self.border_width = size.px();
        self.border_width_explicit = true;
        self
    }

    /// Inner padding.
    /// Set the surface width (default [`Length::Shrink`] — hug content). Use
    /// [`Length::Fill`] to participate as a fill-width sibling.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the surface height (default [`Length::Shrink`]).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Fill both axes ([`Length::Fill`] width and height).
    pub fn fill_size(mut self) -> Self {
        self.width = Length::Fill;
        self.height = Length::Fill;
        self
    }

    /// Fill the cross axis (width) only — the common "stretch to row width"
    /// case.
    pub fn fill_width(mut self) -> Self {
        self.width = Length::Fill;
        self
    }

    /// Set the inner padding. Defaults to zero padding.
    ///
    /// A surface with a [`Self::material`] grows padding smaller than that
    /// material's [`MaterialTokens::protected_inset`] up to it, so its content
    /// lands where contrast protection is at full strength.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Lay grain on the surface: a [`Grain`](enzui_theme::Grain) of its own,
    /// or [`GrainSpec::Theme`](enzui_theme::GrainSpec::Theme) for the theme's.
    ///
    /// The grain covers the whole surface, padding included, masked to its
    /// corners; it lies under the content unless the grain's placement is
    /// [`Over`](enzui_theme::GrainPlacement::Over). Works with materials:
    /// the grain lies on the glass. See [`crate::grain`].
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Ask for a sampled, filtered backdrop instead of a flat fill.
    ///
    /// Radius, border, elevation and child layout unchanged. When the renderer
    /// cannot sample, the platform reduces transparency / raises contrast, the
    /// style is [`Solid`](MaterialStyle::Solid), or the surface nests inside
    /// another material: paints
    /// [`surface_color`](MaterialStyle::surface_color) opaquely instead of
    /// reporting an unfelt filter. An explicit [`fill`](Self::fill) /
    /// [`fill_resolved`](Self::fill_resolved) becomes that fallback fill.
    /// Padding rises to the protected inset (see the [module docs](self)).
    pub fn material(mut self, style: MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// State what the material owes the ink its own content paints.
    ///
    /// [`Protected`](MaterialInk::Protected) (default) grows padding to keep
    /// content inside the inset. [`Strong`](MaterialInk::Strong) is the
    /// explicit opt-out for compact icon-only controls (solid-shape glyphs):
    /// padding may drop below the inset, and the surface stops claiming one.
    /// Use Protected for text on optical styles; FlatFrosted has no lens or
    /// required inset.
    pub fn material_ink(mut self, ink: MaterialInk) -> Self {
        self.material_ink = ink;
        self
    }

    /// Override material foreground adaptation without changing optical tokens.
    /// Defaults to the material style's ink policy. This is independent of
    /// [`Self::material_ink`], which controls protected content clearance.
    /// `GlassInk::Theme` adds a neutral wash unless the fill is explicitly pinned.
    /// Has no effect without [`Self::material`].
    pub fn glass_ink(mut self, ink: enzui_theme::GlassInk) -> Self {
        self.glass_ink = Some(ink);
        self
    }

    /// Override the accessibility policy this material obeys.
    ///
    /// Without it the material reads the live
    /// [`material_policy`](crate::theme_lookup::material_policy) (theme motion
    /// policy + desktop answers + runtime user override). The override is for
    /// a caller already holding the preference, or a test avoiding the
    /// process-wide store; it publishes nothing — this surface only.
    pub fn material_policy(mut self, policy: MaterialPolicy) -> Self {
        self.material_policy = Some(policy);
        self
    }

    /// Choose what the material samples. [`Backdrop::Window`](crate::desktop_glass::Backdrop::Window)
    /// shows the desktop and the windows behind this one, painted by the
    /// compositor; it needs an [`enz_desktop_glass`](crate::desktop_glass::enz_desktop_glass)
    /// around the view and keeps in-app glass where the compositor has none.
    /// Has no effect without [`Self::material`].
    pub fn backdrop(mut self, backdrop: crate::desktop_glass::Backdrop) -> Self {
        self.backdrop = backdrop;
        self
    }

    /// A window's frame (the app shell's backdrop layer): with
    /// [`Backdrop::Window`](crate::desktop_glass::Backdrop::Window) glass,
    /// opaque wherever the compositor does not paint it, and its content
    /// keeps its own look on the glass.
    pub(crate) fn window_frame(mut self, frame: bool) -> Self {
        self.window_frame = frame;
        self
    }

    /// Override the material's tokens, keeping its semantic style.
    ///
    /// Values are clamped to their documented ranges, so a theme cannot ask
    /// for an unrenderable material. Has no effect without [`Self::material`].
    pub fn material_tokens(mut self, tokens: MaterialTokens) -> Self {
        self.material_tokens = Some(tokens);
        self
    }

    /// Sets local material tint strength, `0..=1`, overriding the token field.
    ///
    /// Values clamp to the range; non-finite input leaves the tokens unchanged.
    /// At one the existing opaque fallback is used. Has no effect without a material.
    pub fn material_tint_opacity(mut self, opacity: f32) -> Self {
        self.material_tint_opacity = Some(opacity);
        self
    }

    /// Elevation level 0-5 (drop shadow). Routed through the shared
    /// [`crate::elevation::iced_shadow_for_theme`] policy so Surface, Card and
    /// Elevation agree, and all three follow the runtime theme's
    /// [`enzui_theme::ElevationScale`].
    /// Level 0 (default) adds no explicit shadow. Values > 5 clamp to 5.
    /// Positive bevels with adaptive dark ink can add a silhouette shadow.
    pub fn elevation(mut self, level: u8) -> Self {
        self.elevation = level.min(5);
        self
    }

    /// Whether to clip content to the surface. Defaults to `true`.
    ///
    /// # Clipping follows the corner radius
    ///
    /// The child masks to the surface's ROUNDED path, not its bounding box:
    /// full-bleed children carve to the corner arc, never the transparent
    /// cutouts.
    ///
    /// | | masked by `clip(true)` |
    /// |---|---|
    /// | child overflowing the straight top/bottom/left/right edges | yes |
    /// | child inside the rounded-corner cutouts | yes |
    ///
    /// Measured, not assumed: 48 px radius over a full-bleed child leaves
    /// 0 px in the cutouts vs 1628 px for a rectangular clip, with
    /// straight-edge overflow still contained (0 px escaped) — identical on
    /// both backends (`tests/card_rounded_clip_masks_corners.rs`).
    ///
    /// # How
    ///
    /// The child draws inside a [`RoundedClip`](crate::rounded_clip::RoundedClip)
    /// (renderer layer masked to the arc — `with_layer_rounded`, an enzICED
    /// fork entry point). `Container::clip` alone cannot: it intersects two
    /// `Rectangle`s (radius never enters the clip path).
    ///
    /// # Residual gap
    ///
    /// Rounded masking covers quads, meshes and images on both backends; on
    /// wgpu TEXT keeps the rectangular clip (glyphs render via the external
    /// `cryoglyph` crate, outside this tree). Glyphs essentially never reach a
    /// cutout: documented, not worked around. `clip(false)` masks nothing.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Override the resolved fill colour directly, bypassing the tone
    /// lookup. Wins over `.tone()`.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill_override = Some(color);
        self
    }

    /// Override the resolved border colour directly, bypassing the
    /// border-style lookup. Wins over `.border_style()` colour.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color_override = Some(color);
        self
    }

    /// Override the colour tokens (pins them; skips runtime-theme resolution).
    pub fn tokens(mut self, tokens: SurfaceTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Resolve the fill colour from the runtime `iced::Theme` on every
    /// render via `resolver`.
    ///
    /// For widget-owned fill tokens `surface.rs` cannot know (`CardTokens`'
    /// tone-derived fill + per-field pins + `transparent` override) that must
    /// still re-tint in lock-step on a theme switch. The surface-side twin of
    /// [`color_resolved`](crate::enz_text::EnzText::color_resolved). Wins over
    /// [`fill`](Self::fill), [`tone`](Self::tone), [`tokens`](Self::tokens):
    /// the installing widget owns this surface's fill outright.
    pub fn fill_resolved(mut self, resolver: impl Fn(&iced::Theme) -> Color + 'static) -> Self {
        self.fill_resolver = Some(Rc::new(resolver));
        self
    }

    /// Resolve the border colour from the runtime `iced::Theme` on every
    /// render via `resolver`. The border-colour twin of
    /// [`Self::fill_resolved`]; wins over [`Self::border_color`] and the
    /// border-style token lookup.
    pub fn border_color_resolved(
        mut self,
        resolver: impl Fn(&iced::Theme) -> Color + 'static,
    ) -> Self {
        self.border_color_resolver = Some(Rc::new(resolver));
        self
    }

    /// Resolve the drop shadow from the runtime `iced::Theme` on every
    /// render via `resolver`.
    ///
    /// Wins over the [`elevation`](Self::elevation) ladder. For shadows that
    /// are a FUNCTION of live state, not a level — EnzCard's
    /// elevation-transition animation (advances every frame between levels,
    /// snaps under reduced-motion). The resolver still receives the runtime
    /// theme, so it can read the elevation ladder itself.
    pub fn shadow_resolved(mut self, resolver: impl Fn(&iced::Theme) -> Shadow + 'static) -> Self {
        self.shadow_resolver = Some(Rc::new(resolver));
        self
    }

    /// Convert into an iced [`Element`].
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.resolved_material();
        let material_policy = self.material_policy;
        let glass_ink = self.glass_ink;
        let backdrop = self.backdrop;
        let window_frame = self.window_frame;
        let padding = self.resolved_padding();
        let pinned = self.tokens;
        let tone = self.tone;
        let border_style = self.border_style;
        let border_width = sane_dim(self.border_width, 0.0);
        let radius = sane_corners(self.resolved_radii());
        let radius_input = self.radius;
        let radius_scale = self.radius_scale;
        let radius_resolver = self.radius_resolver;
        let elevation = self.elevation;
        let fill_override = self.fill_override;
        let border_color_override = self.border_color_override;
        let fill_resolver = self.fill_resolver;
        let border_color_resolver = self.border_color_resolver;
        // One shadow source for both painters: the container paints it around
        // a flat surface, the material boundary after its backdrop is taken.
        let shadow_of: ThemeResolver<Shadow> = match self.shadow_resolver {
            Some(resolver) => resolver,
            None => Rc::new(move |theme: &iced::Theme| {
                crate::elevation::iced_shadow_for_theme(theme, elevation)
            }),
        };

        let border_radius = Radius {
            top_left: radius[0],
            top_right: radius[1],
            bottom_left: radius[2],
            bottom_right: radius[3],
        };
        let drawn_radius = Rc::new(Cell::new(border_radius));
        // The per-draw shape, sampled once by the chrome's style; the rounded
        // clip and the material region read what it stored.
        let shape_of: ThemeResolver<Radius> = Rc::new(move |theme: &iced::Theme| {
            let radius = match &radius_resolver {
                Some(resolve) => {
                    let r = resolve(theme);
                    sane_corners([r.top_left, r.top_right, r.bottom_left, r.bottom_right])
                }
                None => sane_corners(radius_input.resolve(|| {
                    radius_scale.unwrap_or_else(|| crate::theme_lookup::radius(theme))
                })),
            };
            Radius {
                top_left: radius[0],
                top_right: radius[1],
                bottom_left: radius[2],
                bottom_right: radius[3],
            }
        });

        // A material region is the box this surface declares, not the box its
        // content happens to hug. The boundary and the rounded clip below both
        // take their geometry from this child, so a content that shrinks would
        // otherwise paint a panel smaller than the surface, with the
        // container's own chrome already suppressed underneath it.
        let grain = self.grain;
        let grainy = grain != enzui_theme::GrainSpec::None;
        let content: Element<'a, Message> = if material.is_some() || grainy {
            container(self.content)
                .width(stretched(self.width))
                .height(stretched(self.height))
                .into()
        } else {
            self.content
        };

        // Mask the child to the ROUNDED outline. `Container::clip(true)` below
        // is rectangular (intersects two `Rectangle`s) and sets the child's
        // viewport — alone, the chrome would round while full-bleed content
        // spilled through the corner arc. `RoundedClip` records the child in an
        // arc-masked renderer layer (`Renderer::with_layer_rounded`), with our
        // padding as `outset` so the mask reproduces this container's bounds.
        // It wraps the CHILD, never the container: a layer clips everything
        // recorded in it, and wrapping the container would clip away the drop
        // shadow painting outside the bounds.
        // Grain lies on the fill under the content (or over it), across the
        // padding too; inside the rounded clip below, it follows the corners.
        let content: Element<'a, Message> = if grainy {
            let layer = crate::grain::GrainLayer::new(content, grain).outset(padding);
            if self.clip {
                layer.into()
            } else {
                layer.radius(border_radius).into()
            }
        } else {
            content
        };

        let content: Element<'a, Message> = if self.clip {
            let drawn_radius = Rc::clone(&drawn_radius);
            crate::rounded_clip::rounded_clip(content, border_radius)
                .radius_resolved(move |_| drawn_radius.get())
                .outset(padding)
                .into()
        } else {
            content
        };

        // A material surface's panel chrome is painted by the boundary, not by
        // the container around it. The container paints *before* the boundary,
        // so its chrome quad would land in the material's own backdrop and the
        // glass would sample its own fill. The shadow moves for the same
        // reason: the boundary paints it after the backdrop is taken, or the
        // blur would pull it across the rim.
        let chrome_fill: Rc<dyn Fn(&iced::Theme) -> Color> = {
            let fill_resolver = fill_resolver.clone();

            Rc::new(move |theme: &iced::Theme| {
                let tokens = pinned.unwrap_or_else(|| {
                    SurfaceTokens::from_palette(&crate::theme_lookup::palette(theme))
                });

                match (&fill_resolver, fill_override, material) {
                    (Some(resolver), _, _) => resolver(theme),
                    (None, Some(fill), _) => fill,
                    // A material's opaque fallback is its own style colour, not
                    // the tone's flat fill: falling back must look like the
                    // panel it replaces.
                    (None, None, Some((style, _))) => {
                        style.surface_color(&crate::theme_lookup::palette(theme))
                    }
                    (None, None, None) => tokens.fill_for(tone),
                }
            })
        };
        let chrome_border: Rc<dyn Fn(&iced::Theme) -> Color> = {
            let border_color_resolver = border_color_resolver.clone();
            let chrome_fill = Rc::clone(&chrome_fill);

            Rc::new(move |theme: &iced::Theme| {
                let tokens = pinned.unwrap_or_else(|| {
                    SurfaceTokens::from_palette(&crate::theme_lookup::palette(theme))
                });

                match (&border_color_resolver, border_color_override) {
                    (Some(resolver), _) => resolver(theme),
                    (None, Some(color)) => color,
                    // `Thin` on an opaque fill is a shade of that fill, not a
                    // line in another colour. Over a see-through fill
                    // there is nothing to shade, so it stays the `border` line.
                    (None, None) if border_style == BorderStyle::Thin && pinned.is_none() => {
                        let fill = chrome_fill(theme);
                        if fill.a >= 0.99 {
                            crate::surface_edge::edge_color(
                                fill,
                                crate::surface_edge::DEFAULT_EDGE_DEPTH,
                            )
                        } else {
                            tokens.border
                        }
                    }
                    (None, None) => tokens.border_for(border_style),
                }
            })
        };

        let content: Element<'a, Message> = match material {
            Some((style, tokens)) => crate::material_boundary::MaterialBoundary::new(
                content,
                tokens,
                Rc::clone(&drawn_radius),
                crate::material_boundary::Chrome {
                    fill: Rc::clone(&chrome_fill),
                    border_color: Rc::clone(&chrome_border),
                    border_width,
                    shadow: Rc::clone(&shadow_of),
                    fill_pinned: fill_override.is_some() || fill_resolver.is_some(),
                },
            )
            .policy(material_policy)
            .backdrop(backdrop)
            .window_frame(window_frame)
            .outset(padding)
            .style(style)
            .ink(glass_ink)
            .into(),
            None => content,
        };

        container(content)
            .padding(padding)
            .width(self.width)
            .height(self.height)
            // Rectangular half of the clip contract (clip defaults true):
            // stops overflow at the straight edges. The rounded half is
            // the `RoundedClip` above.
            .clip(self.clip)
            // Fill/border follow the runtime theme: unpinned tokens
            // resolve per-render from `theme_lookup::palette(theme)`; pinned
            // tokens keep their fixed colours. Explicit `.fill()`/`.border_color()`
            // win over the token lookup.
            .style(move |theme: &iced::Theme| {
                let border_radius = shape_of(theme);
                // Container styles run before child drawing; masks and materials
                // share this frame's shape, even for a stateful resolver.
                drawn_radius.set(border_radius);
                // A resolver wins over the static override, which wins over the
                // token lookup; with a material, the boundary inside paints
                // both fill and border instead, and only the shadow is left
                // here.
                let (fill, border_color, width) = if material.is_some() {
                    (Color::TRANSPARENT, Color::TRANSPARENT, 0.0)
                } else {
                    (chrome_fill(theme), chrome_border(theme), border_width)
                };
                // `container::Style::shadow` is produced in this draw-time
                // closure, so the elevation ladder resolves from the runtime
                // theme rather than the compile-time table. A
                // `shadow_resolved` closure wins over the level lookup.
                //
                // A material's shadow is painted by its boundary instead:
                // painted here, before the backdrop is sampled, the glass
                // would blur its own shadow into its rim.
                let shadow = if material.is_some() {
                    Shadow::default()
                } else {
                    shadow_of(theme)
                };
                container::Style {
                    background: Some(Background::Color(fill)),
                    border: Border {
                        color: border_color,
                        width,
                        radius: border_radius,
                    },
                    shadow,
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a, Message: 'a> From<EnzSurface<'a, Message>> for Element<'a, Message> {
    fn from(s: EnzSurface<'a, Message>) -> Self {
        s.into_element()
    }
}

/// Construct a themed surface wrapping `content` at the live `radius.md` rung.
///
/// Clips content by default, following the corner
/// radius: both the straight edges and the rounded-corner cutouts are masked.
/// See [`EnzSurface::clip`] for the measurements and the residual gap.
pub fn enz_surface<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> EnzSurface<'a, Message> {
    EnzSurface {
        content: content.into(),
        tone: SurfaceTone::default(),
        border_style: BorderStyle::default(),
        radius: RadiusInput::Size(RadiusSize::Md),
        radius_scale: None,
        radius_resolver: None,
        border_width: 0.0,
        border_width_explicit: false,
        padding: Padding::from(0),
        elevation: 0,
        clip: true,
        width: Length::Shrink,
        height: Length::Shrink,
        tokens: None,
        material: None,
        material_ink: MaterialInk::Protected,
        glass_ink: None,
        material_policy: None,
        material_tokens: None,
        material_tint_opacity: None,
        grain: enzui_theme::GrainSpec::None,
        backdrop: crate::desktop_glass::Backdrop::App,
        window_frame: false,
        fill_override: None,
        border_color_override: None,
        fill_resolver: None,
        border_color_resolver: None,
        shadow_resolver: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
