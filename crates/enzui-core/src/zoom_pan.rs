//! Controlled, validated zoom and pan geometry for inspectable images.
//!
//! Applications own [`ViewTransform`] values; operations constrain scale and
//! offset, anchor zoom at a focus point subject to clamping, and keep drag state
//! transient in [`GrabSession`]. For overflowing content, [`pannable_extent`]
//! differs from iced's native viewer by preventing an empty margin at maximum
//! displacement. Callers provide already fitted content geometry.

use iced_core::{ContentFit, Point, Size, Vector};

use crate::metrics::{FinitePositive, to_i32_saturating};
use crate::semantics::{NumericValue, Role, Semantics};

/// Default multiplicative step for one wheel notch or one `+`/`-` press.
///
/// Matches the native viewer's `scale_step` (iced's `image::viewer`).
pub const DEFAULT_ZOOM_STEP: f32 = 0.10;

/// Default arrow-key pan distance in logical pixels.
pub const DEFAULT_PAN_STEP: f32 = 24.0;

/// Default minimum scale. Matches the native viewer (iced's `image::viewer`).
pub const DEFAULT_MIN_SCALE: f32 = 0.25;

/// Default maximum scale. Matches the native viewer (iced's `image::viewer`).
pub const DEFAULT_MAX_SCALE: f32 = 10.0;

/// A finite, strictly positive zoom interval with `min <= max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomRange {
    min: FinitePositive,
    max: FinitePositive,
}

impl Default for ZoomRange {
    fn default() -> Self {
        Self {
            min: FinitePositive::new_or(DEFAULT_MIN_SCALE, DEFAULT_MIN_SCALE),
            max: FinitePositive::new_or(DEFAULT_MAX_SCALE, DEFAULT_MAX_SCALE),
        }
    }
}

impl ZoomRange {
    /// Build a range from finite, positive, correctly ordered endpoints.
    ///
    /// Inverted endpoints are rejected rather than swapped.
    #[must_use]
    pub fn new(min: f32, max: f32) -> Option<Self> {
        let min = FinitePositive::new(min)?;
        let max = FinitePositive::new(max)?;
        (min.get() <= max.get()).then_some(Self { min, max })
    }

    /// Build a range, falling back to [`ZoomRange::default`] if the arguments
    /// are not a valid range.
    #[must_use]
    pub fn new_or_default(min: f32, max: f32) -> Self {
        Self::new(min, max).unwrap_or_default()
    }

    /// The minimum scale.
    #[must_use]
    pub fn min(self) -> f32 {
        self.min.get()
    }

    /// The maximum scale.
    #[must_use]
    pub fn max(self) -> f32 {
        self.max.get()
    }

    /// Clamp a scale into the range, resolving non-finite values to the minimum.
    #[must_use]
    pub fn clamp(self, scale: f32) -> f32 {
        if scale.is_finite() {
            scale.clamp(self.min(), self.max())
        } else {
            self.min()
        }
    }

    /// Whether a scale is already inside the range.
    #[must_use]
    pub fn contains(self, scale: f32) -> bool {
        scale.is_finite() && scale >= self.min() && scale <= self.max()
    }

    /// Whether `min == max`, meaning zoom controls cannot change the scale.
    #[must_use]
    pub fn is_pinned(self) -> bool {
        self.min() == self.max()
    }
}

/// A finite scale and viewport-relative scroll offset.
///
/// The offset displaces content from center in logical pixels; positive `x`
/// moves content left. Non-finite fields cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    scale: f32,
    offset: Vector,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl ViewTransform {
    /// Scale 1, no offset.
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        offset: Vector::new(0.0, 0.0),
    };

    /// Build a transform, returning `None` if the scale is not finite and
    /// strictly positive or either offset component is not finite.
    #[must_use]
    pub fn new(scale: f32, offset: Vector) -> Option<Self> {
        let scale = FinitePositive::new(scale)?;
        (offset.x.is_finite() && offset.y.is_finite()).then_some(Self {
            scale: scale.get(),
            offset,
        })
    }

    /// Repair each invalid field independently to its identity value.
    ///
    /// A corrupt persisted offset component does not discard a valid scale.
    #[must_use]
    pub fn sanitized(scale: f32, offset: Vector) -> Self {
        Self {
            scale: FinitePositive::new_or(scale, 1.0).get(),
            offset: Vector::new(
                if offset.x.is_finite() { offset.x } else { 0.0 },
                if offset.y.is_finite() { offset.y } else { 0.0 },
            ),
        }
    }

    /// The current scale.
    #[must_use]
    pub fn scale(self) -> f32 {
        self.scale
    }

    /// The current offset.
    #[must_use]
    pub fn offset(self) -> Vector {
        self.offset
    }

    /// Whether this is the identity transform (fit, unpanned).
    #[must_use]
    pub fn is_identity(self) -> bool {
        self.scale == 1.0 && self.offset.x == 0.0 && self.offset.y == 0.0
    }

    /// Returns the zoom level as a rounded whole percentage for display and accessibility, saturating at `i32::MAX`.
    #[must_use]
    pub fn zoom_percent(self) -> i32 {
        to_i32_saturating(self.scale * 100.0)
    }
}

/// The geometry a transform is resolved against: the visible frame, the
/// unscaled fitted content size, and the raw (intrinsic) image size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportGeometry {
    /// The visible frame, in logical pixels.
    pub bounds: Size,
    /// The content size at scale 1 after content fitting.
    pub content: Size,
    /// The source image's intrinsic size, used to compute actual-size scale.
    ///
    /// `Size::ZERO` means the intrinsic size is unknown or unmeasurable.
    pub image: Size,
}

impl ViewportGeometry {
    /// Compute fitted geometry, yielding empty geometry for a non-positive or non-finite image or frame.
    #[must_use]
    pub fn new(image: Size, bounds: Size, fit: ContentFit) -> Self {
        let usable = has_area(image) && has_area(bounds);
        let content = if usable {
            let fitted = fit.fit(image, bounds);
            if has_area(fitted) { fitted } else { Size::ZERO }
        } else {
            Size::ZERO
        };

        Self {
            bounds: if has_area(bounds) { bounds } else { Size::ZERO },
            content,
            image: if has_area(image) { image } else { Size::ZERO },
        }
    }

    /// The content size at a given scale.
    #[must_use]
    pub fn scaled(self, scale: f32) -> Size {
        Size::new(self.content.width * scale, self.content.height * scale)
    }

    /// Whether the fitted content or viewport lacks finite positive area.
    #[must_use]
    pub fn is_empty(self) -> bool {
        !has_area(self.content) || !has_area(self.bounds)
    }

    /// Return the intrinsic-to-fitted width ratio used for actual-size scaling.
    ///
    /// Returns `None` when either geometry is empty or the ratio is non-finite.
    #[must_use]
    pub fn actual_scale(self) -> Option<f32> {
        if !has_area(self.image) || !has_area(self.content) {
            return None;
        }
        let scale = self.image.width / self.content.width;
        if scale.is_finite() && scale > 0.0 {
            Some(scale)
        } else {
            None
        }
    }
}

fn has_area(size: Size) -> bool {
    size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0
}

/// Return the maximum centered displacement per axis.
///
/// Each component is `max((scaled - bounds) / 2, 0)`. On an overflowing axis,
/// maximum displacement aligns one content edge with the viewport edge.
#[must_use]
pub fn pannable_extent(scaled: Size, bounds: Size) -> Vector {
    Vector::new(
        ((scaled.width - bounds.width) / 2.0).max(0.0),
        ((scaled.height - bounds.height) / 2.0).max(0.0),
    )
}

/// Clamp an offset to the pannable extent for the given geometry and scale.
#[must_use]
pub fn clamp_offset(offset: Vector, geometry: ViewportGeometry, scale: f32) -> Vector {
    let extent = pannable_extent(geometry.scaled(scale), geometry.bounds);

    Vector::new(
        clamp_symmetric(offset.x, extent.x),
        clamp_symmetric(offset.y, extent.y),
    )
}

fn clamp_symmetric(v: f32, extent: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-extent, extent)
    } else {
        0.0
    }
}

/// Clamp both scale and offset to the supplied range and geometry.
///
/// Empty geometry preserves the clamped scale and clears the offset because no pannable extent exists.
#[must_use]
pub fn clamp_transform(
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
) -> ViewTransform {
    let scale = range.clamp(transform.scale());

    if geometry.is_empty() {
        return ViewTransform {
            scale,
            offset: Vector::new(0.0, 0.0),
        };
    }

    ViewTransform {
        scale,
        offset: clamp_offset(transform.offset(), geometry, scale),
    }
}

/// Apply a clamped scale, preserving the content point under `focus` unless offset clamping prevents it.
///
/// `focus` uses viewport coordinates. Before offset clamping, the offset follows
/// `offset' = offset * k + (focus - centre) * (k - 1)`, where `k` is the new
/// scale divided by the old scale.
#[must_use]
pub fn apply_zoom(
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
    target_scale: f32,
    focus: Point,
) -> ViewTransform {
    let previous = transform.scale();
    let scale = range.clamp(target_scale);

    if geometry.is_empty() || scale == previous {
        return clamp_transform(
            ViewTransform {
                scale,
                offset: transform.offset(),
            },
            geometry,
            range,
        );
    }

    let k = scale / previous;
    let centre = Point::new(geometry.bounds.width / 2.0, geometry.bounds.height / 2.0);
    let focus_to_centre = Vector::new(focus.x - centre.x, focus.y - centre.y);

    let offset = Vector::new(
        transform.offset().x * k + focus_to_centre.x * (k - 1.0),
        transform.offset().y * k + focus_to_centre.y * (k - 1.0),
    );

    clamp_transform(ViewTransform { scale, offset }, geometry, range)
}

/// Zoom about `focus` with reciprocal multiplicative steps.
///
/// A non-positive or non-finite `step` uses [`DEFAULT_ZOOM_STEP`]; zooming in
/// multiplies by `1 + step`, while zooming out divides by it.
#[must_use]
pub fn zoom_by_step(
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
    step: f32,
    direction: ZoomDirection,
    focus: Point,
) -> ViewTransform {
    let step = if step.is_finite() && step > 0.0 {
        step
    } else {
        DEFAULT_ZOOM_STEP
    };

    let target = match direction {
        ZoomDirection::In => transform.scale() * (1.0 + step),
        ZoomDirection::Out => transform.scale() / (1.0 + step),
    };

    apply_zoom(transform, geometry, range, snap_to_unity(target), focus)
}

/// Multiplies the scale by `factor` (a pinch's growth) keeping `focus` fixed,
/// clamped to `range`. A non-finite or non-positive factor changes nothing.
#[must_use]
pub fn zoom_by_factor(
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
    factor: f32,
    focus: Point,
) -> ViewTransform {
    if !factor.is_finite() || factor <= 0.0 {
        return transform;
    }
    apply_zoom(
        transform,
        geometry,
        range,
        transform.scale() * factor,
        focus,
    )
}

// Snap stepped values near unity to erase reciprocal floating-point drift; direct scales remain untouched.
#[must_use]
fn snap_to_unity(scale: f32) -> f32 {
    if (scale - 1.0).abs() < UNITY_SNAP {
        1.0
    } else {
        scale
    }
}

/// Tolerance used to snap stepped scales to exactly 1.0.
pub const UNITY_SNAP: f32 = 1e-4;

/// Which way a stepped zoom goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomDirection {
    /// Magnify.
    In,
    /// Shrink.
    Out,
}

impl ZoomDirection {
    /// Classify a wheel delta. Returns `None` for a zero or non-finite delta,
    /// so a horizontal-only scroll does not produce a spurious zoom.
    #[must_use]
    pub fn from_scroll(y: f32) -> Option<Self> {
        if !y.is_finite() || y == 0.0 {
            None
        } else if y > 0.0 {
            Some(ZoomDirection::In)
        } else {
            Some(ZoomDirection::Out)
        }
    }
}

/// Adjust the scroll offset by `delta` logical pixels and re-clamp.
#[must_use]
pub fn pan_by(
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
    delta: Vector,
) -> ViewTransform {
    let offset = Vector::new(
        transform.offset().x + if delta.x.is_finite() { delta.x } else { 0.0 },
        transform.offset().y + if delta.y.is_finite() { delta.y } else { 0.0 },
    );

    clamp_transform(
        ViewTransform {
            scale: transform.scale(),
            offset,
        },
        geometry,
        range,
    )
}

/// Return a centered transform at scale 1, clamped to `range`.
///
/// Scale 1 represents fit because [`ViewportGeometry::content`] is already
/// fitted. If `range` excludes 1, the nearest endpoint is used.
#[must_use]
pub fn fit_transform(range: ZoomRange) -> ViewTransform {
    ViewTransform {
        scale: range.clamp(1.0),
        offset: Vector::new(0.0, 0.0),
    }
}

/// Whether content overflows the viewport on either axis.
///
/// Callers can use this to disable pan controls that cannot change the view.
#[must_use]
pub fn is_pannable(geometry: ViewportGeometry, scale: f32) -> bool {
    if geometry.is_empty() {
        return false;
    }

    let extent = pannable_extent(geometry.scaled(scale), geometry.bounds);
    extent.x > 0.0 || extent.y > 0.0
}

/// Transient state for an origin-relative pointer drag.
///
/// Computing from the initial offset avoids lag after a drag crosses the clamp.
/// The session is not part of persistent view state and must be rebased if the scale changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrabSession {
    origin: Point,
    start_offset: Vector,
    scale: f32,
}

impl GrabSession {
    /// Begin a grab at `origin` with the given transform in effect.
    #[must_use]
    pub fn begin(origin: Point, transform: ViewTransform) -> Self {
        Self {
            origin,
            start_offset: transform.offset(),
            scale: transform.scale(),
        }
    }

    /// Where the grab started.
    #[must_use]
    pub fn origin(self) -> Point {
        self.origin
    }

    /// Restart the grab at `origin` using a new transform, preserving continuity after a mid-drag scale change.
    #[must_use]
    pub fn rebase(self, origin: Point, transform: ViewTransform) -> Self {
        Self::begin(origin, transform)
    }

    /// The transform for a pointer now at `position`.
    ///
    /// The content follows the pointer, so dragging right moves the picture
    /// right, which means the scroll offset decreases — hence the subtraction.
    #[must_use]
    pub fn transform_at(
        self,
        position: Point,
        geometry: ViewportGeometry,
        range: ZoomRange,
    ) -> ViewTransform {
        let delta = Vector::new(position.x - self.origin.x, position.y - self.origin.y);

        let offset = Vector::new(
            self.start_offset.x - if delta.x.is_finite() { delta.x } else { 0.0 },
            self.start_offset.y - if delta.y.is_finite() { delta.y } else { 0.0 },
        );

        clamp_transform(
            ViewTransform {
                scale: self.scale,
                offset,
            },
            geometry,
            range,
        )
    }
}

/// Build semantics for an inspectable image region.
///
/// The image exposes alt text and zoom as a numeric value. `interactive` alone
/// controls the adjustable capability and derived step actions; `focused` only
/// reports focus, so an inert viewer remains readable without advertising an
/// action it cannot perform.
pub fn viewer_semantics(
    alt_text: Option<&str>,
    transform: ViewTransform,
    range: ZoomRange,
    interactive: bool,
    focused: bool,
) -> Semantics {
    Semantics::new(Role::Image)
        .label(alt_text.filter(|t| !t.trim().is_empty()).map_or_else(
            || crate::localization::text("enzui-zoom-pan-image"),
            str::to_owned,
        ))
        .value(format!("{}%", transform.zoom_percent()))
        .adjustable(interactive)
        .numeric(NumericValue {
            value: f64::from(transform.scale()),
            min: f64::from(range.min()),
            max: f64::from(range.max()),
            step: None,
        })
        .focused(focused)
}

/// Semantics for a zoom in/out toolbar control.
///
/// `enabled` reflects whether the step can actually change the scale, so a
/// control pinned at a range end is announced as disabled rather than as an
/// available action that does nothing.
pub fn zoom_control_semantics(direction: ZoomDirection, enabled: bool) -> Semantics {
    let label = match direction {
        ZoomDirection::In => crate::localization::text("enzui-zoom-pan-zoom-in"),
        ZoomDirection::Out => crate::localization::text("enzui-zoom-pan-zoom-out"),
    };

    Semantics::new(Role::Button)
        .label(label)
        .disabled(!enabled)
        .default_action()
}

/// Semantics for the reset/fit control.
pub fn fit_control_semantics(enabled: bool) -> Semantics {
    Semantics::new(Role::Button)
        .label(crate::localization::text("enzui-zoom-pan-fit-to-window"))
        .disabled(!enabled)
        .default_action()
}

/// Whether an in-range transform is away from the range boundary in the given direction.
#[must_use]
pub fn can_zoom(transform: ViewTransform, range: ZoomRange, direction: ZoomDirection) -> bool {
    match direction {
        ZoomDirection::In => transform.scale() < range.max(),
        ZoomDirection::Out => transform.scale() > range.min(),
    }
}
