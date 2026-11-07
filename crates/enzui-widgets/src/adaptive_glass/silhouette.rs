//! The shape adaptive glass fills: a signed distance field with each
//! connected component's centre and width.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest side an SVG silhouette is rasterised to.
const RASTER: u32 = 1024;

/// A shape for [`enz_adaptive_glass`](super::enz_adaptive_glass): glyphs, a
/// pill, any outline.
///
/// Every connected component (each numeral of a clock, say) becomes its own
/// lens, bending the backdrop towards its own centre. Holes, such as a
/// digit's counters, stay clear. Build it once and share it; the GPU copy is
/// uploaded on first use.
#[derive(Debug)]
pub struct GlassSilhouette {
    pub(super) id: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    /// Per texel: signed distance in texels, component centre x and y, and
    /// component width, as little-endian `f32`s.
    pub(super) texels: Vec<u8>,
    /// Coverage, for the plain fallback where glass cannot render.
    alpha: Vec<u8>,
    /// The fallback image last built, and the colour it was built in.
    fallback: std::sync::Mutex<Option<([u8; 4], iced::advanced::image::Handle)>>,
}

/// Why an SVG document could not become a [`GlassSilhouette`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SilhouetteError(String);

impl std::fmt::Display for SilhouetteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "silhouette: {}", self.0)
    }
}

impl std::error::Error for SilhouetteError {}

impl GlassSilhouette {
    /// The shape painted by an SVG `document` whose view box is `width` by
    /// `height`. Fill colour does not matter; coverage does.
    ///
    /// # Errors
    ///
    /// When the document does not parse or has no area.
    pub fn from_svg(document: &str, width: f32, height: f32) -> Result<Self, SilhouetteError> {
        if !(width > 0.0 && height > 0.0) {
            return Err(SilhouetteError("empty view box".into()));
        }
        let tree = resvg::usvg::Tree::from_str(document, &resvg::usvg::Options::default())
            .map_err(|error| SilhouetteError(error.to_string()))?;
        let (raster_width, raster_height) = if height > width {
            let w = ((RASTER as f32 * width / height) as u32).clamp(32, RASTER / 2);
            (w, (w as f32 * height / width).ceil() as u32)
        } else {
            (
                RASTER,
                (RASTER as f32 * height / width).ceil().max(1.0) as u32,
            )
        };
        let mut pixels = resvg::tiny_skia::Pixmap::new(raster_width, raster_height)
            .ok_or_else(|| SilhouetteError("raster too large".into()))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(
                raster_width as f32 / width,
                raster_height as f32 / height,
            ),
            &mut pixels.as_mut(),
        );
        let alpha: Vec<u8> = pixels.pixels().iter().map(|p| p.alpha()).collect();
        Ok(Self::from_alpha(raster_width, raster_height, &alpha))
    }

    /// The shape covered by an 8-bit `alpha` raster of `width` by `height`.
    ///
    /// # Panics
    ///
    /// When `alpha` does not hold `width * height` values.
    #[must_use]
    pub fn from_alpha(width: u32, height: u32, alpha: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let (w, h) = (width as usize, height as usize);
        assert_eq!(alpha.len(), w * h, "alpha must cover width x height");
        let inside: Vec<bool> = alpha.iter().map(|a| *a >= 128).collect();
        let inner = distances(w, h, &inside, false);
        let outer = distances(w, h, &inside, true);
        let (labels, centres) = components(w, h, alpha);
        let texels = alpha
            .iter()
            .enumerate()
            .flat_map(|(i, a)| {
                // Partial coverage places the edge within the texel itself.
                let signed = if *a > 0 && *a < 255 {
                    0.5 - f32::from(*a) / 255.0
                } else if inside[i] {
                    -(inner[i] - 0.5)
                } else {
                    outer[i] - 0.5
                };
                let [x, y, component_width] = centres[labels[i]];
                [signed, x, y, component_width]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
            })
            .collect();
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            texels,
            alpha: alpha.to_vec(),
            fallback: std::sync::Mutex::default(),
        }
    }

    /// The shape filled with `color`, built once per colour.
    pub(super) fn fallback_image(&self, color: iced::Color) -> iced::advanced::image::Handle {
        let rgba8 = color.into_rgba8();
        let mut cache = self
            .fallback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((known, handle)) = &*cache
            && *known == rgba8
        {
            return handle.clone();
        }
        let [r, g, b, a] = rgba8;
        let pixels: Vec<u8> = self
            .alpha
            .iter()
            .flat_map(|coverage| [r, g, b, ((u16::from(*coverage) * u16::from(a)) / 255) as u8])
            .collect();
        let handle = iced::advanced::image::Handle::from_rgba(self.width, self.height, pixels);
        *cache = Some((rgba8, handle.clone()));
        handle
    }

    /// Width over height.
    #[must_use]
    pub fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height as f32
    }
}

/// Labels every texel with a connected component and returns each
/// component's centre and width, in texels.
fn components(width: usize, height: usize, alpha: &[u8]) -> (Vec<usize>, Vec<[f32; 3]>) {
    let mut labels = vec![usize::MAX; alpha.len()];
    let mut centres = Vec::new();
    let mut queue = VecDeque::new();
    let neighbours = |i: usize| {
        [
            i.checked_sub(width),
            (i + width < alpha.len()).then_some(i + width),
            (!i.is_multiple_of(width)).then(|| i - 1),
            (i % width + 1 < width).then_some(i + 1),
        ]
    };
    for seed in 0..alpha.len() {
        if alpha[seed] == 0 || labels[seed] != usize::MAX {
            continue;
        }
        let id = centres.len();
        let (mut left, mut right, mut top, mut bottom) =
            (seed % width, seed % width, seed / width, seed / width);
        labels[seed] = id;
        queue.push_back(seed);
        while let Some(i) = queue.pop_front() {
            left = left.min(i % width);
            right = right.max(i % width);
            top = top.min(i / width);
            bottom = bottom.max(i / width);
            for j in neighbours(i).into_iter().flatten() {
                if alpha[j] > 0 && labels[j] == usize::MAX {
                    labels[j] = id;
                    queue.push_back(j);
                }
            }
        }
        centres.push([
            (left + right + 1) as f32 * 0.5,
            (top + bottom + 1) as f32 * 0.5,
            (right - left + 1) as f32,
        ]);
    }
    if centres.is_empty() {
        centres.push([width as f32 * 0.5, height as f32 * 0.5, width as f32]);
        labels.fill(0);
    }
    // Texels outside every shape take their nearest component, so sampling
    // across an edge or a counter never mixes a centre with nothing.
    queue.extend(
        labels
            .iter()
            .enumerate()
            .filter_map(|(i, id)| (*id != usize::MAX).then_some(i)),
    );
    while let Some(i) = queue.pop_front() {
        for j in neighbours(i).into_iter().flatten() {
            if labels[j] == usize::MAX {
                labels[j] = labels[i];
                queue.push_back(j);
            }
        }
    }
    (labels, centres)
}

/// Euclidean distance from every texel to the nearest texel whose `inside`
/// equals `seed` (Felzenszwalb and Huttenlocher's two-pass transform).
fn distances(width: usize, height: usize, inside: &[bool], seed: bool) -> Vec<f32> {
    let mut values: Vec<f32> = inside
        .iter()
        .map(|v| if *v == seed { 0.0 } else { 1e6 })
        .collect();
    let n = width.max(height);
    let mut input = vec![0.0; n];
    let mut output = vec![0.0; n];
    let mut sites = vec![0usize; n];
    let mut cuts = vec![0.0; n + 1];
    for y in 0..height {
        input[..width].copy_from_slice(&values[y * width..(y + 1) * width]);
        distance_line(&input[..width], &mut output[..width], &mut sites, &mut cuts);
        values[y * width..(y + 1) * width].copy_from_slice(&output[..width]);
    }
    for x in 0..width {
        for y in 0..height {
            input[y] = values[y * width + x];
        }
        distance_line(
            &input[..height],
            &mut output[..height],
            &mut sites,
            &mut cuts,
        );
        for y in 0..height {
            values[y * width + x] = output[y].sqrt();
        }
    }
    values
}

/// One-dimensional squared distance transform along a line.
fn distance_line(input: &[f32], output: &mut [f32], sites: &mut [usize], cuts: &mut [f32]) {
    let mut k = 0;
    sites[0] = 0;
    cuts[0] = f32::NEG_INFINITY;
    cuts[1] = f32::INFINITY;
    for q in 1..input.len() {
        let mut s;
        loop {
            let p = sites[k];
            s = ((input[q] + (q * q) as f32) - (input[p] + (p * p) as f32))
                / (2.0 * (q as f32 - p as f32));
            if s > cuts[k] {
                break;
            }
            k -= 1;
        }
        k += 1;
        sites[k] = q;
        cuts[k] = s;
        cuts[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, value) in output.iter_mut().enumerate() {
        while cuts[k + 1] < q as f32 {
            k += 1;
        }
        let offset = q as f32 - sites[k] as f32;
        *value = offset * offset + input[sites[k]];
    }
}

#[cfg(test)]
mod tests {}
