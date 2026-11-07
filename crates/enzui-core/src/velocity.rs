//! Pointer velocity at release, for flings and springs that carry it on.
//!
//! [`VelocityTracker`] keeps the last pointer positions of a drag and
//! estimates the velocity at the moment of release by fitting a quadratic
//! through the samples of the last 100 ms (least squares, the estimator
//! Android and Flutter use for touch). One noisy sample, a duplicate event or
//! an uneven event rate barely moves the estimate, where the difference of
//! the last two samples can swing by thousands of pixels per second.
//!
//! A pointer that stopped before lifting has no velocity: samples older than
//! 40 ms before the newest one, or a release 40 ms after it, count as a stop.
//!
//! ```
//! use enzui_core::velocity::VelocityTracker;
//! use std::time::{Duration, Instant};
//!
//! let start = Instant::now();
//! let mut tracker = VelocityTracker::new();
//! for step in 0..8 {
//!     let at = start + Duration::from_millis(8 * step);
//!     tracker.add(at, [step as f32 * 4.0, 0.0]);
//! }
//! let [vx, _] = tracker.velocity(start + Duration::from_millis(56));
//! assert!((vx - 500.0).abs() < 1.0, "4 px every 8 ms: {vx}");
//! ```

use std::time::{Duration, Instant};

/// How far back samples are fitted.
pub const HORIZON: Duration = Duration::from_millis(100);

/// A gap this long between samples, or between the last sample and the
/// release, means the pointer stopped.
pub const STOP_GAP: Duration = Duration::from_millis(40);

/// The most samples kept.
const CAPACITY: usize = 20;

/// The fastest velocity reported, in units per second, a bound for a
/// glitching device that reports a jump in one event.
pub const MAX_VELOCITY: f32 = 16_000.0;

/// Recent positions of one pointer, in two dimensions. See the [module](self)
/// documentation.
///
/// `Copy` and allocation-free, so it can sit inside gesture state that is.
/// Time is either real instants ([`Self::add`], [`Self::velocity`]) or a
/// running clock in seconds the caller keeps ([`Self::add_secs`],
/// [`Self::velocity_secs`]), for gestures that measure time between frames
/// rather than by wall clock. Use one or the other for a drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VelocityTracker {
    /// Ring of `(seconds, position)`, oldest first from `head`.
    samples: [(f64, [f32; 2]); CAPACITY],
    len: usize,
    head: usize,
    /// The instant at second zero, for the `Instant` API.
    origin: Option<Instant>,
}

impl Default for VelocityTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl VelocityTracker {
    /// A tracker with no samples.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            samples: [(0.0, [0.0; 2]); CAPACITY],
            len: 0,
            head: 0,
            origin: None,
        }
    }

    /// Forget every sample, at a new press.
    pub const fn reset(&mut self) {
        *self = Self::new();
    }

    /// Record the pointer at `position` at `at`. A non-finite position is
    /// ignored; a sample older than the newest restarts the track, since time
    /// cannot run backwards within one drag.
    pub fn add(&mut self, at: Instant, position: [f32; 2]) {
        let origin = *self.origin.get_or_insert(at);
        let seconds = match at.checked_duration_since(origin) {
            Some(elapsed) => elapsed.as_secs_f64(),
            None => {
                self.reset();
                self.origin = Some(at);
                0.0
            }
        };
        self.add_secs(seconds, position);
    }

    /// Record the pointer at `position` at `seconds` on the caller's clock.
    pub fn add_secs(&mut self, seconds: f64, position: [f32; 2]) {
        if !seconds.is_finite() || !position.iter().all(|value| value.is_finite()) {
            return;
        }
        if self.newest().is_some_and(|(last, _)| seconds < last) {
            let origin = self.origin;
            self.reset();
            self.origin = origin;
        }
        if self.len == CAPACITY {
            self.head = (self.head + 1) % CAPACITY;
            self.len -= 1;
        }
        self.samples[(self.head + self.len) % CAPACITY] = (seconds, position);
        self.len += 1;
    }

    fn newest(&self) -> Option<(f64, [f32; 2])> {
        (self.len > 0).then(|| self.samples[(self.head + self.len - 1) % CAPACITY])
    }

    /// The velocity at `release`, in units per second per axis: zero when
    /// the pointer stopped, or with fewer than two recent samples.
    #[must_use]
    pub fn velocity(&self, release: Instant) -> [f32; 2] {
        let Some(origin) = self.origin else {
            return [0.0; 2];
        };
        match release.checked_duration_since(origin) {
            Some(elapsed) => self.velocity_secs(elapsed.as_secs_f64()),
            None => [0.0; 2],
        }
    }

    /// [`Self::velocity`] at `release` seconds on the caller's clock.
    #[must_use]
    pub fn velocity_secs(&self, release: f64) -> [f32; 2] {
        let Some((newest, _)) = self.newest() else {
            return [0.0; 2];
        };
        let horizon = HORIZON.as_secs_f64();
        let stop = STOP_GAP.as_secs_f64();
        if release - newest > stop {
            return [0.0; 2];
        }
        // The samples of this movement: within the horizon, and back to the
        // first stop between two of them.
        let mut recent = [(0.0, [0.0_f32; 2]); CAPACITY];
        let mut count = 0;
        let mut later = newest;
        for index in (0..self.len).rev() {
            let (at, position) = self.samples[(self.head + index) % CAPACITY];
            if newest - at > horizon || later - at > stop {
                break;
            }
            recent[count] = (at - newest, position);
            count += 1;
            later = at;
        }
        let recent = &recent[..count];
        if recent.len() < 2 {
            return [0.0; 2];
        }
        [0, 1].map(|axis| {
            let mut points = [(0.0, 0.0); CAPACITY];
            for (point, (t, position)) in points.iter_mut().zip(recent) {
                *point = (*t, f64::from(position[axis]));
            }
            let points = &points[..recent.len()];
            let slope = if points.len() >= 3 {
                quadratic_slope(points).unwrap_or_else(|| linear_slope(points))
            } else {
                linear_slope(points)
            };
            (within_recent_segments(slope, points) as f32).clamp(-MAX_VELOCITY, MAX_VELOCITY)
        })
    }
}

/// `slope` kept near what the pointer actually did last: within the range
/// of the newest three segment velocities, widened by a quarter of their
/// spread. A parabola fitted across a sudden change of speed extrapolates far
/// past either speed; this keeps a flick from launching faster than the
/// finger ever moved. `points` run newest first.
fn within_recent_segments(slope: f64, points: &[(f64, f64)]) -> f64 {
    let segments = points
        .windows(2)
        .filter_map(|pair| {
            let ((t1, x1), (t0, x0)) = (pair[0], pair[1]);
            let dt = t1 - t0;
            (dt > 1e-6).then(|| (x1 - x0) / dt)
        })
        .take(3);
    let (low, high) = segments.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), v| {
        (low.min(v), high.max(v))
    });
    if !low.is_finite() {
        return slope;
    }
    let margin = (high - low) * 0.25;
    slope.clamp(low - margin, high + margin)
}

/// The slope at `t = 0` of the least-squares line through `points`.
fn linear_slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    let (st, sx) = points
        .iter()
        .fold((0.0, 0.0), |(st, sx), (t, x)| (st + t, sx + x));
    let (mt, mx) = (st / n, sx / n);
    let (cov, var) = points.iter().fold((0.0, 0.0), |(cov, var), (t, x)| {
        (cov + (t - mt) * (x - mx), var + (t - mt) * (t - mt))
    });
    if var <= f64::EPSILON { 0.0 } else { cov / var }
}

/// The slope at `t = 0` of the least-squares parabola through `points`, or
/// `None` when the samples cannot determine one (all at two instants).
fn quadratic_slope(points: &[(f64, f64)]) -> Option<f64> {
    // Normal equations for x = a + b t + c t², solved by Cramer's rule.
    let mut s = [0.0_f64; 5];
    let mut r = [0.0_f64; 3];
    for &(t, x) in points {
        let mut power = 1.0;
        for (index, sum) in s.iter_mut().enumerate() {
            *sum += power;
            if index < 3 {
                r[index] += power * x;
            }
            power *= t;
        }
    }
    let m = [[s[0], s[1], s[2]], [s[1], s[2], s[3]], [s[2], s[3], s[4]]];
    let det = determinant(m);
    if det.abs() <= 1e-18 * s[4].max(1e-12) {
        return None;
    }
    let with_b = [
        [m[0][0], r[0], m[0][2]],
        [m[1][0], r[1], m[1][2]],
        [m[2][0], r[2], m[2][2]],
    ];
    Some(determinant(with_b) / det)
}

fn determinant(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}
