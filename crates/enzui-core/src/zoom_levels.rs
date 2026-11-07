//! Discrete zoom levels and Ctrl+wheel zoom steps.
//!
//! A file grid, an icon view or a list's density zooms through a short
//! ladder of sizes rather than a continuous factor: each level is a size the
//! layout was designed for, so Ctrl+plus from the default always lands on the
//! same, reviewable geometry. [`ZoomLadder`] is that ladder: bounded, with a
//! default for Ctrl+0, and [`ZoomLadder::nearest`] to map a persisted factor
//! back onto it after the ladder changes between releases.
//!
//! [`WheelZoom`] turns Ctrl+wheel into ladder steps. A notched wheel sends one
//! line per notch and should step once per notch; a touchpad sends a stream of
//! small pixel deltas that must add up before anything happens, and a flick
//! must not run from the smallest level to the largest in one event. So
//! deltas accumulate, at most one step leaves per event, and reversing
//! direction starts the count again.
//!
//! This is pure policy. Deciding that Ctrl is held and that the pointer is
//! over the zoomable view is the caller's; so is binding Ctrl+plus/minus/0
//! (usually through the app's shortcut table, so the shortcuts dialog lists
//! them).
//!
//! For a continuous zoom over an image or page, see [`crate::zoom_pan`].

use iced_core::mouse::ScrollDelta;

/// One zoom command, whatever produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomStep {
    /// One level larger.
    In,
    /// One level smaller.
    Out,
    /// Back to the ladder's default level.
    Reset,
}

/// A bounded ladder of zoom factors, smallest first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomLadder {
    factors: &'static [f32],
    default: usize,
}

impl ZoomLadder {
    /// A ladder over `factors` (ascending, finite, positive) whose Ctrl+0
    /// level is `factors[default]`.
    ///
    /// # Panics
    ///
    /// If `factors` is empty, not strictly ascending, holds a non-finite or
    /// non-positive factor, or `default` is out of range. These are
    /// programming errors in a constant table, caught at first use.
    #[must_use]
    pub fn new(factors: &'static [f32], default: usize) -> Self {
        assert!(
            !factors.is_empty(),
            "a zoom ladder needs at least one level"
        );
        assert!(default < factors.len(), "the default level is out of range");
        assert!(
            factors.iter().all(|f| f.is_finite() && *f > 0.0),
            "zoom factors are finite and positive"
        );
        assert!(
            factors.windows(2).all(|pair| pair[0] < pair[1]),
            "zoom factors ascend strictly"
        );
        Self { factors, default }
    }

    /// The default level's index.
    #[must_use]
    pub fn default_index(&self) -> usize {
        self.default
    }

    /// How many levels there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.factors.len()
    }

    /// Always false: a ladder has at least one level.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// `index` clamped into the ladder.
    #[must_use]
    pub fn clamp(&self, index: usize) -> usize {
        index.min(self.factors.len() - 1)
    }

    /// The factor at `index` (clamped).
    #[must_use]
    pub fn factor(&self, index: usize) -> f32 {
        self.factors[self.clamp(index)]
    }

    /// The level `step` leads to from `index`, saturating at either end.
    #[must_use]
    pub fn apply(&self, index: usize, step: ZoomStep) -> usize {
        let index = self.clamp(index);
        match step {
            ZoomStep::In => self.clamp(index + 1),
            ZoomStep::Out => index.saturating_sub(1),
            ZoomStep::Reset => self.default,
        }
    }

    /// Whether Zoom In can still do anything from `index`.
    #[must_use]
    pub fn can_zoom_in(&self, index: usize) -> bool {
        self.clamp(index) + 1 < self.factors.len()
    }

    /// Whether Zoom Out can still do anything from `index`.
    #[must_use]
    pub fn can_zoom_out(&self, index: usize) -> bool {
        self.clamp(index) > 0
    }

    /// The level whose factor is closest to `factor`, for restoring a
    /// persisted zoom; a non-finite value restores the default.
    #[must_use]
    pub fn nearest(&self, factor: f32) -> usize {
        if !factor.is_finite() {
            return self.default;
        }
        self.factors
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (*a - factor).abs().total_cmp(&(*b - factor).abs()))
            .map_or(self.default, |(index, _)| index)
    }
}

/// Pixels of touchpad scroll that count as one wheel notch.
pub const PIXELS_PER_STEP: f32 = 50.0;

/// Accumulates Ctrl+wheel scroll into [`ZoomStep`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelZoom {
    pending: f32,
}

impl WheelZoom {
    /// A fresh accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one scroll event received while Ctrl was held. Scrolling up
    /// (away from the user) zooms in. Returns at most one step.
    pub fn feed(&mut self, delta: ScrollDelta) -> Option<ZoomStep> {
        let notches = match delta {
            ScrollDelta::Lines { y, .. } => y,
            ScrollDelta::Pixels { y, .. } => y / PIXELS_PER_STEP,
        };
        if !notches.is_finite() || notches == 0.0 {
            return None;
        }
        if notches.signum() != self.pending.signum() {
            self.pending = 0.0;
        }
        self.pending += notches;
        if self.pending.abs() < 1.0 {
            return None;
        }
        let step = if self.pending > 0.0 {
            ZoomStep::In
        } else {
            ZoomStep::Out
        };
        self.pending = 0.0;
        Some(step)
    }

    /// Forget a partial gesture (Ctrl released, pointer left the view).
    pub fn reset(&mut self) {
        self.pending = 0.0;
    }
}

/// The pinch growth that counts as one zoom step: a quarter larger.
pub const PINCH_PER_STEP: f32 = 1.25;

/// Accumulates a touchpad pinch into [`ZoomStep`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PinchZoom {
    /// The logarithm of the growth not yet turned into a step.
    pending: f32,
}

impl PinchZoom {
    /// A fresh accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one pinch update: `scale` is the relative change since the last
    /// one, positive when the fingers spread. Returns at most one step; a
    /// pinch that keeps going yields one step per [`PINCH_PER_STEP`] of growth.
    pub fn feed(&mut self, scale: f32) -> Option<ZoomStep> {
        let factor = 1.0 + scale;
        if !factor.is_finite() || factor <= 0.0 {
            return None;
        }
        self.pending += factor.ln();
        let threshold = PINCH_PER_STEP.ln();
        if self.pending >= threshold {
            self.pending -= threshold;
            Some(ZoomStep::In)
        } else if self.pending <= -threshold {
            self.pending += threshold;
            Some(ZoomStep::Out)
        } else {
            None
        }
    }

    /// Forget a partial pinch (the fingers lifted).
    pub fn reset(&mut self) {
        self.pending = 0.0;
    }
}
