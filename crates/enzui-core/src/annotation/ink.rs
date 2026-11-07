//! Bounded incremental sampling for freehand strokes.
//!
//! Samples use document coordinates, remain capped by the policy capacity, and preserve stroke endpoints.

use iced_core::Point;

use crate::image_space::ImageProjection;

use super::ShapeGeometry;

/// How a [`StrokeSampler`] bounds its retained points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplingPolicy {
    min_distance: f32,
    capacity: usize,
}

impl Default for SamplingPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl SamplingPolicy {
    /// A one-document-unit spacing with a 512-point capacity.
    pub const DEFAULT: Self = Self {
        min_distance: 1.0,
        capacity: 512,
    };

    /// Creates a policy with document-unit spacing and a point capacity.
    ///
    /// Returns `None` for a negative or non-finite distance or a capacity below two. A zero distance disables spacing-based rejection.
    #[must_use]
    pub fn new(min_distance: f32, capacity: usize) -> Option<Self> {
        (min_distance.is_finite() && min_distance >= 0.0 && capacity >= 2).then_some(Self {
            min_distance,
            capacity,
        })
    }

    /// Converts display-pixel spacing to document units using the projection’s geometric-mean scale.
    ///
    /// Returns `None` under the same conditions as [`Self::new`].
    #[must_use]
    pub fn from_screen(
        min_screen_distance: f32,
        capacity: usize,
        projection: ImageProjection,
    ) -> Option<Self> {
        let s = projection.scale();
        let mean = (s.x.abs() * s.y.abs()).sqrt();
        Self::new(min_screen_distance / mean, capacity)
    }

    /// The initial sampling threshold, in document units.
    #[must_use]
    pub fn min_distance(self) -> f32 {
        self.min_distance
    }

    /// The maximum number of points a stroke may retain.
    #[must_use]
    pub fn capacity(self) -> usize {
        self.capacity
    }
}

/// Incrementally retains a capacity-bounded subset of a freehand stroke.
///
/// Feed samples through [`Self::push`] and finalize them with [`Self::finish`].
#[derive(Debug, Clone)]
pub struct StrokeSampler {
    policy: SamplingPolicy,
    // Doubles after each decimation so future samples match the coarser survivor spacing.
    threshold: f32,
    points: Vec<Point>,
    // Retains the latest offered point so `finish` can preserve the release endpoint.
    last_offered: Option<Point>,
    considered: u64,
    decimations: u32,
}

impl StrokeSampler {
    /// A sampler under `policy`, with no points yet.
    #[must_use]
    pub fn new(policy: SamplingPolicy) -> Self {
        Self {
            policy,
            threshold: policy.min_distance(),
            points: Vec::new(),
            last_offered: None,
            considered: 0,
            decimations: 0,
        }
    }

    /// A sampler under [`SamplingPolicy::DEFAULT`].
    #[must_use]
    pub fn with_default_policy() -> Self {
        Self::new(SamplingPolicy::DEFAULT)
    }

    /// The policy in force.
    #[must_use]
    pub fn policy(&self) -> SamplingPolicy {
        self.policy
    }

    /// Offers one sample in document coordinates.
    ///
    /// Returns whether it was retained. Non-finite points are ignored and do not increment [`Self::considered`].
    pub fn push(&mut self, point: Point) -> bool {
        if !(point.x.is_finite() && point.y.is_finite()) {
            return false;
        }
        self.considered += 1;
        self.last_offered = Some(point);

        match self.points.last() {
            None => {
                self.points.push(point);
                true
            }
            Some(&last) => {
                if distance(last, point) < self.threshold {
                    return false;
                }
                self.points.push(point);
                if self.points.len() >= self.policy.capacity {
                    self.decimate();
                }
                true
            }
        }
    }

    // Keep both endpoints while thinning alternate interior points.
    fn decimate(&mut self) {
        let n = self.points.len();
        debug_assert!(n >= 2, "decimation requires both endpoints");
        let last = self.points[n - 1];
        let mut kept: Vec<Point> = self
            .points
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, p)| p)
            .collect();
        if kept.last() != Some(&last) {
            kept.push(last);
        }
        self.points = kept;
        self.threshold *= 2.0;
        self.decimations += 1;
    }

    /// Returns the retained points in draw order, including during an active stroke.
    #[must_use]
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Returns the number of finite samples offered.
    #[must_use]
    pub fn considered(&self) -> u64 {
        self.considered
    }

    /// Returns the number of capacity-triggered decimations.
    #[must_use]
    pub fn decimations(&self) -> u32 {
        self.decimations
    }

    /// The live threshold, in document units. Doubles on every decimation.
    #[must_use]
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Finishes the stroke and returns its retained points.
    ///
    /// The final offered point is retained even when it is closer than the live threshold.
    #[must_use]
    pub fn finish(mut self) -> Vec<Point> {
        if let (Some(last), Some(&tail)) = (self.last_offered, self.points.last())
            && last != tail
        {
            self.points.push(last);
            if self.points.len() > self.policy.capacity {
                self.decimate();
            }
        }
        self.points
    }

    /// Finishes the stroke as freehand geometry, returning `None` when no finite sample was offered.
    #[must_use]
    pub fn into_geometry(self) -> Option<ShapeGeometry> {
        ShapeGeometry::freehand(self.finish())
    }
}

fn distance(a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}
