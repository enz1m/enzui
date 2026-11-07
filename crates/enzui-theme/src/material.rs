//! Semantic tokens for sampled-backdrop glass and flat tinted blur.
//!
//! [`MaterialStyle`] names the intent a caller has; [`MaterialTokens`] is the
//! validated numeric contract a renderer consumes. Every length is in
//! logical pixels and every ratio is unitless, so one token set describes
//! the material at any scale factor.
//!
//! Styles provide presets for blur, refraction, tint and ink selection.
//! [`MaterialPolicy`] composes system preferences and application overrides
//! per dimension. The material boundary applies that policy; converting tokens
//! with [`MaterialTokens::to_material`] alone does not enforce it.
//!
//! Adaptive ink, an opt-in, follows measured backdrop brightness. Mixed light and dark
//! content can still reduce legibility. Contrast protection adjusts backdrop
//! luminance, but is not a contrast guarantee; later lighting also affects it.
//! [`MaterialTokens::protected_inset`] is content-layout metadata, not a shader
//! mask. Foreground content is painted after the backdrop filter.
//!
//! # Example
//!
//! ```
//! use enzui_theme::material::{MaterialStyle, MaterialTokens};
//!
//! let tokens: MaterialTokens = MaterialStyle::Regular.tokens();
//! assert!(tokens.is_valid());
//! assert_eq!(tokens.protected_inset, MaterialTokens::PROTECTED_INSET);
//! ```

use crate::Palette;
use iced_core::Color;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A semantic material treatment for a sampled application backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaterialStyle {
    /// The default: clear glass with a bending rim, the library's defaults,
    /// with the theme's ink.
    #[default]
    Regular,
    /// Media glass: light ink over glass that dims bright content.
    Clear,
    /// Strong refraction and fringe on a deep rim, for hero elements.
    Lens,
    /// A light frost, slightly darker, for controls over busy content.
    Subtle,
    /// Frosted and darker with maximum luminance roll-off, for text.
    Frosted,
    /// Diagnostic geometry with zero tint, configurable highlight, diffusion
    /// and contrast-protection tokens.
    Optical,
    /// Flat theme-tinted blur with protected theme ink, no lens or rim lighting.
    /// Uses a 45% tint and no protected inset because sampling is undistorted.
    FlatFrosted,
    /// The explicit opaque style; no sampled backdrop is requested.
    Solid,
}

impl MaterialStyle {
    /// Every style, in presentation order.
    pub const ALL: [Self; 8] = [
        Self::Regular,
        Self::Clear,
        Self::Lens,
        Self::Subtle,
        Self::Frosted,
        Self::Optical,
        Self::FlatFrosted,
        Self::Solid,
    ];

    /// Whose ink sits on glass of this style: the theme's, so the glass keeps
    /// the app's text and fill whatever passes beneath, except media
    /// [`Clear`](Self::Clear) glass, always light. Following the backdrop is
    /// the [`GlassInk::Adaptive`] opt-in.
    pub fn ink(self) -> GlassInk {
        match self {
            Self::Clear => GlassInk::Light,
            Self::Regular
            | Self::Subtle
            | Self::Lens
            | Self::Frosted
            | Self::Optical
            | Self::FlatFrosted
            | Self::Solid => GlassInk::Theme,
        }
    }

    /// The validated token set for this style.
    pub fn tokens(self) -> MaterialTokens {
        #[cfg(feature = "material-tuning")]
        if let Some(tokens) = tuning::get(self) {
            return tokens;
        }
        self.builtin_tokens()
    }

    /// The shipped token set, ignoring any tuning override.
    pub fn builtin_tokens(self) -> MaterialTokens {
        // Presets derive from ybouane/liquidglass (MIT).
        let base = MaterialTokens {
            blur_sigma: 0.0,
            refraction: 0.69,
            bevel_width: 40.0,
            index_of_refraction: 1.5,
            dispersion: 0.05,
            color_diffusion: 0.0,
            color_reach: 24.0,
            optical_depth: 40.0,
            tint_opacity: 0.0,
            saturation: 1.0,
            highlight: 0.05,
            brightness: 0.0,
            light_angle: 250.0,
            profile_curvature: 2.0,
            contrast_protection: 0.0,
            protected_inset: MaterialTokens::PROTECTED_INSET,
        };
        let adaptive = MaterialTokens {
            contrast_protection: 0.85,
            ..base
        };
        match self {
            Self::Regular => adaptive,
            Self::Clear => MaterialTokens {
                dispersion: 0.0,
                highlight: 0.0,
                contrast_protection: 0.85,
                ..base
            },
            Self::Lens => MaterialTokens {
                blur_sigma: 3.3,
                refraction: 1.2,
                bevel_width: 60.0,
                dispersion: 0.2,
                brightness: -0.2,
                ..adaptive
            },
            Self::Subtle => MaterialTokens {
                blur_sigma: 3.3,
                brightness: -0.1,
                ..adaptive
            },
            Self::Frosted => MaterialTokens {
                blur_sigma: 5.5,
                brightness: -0.3,
                contrast_protection: 1.0,
                ..base
            },
            Self::Optical => MaterialTokens {
                blur_sigma: 0.0,
                refraction: 0.8,
                bevel_width: 32.0,
                dispersion: 0.0,
                color_diffusion: 0.0,
                color_reach: 32.0,
                tint_opacity: 0.0,
                saturation: 1.0,
                highlight: 0.0,
                brightness: 0.0,
                contrast_protection: 0.0,
                ..base
            },
            Self::FlatFrosted => MaterialTokens {
                blur_sigma: 5.5,
                refraction: 0.0,
                dispersion: 0.0,
                optical_depth: 0.0,
                bevel_width: 0.0,
                highlight: 0.0,
                brightness: 0.0,
                tint_opacity: 0.45,
                contrast_protection: 1.0,
                protected_inset: 0.0,
                ..base
            },
            Self::Solid => MaterialTokens {
                blur_sigma: 0.0,
                refraction: 0.0,
                dispersion: 0.0,
                color_diffusion: 0.0,
                tint_opacity: 1.0,
                highlight: 0.2,
                ..base
            },
        }
    }

    /// The palette colour this style paints.
    ///
    /// It is the tint the optical path mixes toward under
    /// [`MaterialTokens::tint_opacity`], and the fill an opaque fallback paints
    /// outright. Always fully opaque, so a fallback cannot leak a hole in a
    /// palette whose surface carries alpha.
    pub fn surface_color(self, palette: &Palette) -> Color {
        Color {
            a: 1.0,
            ..palette.surface_elevated
        }
    }
}

/// Foreground ink selection for a glass surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassInk {
    /// Follows the backdrop: light ink over dark content, dark ink over light,
    /// switching only after the change has lasted, with a short crossfade.
    Adaptive,
    /// Always light, independent of application mode.
    Light,
    /// Preserve the application theme's ink.
    Theme,
}

/// One material's validated numeric contract.
///
/// Lengths are logical pixels; ratios are unitless. Construct from
/// [`MaterialStyle::tokens`] and adjust fields for a theme override, then call
/// [`MaterialTokens::validated`] before handing the result to a renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct MaterialTokens {
    /// Backdrop Gaussian sigma, `0..=24` logical pixels; zero disables this blur.
    /// Local colour diffusion can still blur the sample independently.
    pub blur_sigma: f32,
    /// Lens displacement gain, `0..=2`. Zero leaves chromatic dispersion active.
    pub refraction: f32,
    /// Rounded-edge radius, `0..=80` logical pixels, capped at half the
    /// smaller dimension. Zero disables the lens and all optical rim lighting.
    pub bevel_width: f32,
    /// Refraction index, `1..=2`. At `1`, the main bend vanishes;
    /// centre pull and chromatic dispersion can still displace samples.
    pub index_of_refraction: f32,
    /// Chromatic gain, `0..=0.5`. Red and blue shift in opposite directions
    /// by up to 36 logical pixels per unit, relative to green.
    pub dispersion: f32,
    /// Weight of local colour diffusion, `0..=1`.
    pub color_diffusion: f32,
    /// Local colour diffusion sigma, `0..=96` logical pixels, independent of
    /// [`MaterialTokens::blur_sigma`] and dispersion.
    pub color_reach: f32,
    /// Main bend scale, `0..=96` logical pixels. Zero removes the main bend,
    /// but leaves centre pull and chromatic dispersion active.
    pub optical_depth: f32,
    /// Tint weight, `0..=1`. At `1`, the material boundary chooses an opaque
    /// fill instead of requesting a sampled backdrop.
    pub tint_opacity: f32,
    /// Sampled-backdrop saturation multiplier, `0..=2`.
    pub saturation: f32,
    /// Configurable edge-light gain, `0..=1`; reflection and Fresnel lighting
    /// remain active at zero when the bevel width is positive.
    pub highlight: f32,
    /// Brightness adjustment, `-0.5..=0.5`, applied to dark glass.
    /// Light glass is unchanged by this adjustment.
    pub brightness: f32,
    /// Light direction, `0..=360` degrees: `90` from below, `270` from above.
    pub light_angle: f32,
    /// Compatibility value, `1.5..=4`; unused by the current shader.
    pub profile_curvature: f32,
    /// Luminance roll-off weight, `0..=1`. Light frosted glass also applies
    /// a minimum weight. Later lighting prevents a guaranteed contrast ratio.
    pub contrast_protection: f32,
    /// Minimum content inset for [`MaterialInk::Protected`], `0..=64` logical
    /// pixels. This is layout metadata; the shader does not mask protection
    /// to this inset.
    pub protected_inset: f32,
}

// Inclusive `(low, high)` bound of each token, in declaration order.
const RANGES: [(f32, f32); 16] = [
    (0.0, 24.0),
    (0.0, 2.0),
    (0.0, 80.0),
    (1.0, 2.0),
    (0.0, 0.5),
    (0.0, 1.0),
    (0.0, 96.0),
    (0.0, 96.0),
    (0.0, 1.0),
    (0.0, 2.0),
    (0.0, 1.0),
    (-0.5, 0.5),
    (0.0, 360.0),
    (1.5, 4.0),
    (0.0, 1.0),
    (0.0, 64.0),
];

impl MaterialTokens {
    /// Default protected-content inset, in logical pixels.
    pub const PROTECTED_INSET: f32 = 12.0;

    /// The number of validated numeric tokens.
    pub const COUNT: usize = RANGES.len();

    fn fields(&self) -> [f32; Self::COUNT] {
        [
            self.blur_sigma,
            self.refraction,
            self.bevel_width,
            self.index_of_refraction,
            self.dispersion,
            self.color_diffusion,
            self.color_reach,
            self.optical_depth,
            self.tint_opacity,
            self.saturation,
            self.highlight,
            self.brightness,
            self.light_angle,
            self.profile_curvature,
            self.contrast_protection,
            self.protected_inset,
        ]
    }

    fn from_fields(values: [f32; Self::COUNT]) -> Self {
        Self {
            blur_sigma: values[0],
            refraction: values[1],
            bevel_width: values[2],
            index_of_refraction: values[3],
            dispersion: values[4],
            color_diffusion: values[5],
            color_reach: values[6],
            optical_depth: values[7],
            tint_opacity: values[8],
            saturation: values[9],
            highlight: values[10],
            brightness: values[11],
            light_angle: values[12],
            profile_curvature: values[13],
            contrast_protection: values[14],
            protected_inset: values[15],
        }
    }

    /// Clamp every token into its documented range.
    ///
    /// A non-finite value is inexpressible and degrades to
    /// [`MaterialStyle::Regular`]'s value for that token.
    pub fn validated(self) -> Self {
        let regular = MaterialStyle::Regular.builtin_tokens().fields();
        let mut values = self.fields();
        for (index, value) in values.iter_mut().enumerate() {
            let (low, high) = RANGES[index];
            *value = if value.is_finite() {
                value.clamp(low, high)
            } else {
                regular[index]
            };
        }
        Self::from_fields(values)
    }

    /// Whether every token already sits inside its documented range.
    pub fn is_valid(&self) -> bool {
        self.fields()
            .iter()
            .zip(RANGES)
            .all(|(value, (low, high))| value.is_finite() && *value >= low && *value <= high)
    }

    /// Convert validated numeric tokens into renderer parameters.
    ///
    /// `radii` and `tint` are copied without validation; callers must supply
    /// valid geometry and an opaque fallback colour. Accessibility policy is
    /// applied separately by the material boundary.
    pub fn to_material(&self, radii: [f32; 4], tint: Color) -> iced_core::material::Material {
        let tokens = self.validated();

        iced_core::material::Material {
            radii,
            blur_sigma: tokens.blur_sigma,
            refraction: tokens.refraction,
            bevel_width: tokens.bevel_width,
            index_of_refraction: tokens.index_of_refraction,
            dispersion: tokens.dispersion,
            color_diffusion: tokens.color_diffusion,
            color_reach: tokens.color_reach,
            optical_depth: tokens.optical_depth,
            tint,
            tint_opacity: tokens.tint_opacity,
            saturation: tokens.saturation,
            highlight: tokens.highlight,
            brightness: tokens.brightness,
            light_angle: tokens.light_angle,
            profile_curvature: tokens.profile_curvature,
            contrast_protection: tokens.contrast_protection,
            protected_inset: tokens.protected_inset,
            probe: 0,
            measure_only: false,
        }
    }

    /// Whether these tokens describe a fully opaque surface, which needs no
    /// sampled backdrop and no material pass.
    pub fn is_opaque(&self) -> bool {
        self.tint_opacity >= 1.0
    }

    /// Whether gain, depth and index enable the main refractive bend.
    ///
    /// This raw predicate does not validate tokens or account for centre pull
    /// and chromatic dispersion, which can displace samples independently.
    pub fn refracts(&self) -> bool {
        self.refraction > 0.0 && self.optical_depth > 0.0 && self.index_of_refraction > 1.0
    }
}

/// Where a [`MaterialPolicy`] came from.
///
/// Recorded so a decision is auditable: "the desktop said no reduction" and
/// "nobody ever asked the desktop" produce the same booleans and are not the
/// same statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaterialPolicySource {
    /// No transport supplied a value. The booleans are this framework's stable
    /// defaults, not anyone's answer.
    #[default]
    Unset,
    /// An operating-system preference transport supplied at least one value.
    System,
    /// An in-application user override supplied at least one value. It takes
    /// precedence over the system transport per dimension, so this is the
    /// source whenever an override is in force for any of the three.
    UserOverride,
}

/// One transport's answers; `None` leaves a dimension unanswered.
///
/// `Some(false)` explicitly declines reduction. The composed policy records
/// one aggregate source; [`published_layers`] retains per-dimension answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[must_use]
pub struct PreferenceSet {
    /// The user asked not to see through surfaces.
    pub reduced_transparency: Option<bool>,
    /// The user asked for maximum separation between a panel and its backdrop.
    pub increased_contrast: Option<bool>,
    /// The user asked not to be shown motion.
    pub reduced_motion: Option<bool>,
}

impl PreferenceSet {
    /// A set that answers nothing.
    pub const NOTHING: Self = Self {
        reduced_transparency: None,
        increased_contrast: None,
        reduced_motion: None,
    };

    /// Whether any dimension was answered.
    pub fn is_answered(&self) -> bool {
        self.reduced_transparency.is_some()
            || self.increased_contrast.is_some()
            || self.reduced_motion.is_some()
    }

    /// The number of answered dimensions, for a status report.
    pub fn answered(&self) -> usize {
        usize::from(self.reduced_transparency.is_some())
            + usize::from(self.increased_contrast.is_some())
            + usize::from(self.reduced_motion.is_some())
    }

    /// This set with `reduced_transparency` answered.
    pub fn with_reduced_transparency(mut self, value: bool) -> Self {
        self.reduced_transparency = Some(value);
        self
    }

    /// This set with `increased_contrast` answered.
    pub fn with_increased_contrast(mut self, value: bool) -> Self {
        self.increased_contrast = Some(value);
        self
    }

    /// This set with `reduced_motion` answered.
    pub fn with_reduced_motion(mut self, value: bool) -> Self {
        self.reduced_motion = Some(value);
        self
    }
}

/// Accessibility restrictions for a sampled-backdrop material.
///
/// Reduced transparency and increased contrast forbid the sampled backdrop.
/// Reduced motion disables lens displacement and dispersion through
/// [`MaterialPolicy::settle`], but the filtered image still follows the backdrop.
/// Defaults request no reduction and record [`MaterialPolicySource::Unset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[must_use]
pub struct MaterialPolicy {
    /// The user asked for reduced transparency.
    pub reduced_transparency: bool,
    /// The user asked for increased contrast.
    pub increased_contrast: bool,
    /// The user asked for reduced motion.
    pub reduced_motion: bool,
    /// Where these values came from.
    pub source: MaterialPolicySource,
}

impl MaterialPolicy {
    /// Whether this policy forbids a sampled backdrop entirely.
    pub fn forbids_material(&self) -> bool {
        self.reduced_transparency || self.increased_contrast
    }

    /// Disable lens displacement and dispersion when reduced motion is set.
    ///
    /// Other tokens remain unchanged; this does not freeze the backdrop or
    /// enforce transparency and contrast restrictions.
    pub fn settle(&self, tokens: MaterialTokens) -> MaterialTokens {
        if !self.reduced_motion {
            return tokens;
        }

        MaterialTokens {
            refraction: 0.0,
            dispersion: 0.0,
            ..tokens
        }
    }

    /// Join a theme motion flag with this policy's motion restriction.
    ///
    /// Any aggregate `UserOverride` source leaves the entire policy unchanged.
    /// Otherwise motion is ORed with the theme flag, and a true flag changes
    /// an unset source to `System`. Use [`policy_with_theme_motion`] for
    /// per-dimension precedence against the live store.
    pub fn with_theme_reduced_motion(mut self, reduced_motion: bool) -> Self {
        if self.source == MaterialPolicySource::UserOverride {
            return self;
        }

        self.reduced_motion = self.reduced_motion || reduced_motion;

        if reduced_motion && self.source == MaterialPolicySource::Unset {
            self.source = MaterialPolicySource::System;
        }

        self
    }
}

/// Content-layout policy for keeping foreground ink away from the glass rim.
///
/// Foreground content is not refracted with the backdrop. The inset adds space
/// from the optically active edge; it does not guarantee contrast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaterialInk {
    /// Grow content padding to [`MaterialTokens::protected_inset`] when needed.
    /// This is the default policy for text.
    #[default]
    Protected,
    /// Add no material-specific inset; the caller controls content spacing
    /// and legibility, as for compact icon controls.
    Strong,
}

impl MaterialInk {
    /// The padding this ink policy requires for `tokens`, in logical pixels.
    ///
    /// [`MaterialInk::Strong`] requires none; a caller's own padding still
    /// applies.
    pub fn required_inset(self, tokens: &MaterialTokens) -> f32 {
        match self {
            Self::Protected => tokens.protected_inset.max(0.0),
            Self::Strong => 0.0,
        }
    }
}

impl Default for MaterialTokens {
    fn default() -> Self {
        MaterialStyle::Regular.tokens()
    }
}

// Each dimension uses two bits: value and presence. System occupies bits 0-5,
// user overrides bits 8-13; one atomic snapshot preserves both layers.
static POLICY: AtomicU32 = AtomicU32::new(0);

// Superseded generations cannot publish or clear the current store.
static OWNER: AtomicU64 = AtomicU64::new(0);
static POLICY_WRITE_LOCK: Mutex<()> = Mutex::new(());

const SYSTEM_SHIFT: u32 = 0;
const USER_SHIFT: u32 = 8;
const HALF_MASK: u32 = 0b0011_1111;

// Dimension order inside a half: transparency, contrast, motion.
const TRANSPARENCY: u32 = 0;
const CONTRAST: u32 = 1;
const MOTION: u32 = 2;

fn pack(set: PreferenceSet) -> u32 {
    let bits = |dimension: u32, answer: Option<bool>| match answer {
        Some(value) => (u32::from(value) | 0b10) << (2 * dimension),
        None => 0,
    };

    bits(TRANSPARENCY, set.reduced_transparency)
        | bits(CONTRAST, set.increased_contrast)
        | bits(MOTION, set.reduced_motion)
}

fn unpack(half: u32) -> PreferenceSet {
    let answer = |dimension: u32| {
        let pair = (half >> (2 * dimension)) & 0b11;
        (pair & 0b10 != 0).then_some(pair & 0b01 != 0)
    };

    PreferenceSet {
        reduced_transparency: answer(TRANSPARENCY),
        increased_contrast: answer(CONTRAST),
        reduced_motion: answer(MOTION),
    }
}

fn compose(word: u32) -> MaterialPolicy {
    compose_layers(
        unpack((word >> SYSTEM_SHIFT) & HALF_MASK),
        unpack((word >> USER_SHIFT) & HALF_MASK),
    )
}

// Precedence is per dimension, not per policy: a user override of one
// preference must not decide the two it said nothing about.
fn compose_layers(system: PreferenceSet, user: PreferenceSet) -> MaterialPolicy {
    let resolve = |user: Option<bool>, system: Option<bool>| user.or(system).unwrap_or(false);

    MaterialPolicy {
        reduced_transparency: resolve(user.reduced_transparency, system.reduced_transparency),
        increased_contrast: resolve(user.increased_contrast, system.increased_contrast),
        reduced_motion: resolve(user.reduced_motion, system.reduced_motion),
        source: if user.is_answered() {
            MaterialPolicySource::UserOverride
        } else if system.is_answered() {
            MaterialPolicySource::System
        } else {
            MaterialPolicySource::Unset
        },
    }
}

/// Read the current process-wide policy without blocking.
///
/// Before publication, all restrictions are false and the source is `Unset`.
/// Transport failures retain previously published answers until replaced or
/// cleared by the owner.
pub fn current_policy() -> MaterialPolicy {
    compose(POLICY.load(Ordering::Relaxed))
}

/// Join the current layers with a theme motion restriction.
///
/// A true theme flag replaces the system motion answer with `Some(true)`;
/// false preserves the published answer. User overrides win per dimension.
/// This reads a snapshot without updating the store.
pub fn policy_with_theme_motion(theme_reduced_motion: bool) -> MaterialPolicy {
    let word = POLICY.load(Ordering::Relaxed);
    let mut system = unpack((word >> SYSTEM_SHIFT) & HALF_MASK);
    let user = unpack((word >> USER_SHIFT) & HALF_MASK);

    if theme_reduced_motion {
        system.reduced_motion = Some(true);
    }

    compose_layers(system, user)
}

/// The system and user layers as published, for a diagnostic that needs to see
/// which dimensions were answered rather than the composed result.
pub fn published_layers() -> (PreferenceSet, PreferenceSet) {
    let word = POLICY.load(Ordering::Relaxed);

    (
        unpack((word >> SYSTEM_SHIFT) & HALF_MASK),
        unpack((word >> USER_SHIFT) & HALF_MASK),
    )
}

/// Clear the process-wide policy store and install its sole publisher.
///
/// Installing supersedes the previous owner. Superseded writes and drops
/// leave the store unchanged; dropping the current owner clears both layers.
/// These operations are serialized while draw-path reads remain lock-free.
///
/// # Panics
///
/// Panics if the owner generation would overflow `u64`.
pub fn install_policy_owner() -> MaterialPolicyOwner {
    let _guard = POLICY_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let generation = OWNER
        .load(Ordering::Relaxed)
        .checked_add(1)
        .expect("material policy owner generation exhausted");
    POLICY.store(0, Ordering::Release);
    OWNER.store(generation, Ordering::Release);

    MaterialPolicyOwner { generation }
}

/// The right to publish into the process-wide policy store.
///
/// Held by whatever owns the preference transport for one application runtime —
/// in enzUI, `MaterialPreferenceFeed`. Obtained from [`install_policy_owner`].
#[derive(Debug)]
pub struct MaterialPolicyOwner {
    generation: u64,
}

impl MaterialPolicyOwner {
    /// Whether this token still owns the store.
    pub fn is_current(&self) -> bool {
        OWNER.load(Ordering::Acquire) == self.generation
    }

    /// Publishes the operating system's answers, replacing the previous system
    /// layer wholesale.
    ///
    /// Replacement rather than merge is deliberate: a desktop that stops
    /// exporting a key must stop answering for it, or a preference would
    /// outlive the setting that expressed it. Any user override stays in force.
    ///
    /// Returns whether the composed policy changed. A publish from a superseded
    /// owner changes nothing and returns `false`.
    pub fn publish_system(&self, preferences: PreferenceSet) -> bool {
        self.write(SYSTEM_SHIFT, pack(preferences))
    }

    /// Publishes the in-application user override, replacing it wholesale.
    ///
    /// Each answered dimension wins over the system's answer for that
    /// dimension; an unanswered one defers to it. Clearing is
    /// [`clear_user_override`](Self::clear_user_override), not "publish false":
    /// the two are different statements and only one of them defers.
    pub fn set_user_override(&self, preferences: PreferenceSet) -> bool {
        self.write(USER_SHIFT, pack(preferences))
    }

    /// Drops the user override, so the system layer answers again.
    pub fn clear_user_override(&self) -> bool {
        self.write(USER_SHIFT, 0)
    }

    fn write(&self, shift: u32, half: u32) -> bool {
        let _guard = POLICY_WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if OWNER.load(Ordering::Acquire) != self.generation {
            return false;
        }

        let previous = POLICY.load(Ordering::Relaxed);
        let next = (previous & !(HALF_MASK << shift)) | ((half & HALF_MASK) << shift);
        POLICY.store(next, Ordering::Release);
        compose(previous) != compose(next)
    }
}

impl Drop for MaterialPolicyOwner {
    fn drop(&mut self) {
        let _guard = POLICY_WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if OWNER.load(Ordering::Acquire) == self.generation {
            POLICY.store(0, Ordering::Release);
        }
    }
}

// Tests that own the process-wide policy store share this lock.
#[cfg(test)]
pub(crate) static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn store_lock() -> std::sync::MutexGuard<'static, ()> {
    STORE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Live overrides of the built-in styles, for a tuning tool. Every widget
/// that resolves a style through [`MaterialStyle::tokens`] picks the override
/// up on its next view.
#[cfg(feature = "material-tuning")]
pub mod tuning {
    use super::{MaterialStyle, MaterialTokens, RANGES};
    use std::sync::Mutex;

    static OVERRIDES: Mutex<[Option<MaterialTokens>; MaterialStyle::ALL.len()]> =
        Mutex::new([None; MaterialStyle::ALL.len()]);

    /// Each token's name and inclusive range, in [`values`] order.
    pub const FIELDS: [(&str, f32, f32); MaterialTokens::COUNT] = {
        const NAMES: [&str; MaterialTokens::COUNT] = [
            "blur_sigma",
            "refraction",
            "bevel_width",
            "index_of_refraction",
            "dispersion",
            "color_diffusion",
            "color_reach",
            "optical_depth",
            "tint_opacity",
            "saturation",
            "highlight",
            "brightness",
            "light_angle",
            "profile_curvature",
            "contrast_protection",
            "protected_inset",
        ];
        let mut out = [("", 0.0, 0.0); MaterialTokens::COUNT];
        let mut i = 0;
        while i < MaterialTokens::COUNT {
            out[i] = (NAMES[i], RANGES[i].0, RANGES[i].1);
            i += 1;
        }
        out
    };

    fn slot(style: MaterialStyle) -> usize {
        MaterialStyle::ALL
            .iter()
            .position(|s| *s == style)
            .unwrap_or(0)
    }

    /// Validate and replace `style`'s tokens, or restore built-ins with `None`.
    /// This does not schedule a redraw or rebuild existing views.
    pub fn set(style: MaterialStyle, tokens: Option<MaterialTokens>) {
        let mut overrides = OVERRIDES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        overrides[slot(style)] = tokens.map(MaterialTokens::validated);
    }

    /// `style`'s override, if one is set.
    pub fn get(style: MaterialStyle) -> Option<MaterialTokens> {
        OVERRIDES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[slot(style)]
    }

    /// The tokens as an array, in [`FIELDS`] order.
    pub fn values(tokens: &MaterialTokens) -> [f32; MaterialTokens::COUNT] {
        tokens.fields()
    }

    /// Tokens from an array in [`FIELDS`] order, without validation.
    pub fn from_values(values: [f32; MaterialTokens::COUNT]) -> MaterialTokens {
        MaterialTokens::from_fields(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_policy_is_a_default_and_not_an_approval() {
        let unset = MaterialPolicy::default();

        assert_eq!(unset.source, MaterialPolicySource::Unset);
        assert!(!unset.forbids_material());

        let from_theme = unset.with_theme_reduced_motion(true);
        assert!(from_theme.reduced_motion);
        assert_eq!(from_theme.source, MaterialPolicySource::System);

        let system = MaterialPolicy {
            reduced_motion: true,
            source: MaterialPolicySource::System,
            ..MaterialPolicy::default()
        };
        assert!(system.with_theme_reduced_motion(false).reduced_motion);
        assert_eq!(
            unset.with_theme_reduced_motion(false).source,
            MaterialPolicySource::Unset
        );

        let override_off = MaterialPolicy {
            reduced_motion: false,
            source: MaterialPolicySource::UserOverride,
            ..MaterialPolicy::default()
        };
        let kept = override_off.with_theme_reduced_motion(true);
        assert!(!kept.reduced_motion);
        assert_eq!(kept.source, MaterialPolicySource::UserOverride);
    }

    #[test]
    fn glass_keeps_the_theme_ink_unless_asked_to_adapt() {
        for style in MaterialStyle::ALL {
            let expected = if style == MaterialStyle::Clear {
                GlassInk::Light
            } else {
                GlassInk::Theme
            };
            assert_eq!(style.ink(), expected, "{style:?}");
        }
    }
}
