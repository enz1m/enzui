//! Document-tab activation, density, sizing and cell context.

/// Choose whether keyboard navigation activates its destination.
///
/// Defaults to manual activation; the application applies the emitted event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivationMode {
    /// Arrow, Home and End navigation emits [`TabEvent::Activated`](super::TabEvent::Activated).
    Automatic,
    /// Navigation emits [`TabEvent::FocusMoved`](super::TabEvent::FocusMoved);
    /// Enter or Space activates the focused tab.
    #[default]
    Manual,
}

/// Choose cell presentation and an optional along-axis extent limit.
///
/// The density is passed to the tab builder, which supplies compact content and
/// any title tooltip. A valid compact rail limits both pinned and session tab
/// extents before layout and hit-testing, for either strip orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TabDensity {
    /// Use the caller’s full cell presentation without a density extent limit.
    /// This is the default.
    Full,
    /// Request compact cell presentation and limit along-axis extents.
    Compact {
        /// Maximum along-axis extent in pixels: horizontal width or vertical height.
        /// Smaller tabs are not enlarged. Non-positive or non-finite values leave
        /// extents unchanged.
        rail: f32,
    },
}

impl TabDensity {
    pub(crate) fn rail_width(self) -> Option<f32> {
        match self {
            TabDensity::Full => None,
            TabDensity::Compact { rail } if rail.is_finite() && rail > 0.0 => Some(rail),
            TabDensity::Compact { .. } => None,
        }
    }
}

/// Size session tabs along the strip axis against the remaining viewport.
///
/// Pinned tabs are excluded. Empty sessions, non-positive or non-finite viewport
/// extents, and non-finite policy bounds leave the caller’s extents unchanged.
/// A compact density limit is applied after this policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizingPolicy {
    /// Keep per-tab extents from `width_of`, scrolling on overflow. This is the default.
    Scroll,
    /// Keep natural extents when their total fits the viewport.
    ///
    /// On overflow, assign every session tab the same extent: viewport divided by
    /// tab count, raised to `min` and then lowered to `max`. An individual narrow tab
    /// may grow. The bounds apply only on overflow; if `min` exceeds `max`, `max` wins.
    Automatic {
        /// Minimum uniform extent in pixels; overflow can still require scrolling.
        min: f32,
        /// Upper bound in pixels for the uniform overflow extent.
        max: f32,
    },
    /// Divide the remaining viewport equally between session tabs, then apply `min`.
    /// This can grow or shrink tabs; extents above the viewport share require scrolling.
    Fill {
        /// Minimum uniform extent in pixels; overflow can still require scrolling.
        min: f32,
    },
}

impl SizingPolicy {
    pub(crate) fn resolve(self, natural: f64, count: usize, available: f64) -> Option<f32> {
        if count == 0 || !available.is_finite() || available <= 0.0 {
            return None;
        }
        match self {
            SizingPolicy::Scroll => None,
            SizingPolicy::Automatic { min, max } => {
                if !min.is_finite() || !max.is_finite() {
                    return None;
                }
                if natural <= available {
                    return None;
                }
                let each = available / count as f64;
                let clamped = each.max(f64::from(min)).min(f64::from(max));
                Some(clamped as f32)
            }
            SizingPolicy::Fill { min } => {
                if !min.is_finite() {
                    return None;
                }
                let each = available / count as f64;
                Some(each.max(f64::from(min)) as f32)
            }
        }
    }
}

/// Cell state supplied to the document-tab builder.
#[derive(Debug, Clone, Copy)]
pub struct TabContext {
    /// Whether this tab is selected.
    pub selected: bool,
    /// Whether this tab holds the roving keyboard focus stop.
    pub focused: bool,
    /// Whether this tab belongs to the fixed pinned cluster.
    pub pinned: bool,
    /// 1-based position across the whole strip, for `set_position`.
    pub position: usize,
    /// Total tab count, for `set_position`.
    pub count: usize,
    /// Presentation requested by the strip; the builder supplies the compact content
    /// and title tooltip when appropriate.
    pub density: TabDensity,
}
