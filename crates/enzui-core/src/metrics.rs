//! Validated scalar and ordered-range types for public geometry and widget APIs.
//!
//! Fallible constructors reject invalid values, while normalizing constructors use documented fallbacks. Conversion helpers saturate values before they reach layout or bounded state.

use core::fmt;

/// A finite `f32` strictly greater than zero, suitable for nondegenerate extents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FinitePositive(f32);

impl FinitePositive {
    /// The multiplicative identity (`1.0`), always valid.
    pub const ONE: Self = Self(1.0);

    /// Reject invalid input (NaN / inf / `<= 0`), returning `None`.
    #[must_use]
    pub fn new(v: f32) -> Option<Self> {
        if v.is_finite() && v > 0.0 {
            Some(Self(v))
        } else {
            None
        }
    }

    /// Substitutes `fallback` for invalid input, or [`Self::ONE`] when the fallback is also invalid.
    ///
    /// Invalid fallbacks trigger a debug assertion.
    #[must_use]
    pub fn new_or(v: f32, fallback: f32) -> Self {
        debug_assert!(
            fallback.is_finite() && fallback > 0.0,
            "FinitePositive fallback must itself be finite and positive"
        );
        Self::new(v).unwrap_or_else(|| Self::new(fallback).unwrap_or(Self::ONE))
    }

    /// The validated inner value.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

impl fmt::Display for FinitePositive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A finite, non-negative `f32`, suitable for geometry where zero is valid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FiniteNonNeg(f32);

impl FiniteNonNeg {
    /// The additive identity (`0.0`), always valid.
    pub const ZERO: Self = Self(0.0);

    /// Reject invalid input (NaN / inf / negative), returning `None`.
    #[must_use]
    pub fn new(v: f32) -> Option<Self> {
        if v.is_finite() && v >= 0.0 {
            Some(Self(v))
        } else {
            None
        }
    }

    /// Substitutes `fallback` for invalid input, or [`Self::ZERO`] when the fallback is also invalid.
    ///
    /// Invalid fallbacks trigger a debug assertion.
    #[must_use]
    pub fn new_or(v: f32, fallback: f32) -> Self {
        debug_assert!(
            fallback.is_finite() && fallback >= 0.0,
            "FiniteNonNeg fallback must itself be finite and non-negative"
        );
        Self::new(v).unwrap_or_else(|| Self::new(fallback).unwrap_or(Self::ZERO))
    }

    /// The validated inner value.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

impl fmt::Display for FiniteNonNeg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A finite `f32` of any sign, suitable for signed geometry such as offsets and deltas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FiniteF32(f32);

impl FiniteF32 {
    /// The additive identity (`0.0`), always valid.
    pub const ZERO: Self = Self(0.0);

    /// Reject non-finite input (NaN / `±inf`), returning `None`. Any finite
    /// value — positive, zero, or negative — is accepted.
    #[must_use]
    pub fn new(v: f32) -> Option<Self> {
        if v.is_finite() { Some(Self(v)) } else { None }
    }

    /// Preserves finite input, otherwise using `fallback` or [`Self::ZERO`] if the fallback is non-finite.
    ///
    /// Invalid fallbacks trigger a debug assertion.
    #[must_use]
    pub fn new_or(v: f32, fallback: f32) -> Self {
        debug_assert!(
            fallback.is_finite(),
            "FiniteF32 fallback must itself be finite"
        );
        Self::new(v).unwrap_or_else(|| Self::new(fallback).unwrap_or(Self::ZERO))
    }

    /// The validated inner value.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

impl fmt::Display for FiniteF32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A finite `f32` in the closed interval `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitInterval(f32);

impl UnitInterval {
    /// The lower bound (`0.0`).
    pub const ZERO: Self = Self(0.0);
    /// The upper bound (`1.0`).
    pub const ONE: Self = Self(1.0);

    /// Returns `None` for non-finite values or values outside `0.0..=1.0`.
    #[must_use]
    pub fn new(v: f32) -> Option<Self> {
        if v.is_finite() && (0.0..=1.0).contains(&v) {
            Some(Self(v))
        } else {
            None
        }
    }

    /// Maps non-finite input to [`Self::ZERO`] and clamps finite input into `0.0..=1.0`.
    #[must_use]
    pub fn clamped(v: f32) -> Self {
        if v.is_finite() {
            Self(v.clamp(0.0, 1.0))
        } else {
            Self::ZERO
        }
    }

    /// The validated inner value.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

impl fmt::Display for UnitInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An ordered finite `f32` range whose validated bounds make [`Self::clamp`] non-panicking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrderedRangeF32 {
    lo: f32,
    hi: f32,
}

impl OrderedRangeF32 {
    /// Reject non-finite bounds or a reversed pair (`lo > hi`).
    #[must_use]
    pub const fn new(lo: f32, hi: f32) -> Option<Self> {
        if lo.is_finite() && hi.is_finite() && lo <= hi {
            Some(Self { lo, hi })
        } else {
            None
        }
    }

    /// Sorts finite bounds into order, returning `None` for non-finite input; equal bounds are allowed.
    #[must_use]
    pub fn new_sorted(a: f32, b: f32) -> Option<Self> {
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        Some(if a <= b {
            Self { lo: a, hi: b }
        } else {
            Self { lo: b, hi: a }
        })
    }

    /// Lower bound.
    #[must_use]
    pub fn lo(self) -> f32 {
        self.lo
    }

    /// Upper bound.
    #[must_use]
    pub fn hi(self) -> f32 {
        self.hi
    }

    /// Clamps a finite value into `[lo, hi]`; NaN is returned unchanged, and validated bounds prevent panics.
    #[must_use]
    pub fn clamp(self, v: f32) -> f32 {
        v.clamp(self.lo, self.hi)
    }

    /// Linearly interpolates at `t` using `lo * (1 - t) + hi * t`, avoiding overflow in `hi - lo` for extreme finite bounds.
    #[must_use]
    pub fn lerp(self, t: UnitInterval) -> f32 {
        let t = t.get();
        self.lo * (1.0 - t) + self.hi * t
    }
}

/// An ordered `i32` range whose validated bounds make [`Self::clamp`] non-panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderedRangeI32 {
    lo: i32,
    hi: i32,
}

impl OrderedRangeI32 {
    /// Reject a reversed pair (`lo > hi`).
    #[must_use]
    pub fn new(lo: i32, hi: i32) -> Option<Self> {
        if lo <= hi {
            Some(Self { lo, hi })
        } else {
            None
        }
    }

    /// Normalize a possibly-reversed pair by sorting; cannot fail.
    #[must_use]
    pub fn new_sorted(a: i32, b: i32) -> Self {
        if a <= b {
            Self { lo: a, hi: b }
        } else {
            Self { lo: b, hi: a }
        }
    }

    /// Lower bound.
    #[must_use]
    pub fn lo(self) -> i32 {
        self.lo
    }

    /// Upper bound.
    #[must_use]
    pub fn hi(self) -> i32 {
        self.hi
    }

    /// Clamp a value into `[lo, hi]`. Never panics because construction
    /// guaranteed `lo <= hi`.
    #[must_use]
    pub fn clamp(self, v: i32) -> i32 {
        v.clamp(self.lo, self.hi)
    }
}

/// The columns of a row-major grid that can hold an item, separated from the caller-declared count.
///
/// A declared column count is external input and may exceed the number of items
/// packed into it by any margin, up to `usize::MAX` beside a single item. Row
/// major packing puts item `i` in column `i % declared`, so only the first
/// [`Self::occupied`] columns can ever receive one: transient per-column storage
/// (track widths, cumulative edges, a row's cells) is sized from that, never
/// from [`Self::declared`], which alone keeps deciding geometry, spacing,
/// placement and the column count reported to callers. Zero declared columns
/// normalize to one, since row-major packing needs a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowMajorColumns {
    declared: usize,
    occupied: usize,
}

impl RowMajorColumns {
    /// Resolves a declared column count against the number of items packed into it.
    #[must_use]
    pub fn new(declared_columns: usize, item_count: usize) -> Self {
        let declared = declared_columns.max(1);
        RowMajorColumns {
            declared,
            occupied: declared.min(item_count),
        }
    }

    /// The declared column count, normalized to at least one.
    #[must_use]
    pub fn declared(self) -> usize {
        self.declared
    }

    /// How many leading columns hold at least one item: `min(declared, item_count)`.
    #[must_use]
    pub fn occupied(self) -> usize {
        self.occupied
    }

    /// Cumulative-edge count for the occupied columns, saturating instead of wrapping to zero.
    #[must_use]
    pub fn occupied_edges(self) -> usize {
        self.occupied.saturating_add(1)
    }
}

/// Converts `f32` to `u16`, mapping NaN and non-positive values to zero, values at or above `u16::MAX` to `u16::MAX`, and rounding the remainder.
#[must_use]
pub fn to_u16_saturating(v: f32) -> u16 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= f32::from(u16::MAX) {
        u16::MAX
    } else {
        v.round() as u16
    }
}

/// Converts `f32` to `i32`, mapping NaN to zero, saturating out-of-range values including infinities, and rounding remaining finite values.
#[must_use]
pub fn to_i32_saturating(v: f32) -> i32 {
    if v.is_nan() {
        0
    } else if v >= i32::MAX as f32 {
        i32::MAX
    } else if v <= i32::MIN as f32 {
        i32::MIN
    } else {
        v.round() as i32
    }
}

#[cfg(test)]
mod tests {

    // Invalid fallbacks trigger debug assertions and degrade to the type identity in release builds.
}
