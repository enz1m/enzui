//! Grain on any surface: a seamless speckle tile, made once per device scale
//! and repeated over the surface's rounded bounds.
//!
//! [`Grain`] (in `enzui-theme`) describes the texture; this module draws it.
//! Three ways to put it on screen:
//!
//! * `enz_surface(..).grain(Grain::default())`, or `.grain(GrainSpec::Theme)`
//!   for the theme's default: the grain lies on the surface's fill, masked to
//!   its corners, under the content (or over it with
//!   [`GrainPlacement::Over`]).
//! * A shell pane through `PaneStyle::grain`: a grainy sidebar or titlebar.
//! * [`enz_grain`] around any element: the grain is drawn over it.
//!
//! # How it draws
//!
//! The tile is made at the window's device scale times [`Grain::scale`], so
//! specks land on whole device pixels and stay crisp. The built-in source is
//! fractal gradient noise whose lattice wraps at the tile edge, sampled at
//! texel centres: the tile repeats without a seam by construction. An SVG
//! source is rendered by `resvg`. Under
//! [`GrainMapping::Specks`] each texel's
//! value is measured against the tile's own mean and spread; the strongest
//! [`Grain::density`] share become specks in the dark or light ink, fading in
//! from the threshold so specks have soft edges.
//!
//! Tiles are cached by everything that changes their pixels, so a surface
//! redrawn every frame (a sliding sidebar) costs one image draw per visible
//! tile. Tiles anchor to the surface's corner: grain travels with the surface
//! instead of swimming under it. High-contrast palettes draw no grain.
//!
//! Changing a knob stays cheap. A tile is rasterised at full intensity and
//! drawn at [`Grain::intensity`] times [`Grain::opacity`], so neither makes a
//! new tile. The built-in noise tile spans [`Grain::tile`] at device scale
//! whatever [`Grain::scale`] is (a larger scale samples fewer, larger noise
//! cells), and the noise field is cached apart from its speck mapping, so
//! density, tone and inks only remap it.

use enzui_theme::{
    Grain, GrainMapping, GrainPlacement, GrainSource, GrainSpec, GrainTiling, GrainTone,
};
use iced::advanced::image::{self as advanced_image, FilterMethod};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::border::Radius;
use iced::time::{Duration, Instant};
use iced::{Element, Event, Length, Padding, Rectangle, Renderer, Size, Theme, Vector, window};
use std::cell::RefCell;
use std::rc::Rc;

/// How many rasterised tiles stay cached.
const TILE_CACHE: usize = 24;

/// How far the built-in noise's frequency keeps from a whole number.
const BEAT_MARGIN: f32 = 0.15;

/// The frequency nearest `frequency`, clear of whole numbers, at which a
/// `size`-unit tile holds a whole number of noise cells.
///
/// Near a whole number of cells per pixel the noise beats against the pixel
/// grid: texels drift slowly through their cells and a wide patch samples
/// each cell at nearly the same place, a flat spot in the grain.
pub fn stitched_frequency(frequency: f32, size: f32) -> f32 {
    let whole = frequency.round();
    let frequency = if whole >= 1.0 && (frequency - whole).abs() < BEAT_MARGIN {
        if frequency < whole {
            whole - BEAT_MARGIN
        } else {
            whole + BEAT_MARGIN
        }
    } else {
        frequency
    };
    (frequency * size).round().max(1.0) / size
}

/// A well-mixed 32-bit hash of a lattice point.
fn lattice_hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h =
        x.wrapping_mul(0x8DA6_B343) ^ y.wrapping_mul(0xD816_3841) ^ seed.wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// Unit gradients at 256 evenly spaced angles. A lattice point's hash picks
/// one, which spares a sine and a cosine at every cell corner.
type Gradients = [(f32, f32); 256];

static GRADIENTS: std::sync::LazyLock<Gradients> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|i| {
        let angle = i as f32 / 256.0 * std::f32::consts::TAU;
        (angle.cos(), angle.sin())
    })
});

/// Periodic gradient noise at `(u, v)` in cells, repeating every `period`
/// cells on both axes: the lattice wraps, so the field has no seam.
fn periodic_noise(gradients: &Gradients, u: f32, v: f32, period: u32, seed: u32) -> f32 {
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let wrap = |c: f32| (c as i64).rem_euclid(i64::from(period)) as u32;
    let gradient = |cx: f32, cy: f32, dx: f32, dy: f32| {
        let h = lattice_hash(wrap(cx), wrap(cy), seed);
        let (cos, sin) = gradients[(h >> 24) as usize];
        cos * dx + sin * dy
    };
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let n00 = gradient(x0, y0, fx, fy);
    let n10 = gradient(x0 + 1.0, y0, fx - 1.0, fy);
    let n01 = gradient(x0, y0 + 1.0, fx, fy - 1.0);
    let n11 = gradient(x0 + 1.0, y0 + 1.0, fx - 1.0, fy - 1.0);
    let (sx, sy) = (fade(fx), fade(fy));
    let top = n00 + (n10 - n00) * sx;
    let bottom = n01 + (n11 - n01) * sx;
    top + (bottom - top) * sy
}

/// The built-in grain's values for a `width` by `height` texel tile of
/// `tile` logical pixels: fractal periodic noise, sampled at texel centres.
/// [`Grain::scale`] divides the frequency, so larger specks come from fewer
/// noise cells over the same texels.
fn noise_values(grain: &Grain, width: u32, height: u32) -> Vec<f32> {
    let frequency = stitched_frequency(grain.frequency / grain.scale.max(0.01), grain.tile);
    let cells = (frequency * grain.tile).round().max(1.0) as u32;
    let gradients = &*GRADIENTS;
    let row = |y: u32, out: &mut [f32]| {
        for (x, value) in (0..width).zip(out) {
            let (mut sum, mut amplitude) = (0.0, 1.0);
            for octave in 0..u32::from(grain.octaves) {
                let period = cells << octave;
                let u = (x as f32 + 0.5) / width as f32 * period as f32;
                let v = (y as f32 + 0.5) / height as f32 * period as f32;
                sum += amplitude
                    * periodic_noise(gradients, u, v, period, grain.seed.wrapping_add(octave));
                amplitude *= 0.5;
            }
            *value = sum;
        }
    };
    let mut values = vec![0.0; (width * height) as usize];
    // Rows are independent: a large tile (a hi-dpi screen) spreads them over
    // the cores, so a new tile costs a frame, not several.
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(8);
    let rows_per_thread = height.div_ceil(threads as u32).max(1);
    if threads > 1 && values.len() >= PARALLEL_TEXELS {
        std::thread::scope(|scope| {
            for (chunk, rows) in values
                .chunks_mut((rows_per_thread * width) as usize)
                .enumerate()
            {
                let row = &row;
                let _ = scope.spawn(move || {
                    let first = chunk as u32 * rows_per_thread;
                    for (y, out) in (first..).zip(rows.chunks_mut(width as usize)) {
                        row(y, out);
                    }
                });
            }
        });
    } else {
        for (y, out) in (0..).zip(values.chunks_mut(width as usize)) {
            row(y, out);
        }
    }
    values
}

/// Texels from which noise is worth spreading over threads.
const PARALLEL_TEXELS: usize = 128 * 128;

/// One rasterised tile.
#[derive(Debug, Clone)]
pub struct GrainTile {
    /// The tile's pixels, straight-alpha RGBA.
    pub handle: advanced_image::Handle,
    /// Width and height in device pixels.
    pub pixels: (u32, u32),
    /// Width and height in logical pixels at the scale it was made for.
    pub logical: Size,
}

/// Rasterise `grain` for a device scale of `factor`: straight-alpha RGBA,
/// its size in device pixels, and its size in logical pixels. `None` when its
/// SVG does not parse or it has no area.
pub fn rasterize(grain: &Grain, factor: f32) -> Option<(Vec<u8>, (u32, u32), Size)> {
    let factor = if factor.is_finite() && factor > 0.0 {
        factor
    } else {
        1.0
    };
    let (mut rgba, width, height) = match (&grain.source, grain.mapping) {
        (GrainSource::Svg(svg), GrainMapping::Artwork) => {
            let pixmap = render_svg(*svg, factor * grain.scale)?;
            let (width, height) = (pixmap.width(), pixmap.height());
            let rgba = artwork(grain, pixmap.data());
            let rgba = if grain.tiling == GrainTiling::Blend {
                blended_rgba(&rgba, width, height)
            } else {
                rgba
            };
            (rgba, width, height)
        }
        _ => {
            let field = field(grain, factor)?;
            (specks(grain, &field.scores), field.width, field.height)
        }
    };
    let (mut w, mut h) = (width, height);
    if grain.tiling == GrainTiling::Mirror {
        rgba = mirrored(&rgba, w, h);
        w *= 2;
        h *= 2;
    }
    Some((
        rgba,
        (w, h),
        Size::new(w as f32 / factor, h as f32 / factor),
    ))
}

/// `svg` rendered at `scale` device pixels per unit.
fn render_svg(svg: enzui_theme::GrainSvg, scale: f32) -> Option<resvg::tiny_skia::Pixmap> {
    let document = svg.document();
    let tree = resvg::usvg::Tree::from_str(&document, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        ),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// The per-texel values specks are cut from, before mirroring.
struct Field {
    scores: Scores,
    width: u32,
    height: u32,
}

/// How many fields stay cached: a drag over density or tone reuses one.
const FIELD_CACHE: usize = 4;

thread_local! {
    static FIELDS: RefCell<Vec<(u64, Option<Rc<Field>>)>> = const { RefCell::new(Vec::new()) };
}

/// The values `grain` maps to specks at `factor`, computed once per source,
/// scale and noise settings: density, tone and inks only remap them.
fn field(grain: &Grain, factor: f32) -> Option<Rc<Field>> {
    let key = field_key(grain, factor);
    FIELDS.with(|fields| {
        let mut fields = fields.borrow_mut();
        if let Some(index) = fields.iter().position(|(k, _)| *k == key) {
            let entry = fields.remove(index);
            let field = entry.1.clone();
            fields.push(entry);
            return field;
        }
        let field = match &grain.source {
            GrainSource::Noise => {
                let side = (grain.tile * factor).round().max(1.0) as u32;
                Some(Field {
                    scores: Scores::new(&noise_values(grain, side, side)),
                    width: side,
                    height: side,
                })
            }
            GrainSource::Svg(svg) => render_svg(*svg, factor * grain.scale).map(|pixmap| {
                let (width, height) = (pixmap.width(), pixmap.height());
                let mut values = brightness(pixmap.data());
                if grain.tiling == GrainTiling::Blend {
                    values = blended_values(&values, width, height);
                }
                Field {
                    scores: Scores::new(&values),
                    width,
                    height,
                }
            }),
        }
        .map(Rc::new);
        if fields.len() >= FIELD_CACHE {
            fields.remove(0);
        }
        fields.push((key, field.clone()));
        field
    })
}

/// How far into the tile, as a share of its side, [`GrainTiling::Blend`]
/// cross-fades toward the shifted copy.
const BLEND_REACH: f32 = 0.25;

/// The weight of the tile itself (against its half-shifted copy) at texel
/// `i` of `n`: one in the middle, falling to zero at the edges.
fn blend_weight(i: usize, n: usize) -> f32 {
    let t = (i as f32 + 0.5) / n as f32;
    let edge = t.min(1.0 - t) / BLEND_REACH;
    let edge = edge.clamp(0.0, 1.0);
    edge * edge * (3.0 - 2.0 * edge)
}

/// Zero-mean `values` cross-faded with a half-tile-shifted copy near the
/// edges, so the wrap-around continues the copy's interior. The weights keep
/// the variance, or the faded band would read paler than the rest.
fn blended_values(values: &[f32], w: u32, h: u32) -> Vec<f32> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0.0; values.len()];
    for y in 0..h {
        for x in 0..w {
            let a = blend_weight(x, w) * blend_weight(y, h);
            let b = 1.0 - a;
            let shifted = values[((y + h / 2) % h) * w + (x + w / 2) % w];
            let norm = (a * a + b * b).sqrt().max(1e-3);
            out[y * w + x] = (a * values[y * w + x] + b * shifted) / norm;
        }
    }
    out
}

/// `rgba` cross-faded with a half-tile-shifted copy near the edges.
fn blended_rgba(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let a = blend_weight(x, w) * blend_weight(y, h);
            let here = (y * w + x) * 4;
            let there = (((y + h / 2) % h) * w + (x + w / 2) % w) * 4;
            for c in 0..4 {
                let v = a * f32::from(rgba[here + c]) + (1.0 - a) * f32::from(rgba[there + c]);
                out[here + c] = v.round() as u8;
            }
        }
    }
    out
}

/// Each texel's brightness, transparent texels reading as the tile's mean.
fn brightness(premultiplied: &[u8]) -> Vec<f32> {
    let texels: Vec<[f32; 4]> = premultiplied
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(straight)
        .collect();
    let luminance = |p: &[f32; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let (mut sum, mut weight) = (0.0f64, 0.0f64);
    for p in &texels {
        sum += f64::from(luminance(p) * p[3]);
        weight += f64::from(p[3]);
    }
    let mean = if weight > 0.0 {
        (sum / weight) as f32
    } else {
        0.5
    };
    texels
        .iter()
        .map(|p| luminance(p) * p[3] + mean * (1.0 - p[3]) - mean)
        .collect()
}

/// Straight-alpha `[r, g, b, a]` in `0..1` from a premultiplied pixel.
fn straight(px: [u8; 4]) -> [f32; 4] {
    let a = f32::from(px[3]) / 255.0;
    if a <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [
        f32::from(px[0]) / 255.0 / a,
        f32::from(px[1]) / 255.0 / a,
        f32::from(px[2]) / 255.0 / a,
        a,
    ]
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Histogram bins over [`Z_RANGE`]: a quantile lands within 0.003 of a
/// standard deviation.
const BINS: usize = 4096;
/// Standard scores the histograms span, either side of zero; rarer texels
/// count in the end bins.
const Z_RANGE: f32 = 6.0;

/// A field's values as standard scores, with their distribution: what a
/// speck mapping needs, so a density or tone step reads its thresholds
/// instead of sorting the texels.
struct Scores {
    z: Vec<f32>,
    /// Counts of `z` over `-Z_RANGE..Z_RANGE`.
    signed: Vec<u32>,
    /// Counts of `|z|` over `0..Z_RANGE`.
    magnitude: Vec<u32>,
}

impl Scores {
    fn new(values: &[f32]) -> Self {
        let n = values.len().max(1) as f32;
        let mean = values.iter().sum::<f32>() / n;
        let spread = (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n)
            .sqrt()
            .max(1e-6);
        let z: Vec<f32> = values.iter().map(|v| (v - mean) / spread).collect();
        let bin = |x: f32, low: f32, high: f32| {
            (((x - low) / (high - low) * BINS as f32).max(0.0) as usize).min(BINS - 1)
        };
        let (mut signed, mut magnitude) = (vec![0u32; BINS], vec![0u32; BINS]);
        for z in &z {
            signed[bin(*z, -Z_RANGE, Z_RANGE)] += 1;
            magnitude[bin(z.abs(), 0.0, Z_RANGE)] += 1;
        }
        Self {
            z,
            signed,
            magnitude,
        }
    }

    /// The value below which `share` of the counts in `histogram` (over
    /// `low..high`) lie, interpolated within its bin.
    fn quantile(&self, histogram: &[u32], low: f32, high: f32, share: f32) -> f32 {
        let wanted = share.clamp(0.0, 1.0) * self.z.len() as f32;
        let mut below = 0.0;
        for (index, count) in histogram.iter().enumerate() {
            let count = *count as f32;
            if below + count >= wanted && count > 0.0 {
                let within = ((wanted - below) / count).clamp(0.0, 1.0);
                return low + (index as f32 + within) / BINS as f32 * (high - low);
            }
            below += count;
        }
        high
    }

    /// The `share` quantile of how far texels lean toward `tone`.
    fn lean_quantile(&self, tone: GrainTone, share: f32) -> f32 {
        match tone {
            GrainTone::Both => self.quantile(&self.magnitude, 0.0, Z_RANGE, share),
            GrainTone::Dark => -self.quantile(&self.signed, -Z_RANGE, Z_RANGE, 1.0 - share),
            GrainTone::Light => self.quantile(&self.signed, -Z_RANGE, Z_RANGE, share),
        }
    }
}

/// Light and dark specks from a field's scores: the strongest
/// [`Grain::density`] share leaning toward the kept tone(s) become specks in
/// the grain's inks.
fn specks(grain: &Grain, scores: &Scores) -> Vec<u8> {
    let threshold = scores.lean_quantile(grain.tone, 1.0 - grain.density);
    // The strongest texels reach full intensity; specks fade in from the
    // threshold, so they have soft edges rather than hard dots.
    let peak = scores
        .lean_quantile(grain.tone, 0.995)
        .max(threshold + 1e-3);
    let ink = |ink: iced::Color| {
        (
            [to_byte(ink.r), to_byte(ink.g), to_byte(ink.b)],
            ink.a * grain.intensity,
        )
    };
    let (dark, light) = (ink(grain.dark_ink), ink(grain.light_ink));
    let mut out = Vec::with_capacity(scores.z.len() * 4);
    for z in &scores.z {
        let lean = match grain.tone {
            GrainTone::Both => z.abs(),
            GrainTone::Dark => -z,
            GrainTone::Light => *z,
        };
        let ([r, g, b], a) = if *z < 0.0 { dark } else { light };
        let alpha = if lean > threshold && grain.density > 0.0 {
            let strength = ((lean - threshold) / (peak - threshold)).clamp(0.0, 1.0);
            strength * strength * (3.0 - 2.0 * strength) * a
        } else {
            0.0
        };
        out.extend_from_slice(&[r, g, b, to_byte(alpha)]);
    }
    out
}

fn artwork(grain: &Grain, premultiplied: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(premultiplied.len());
    for p in premultiplied
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(straight)
    {
        out.extend_from_slice(&[
            to_byte(p[0]),
            to_byte(p[1]),
            to_byte(p[2]),
            to_byte(p[3] * grain.intensity),
        ]);
    }
    out
}

/// `rgba` (`w` by `h`) mirrored into a 2x2 block that repeats seamlessly.
fn mirrored(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 16];
    for y in 0..h * 2 {
        let sy = if y < h { y } else { 2 * h - 1 - y };
        for x in 0..w * 2 {
            let sx = if x < w { x } else { 2 * w - 1 - x };
            let from = (sy * w + sx) * 4;
            let to = (y * w * 2 + x) * 4;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    out
}

/// The field cache key: everything that changes a field's values.
fn field_key(grain: &Grain, factor: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match &grain.source {
        GrainSource::Noise => 0u8.hash(&mut hasher),
        GrainSource::Svg(svg) => {
            1u8.hash(&mut hasher);
            svg.hash(&mut hasher);
            (grain.tiling == GrainTiling::Blend).hash(&mut hasher);
        }
    }
    for value in [grain.scale, grain.frequency, grain.tile, factor] {
        value.to_bits().hash(&mut hasher);
    }
    grain.octaves.hash(&mut hasher);
    grain.seed.hash(&mut hasher);
    hasher.finish()
}

/// The tile cache key: everything that changes a tile's pixels.
fn key(grain: &Grain, factor: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match &grain.source {
        GrainSource::Noise => 0u8.hash(&mut hasher),
        GrainSource::Svg(svg) => {
            1u8.hash(&mut hasher);
            svg.hash(&mut hasher);
        }
    }
    (grain.mapping as u8).hash(&mut hasher);
    (grain.tone as u8).hash(&mut hasher);
    (grain.tiling as u8).hash(&mut hasher);
    for value in [
        grain.intensity,
        grain.density,
        grain.scale,
        grain.frequency,
        grain.tile,
        factor,
        grain.dark_ink.r,
        grain.dark_ink.g,
        grain.dark_ink.b,
        grain.dark_ink.a,
        grain.light_ink.r,
        grain.light_ink.g,
        grain.light_ink.b,
        grain.light_ink.a,
    ] {
        value.to_bits().hash(&mut hasher);
    }
    grain.octaves.hash(&mut hasher);
    grain.seed.hash(&mut hasher);
    hasher.finish()
}

thread_local! {
    static TILES: RefCell<Vec<(u64, Option<Rc<GrainTile>>)>> = const { RefCell::new(Vec::new()) };
}

/// The tile for `grain` at `factor`, rasterised at full intensity on first
/// use and cached: draw it at `grain.intensity * grain.opacity`.
pub fn tile(grain: &Grain, factor: f32) -> Option<Rc<GrainTile>> {
    // A speck's alpha is linear in the intensity, so the draw applies it and
    // an intensity change reuses the tile.
    let grain = &Grain {
        intensity: 1.0,
        ..*grain
    };
    let key = key(grain, factor);
    TILES.with(|tiles| {
        let mut tiles = tiles.borrow_mut();
        if let Some(index) = tiles.iter().position(|(k, _)| *k == key) {
            let entry = tiles.remove(index);
            let tile = entry.1.clone();
            tiles.push(entry);
            return tile;
        }
        let tile = rasterize(grain, factor).map(|(rgba, pixels, logical)| {
            Rc::new(GrainTile {
                handle: advanced_image::Handle::from_rgba(pixels.0, pixels.1, rgba),
                pixels,
                logical,
            })
        });
        if tiles.len() >= TILE_CACHE {
            tiles.remove(0);
        }
        tiles.push((key, tile.clone()));
        tile
    })
}

/// A pseudo-random tile offset for film-grain frame `frame`, in whole device
/// pixels.
fn shimmer(frame: u64, tile: Size, factor: f32) -> Vector {
    let mut x = frame.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    let fx = (x & 0xFFFF) as f32 / 65_535.0;
    let fy = ((x >> 16) & 0xFFFF) as f32 / 65_535.0;
    Vector::new(
        (fx * tile.width * factor).round() / factor,
        (fy * tile.height * factor).round() / factor,
    )
}

/// Draw `grain` over `region`, tiles anchored at the region's corner (moved
/// by `offset`), clipped to `region` and the `viewport`.
pub(crate) fn draw(
    renderer: &mut Renderer,
    grain: &Grain,
    region: Rectangle,
    viewport: &Rectangle,
    offset: Vector,
) {
    use iced::advanced::image::Renderer as _;
    if !grain.is_visible() || region.width <= 0.0 || region.height <= 0.0 {
        return;
    }
    let Some(clip) = region.intersection(viewport) else {
        return;
    };
    let factor = device_scale(renderer);
    let Some(tile) = tile(grain, factor) else {
        return;
    };
    let (tw, th) = (tile.logical.width, tile.logical.height);
    if tw <= 0.0 || th <= 0.0 {
        return;
    }
    // Snap the anchor to the device grid so every texel lands on a pixel.
    let snap = |v: f32| (v * factor).round() / factor;
    let origin_x = snap(region.x) - offset.x.rem_euclid(tw);
    let origin_y = snap(region.y) - offset.y.rem_euclid(th);
    let first_col = ((clip.x - origin_x) / tw).floor() as i32;
    let last_col = ((clip.x + clip.width - origin_x) / tw).ceil() as i32;
    let first_row = ((clip.y - origin_y) / th).floor() as i32;
    let last_row = ((clip.y + clip.height - origin_y) / th).ceil() as i32;
    for row in first_row..last_row {
        for col in first_col..last_col {
            let bounds = Rectangle {
                x: origin_x + col as f32 * tw,
                y: origin_y + row as f32 * th,
                width: tw,
                height: th,
            };
            renderer.draw_image(
                advanced_image::Image::new(tile.handle.clone())
                    .filter_method(FilterMethod::Nearest)
                    .opacity(grain.opacity * grain.intensity),
                bounds,
                clip,
            );
        }
    }
}

/// The window's device scale: how many device pixels a logical pixel spans.
fn device_scale(renderer: &Renderer) -> f32 {
    iced::advanced::Renderer::scale(renderer)
        .map(iced::advanced::renderer::Scale::total)
        .filter(|factor| factor.is_finite() && *factor > 0.0)
        .unwrap_or(1.0)
}

/// Which grain `spec` draws under `theme`, or `None`. High-contrast palettes
/// draw none: a texture under text only costs legibility there.
pub(crate) fn resolve(spec: &GrainSpec, theme: &Theme) -> Option<Grain> {
    if crate::theme_lookup::palette(theme).is_high_contrast() {
        return None;
    }
    match spec {
        GrainSpec::None => None,
        GrainSpec::Theme => Some(crate::theme_lookup::grain(theme)),
        GrainSpec::Custom(grain) => Some(*grain),
    }
    .filter(Grain::is_visible)
}

#[derive(Debug)]
struct GrainState {
    /// The scene node the grain's tiles are recorded in. Tiles are plain
    /// images no layout declaration describes; recorded in a node of their
    /// own, the render lane carries them along when it scrolls the page.
    node: iced::advanced::renderer::NodeId,
    /// The film-grain frame last drawn, while moving.
    frame: u64,
    /// The theme's grain and reduced-motion policy, as the last draw saw them.
    motion: std::cell::Cell<Option<f32>>,
    reduced: std::cell::Cell<bool>,
}

/// A child with grain drawn behind it (on the area `outset` beyond it) or over
/// it, optionally masked to rounded corners.
pub(crate) struct GrainLayer<'a, Message> {
    child: Element<'a, Message>,
    spec: GrainSpec,
    outset: Padding,
    radius: Option<Radius>,
    placement: Option<GrainPlacement>,
}

impl<'a, Message> GrainLayer<'a, Message> {
    pub(crate) fn new(child: impl Into<Element<'a, Message>>, spec: GrainSpec) -> Self {
        Self {
            child: child.into(),
            spec,
            outset: Padding::ZERO,
            radius: None,
            placement: None,
        }
    }

    /// Paint grain this far beyond the child: a surface's padding.
    pub(crate) fn outset(mut self, outset: Padding) -> Self {
        self.outset = outset;
        self
    }

    /// Mask the grain to these corners.
    pub(crate) fn radius(mut self, radius: Radius) -> Self {
        self.radius = Some(radius);
        self
    }

    /// Override the grain's own placement.
    pub(crate) fn placement(mut self, placement: GrainPlacement) -> Self {
        self.placement = Some(placement);
        self
    }

    fn paint(
        &self,
        renderer: &mut Renderer,
        node: iced::advanced::renderer::NodeId,
        grain: &Grain,
        region: Rectangle,
        viewport: &Rectangle,
        offset: Vector,
    ) {
        use iced::advanced::Renderer as _;
        // A layer of its own: within one layer a renderer draws images after
        // quads, whatever order they were issued in, so grain sharing the
        // content's layer would land on its fills.
        let Some(clip) = region.intersection(viewport) else {
            return;
        };
        let tiles = |renderer: &mut Renderer| {
            renderer.with_motion(node, Vector::ZERO, |renderer| {
                draw(renderer, grain, region, viewport, offset);
            });
        };
        match self.radius {
            Some(radius) => renderer.with_layer_rounded(region, radius, tiles),
            None => renderer.with_layer(clip, tiles),
        }
    }
}

impl<Message> Widget<Message, Theme, Renderer> for GrainLayer<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GrainState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(GrainState {
            node: crate::motion::next_scene_node(),
            frame: 0,
            motion: std::cell::Cell::new(None),
            reduced: std::cell::Cell::new(false),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        // Film grain: one new offset per frame period, and a wake for the next.
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<GrainState>();
            let Some(fps) = state.motion.get() else {
                return;
            };
            if state.reduced.get() || !crate::viewport_gate::is_visible(layout.bounds(), viewport) {
                return;
            }
            let period = Duration::from_secs_f32(1.0 / fps);
            let frame = epoch_frame(*now, period);
            if frame != state.frame {
                state.frame = frame;
                shell.request_redraw();
            }
            shell.schedule_redraw_at(next_frame(*now, period));
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<GrainState>();
        let grain = resolve(&self.spec, theme);
        state.motion.set(grain.as_ref().and_then(|g| g.motion));
        state
            .reduced
            .set(crate::theme_lookup::reduced_motion(theme));
        let draw_child = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        };
        let Some(grain) = grain else {
            draw_child(renderer);
            return;
        };
        let bounds = layout.bounds();
        let region = Rectangle {
            x: bounds.x - self.outset.left,
            y: bounds.y - self.outset.top,
            width: bounds.width + self.outset.left + self.outset.right,
            height: bounds.height + self.outset.top + self.outset.bottom,
        };
        let offset = if grain.motion.is_some() && !state.reduced.get() {
            let factor = device_scale(renderer);
            tile(&grain, factor).map_or(Vector::ZERO, |tile| {
                shimmer(state.frame, tile.logical, factor)
            })
        } else {
            Vector::ZERO
        };
        match self.placement.unwrap_or(grain.placement) {
            GrainPlacement::Behind => {
                use iced::advanced::Renderer as _;
                self.paint(renderer, state.node, &grain, region, viewport, offset);
                // The content follows in a later layer, so it covers the grain.
                renderer.with_layer(*viewport, draw_child);
            }
            GrainPlacement::Over => {
                draw_child(renderer);
                self.paint(renderer, state.node, &grain, region, viewport, offset);
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<GrainLayer<'a, Message>> for Element<'a, Message> {
    fn from(layer: GrainLayer<'a, Message>) -> Self {
        Element::new(layer)
    }
}

thread_local! {
    /// The film-grain clock's origin, shared so every grainy surface shimmers
    /// in step.
    static EPOCH: Instant = Instant::now();
}

/// Film-grain frames since the shared epoch.
fn epoch_frame(now: Instant, period: Duration) -> u64 {
    let since = EPOCH.with(|epoch| now.saturating_duration_since(*epoch));
    (since.as_nanos() / period.as_nanos().max(1)) as u64
}

/// When the next film-grain frame begins.
fn next_frame(now: Instant, period: Duration) -> Instant {
    let frame = epoch_frame(now, period);
    EPOCH.with(|epoch| *epoch + period.saturating_mul(u32::try_from(frame + 1).unwrap_or(u32::MAX)))
}

/// Grain drawn over `content`; see [`EnzGrain`].
pub fn enz_grain<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> EnzGrain<'a, Message> {
    EnzGrain {
        content: content.into(),
        spec: GrainSpec::Theme,
        radius: None,
        placement: GrainPlacement::Over,
    }
}

/// Grain over any element: by default the theme's grain, drawn over the
/// content and the element's own fill alike. For grain under a surface's
/// content, give the surface `.grain(..)` instead.
pub struct EnzGrain<'a, Message> {
    content: Element<'a, Message>,
    spec: GrainSpec,
    radius: Option<Radius>,
    placement: GrainPlacement,
}

impl<'a, Message: 'a> EnzGrain<'a, Message> {
    /// Draw this grain instead of the theme's.
    #[must_use]
    pub fn grain(mut self, grain: impl Into<GrainSpec>) -> Self {
        self.spec = grain.into();
        self
    }

    /// Mask the grain to these corners, matching a rounded element.
    #[must_use]
    pub fn radius(mut self, radius: impl Into<Radius>) -> Self {
        self.radius = Some(radius.into());
        self
    }

    /// Draw the grain under the content instead: only useful when the content
    /// has no fill of its own.
    #[must_use]
    pub fn behind(mut self) -> Self {
        self.placement = GrainPlacement::Behind;
        self
    }

    /// Build the element.
    pub fn into_element(self) -> Element<'a, Message> {
        let mut layer = GrainLayer::new(self.content, self.spec).placement(self.placement);
        if let Some(radius) = self.radius {
            layer = layer.radius(radius);
        }
        layer.into()
    }
}

impl<'a, Message: 'a> From<EnzGrain<'a, Message>> for Element<'a, Message> {
    fn from(grain: EnzGrain<'a, Message>) -> Self {
        grain.into_element()
    }
}
