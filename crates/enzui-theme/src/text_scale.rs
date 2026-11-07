//! Bounded logical text-size scaling and layered desktop preference policy.
//!
//! [`TextScalePolicy`] chooses the highest set layer: user, theme, platform,
//! then application. With no layer set it returns the 100% baseline. Factors
//! apply to logical text sizes independently of the window's device-pixel ratio.
//!
//! Publish the chosen factor with [`publish`] and use that same value in
//! [`TypographyScale::with_text_scale`](crate::TypographyScale::with_text_scale).
//! Theme rungs already contain their factor; ambient readers scale their own
//! ramp. Matching factors prevent double scaling but do not make different
//! authored ramps or line-height rules identical. Explicit widget pixel sizes
//! stay absolute unless the caller applies [`TextScale::apply_px`] to them.
//!
//! ```
//! use enzui_theme::text_scale::{TextScale, TextScalePolicy, TextScaleSource};
//!
//! let mut policy = TextScalePolicy::default();
//! assert_eq!(policy.effective(), TextScale::BASELINE);
//!
//! policy.set_platform(Some(TextScale::from_percent(150)));
//! assert_eq!(policy.source(), TextScaleSource::Platform);
//!
//! policy.set_user(Some(TextScale::from_percent(200)));
//! assert_eq!(policy.effective().percent().round(), 200.0);
//!
//! policy.set_user(None);
//! assert_eq!(policy.effective().percent().round(), 150.0);
//! ```
//!
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

/// Smallest accepted factor (50%).
pub const MIN_FACTOR: f32 = 0.5;
/// Largest accepted factor (300%).
pub const MAX_FACTOR: f32 = 3.0;

/// The session variable a desktop, session script or launcher sets to deliver a
/// text scale where no desktop feed is wired up.
pub const TEXT_SCALE_ENV: &str = "ENZUI_TEXT_SCALE";

/// A bounded multiplier for logical type sizes and their line boxes.
///
/// Always finite and inside `[MIN_FACTOR, MAX_FACTOR]`; there is no way to
/// construct one that is not. Untrusted numbers enter through
/// [`TextScale::admit`] (or its lossy shorthand [`TextScale::from_backend`]),
/// which clamps what is merely out of range and rejects what is not a scale at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct TextScale(f32);

impl Default for TextScale {
    fn default() -> Self {
        Self::BASELINE
    }
}

impl TextScale {
    /// 100%: the compiled type ramp, unchanged.
    pub const BASELINE: TextScale = TextScale(1.0);

    /// A factor inside `[MIN_FACTOR, MAX_FACTOR]`, or `None` for anything else
    /// (including `NaN` and infinities).
    #[must_use]
    pub fn new(factor: f32) -> Option<Self> {
        (factor.is_finite() && (MIN_FACTOR..=MAX_FACTOR).contains(&factor)).then_some(Self(factor))
    }

    /// A percentage: `from_percent(150)` is 1.5. Percentages outside the
    /// supported range clamp to it; zero selects the baseline.
    #[must_use]
    pub fn from_percent(percent: u16) -> Self {
        Self::from_backend(f32::from(percent) / 100.0)
    }

    /// Sanitizes an untrusted number into a usable factor and reports what had
    /// to be done to it.
    ///
    /// A positive finite number outside the supported range is clamped;
    /// `NaN`, an infinity and any value at or below zero are not a scale at all
    /// and fall back to [`TextScale::BASELINE`], so an unusable backend value
    /// leaves text at its compiled size rather than at a guess.
    #[must_use]
    pub fn admit(factor: f32) -> (Self, TextScaleAdmission) {
        if !factor.is_finite() || factor <= 0.0 {
            return (Self::BASELINE, TextScaleAdmission::Rejected);
        }
        if factor < MIN_FACTOR {
            return (Self(MIN_FACTOR), TextScaleAdmission::Clamped);
        }
        if factor > MAX_FACTOR {
            return (Self(MAX_FACTOR), TextScaleAdmission::Clamped);
        }
        (Self(factor), TextScaleAdmission::Accepted)
    }

    /// [`TextScale::admit`] without the verdict, for a caller that has no way
    /// to report one.
    #[must_use]
    pub fn from_backend(factor: f32) -> Self {
        Self::admit(factor).0
    }

    /// The multiplier itself.
    #[must_use]
    pub const fn factor(self) -> f32 {
        self.0
    }

    /// The multiplier as a percentage (`1.25` reads as `125.0`).
    #[must_use]
    pub fn percent(self) -> f32 {
        self.0 * 100.0
    }

    /// Whether this is the unscaled compiled ramp.
    #[must_use]
    pub fn is_baseline(self) -> bool {
        self.0 == Self::BASELINE.0
    }

    /// Multiply a logical measurement, preserving its bits at baseline.
    /// The measurement is not validated; multiplication can overflow.
    #[must_use]
    pub fn apply_px(self, px: f32) -> f32 {
        if self.is_baseline() { px } else { px * self.0 }
    }

    /// Recovers a measurement this factor was applied to.
    ///
    /// Exact at baseline; finite intermediate values round-trip through
    /// [`apply_px`](Self::apply_px) subject to floating-point rounding.
    /// This cannot recover a value whose scaled result overflowed.
    #[must_use]
    pub fn unapply_px(self, px: f32) -> f32 {
        if self.is_baseline() { px } else { px / self.0 }
    }
}

/// What [`TextScale::admit`] had to do with an untrusted number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextScaleAdmission {
    /// Used as supplied.
    Accepted,
    /// A positive finite value outside `[MIN_FACTOR, MAX_FACTOR]`, clamped.
    Clamped,
    /// Not a scale (`NaN`, infinite, zero or negative): the baseline was used.
    Rejected,
}

impl TextScaleAdmission {
    /// Whether the supplied number was usable as written.
    #[must_use]
    pub const fn is_accepted(self) -> bool {
        matches!(self, TextScaleAdmission::Accepted)
    }
}

/// Which layer of [`TextScalePolicy`] the effective factor came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextScaleSource {
    /// No layer is set; the effective factor is [`TextScale::BASELINE`].
    Baseline,
    /// The application's compiled default.
    Application,
    /// Session or desktop delivery.
    Platform,
    /// The shared theme file's `typography.text_scale`.
    Theme,
    /// An explicit choice made in the running application.
    User,
}

/// Layered resolution of the effective text scale.
///
/// Each layer is independently settable and clearable; [`effective`](Self::effective)
/// returns the highest one that is set, and [`source`](Self::source) names it.
/// Setters report changes to the effective factor, excluding source-only
/// changes. Updating this policy does not publish it or rebuild a theme.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextScalePolicy {
    user: Option<TextScale>,
    theme: Option<TextScale>,
    platform: Option<TextScale>,
    application: Option<TextScale>,
}

impl TextScalePolicy {
    /// A policy with no layer set, resolving to [`TextScale::BASELINE`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The factor in force after precedence.
    #[must_use]
    pub fn effective(&self) -> TextScale {
        self.user
            .or(self.theme)
            .or(self.platform)
            .or(self.application)
            .unwrap_or(TextScale::BASELINE)
    }

    /// Which layer [`effective`](Self::effective) came from.
    #[must_use]
    pub fn source(&self) -> TextScaleSource {
        if self.user.is_some() {
            TextScaleSource::User
        } else if self.theme.is_some() {
            TextScaleSource::Theme
        } else if self.platform.is_some() {
            TextScaleSource::Platform
        } else if self.application.is_some() {
            TextScaleSource::Application
        } else {
            TextScaleSource::Baseline
        }
    }

    /// Reads back one layer, whether or not it is the effective one.
    #[must_use]
    pub fn layer(&self, source: TextScaleSource) -> Option<TextScale> {
        match source {
            TextScaleSource::User => self.user,
            TextScaleSource::Theme => self.theme,
            TextScaleSource::Platform => self.platform,
            TextScaleSource::Application => self.application,
            TextScaleSource::Baseline => None,
        }
    }

    /// Sets or clears the explicit in-application choice.
    pub fn set_user(&mut self, scale: Option<TextScale>) -> bool {
        self.set(TextScaleSource::User, scale)
    }

    /// Sets or clears the shared theme file's request.
    pub fn set_theme(&mut self, scale: Option<TextScale>) -> bool {
        self.set(TextScaleSource::Theme, scale)
    }

    /// Sets or clears session/desktop delivery. Clearing is what an absent or
    /// withdrawn backend does, and it restores the layers below.
    pub fn set_platform(&mut self, scale: Option<TextScale>) -> bool {
        self.set(TextScaleSource::Platform, scale)
    }

    /// Sets or clears the application's compiled default.
    pub fn set_application(&mut self, scale: Option<TextScale>) -> bool {
        self.set(TextScaleSource::Application, scale)
    }

    /// Apply a delivered value to the platform layer, or clear it for
    /// unavailable and unreadable deliveries. A rejected numeric value is
    /// delivered as baseline and therefore sets the platform layer.
    pub fn accept_delivery(&mut self, delivery: &TextScaleDelivery) -> bool {
        self.set_platform(delivery.scale())
    }

    /// Clears every layer, returning to [`TextScale::BASELINE`].
    pub fn reset(&mut self) -> bool {
        let before = self.effective();
        *self = Self::default();
        before != self.effective()
    }

    fn set(&mut self, source: TextScaleSource, scale: Option<TextScale>) -> bool {
        let before = self.effective();
        match source {
            TextScaleSource::User => self.user = scale,
            TextScaleSource::Theme => self.theme = scale,
            TextScaleSource::Platform => self.platform = scale,
            TextScaleSource::Application => self.application = scale,
            TextScaleSource::Baseline => {}
        }
        before != self.effective()
    }
}

/// What a text-scale source had to say.
///
/// The distinction that matters to policy is between "this desktop reports
/// 150%" and "there is nothing to read here": only the first sets a layer, and
/// the second must leave the layers below it in force.
#[derive(Debug, Clone, PartialEq)]
pub enum TextScaleDelivery {
    /// The reader returned no value, or the returned value was blank.
    Unavailable,
    /// A usable factor, and what had to be done to the raw value.
    Delivered {
        /// The factor to apply.
        scale: TextScale,
        /// Whether the raw value was usable as written.
        admission: TextScaleAdmission,
    },
    /// The source held something that is not a number at all.
    Unreadable {
        /// Trimmed input text for a diagnostic.
        value: String,
    },
}

impl TextScaleDelivery {
    /// The delivered factor, or `None` when there is nothing to apply.
    #[must_use]
    pub fn scale(&self) -> Option<TextScale> {
        match self {
            TextScaleDelivery::Delivered { scale, .. } => Some(*scale),
            TextScaleDelivery::Unavailable | TextScaleDelivery::Unreadable { .. } => None,
        }
    }
}

/// Reads a text scale from the session environment through an injected reader.
///
/// Calls the injected reader once with [`TEXT_SCALE_ENV`]. Unset or blank
/// input is unavailable; nonnumeric text is unreadable. Positive finite
/// factors outside the supported range clamp to it. Non-finite, zero and
/// negative numbers deliver the baseline with a rejected admission, rather
/// than releasing the platform layer. No process environment is read directly.
pub fn read_env_text_scale(read: impl Fn(&str) -> Option<String>) -> TextScaleDelivery {
    let Some(raw) = read(TEXT_SCALE_ENV) else {
        return TextScaleDelivery::Unavailable;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return TextScaleDelivery::Unavailable;
    }
    let Ok(parsed) = trimmed.parse::<f32>() else {
        return TextScaleDelivery::Unreadable {
            value: trimmed.to_owned(),
        };
    };
    let (scale, admission) = TextScale::admit(parsed);
    TextScaleDelivery::Delivered { scale, admission }
}

// One atomic factor snapshot; publishing does not schedule a frame or rebuild views.
static PUBLISHED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static SCOPED: Cell<Option<TextScale>> = const { Cell::new(None) };
}

/// The factor text resolution should apply when no explicit
/// [`TypographyScale`](crate::TypographyScale) was supplied.
///
/// Returns the innermost [`TextScaleScope`] on this thread, else the value
/// [`publish`] installed, else [`TextScale::BASELINE`].
#[must_use]
pub fn effective() -> TextScale {
    if let Some(scoped) = SCOPED.with(Cell::get) {
        return scoped;
    }
    published()
}

/// `px` at the effective text scale, for a framework widget whose chrome text
/// (and the box around it) should follow the desktop text size.
///
/// An application's own pins stay absolute; this is for sizes a widget chose.
#[must_use]
pub fn ui_px(px: f32) -> f32 {
    effective().apply_px(px * crate::text_defaults::font_size_factor())
}

/// The process-wide factor last given to [`publish`].
#[must_use]
pub fn published() -> TextScale {
    let bits = PUBLISHED.load(Ordering::Relaxed);
    if bits == 0 {
        return TextScale::BASELINE;
    }
    TextScale::from_backend(f32::from_bits(bits))
}

/// Install the process-wide factor and report whether its stored bits changed.
/// The first publication returns true even for the effective baseline.
///
/// The policy layer calls this with the same value it bakes into the theme's
/// [`TypographyScale`](crate::TypographyScale); publishing one without the
/// other leaves text that resolves through the theme disagreeing with text that
/// does not.
pub fn publish(scale: TextScale) -> bool {
    let previous = PUBLISHED.swap(scale.factor().to_bits(), Ordering::Relaxed);
    previous != scale.factor().to_bits()
}

/// A thread-scoped override of [`effective`], restored when dropped.
///
/// Drop nested guards in reverse order on their owning thread to restore the
/// previous override. Out-of-order drops can leave an earlier override active.
/// Other threads use their own scope or the published factor.
///
/// The guard must be dropped on the thread whose override it owns.
#[derive(Debug)]
pub struct TextScaleScope {
    previous: Option<TextScale>,
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl TextScaleScope {
    /// Overrides [`effective`] on this thread until the guard is dropped.
    #[must_use]
    pub fn enter(scale: TextScale) -> Self {
        let previous = SCOPED.with(|cell| cell.replace(Some(scale)));
        Self {
            previous,
            thread_bound: std::marker::PhantomData,
        }
    }
}

impl Drop for TextScaleScope {
    fn drop(&mut self) {
        SCOPED.with(|cell| cell.set(self.previous));
    }
}
