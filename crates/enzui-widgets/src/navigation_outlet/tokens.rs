//! Logical-pixel geometry and initial timing for navigation outlets.

use super::*;

/// Geometry overrides for page translation and the live edge-swipe adapter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavigationOutletTokens {
    /// How far a page slides in from, in logical pixels.
    pub transition_distance: f32,
    /// Width of the logical leading-edge press strip: left in LTR, right in RTL.
    pub back_swipe_edge_width: f32,
    /// Travel required to commit a back swipe, in logical pixels.
    /// Zero is retained by sanitization but uses 72 px in the gesture adapter.
    pub back_swipe_threshold: f32,
}

impl NavigationOutletTokens {
    /// The default geometry: a 32 px slide, a 24 px swipe edge and a 72 px
    /// swipe threshold.
    pub const DEFAULT: Self = Self {
        transition_distance: 32.0,
        back_swipe_edge_width: 24.0,
        back_swipe_threshold: 72.0,
    };

    /// Replace negative or non-finite fields with [`Self::DEFAULT`] values.
    /// Zero is retained in every field. [`EnzNavigationOutlet::tokens`] applies this
    /// boundary before page translation and gesture-policy construction.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::DEFAULT;
        Self {
            transition_distance: FiniteNonNeg::new_or(
                self.transition_distance,
                d.transition_distance,
            )
            .get(),
            back_swipe_edge_width: FiniteNonNeg::new_or(
                self.back_swipe_edge_width,
                d.back_swipe_edge_width,
            )
            .get(),
            back_swipe_threshold: FiniteNonNeg::new_or(
                self.back_swipe_threshold,
                d.back_swipe_threshold,
            )
            .get(),
        }
    }
}

impl Default for NavigationOutletTokens {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Initial transition duration: 180 ms from the default motion scale.
/// [`NavigationTransition::sync_motion`] replaces it with host-supplied timing.
pub const TRANSITION_DURATION: Duration = Duration::from_millis(180);
