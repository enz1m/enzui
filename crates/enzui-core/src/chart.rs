//! Chart data models, scales, decimation, hit testing, traversal, and table projection.
//!
//! Coordinates and domains use `f64` to preserve resolution across wide numeric ranges.

/// Identifies one series within a chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeriesId(pub u32);

/// Identifies one datum within its series. Stable across reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DatumKey(pub u64);

/// One data point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Datum {
    /// X coordinate in data space.
    pub x: f64,
    /// Y coordinate in data space.
    pub y: f64,
    /// Stable identity — survives reordering and incremental updates.
    pub key: DatumKey,
}

/// An ordered series of data points.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// Stable series identity.
    pub id: SeriesId,
    /// Points in order. The series owns the original, un-decimated data.
    pub points: Vec<Datum>,
}

impl Series {
    /// Build a series from `(x, y)` pairs, keyed by original position.
    pub fn from_points(id: SeriesId, points: impl IntoIterator<Item = (f64, f64)>) -> Self {
        Self {
            id,
            points: points
                .into_iter()
                .enumerate()
                .map(|(i, (x, y))| Datum {
                    x,
                    y,
                    key: DatumKey(i as u64),
                })
                .collect(),
        }
    }

    /// Number of points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the series has no points.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// A monotone cubic curve through `points`, sampled `steps` times per segment
/// (Fritsch–Carlson).
///
/// The curve passes through every point and never leaves the range of the
/// two points of a segment, so a peak is not overshot and a non-negative
/// series never dips below zero. Endpoint slopes are zero. A non-finite value
/// is a gap: it is kept as-is and each finite run is smoothed separately.
/// Points with non-increasing `x` are returned unchanged, unsmoothed. This is
/// presentation only; the measured points stay the data.
#[must_use]
pub fn monotone_cubic(points: &[(f64, f64)], steps: usize) -> Vec<(f64, f64)> {
    let steps = steps.max(1);
    let mut curve = Vec::with_capacity(points.len().saturating_mul(steps));
    let mut run: Vec<(f64, f64)> = Vec::new();
    for &point in points {
        if point.0.is_finite() && point.1.is_finite() {
            run.push(point);
        } else {
            curve.extend(monotone_run(&run, steps));
            run.clear();
            curve.push(point);
        }
    }
    curve.extend(monotone_run(&run, steps));
    curve
}

fn monotone_run(points: &[(f64, f64)], steps: usize) -> Vec<(f64, f64)> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let intervals: Vec<f64> = points.windows(2).map(|p| p[1].0 - p[0].0).collect();
    if intervals
        .iter()
        .any(|step| !step.is_finite() || *step <= 0.0)
    {
        return points.to_vec();
    }
    let secants: Vec<f64> = points
        .windows(2)
        .zip(&intervals)
        .map(|(p, h)| (p[1].1 - p[0].1) / h)
        .collect();
    let mut slopes = vec![0.0; points.len()];
    for i in 1..points.len() - 1 {
        let (left, right) = (secants[i - 1], secants[i]);
        // A local extremum or a flat neighbour gets a flat tangent; otherwise
        // the weighted harmonic mean keeps the segment monotone.
        if left * right > 0.0 {
            let w1 = 2.0 * intervals[i] + intervals[i - 1];
            let w2 = intervals[i] + 2.0 * intervals[i - 1];
            slopes[i] = (w1 + w2) / (w1 / left + w2 / right);
        }
    }
    let mut curve = Vec::with_capacity((points.len() - 1) * steps + 1);
    for (i, pair) in points.windows(2).enumerate() {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let h = intervals[i];
        for step in 0..steps {
            let t = step as f64 / steps as f64;
            let (t2, t3) = (t * t, t * t * t);
            let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
                + (t3 - 2.0 * t2 + t) * h * slopes[i]
                + (-2.0 * t3 + 3.0 * t2) * y1
                + (t3 - t2) * h * slopes[i + 1];
            curve.push((x0 + (x1 - x0) * t, y.clamp(y0.min(y1), y0.max(y1))));
        }
    }
    curve.push(points[points.len() - 1]);
    curve
}

/// A smooth curve near `points`, sampled `steps` times per segment: a
/// uniform cubic B-spline with the samples as control points, clamped so it
/// starts and ends on the first and last point.
///
/// Unlike [`monotone_cubic`] it does not pass through every point: a lone
/// spike is drawn as a rounded hump a little below its value. Each stretch
/// of the curve stays within the range of the four points that shape it, so
/// a non-negative series never dips below zero. A non-finite value is a gap,
/// kept as-is, and each finite run is smoothed separately. This is
/// presentation only; the measured points stay the data.
#[must_use]
pub fn smooth_spline(points: &[(f64, f64)], steps: usize) -> Vec<(f64, f64)> {
    let steps = steps.max(1);
    let mut curve = Vec::with_capacity(points.len().saturating_mul(steps));
    let mut run: Vec<(f64, f64)> = Vec::new();
    for &point in points {
        if point.0.is_finite() && point.1.is_finite() {
            run.push(point);
        } else {
            curve.extend(spline_run(&run, steps));
            run.clear();
            curve.push(point);
        }
    }
    curve.extend(spline_run(&run, steps));
    curve
}

fn spline_run(points: &[(f64, f64)], steps: usize) -> Vec<(f64, f64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    // The end points are repeated so the curve starts and ends on them.
    let (first, last) = (points[0], points[points.len() - 1]);
    let control: Vec<(f64, f64)> = [first, first]
        .into_iter()
        .chain(points.iter().copied())
        .chain([last, last])
        .collect();
    let mut curve = Vec::with_capacity((control.len() - 3) * steps + 1);
    for window in control.windows(4) {
        for step in 0..steps {
            let t = step as f64 / steps as f64;
            let (t2, t3) = (t * t, t * t * t);
            let weights = [
                (1.0 - t).powi(3) / 6.0,
                (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
                (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
                t3 / 6.0,
            ];
            let point = window
                .iter()
                .zip(weights)
                .fold((0.0, 0.0), |(x, y), (p, w)| (x + p.0 * w, y + p.1 * w));
            curve.push(point);
        }
    }
    // The repeated end makes the first stretch start on `first`; the last
    // sample of the last stretch is `last` itself.
    curve.push(last);
    curve
}

/// Maps a data domain linearly onto a pixel range.
///
/// A degenerate domain maps every value to the range midpoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearScale {
    /// `(min, max)` in data space.
    pub domain: (f64, f64),
    /// `(min, max)` in pixel space.
    pub range: (f64, f64),
}

impl LinearScale {
    /// Map a data-space value to pixel space.
    #[must_use]
    pub fn map(&self, value: f64) -> f64 {
        let d_span = self.domain.1 - self.domain.0;
        if d_span == 0.0 {
            (self.range.0 + self.range.1) * 0.5
        } else {
            let t = (value - self.domain.0) / d_span;
            self.range.0 + t * (self.range.1 - self.range.0)
        }
    }

    /// Maps a pixel-space value to data space; a zero-width pixel range maps to the domain midpoint.
    #[must_use]
    pub fn invert(&self, pixel: f64) -> f64 {
        let r_span = self.range.1 - self.range.0;
        if r_span == 0.0 {
            (self.domain.0 + self.domain.1) * 0.5
        } else {
            let t = (pixel - self.range.0) / r_span;
            self.domain.0 + t * (self.domain.1 - self.domain.0)
        }
    }
}

/// A category band with a leading edge in domain order and a non-negative pixel width.
///
/// For reversed ranges, the band occupies `[start - width, start]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// Leading pixel edge of the band, in domain order.
    pub start: f64,
    /// Non-negative pixel extent of the band.
    pub width: f64,
}

/// Maps ordered category indices to pixel bands.
///
/// Inner and outer padding are fractions of the step and are clamped to `[0, 1]`. Empty domains yield no bands; zero-width bands are not hittable. Reversed ranges preserve domain order while keeping widths non-negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandScale {
    n: usize,
    range: (f64, f64),
    padding_inner: f64,
    padding_outer: f64,
}

impl BandScale {
    /// Creates an unpadded scale for `n` categories over `range`.
    ///
    /// The range may be reversed for axes whose pixels decrease in domain order.
    #[must_use]
    pub fn new(n: usize, range: (f64, f64)) -> Self {
        Self {
            n,
            range,
            padding_inner: 0.0,
            padding_outer: 0.0,
        }
    }

    /// Sets inner padding as a step fraction clamped to `[0, 1]`.
    ///
    /// Non-finite input leaves the existing inner padding unchanged.
    #[must_use]
    pub fn padding_inner(mut self, fraction: f64) -> Self {
        if fraction.is_finite() {
            self.padding_inner = fraction.clamp(0.0, 1.0);
        }
        self
    }

    /// Sets outer padding as a step fraction clamped to `[0, 1]`.
    ///
    /// Non-finite input leaves the existing outer padding unchanged.
    #[must_use]
    pub fn padding_outer(mut self, fraction: f64) -> Self {
        if fraction.is_finite() {
            self.padding_outer = fraction.clamp(0.0, 1.0);
        }
        self
    }

    /// Sets both padding fractions, each clamped independently to `[0, 1]`.
    #[must_use]
    pub fn padding(self, fraction: f64) -> Self {
        self.padding_inner(fraction).padding_outer(fraction)
    }

    /// Number of categories.
    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Whether the domain is empty (zero categories).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn sign(&self) -> f64 {
        if self.range.1 < self.range.0 {
            -1.0
        } else {
            1.0
        }
    }

    fn span(&self) -> f64 {
        (self.range.1 - self.range.0).abs()
    }

    // The denominator may fall below one for a padded singleton; that case leaves residual span to center.
    fn natural_denom(&self) -> f64 {
        self.n as f64 - self.padding_inner + 2.0 * self.padding_outer
    }

    /// The step: the centre-to-centre distance between adjacent bands, in
    /// pixels (always non-negative). Returns `0.0` for an empty domain.
    #[must_use]
    pub fn step(&self) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        self.span() / self.natural_denom().max(1.0)
    }

    /// The pixel width of a single band. Clamped to `>= 0`: over-padding
    /// yields zero width (expressible intent), never a negative one.
    #[must_use]
    pub fn bandwidth(&self) -> f64 {
        (self.step() * (1.0 - self.padding_inner)).max(0.0)
    }

    // Splits residual span equally before and after the bands when the natural denominator is below one.
    fn leading_offset(&self) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        let step = self.step();
        let layout_span = step * self.natural_denom();
        (self.span() - layout_span).max(0.0) * 0.5
    }

    /// The band for category `index`, or `None` if `index >= n` (which
    /// includes every index when the domain is empty).
    #[must_use]
    pub fn band(&self, index: usize) -> Option<Band> {
        if index >= self.n {
            return None;
        }
        let step = self.step();
        let sign = self.sign();
        let start = self.range.0
            + sign * (self.leading_offset() + step * self.padding_outer + step * index as f64);
        Some(Band {
            start,
            width: self.bandwidth(),
        })
    }

    /// The pixel centre of category `index` — where a tick label or scatter
    /// point belongs — or `None` if out of range.
    #[must_use]
    pub fn center(&self, index: usize) -> Option<f64> {
        let band = self.band(index)?;
        Some(band.start + self.sign() * 0.5 * band.width)
    }

    /// Returns the category whose half-open band contains `pixel`.
    ///
    /// Pixels outside the range or in padding or gaps return `None`; zero-width bands are never hits.
    #[must_use]
    pub fn invert(&self, pixel: f64) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        let step = self.step();
        if step == 0.0 {
            return None;
        }
        let sign = self.sign();
        let offset = sign * (pixel - self.range.0);
        if !offset.is_finite() || offset < 0.0 {
            // Reject non-finite pixels explicitly because NaN would bypass the comparison guards.
            return None;
        }
        if offset >= self.span() {
            return None;
        }
        let inner = offset - self.leading_offset() - step * self.padding_outer;
        if inner < 0.0 {
            return None;
        }
        let cell_f = (inner / step).floor();
        if cell_f < 0.0 || cell_f as usize >= self.n {
            return None;
        }
        let index = cell_f as usize;
        let within = inner - cell_f * step;
        if within < self.bandwidth() {
            Some(index)
        } else {
            None
        }
    }
}

/// A decimated point, carrying the original datum's key and its index in the
/// original `Series::points` vec, so hit-testing can resolve back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecimatedPoint {
    /// X in data space (from the original datum).
    pub x: f64,
    /// Y coordinate in data space from the original datum.
    pub y: f64,
    /// Key of the original datum this came from.
    pub key: DatumKey,
    /// Index into the original `Series::points`.
    pub original_index: usize,
}

/// A bounded decimation result that retains only series identity and reduced points.
///
/// The original series is borrowed separately for hit testing, avoiding a source-sized clone per result.
#[derive(Debug, Clone)]
pub struct DecimatedSeries {
    /// The series identity.
    pub series_id: SeriesId,
    /// Points retained by the selected decimation strategy.
    pub points: Vec<DecimatedPoint>,
}

impl DecimatedSeries {
    /// Number of decimated points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the decimated series is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// Exact input, workspace, and output counts for a decimation run.
///
/// `workspace_slots` reports the allocated temporary vector length rather than elapsed time or an estimated complexity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecimationReport {
    /// Number of source points.
    pub input_points: usize,
    /// Temporary accumulator slots allocated.
    pub workspace_slots: usize,
    /// Decimated points retained.
    pub output_points: usize,
}

/// Retains at most two finite extrema per occupied pixel column.
///
/// Series no longer than `pixel_width` pass through unchanged except that non-finite points are omitted.
#[must_use]
pub fn decimate(series: &Series, pixel_width: u32, x_scale: &LinearScale) -> DecimatedSeries {
    decimate_with_report(series, pixel_width, x_scale).0
}

/// Performs [`decimate`] and returns exact input, workspace, and output counts.
#[must_use]
pub fn decimate_with_report(
    series: &Series,
    pixel_width: u32,
    x_scale: &LinearScale,
) -> (DecimatedSeries, DecimationReport) {
    let n = series.points.len();
    let pw = pixel_width as usize;

    if n == 0 || pw == 0 {
        return (
            DecimatedSeries {
                series_id: series.id,
                points: Vec::new(),
            },
            DecimationReport {
                input_points: n,
                workspace_slots: 0,
                output_points: 0,
            },
        );
    }

    if n <= pw {
        let points: Vec<DecimatedPoint> = series
            .points
            .iter()
            .enumerate()
            .filter(|(_, d)| d.x.is_finite() && d.y.is_finite())
            .map(|(i, d)| DecimatedPoint {
                x: d.x,
                y: d.y,
                key: d.key,
                original_index: i,
            })
            .collect();
        let output_points = points.len();
        return (
            DecimatedSeries {
                series_id: series.id,
                points,
            },
            DecimationReport {
                input_points: n,
                workspace_slots: 0,
                output_points,
            },
        );
    }

    // Keep each occupied column’s finite minimum and maximum, clamping off-range finite x values to the nearest column.

    // Min/max aggregation needs one slot per pixel column and preserves bounded workspace without sorting the input.
    #[derive(Clone, Copy)]
    struct Bucket {
        min_idx: usize,
        max_idx: usize,
    }

    let mut buckets: Vec<Option<Bucket>> = vec![None; pw];
    let workspace_slots = buckets.len();
    for (index, datum) in series.points.iter().enumerate() {
        if !datum.x.is_finite() || !datum.y.is_finite() {
            continue;
        }
        let px = x_scale.map(datum.x).floor() as i64;
        let col = px.clamp(0, (pw - 1) as i64) as usize;
        match &mut buckets[col] {
            None => {
                buckets[col] = Some(Bucket {
                    min_idx: index,
                    max_idx: index,
                });
            }
            Some(bucket) => {
                // Strict comparisons make the first equal extremum in source order win.
                if datum.y < series.points[bucket.min_idx].y {
                    bucket.min_idx = index;
                }
                if datum.y > series.points[bucket.max_idx].y {
                    bucket.max_idx = index;
                }
            }
        }
    }

    let mut result: Vec<DecimatedPoint> = Vec::with_capacity(pw * 2);
    for bucket in buckets.into_iter().flatten() {
        // Emit a column’s extrema in source order so line segments preserve data order.
        let (first, second) = if bucket.min_idx <= bucket.max_idx {
            (bucket.min_idx, bucket.max_idx)
        } else {
            (bucket.max_idx, bucket.min_idx)
        };

        let d = &series.points[first];
        result.push(DecimatedPoint {
            x: d.x,
            y: d.y,
            key: d.key,
            original_index: first,
        });

        if second != first {
            let d = &series.points[second];
            result.push(DecimatedPoint {
                x: d.x,
                y: d.y,
                key: d.key,
                original_index: second,
            });
        }
    }

    let output_points = result.len();
    (
        DecimatedSeries {
            series_id: series.id,
            points: result,
        },
        DecimationReport {
            input_points: n,
            workspace_slots,
            output_points,
        },
    )
}

/// Cap on the temporary cell workspace used by [`decimate_grid`] and [`decimate_grid_with_report`].
///
/// A caller-derived `pixel_width * rows` that would exceed this limit — including product overflow — short-circuits to an empty result rather than attempting the multi-gigabyte `vec![None; pw * rh]` an f32-derived saturating cast or a misconfigured cell size would demand. The cap sits above every realistic display: an 8K scatter (`7680 × 864` with the default 5 px grid cell height) resolves in about 6.6 million cells, comfortably inside `1 << 24 = 16_777_216`. At the cap, the peak temporary allocation is `16_777_216 * size_of::<Option<usize>>() = 16_777_216 * 16 B = 256 MiB`, which is inside an interactive chart process's transient budget and still catches only pathological callers whose alloc would otherwise exhaust process memory.
pub const MAX_GRID_WORKSPACE_CELLS: usize = 1 << 24;

/// Pure admission check for the grid decimation workspace.
///
/// Returns `Some(pixel_width * rows)` when the product is nonzero, does not overflow `usize`, and stays within `cap`; returns `None` otherwise. Production callers pass [`MAX_GRID_WORKSPACE_CELLS`]; the `cap` parameter exists so tests can drive the threshold direction with a small injected cap instead of allocating hundreds of megabytes at the real cap boundary.
#[must_use]
pub(crate) fn grid_workspace_cells(pixel_width: u32, rows: u32, cap: usize) -> Option<usize> {
    let pw = pixel_width as usize;
    let rh = rows as usize;
    if pw == 0 || rh == 0 {
        return None;
    }
    let cells = pw.checked_mul(rh)?;
    if cells > cap { None } else { Some(cells) }
}

/// Retains the first finite datum in each occupied pixel grid cell.
///
/// This is intended for independent scatter points, where per-column extrema would erase interior density. Output is deterministic, source-ordered, and bounded by `pixel_width * rows`; zero dimensions yield no points. Same as the zero-dimension fallback, a `pixel_width * rows` that would exceed [`MAX_GRID_WORKSPACE_CELLS`] — including product overflow — yields no points instead of allocating.
#[must_use]
pub fn decimate_grid(
    series: &Series,
    pixel_width: u32,
    rows: u32,
    x_scale: &LinearScale,
    y_scale: &LinearScale,
) -> DecimatedSeries {
    decimate_grid_with_report(series, pixel_width, rows, x_scale, y_scale).0
}

/// Performs [`decimate_grid`] and returns exact input, workspace, and output counts.
#[must_use]
pub fn decimate_grid_with_report(
    series: &Series,
    pixel_width: u32,
    rows: u32,
    x_scale: &LinearScale,
    y_scale: &LinearScale,
) -> (DecimatedSeries, DecimationReport) {
    let n = series.points.len();

    // The admission helper folds zero dimensions, `usize` overflow, and the
    // production cap into one decision so the `vec![None; pw * rh]` below is
    // reached only for a nonzero, non-overflowing, in-budget product. Non-finite
    // or oversized f32 viewport dimensions saturate to `u32::MAX` at the
    // caller's `as u32` cast, and a small `grid_cell_height` divisor can
    // amplify `rows` into the billions; either would otherwise attempt a
    // multi-gigabyte allocation and panic on out-of-memory.
    let admitted = grid_workspace_cells(pixel_width, rows, MAX_GRID_WORKSPACE_CELLS);
    if n == 0 || admitted.is_none() {
        return (
            DecimatedSeries {
                series_id: series.id,
                points: Vec::new(),
            },
            DecimationReport {
                input_points: n,
                workspace_slots: 0,
                output_points: 0,
            },
        );
    }
    let pw = pixel_width as usize;
    let rh = rows as usize;

    // One first-claim slot per grid cell bounds workspace by plot area rather than input length.
    let mut cells: Vec<Option<usize>> = vec![None; pw * rh];
    let workspace_slots = cells.len();

    for (index, datum) in series.points.iter().enumerate() {
        if !datum.x.is_finite() || !datum.y.is_finite() {
            continue;
        }
        let px = x_scale.map(datum.x);
        let py = y_scale.map(datum.y);
        // Reject non-finite mapped pixels before integer conversion, which would otherwise saturate them into an edge cell.
        if !px.is_finite() || !py.is_finite() {
            continue;
        }
        let col = (px.floor() as i64).clamp(0, (pw - 1) as i64) as usize;
        let row = (py.floor() as i64).clamp(0, (rh - 1) as i64) as usize;
        let slot = &mut cells[row * pw + col];
        if slot.is_none() {
            *slot = Some(index);
        }
    }

    // Source-order emission keeps semantic and keyboard traversal independent of grid scan order.
    let mut indices: Vec<usize> = cells.into_iter().flatten().collect();
    indices.sort_unstable();

    let points: Vec<DecimatedPoint> = indices
        .into_iter()
        .map(|i| {
            let d = &series.points[i];
            DecimatedPoint {
                x: d.x,
                y: d.y,
                key: d.key,
                original_index: i,
            }
        })
        .collect();

    let output_points = points.len();
    (
        DecimatedSeries {
            series_id: series.id,
            points,
        },
        DecimationReport {
            input_points: n,
            workspace_slots,
            output_points,
        },
    )
}

/// A hit-test result referencing an original datum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitResult {
    /// The original datum.
    pub datum: Datum,
    /// Index into the original `Series::points`.
    pub original_index: usize,
    /// Squared pixel distance from the query point.
    pub distance_sq: f64,
}

/// Finds the nearest finite datum in the original series in pixel space.
///
/// `px` and `py` must be finite. The decimation and source must have the same series id; otherwise this returns `None`. Searching the source keeps points discarded by decimation hittable.
#[must_use]
pub fn hit_test(
    decimated: &DecimatedSeries,
    series: &Series,
    px: f64,
    py: f64,
    x_scale: &LinearScale,
    y_scale: &LinearScale,
) -> Option<HitResult> {
    if decimated.series_id != series.id {
        return None;
    }
    let mut best: Option<HitResult> = None;

    for (i, d) in series.points.iter().enumerate() {
        if !d.x.is_finite() || !d.y.is_finite() {
            continue;
        }
        let dx = x_scale.map(d.x) - px;
        let dy = y_scale.map(d.y) - py;
        let dist_sq = dx * dx + dy * dy;

        match &best {
            Some(b) if b.distance_sq <= dist_sq => {}
            _ => {
                best = Some(HitResult {
                    datum: *d,
                    original_index: i,
                    distance_sq: dist_sq,
                });
            }
        }
    }

    best
}

/// A keyboard movement among the points retained for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatumStep {
    /// One drawn point earlier in retained order (`ArrowLeft`).
    Previous,
    /// One drawn point later in retained order (`ArrowRight`).
    Next,
    /// The first drawn point (Home).
    First,
    /// The last drawn point (End).
    Last,
}

/// Moves a keyed cursor among the points retained by decimation.
///
/// Movement stops at either end and returns `None` when no movement occurs. An absent key enters at the first point for `Next` or `First` and at the last point for `Previous` or `Last`. Returned cursors are stable datum keys, not decimated indices.
#[must_use]
pub fn step_datum(
    decimated: &DecimatedSeries,
    current: Option<DatumKey>,
    step: DatumStep,
) -> Option<DatumKey> {
    let points = &decimated.points;
    if points.is_empty() {
        return None;
    }
    let last = points.len() - 1;

    let index = current.and_then(|key| points.iter().position(|p| p.key == key));

    let Some(i) = index else {
        return Some(match step {
            DatumStep::Next | DatumStep::First => points[0].key,
            DatumStep::Previous | DatumStep::Last => points[last].key,
        });
    };

    let target = match step {
        DatumStep::Previous => i.checked_sub(1)?,
        DatumStep::Next if i >= last => return None,
        DatumStep::Next => i + 1,
        DatumStep::First if i == 0 => return None,
        DatumStep::First => 0,
        DatumStep::Last if i == last => return None,
        DatumStep::Last => last,
    };
    Some(points[target].key)
}

/// Returns the drawn point for `key`, its one-based position, and the drawn-point count.
///
/// Missing keys return `None`.
#[must_use]
pub fn locate_datum(
    decimated: &DecimatedSeries,
    key: DatumKey,
) -> Option<(DecimatedPoint, usize, usize)> {
    let i = decimated.points.iter().position(|p| p.key == key)?;
    Some((decimated.points[i], i + 1, decimated.points.len()))
}

/// One original datum in a chart’s companion data table.
///
/// Stable series and datum identities allow table rows to correlate with chart selection and hit results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartTableRow {
    /// Which series this datum belongs to.
    pub series_id: SeriesId,
    /// Index of the series within the chart's series slice (`0`-based), so a
    /// host can label rows "Series 1", "Series 2" without its own counter.
    pub series_index: usize,
    /// Index of this datum within its series' original `points` vec.
    pub original_index: usize,
    /// Stable identity of the datum.
    pub key: DatumKey,
    /// X in original data space.
    pub x: f64,
    /// Y in original data space.
    pub y: f64,
}

/// Builds companion-table rows in series-then-datum order.
///
/// Rows use original data and include non-finite values verbatim; presentation code decides how to display them.
#[must_use]
pub fn data_table(series: &[Series]) -> Vec<ChartTableRow> {
    let mut rows = Vec::new();
    for (series_index, s) in series.iter().enumerate() {
        for (original_index, d) in s.points.iter().enumerate() {
            rows.push(ChartTableRow {
                series_id: s.id,
                series_index,
                original_index,
                key: d.key,
                x: d.x,
                y: d.y,
            });
        }
    }
    rows
}

/// How a chart derives its data-space domain from the series it holds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DomainPolicy {
    /// Span exactly the finite data extent.
    #[default]
    Automatic,
    /// Span the finite data extent, extended to include `0.0`.
    IncludeZero,
    /// Uses finite caller bounds verbatim; non-finite bounds fall back to the measured extent.
    Fixed(f64, f64),
}

/// Returns x and y extents across datums whose two coordinates are finite.
///
/// Returns `None` when no such datum exists and preserves degenerate extents without padding.
#[must_use]
pub fn data_domain(series: &[Series]) -> Option<((f64, f64), (f64, f64))> {
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    let mut any = false;

    for s in series {
        for d in &s.points {
            if !d.x.is_finite() || !d.y.is_finite() {
                continue;
            }
            any = true;
            x_min = x_min.min(d.x);
            x_max = x_max.max(d.x);
            y_min = y_min.min(d.y);
            y_max = y_max.max(d.y);
        }
    }

    any.then_some(((x_min, x_max), (y_min, y_max)))
}

/// Applies a y-domain policy to a measured extent.
///
/// A fixed policy with a non-finite bound falls back to the measured extent.
#[must_use]
pub fn apply_domain_policy(extent: (f64, f64), policy: DomainPolicy) -> (f64, f64) {
    match policy {
        DomainPolicy::Automatic => extent,
        DomainPolicy::IncludeZero => (extent.0.min(0.0), extent.1.max(0.0)),
        DomainPolicy::Fixed(lo, hi) if lo.is_finite() && hi.is_finite() => (lo, hi),
        DomainPolicy::Fixed(_, _) => extent,
    }
}

// Select a 1/2/5×10^n ladder value, rounding to the nearest rung or up to the next rung.
fn nice_num(value: f64, round: bool) -> f64 {
    let exp = value.log10().floor();
    let pow = 10f64.powf(exp);
    let frac = value / pow;

    let nice_frac = if round {
        if frac < 1.5 {
            1.0
        } else if frac < 3.0 {
            2.0
        } else if frac < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if frac <= 1.0 {
        1.0
    } else if frac <= 2.0 {
        2.0
    } else if frac <= 5.0 {
        5.0
    } else {
        10.0
    };

    nice_frac * pow
}

/// Widens `extent` outward to whole multiples of the step [`nice_ticks`]
/// picks for it, so the outermost ticks frame the data instead of sitting
/// inside it.
///
/// Flat, reversed and non-finite extents come back unchanged.
#[must_use]
pub fn nice_extent(extent: (f64, f64), target_count: usize) -> (f64, f64) {
    let (lo, hi) = extent;
    if !lo.is_finite() || !hi.is_finite() || lo >= hi {
        return extent;
    }
    let intervals = target_count.max(1) as f64;
    // Widening can raise the step `nice_ticks` picks, so settle on an extent
    // whose own step lands on both ends.
    let mut current = (lo, hi);
    for _ in 0..4 {
        let step = nice_num(nice_num(current.1 - current.0, false) / intervals, true);
        if step <= 0.0 || !step.is_finite() {
            return extent;
        }
        let next = (
            (current.0 / step).floor() * step,
            (current.1 / step).ceil() * step,
        );
        if !next.0.is_finite() || !next.1.is_finite() {
            return extent;
        }
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// Generates finite ticks on the 1/2/5×10^n ladder within the supplied domain.
///
/// Reversed domains are accepted, flat domains yield one tick, non-finite domains yield none, and counts below two still request one interval. Tick values are computed from integer indices so a zero tick is exact.
#[must_use]
pub fn nice_ticks(min: f64, max: f64, target_count: usize) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() {
        return Vec::new();
    }

    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };

    if lo == hi {
        return vec![lo];
    }

    let intervals = target_count.max(1) as f64;

    let raw_range = hi - lo;
    let nice_range = nice_num(raw_range, false);
    let raw_step = nice_range / intervals;
    let step = nice_num(raw_step, true);

    // A subnormal range can round the chosen step to zero; return the finite endpoints instead of looping.
    if step <= 0.0 || !step.is_finite() {
        return vec![lo, hi];
    }

    // Derive every tick from an integer index so the zero tick remains exactly `0.0`.
    let first_index = (lo / step).floor() as i64;
    let last_index = (hi / step).ceil() as i64;

    // Cap iteration defensively so future step-selection changes cannot hang rendering.
    let max_ticks = target_count.max(1) * 8 + 16;

    let tol = step * 1e-9;

    let mut ticks = Vec::new();
    let mut index = first_index;
    while index <= last_index && ticks.len() < max_ticks {
        // Compute rather than accumulate ticks to preserve exact zero without snapping legitimate near-zero values.
        let value = (index as f64) * step;
        if value >= lo - tol && value <= hi + tol {
            ticks.push(value);
        }
        index += 1;
    }

    // If no ladder point lies in a domain narrower than one step, return its endpoints.
    if ticks.is_empty() {
        ticks.push(lo);
        ticks.push(hi);
    }

    ticks
}
