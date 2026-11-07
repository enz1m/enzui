//! Responsive breakpoint tokens: at what width a layout should reflow.
//!
//! [`BreakpointScale`] holds the edge values; [`EnzSizeClass`] classifies a
//! width into a band. Classify the component width, not the window width.
//!
//! | Band | Default range |
//! |---|---|
//! | `Compact` | `0 < width < 480` |
//! | `Regular` | `480 <= width < 720` |
//! | `Medium` | `720 <= width < 1024` |
//! | `Expanded` | `1024 <= width < 1440` |
//! | `Large` | `width >= 1440`, non-positive, or non-finite |
//!
//! # Example
//!
//! ```
//! use enzui_theme::{BreakpointScale, EnzSizeClass};
//!
//! let bp = BreakpointScale::default();
//! assert_eq!(bp.classify(500.0), EnzSizeClass::Regular);
//! ```

/// A coarse responsive size class of a laid-out width, ordered narrow → wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnzSizeClass {
    /// Positive widths below the scale's `sm` edge; `0 < width < 480` by default.
    Compact,
    /// Widths from `sm` through just below `md`; `480..720` by default.
    Regular,
    /// Widths from `md` through just below `lg`; `720..1024` by default.
    Medium,
    /// Widths from `lg` through just below `xl`; `1024..1440` by default.
    Expanded,
    /// Widths at or above `xl`, plus non-positive and non-finite inputs;
    /// `width >= 1440` by default for valid positive widths.
    Large,
}

/// The semantic breakpoint scale (all values in logical px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BreakpointScale {
    xs: f32,
    sm: f32,
    md: f32,
    lg: f32,
    xl: f32,
}

impl BreakpointScale {
    /// Constructs a scale with finite, non-negative, non-decreasing edges.
    /// Invalid values fall back to their default edge; descending values clamp
    /// to the preceding edge, so equal adjacent edges are permitted.
    pub fn new(xs: f32, sm: f32, md: f32, lg: f32, xl: f32) -> Self {
        let d = Self::raw_default();
        let fix = |v: f32, fb: f32| if v.is_finite() && v >= 0.0 { v } else { fb };
        let xs = fix(xs, d.xs);
        let sm = fix(sm, d.sm).max(xs);
        let md = fix(md, d.md).max(sm);
        let lg = fix(lg, d.lg).max(md);
        let xl = fix(xl, d.xl).max(lg);
        Self { xs, sm, md, lg, xl }
    }

    const fn raw_default() -> Self {
        Self {
            xs: 360.0,
            sm: 480.0,
            md: 720.0,
            lg: 1024.0,
            xl: 1440.0,
        }
    }

    /// Phone-portrait edge, logical px (default 360).
    #[must_use]
    pub const fn xs(&self) -> f32 {
        self.xs
    }
    /// Phone-landscape / narrow-tablet edge, logical px (default 480). This is
    /// the single-threshold edge most "stack below X" decisions compare against.
    #[must_use]
    pub const fn sm(&self) -> f32 {
        self.sm
    }
    /// Tablet-portrait / small-laptop edge, logical px (default 720).
    #[must_use]
    pub const fn md(&self) -> f32 {
        self.md
    }
    /// Laptop / desktop edge, logical px (default 1024).
    #[must_use]
    pub const fn lg(&self) -> f32 {
        self.lg
    }
    /// Wide-desktop / multi-pane edge, logical px (default 1440).
    #[must_use]
    pub const fn xl(&self) -> f32 {
        self.xl
    }

    /// Reports the non-descending edge invariant guaranteed by every public
    /// constructor.
    #[must_use]
    pub fn is_monotonic(&self) -> bool {
        self.xs <= self.sm && self.sm <= self.md && self.md <= self.lg && self.lg <= self.xl
    }

    /// Classifies `width` into half-open bands ending at `sm`, `md`, `lg`, and
    /// `xl`; values on an edge enter the wider band. Non-finite or non-positive
    /// widths return [`EnzSizeClass::Large`] to avoid a transient compact layout
    /// before measurement.
    #[must_use]
    pub fn classify(&self, width: f32) -> EnzSizeClass {
        if !width.is_finite() || width <= 0.0 {
            return EnzSizeClass::Large;
        }
        debug_assert!(
            self.is_monotonic(),
            "BreakpointScale invariant violated: edges must be non-descending"
        );
        if width < self.sm {
            EnzSizeClass::Compact
        } else if width < self.md {
            EnzSizeClass::Regular
        } else if width < self.lg {
            EnzSizeClass::Medium
        } else if width < self.xl {
            EnzSizeClass::Expanded
        } else {
            EnzSizeClass::Large
        }
    }
}

impl Default for BreakpointScale {
    fn default() -> Self {
        Self::raw_default()
    }
}
