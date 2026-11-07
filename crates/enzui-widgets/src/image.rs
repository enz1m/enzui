//! EnzImage: raster image with fit, radius, and placeholder background.
//!
//! Wraps `iced::widget::image` inside a container that supplies the
//! placeholder background so an image that hasn't loaded (or where the
//! source is a bad path) still occupies its slot with the tokenized
//! surface-alt fill.
//!
//! The corner radius has one backend-independent mask for loaded pixels: a
//! nonzero radius wraps the native image in [`crate::rounded_clip::RoundedClip`].
//! The image's own `border_radius` stays unset, avoiding doubled antialias
//! coverage on wgpu while tiny-skia receives the shared rounded layer it can
//! honor. The placeholder background carries the same radius independently,
//! so unloaded images and transparent image corners remain rounded. A zero
//! radius keeps the image on the direct, layer-free path.
//!
//! # Fit modes
//!
//! | enum | iced ContentFit |
//! |---|---|
//! | `EnzImageFit::Cover` | `Cover` (default) |
//! | `EnzImageFit::Contain` | `Contain` |
//! | `EnzImageFit::Fill` | `Fill` |
//! | `EnzImageFit::None` | `None` |
//!
//! An already-decoded source (raw RGBA, encoded bytes, a cached handle) goes
//! through [`enz_image_handle`] instead of a path.
//!
//! # Accessibility
//!
//! A named image (`.alt_text("…")`) publishes a single
//! [`Role::Image`](enzui_core::semantics::Role::Image) leaf carrying the
//! alt text as its accessible name, mirroring `avatar.rs`. An image with
//! empty alt text is treated as decorative and publishes no node at all
//! — a screen reader announcing "image" on every spacer glyph is worse than
//! silence, so empty intent is honoured literally.
//!
//! # Deferred
//!
//! - `EnzAsyncImage` — separate port. Depends on a shared async
//!   image lifecycle (loading / progress / failed) plus a skeleton
//!   primitive, so it needs its own scaffolding.

use iced::widget::{container, image as iced_image};
use iced::{Border, Color, ContentFit, Element, Length};

/// Where the pixels come from. Either a file path resolved into a
/// [`iced_image::Handle`] at build time, or an already-resolved handle
/// (decoded bytes / raw RGBA / a cached handle) supplied by the caller.
///
/// The path variant stays for ergonomics (`enz_image("/hero.png")`); the
/// handle variant lets callers compose with resolved sources — raw RGBA,
/// encoded bytes, generated pixmaps — without round-tripping through the
/// filesystem, which is also what a headless render test needs (path decode
/// is asynchronous and does not surface in a single-frame screenshot).
#[derive(Debug, Clone)]
enum ImageSource {
    Path(String),
    Handle(iced_image::Handle),
}

impl ImageSource {
    /// Resolve to an iced image handle. A path is turned into a
    /// [`iced_image::Handle::from_path`]; a handle is cloned as-is.
    fn to_handle(&self) -> iced_image::Handle {
        match self {
            ImageSource::Path(p) => iced_image::Handle::from_path(p),
            ImageSource::Handle(h) => h.clone(),
        }
    }

    /// The source path, if this image was built from one (`""` for a raw
    /// handle source — a handle carries no path).
    fn path_str(&self) -> &str {
        match self {
            ImageSource::Path(p) => p,
            ImageSource::Handle(_) => "",
        }
    }
}

/// How the image is scaled within its slot (maps to iced [`ContentFit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzImageFit {
    /// Fill the slot, cropping overflow to preserve aspect ratio (the default).
    #[default]
    Cover,
    /// Fit entirely inside the slot, letterboxing to preserve aspect ratio.
    Contain,
    /// Stretch to fill the slot exactly, ignoring aspect ratio.
    Fill,
    /// Draw at native size with no scaling.
    None,
}

impl EnzImageFit {
    /// The equivalent iced [`ContentFit`].
    pub fn to_iced(self) -> ContentFit {
        match self {
            EnzImageFit::Cover => ContentFit::Cover,
            EnzImageFit::Contain => ContentFit::Contain,
            EnzImageFit::Fill => ContentFit::Fill,
            EnzImageFit::None => ContentFit::None,
        }
    }
}

/// Builder for a raster image with fit, radius and placeholder. See
/// [`enz_image`].
#[must_use]
pub struct EnzImage {
    source: ImageSource,
    fit: EnzImageFit,
    radius: f32,
    alt_text: String,
    placeholder_background: Option<Color>,
    hug_picture: bool,
    width: Length,
    height: Length,
}

/// Construct an image from a file-path source. Defaults: [`EnzImageFit::Cover`],
/// 0 radius, fills its slot, theme-resolved placeholder background.
pub fn enz_image(source: impl Into<String>) -> EnzImage {
    EnzImage {
        source: ImageSource::Path(source.into()),
        fit: EnzImageFit::Cover,
        radius: 0.0,
        alt_text: String::new(),
        placeholder_background: None,
        hug_picture: false,
        width: Length::Fill,
        height: Length::Fill,
    }
}

/// Construct an image from an already-resolved [`iced_image::Handle`] — raw
/// RGBA, decoded bytes, a generated pixmap, or a cached handle. Same defaults
/// as [`enz_image`]. Prefer this when composing with an already decoded source, or when
/// the pixels must be present synchronously (path
/// decode is asynchronous).
pub fn enz_image_handle(handle: impl Into<iced_image::Handle>) -> EnzImage {
    EnzImage {
        source: ImageSource::Handle(handle.into()),
        fit: EnzImageFit::Cover,
        radius: 0.0,
        alt_text: String::new(),
        placeholder_background: None,
        hug_picture: false,
        width: Length::Fill,
        height: Length::Fill,
    }
}

impl EnzImage {
    /// Set the fit mode. Defaults to [`EnzImageFit::Cover`].
    pub fn fit(mut self, f: EnzImageFit) -> Self {
        self.fit = f;
        self
    }
    /// Corner radius in px. A nonzero radius masks loaded pixels through the
    /// backend-independent [`crate::rounded_clip::RoundedClip`] and rounds the
    /// placeholder background behind them. The native image radius remains
    /// unset so loaded pixels receive exactly one mask. Non-finite or negative
    /// input falls back to 0. Defaults to 0 (with no rounded layer).
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = enzui_core::metrics::FiniteNonNeg::new_or(r, 0.0).get();
        self
    }
    /// Accessibility alt text. A non-empty value publishes a
    /// [`Role::Image`](enzui_core::semantics::Role::Image) leaf carrying
    /// this text as the accessible name; an empty value marks the image as
    /// decorative and publishes no node (see [`Self::semantics`] and the module
    /// docs). Empty by default.
    pub fn alt_text(mut self, t: impl Into<String>) -> Self {
        self.alt_text = t.into();
        self
    }
    /// Treat the slot as a maximum: the picture is laid out at its own
    /// aspect ratio, as large as fits, and centred, so the corner radius
    /// follows the picture and the rest of the slot stays clear. No
    /// placeholder ground is painted, so a transparent picture shows what is
    /// behind it; [`Self::placeholder_background`] still sets one. For a thumbnail beside others of different shapes, where a
    /// letterbox band around each one would read as a frame. Without it a
    /// `Contain` image is letterboxed inside a slot-sized rounded backdrop.
    pub fn hug_picture(mut self) -> Self {
        self.hug_picture = true;
        self
    }

    /// Whether the slot is only a maximum; see [`Self::hug_picture`].
    pub fn hug_picture_value(&self) -> bool {
        self.hug_picture
    }

    /// Override the placeholder fill shown before the image loads or on a bad
    /// path. Unset by default (resolved to `surface_alt` from the theme).
    pub fn placeholder_background(mut self, c: Color) -> Self {
        self.placeholder_background = Some(c);
        self
    }
    /// Set the slot width. Defaults to [`Length::Fill`].
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }
    /// Set the slot height. Defaults to [`Length::Fill`].
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// The image source path (empty string when the image was built from a
    /// raw [`iced_image::Handle`] via [`enz_image_handle`]).
    pub fn source(&self) -> &str {
        self.source.path_str()
    }
    /// The stored alt text (see [`Self::alt_text`]).
    pub fn alt_text_value(&self) -> &str {
        &self.alt_text
    }
    /// The resolved corner radius in px.
    pub fn radius_value(&self) -> f32 {
        self.radius
    }
    /// The configured fit mode.
    pub fn fit_value(&self) -> EnzImageFit {
        self.fit
    }

    /// The accessibility node the built element publishes as a leaf, or `None`
    /// when the image is decorative. Single source of truth for the
    /// a11y leaf wrapped in [`Self::into_element`].
    ///
    /// A named image (non-empty [`alt_text`](Self::alt_text)) announces as a
    /// [`Role::Image`](enzui_core::semantics::Role::Image) carrying the alt
    /// text as its accessible name, matching `avatar.rs`. An image whose alt
    /// text is empty (the default) is decorative and returns `None`, so it is
    /// absent from the AT tree rather than announcing as an unnamed "image" —
    /// the same expressible-intent rule as the rest of the crate.
    pub fn semantics(&self) -> Option<enzui_core::semantics::Semantics> {
        use enzui_core::semantics::{Role, Semantics};
        let alt = self.alt_text.trim();
        if alt.is_empty() {
            return None;
        }
        Some(Semantics::new(Role::Image).label(alt))
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        // the placeholder ground follows the live theme; a
        // caller-set colour still wins.
        let bg_pin = self.placeholder_background;
        let radius = self.radius;

        // Wrap the iced image in a container that owns the rounded placeholder
        // background. `Container::clip(true)` remains the rectangular scissor
        // for straight-edge overflow.
        //
        // loaded pixels need a real rounded mask, not only rounded
        // chrome behind them. Use the shared rounded layer because it reaches
        // images on both wgpu and tiny-skia. Do not also set the native image's
        // `border_radius`: wgpu would multiply the two masks' edge coverage,
        // damaging the antialias fringe, while tiny-skia ignores that native
        // radius. Keep radius zero on the direct path so the default does not
        // allocate a needless render layer.
        // Hugging the picture, the image itself shrinks to its fitted size
        // inside the slot's limits; filling, a scaled-down picture's node
        // would still span the whole slot.
        let fill = if self.hug_picture {
            Length::Shrink
        } else {
            Length::Fill
        };
        let img: Element<'a, Message> = iced_image(self.source.to_handle())
            .content_fit(self.fit.to_iced())
            .width(fill)
            .height(fill)
            .into();
        let img: Element<'a, Message> = if radius > 0.0 {
            crate::rounded_clip::rounded_clip(img, iced::border::Radius::new(radius)).into()
        } else {
            img
        };

        // Single source of truth for the a11y leaf (see `semantics()`): a
        // named image publishes a Role::Image node; a decorative image (empty
        // alt) publishes nothing.
        let a11y_semantics = self.semantics();

        // Hugging the picture, the frame shrinks to the image's fitted size
        // inside the slot's limits, so the radius and the placeholder wrap
        // the picture itself.
        let (width, height) = if self.hug_picture {
            (Length::Shrink, Length::Shrink)
        } else {
            (self.width, self.height)
        };
        // A hugging frame is the picture itself, so a transparent picture
        // shows what is behind it: no ground unless the caller pinned one.
        let hug = self.hug_picture;
        let placeholder: Element<'a, Message> = container(img)
            .width(width)
            .height(height)
            .clip(true)
            .style(move |theme: &iced::Theme| container::Style {
                background: match bg_pin {
                    Some(color) => Some(color.into()),
                    None if hug => None,
                    None => Some(crate::theme_lookup::palette(theme).surface_alt.into()),
                },
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into();

        // The picture-sized frame, centred in the slot.
        let placeholder: Element<'a, Message> = if self.hug_picture {
            container(placeholder)
                .width(self.width)
                .height(self.height)
                .center_x(self.width)
                .center_y(self.height)
                .into()
        } else {
            placeholder
        };

        match a11y_semantics {
            Some(sem) => {
                crate::semantics_container::SemanticsContainer::leaf(placeholder, sem).into()
            }
            None => placeholder,
        }
    }
}

impl<'a, Message: 'a> From<EnzImage> for Element<'a, Message> {
    fn from(w: EnzImage) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
