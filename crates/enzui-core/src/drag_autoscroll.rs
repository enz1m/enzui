//! Pure drag auto-scroll policy.
//!
//! [`AutoScroll::delta`] returns a content-space scroll increment from pointer position, viewport, and elapsed time. Edge speed ramps linearly, output is bounded to one viewport extent per axis, and pointers outside the viewport or non-finite inputs produce zero.

use iced_core::{Point, Rectangle, Vector};
use std::time::Duration;

/// Drag auto-scroll policy with an edge-zone thickness and maximum speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoScroll {
    edge: f32,
    max_speed: f32,
}

impl AutoScroll {
    /// Creates a policy with `edge` pixels of hot zone and `max_speed` content pixels per second.
    ///
    /// Negative or non-finite values normalize to zero; zero disables scrolling.
    #[must_use]
    pub fn new(edge: f32, max_speed: f32) -> Self {
        Self {
            edge: sane_non_neg(edge),
            max_speed: sane_non_neg(max_speed),
        }
    }

    /// The configured hot-zone thickness in px.
    #[must_use]
    pub fn edge(self) -> f32 {
        self.edge
    }

    /// The configured maximum speed in content-px/second.
    #[must_use]
    pub fn max_speed(self) -> f32 {
        self.max_speed
    }

    /// Computes this frame’s content-space scroll delta.
    ///
    /// Positive axes move toward content end. Disabled policy, zero elapsed time, pointers outside the inclusive viewport, and non-finite inputs return zero. Each axis is bounded to one viewport extent; content bounds remain the scrollable’s responsibility.
    #[must_use]
    pub fn delta(self, pointer: Point, viewport: Rectangle, elapsed: Duration) -> Vector {
        // Reject non-finite inputs before arithmetic so NaN cannot propagate into a scroll offset.
        if !pointer.x.is_finite()
            || !pointer.y.is_finite()
            || !viewport.x.is_finite()
            || !viewport.y.is_finite()
            || !viewport.width.is_finite()
            || !viewport.height.is_finite()
        {
            return Vector::ZERO;
        }

        let seconds = elapsed.as_secs_f32();
        if self.edge <= 0.0 || self.max_speed <= 0.0 || seconds <= 0.0 {
            return Vector::ZERO;
        }

        // Use inclusive bounds: each viewport edge is a full-speed position, while any point beyond it is outside.
        if !inside_inclusive(pointer.x, viewport.x, viewport.width)
            || !inside_inclusive(pointer.y, viewport.y, viewport.height)
        {
            return Vector::ZERO;
        }

        let rate_x = self.axis_rate(pointer.x, viewport.x, viewport.width);
        let rate_y = self.axis_rate(pointer.y, viewport.y, viewport.height);

        Vector::new(
            reachable(rate_x * seconds, viewport.width),
            reachable(rate_y * seconds, viewport.height),
        )
    }

    fn axis_rate(self, p: f32, origin: f32, extent: f32) -> f32 {
        // Limit each edge zone to half the extent so opposing zones meet without overlapping.
        let zone = self.edge.min(extent * 0.5);
        if zone <= 0.0 {
            return 0.0;
        }

        let near_boundary = origin + zone;
        let far_boundary = origin + extent - zone;

        if p < near_boundary {
            let depth = (near_boundary - p).min(zone);
            -(depth / zone) * self.max_speed
        } else if p > far_boundary {
            let depth = (p - far_boundary).min(zone);
            (depth / zone) * self.max_speed
        } else {
            0.0
        }
    }
}

fn sane_non_neg(v: f32) -> f32 {
    if v.is_finite() && v >= 0.0 { v } else { 0.0 }
}

fn inside_inclusive(p: f32, origin: f32, extent: f32) -> bool {
    let far = origin + extent.max(0.0);
    p >= origin && p <= far
}

fn reachable(v: f32, extent: f32) -> f32 {
    let bound = extent.max(0.0);
    let clamped = v.clamp(-bound, bound);
    if clamped.is_finite() { clamped } else { 0.0 }
}
