//! Shared thickness validation and snap selection for divider rules.
//!
//! Zero disables snapping so native rule painting does not promote it to a
//! physical-pixel minimum. Positive thickness remains in logical pixels;
//! native rule paint applies its scale-aware minimum when snapping is enabled.
//! [`crate::divider`], [`crate::toolbar_divider`] and [`crate::toolbar_spacer`]
//! share this policy.

/// Keep finite, nonnegative thickness unchanged, including zero and fractions.
/// Otherwise use a finite, nonnegative fallback, or 1.0 if both are invalid.
/// Values are logical pixels and are not rounded.
pub fn sanitize_thickness(px: f32, fallback: f32) -> f32 {
    if px.is_finite() && px >= 0.0 {
        px
    } else if fallback.is_finite() && fallback >= 0.0 {
        fallback
    } else {
        1.0
    }
}

/// Enable native rule snapping for strictly positive thickness.
/// Call after [`sanitize_thickness`]: raw negative values and NaN also disable
/// snapping, while positive infinity enables it.
#[inline]
pub fn rule_snap(thickness: f32) -> bool {
    thickness > 0.0
}
