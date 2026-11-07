//! Font-backed text measurement, visual-line break offsets and lossless pagination.
//!
//! # Example
//!
//! ```
//! use enzui_widgets::text::TextSize;
//! use enzui_widgets::text_measure::{MeasureTypography, paginate_by_height_with};
//!
//! let text = "A short paragraph for a reader.";
//! let typography = MeasureTypography::new(TextSize::Body)
//!     .pin_size_px(16.0)
//!     .pin_line_height_px(24.0);
//! let pages = paginate_by_height_with(text, 300.0, 240.0, typography);
//! assert_eq!(pages.first().map(|page| page.0), Some(0));
//! assert_eq!(pages.last().map(|page| page.1), Some(text.len()));
//! ```
//!
//! Measurement shapes synchronously against iced's shared font system without
//! a renderer. Pass the painted surface's size, line height, family, weight,
//! shaping and wrapping; surface ellipsis and line clamps are not measured.
//! [`TextMeasurement::line_count`] is derived from height and line height.
//!
//! Pagination uses grapheme-clamped UAX#14 opportunities, or a caller boundary
//! hook. When no legal prefix fits, it emits the first legal span even if that
//! exceeds the height budget. Dictionary segmentation for scripts such as Thai
//! requires a caller hook. Empty text or invalid dimensions produce no pages.
//!
//! [`enz_text_break_offset_with`] reads visual layout runs. [`shape_count`] and
//! [`shape_bytes`] observe this module's shaping requests on the calling thread;
//! they exclude boundary scanning and other modules. [`estimate_text_height`]
//! provides a font-system-free approximation. For growing editors, see
//! [`crate::text_area::EnzTextArea::auto_grow`] and its `row_count` method.

#![allow(
    clippy::string_slice,
    reason = "page breaks come from the text's own break offsets"
)]

use crate::enz_text::{TextShaping, TextSize};
use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
use iced::advanced::text::{LineHeight, Paragraph as _, Text, Wrapping};
use iced::alignment;
use iced::font::{Family, Style, Weight};
use iced::{Font, Pixels, Size};
use std::cell::Cell;

const UNBOUNDED_HEIGHT: f32 = 1.0e7;

thread_local! {
    static SHAPE_COUNT: Cell<u64> = const { Cell::new(0) };

    static SHAPE_BYTES: Cell<u64> = const { Cell::new(0) };
}

/// Number of paragraph construction requests made by this module on the current thread.
/// Take a delta around a call; this excludes other threads and other modules.
#[inline]
pub fn shape_count() -> u64 {
    SHAPE_COUNT.with(std::cell::Cell::get)
}

/// Total input bytes submitted to paragraph construction by this module on the current thread.
/// Repeated shaping counts the bytes again; boundary scanning and font loading are excluded.
#[inline]
pub fn shape_bytes() -> u64 {
    SHAPE_BYTES.with(std::cell::Cell::get)
}

#[inline]
// Keep all paragraph construction here so both observation counters remain complete.
fn shape(text: Text<&str>) -> GraphicsParagraph {
    SHAPE_COUNT.with(|c| c.set(c.get() + 1));
    SHAPE_BYTES.with(|c| c.set(c.get() + text.content.len() as u64));
    GraphicsParagraph::with_text(text)
}

#[inline]
fn is_positive(x: f32) -> bool {
    x.is_finite() && x > 0.0
}

/// Default glyph-family selection when no explicit family is supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextMeasureKind {
    /// Use the ambient UI font role by default.
    #[default]
    Body,
    /// Use generic monospace, installing the bundled upright fallback on first measurement.
    Monospace,
}

/// Typography and pagination boundaries for a measured text run.
///
/// Defaults are body text, upright slant, automatic shaping and word wrapping.
/// Unset size and weight overrides use the forwarded scale or ambient desktop
/// typography. Unset body family uses the ambient UI role.
///
/// Equality compares boundary hooks by presence only: two different `Some` hooks
/// compare equal when the other fields match. Equality alone cannot identify a
/// change of pagination policy.
#[derive(Debug, Clone, Copy)]
#[must_use]
pub struct MeasureTypography {
    /// Semantic text size.
    pub size: TextSize,
    /// Font kind (family).
    pub kind: TextMeasureKind,
    /// Explicit weight override; `None` resolves the size rung through forwarded or ambient typography.
    pub weight: Option<Weight>,
    /// Italic slant.
    pub italic: bool,
    /// Shaping policy; defaults to [`TextShaping::Auto`].
    /// Use the same policy as the painted surface; rich text uses Advanced.
    pub shaping: TextShaping,
    /// Line-wrapping strategy. Defaults to [`Wrapping::Word`].
    pub wrapping: Wrapping,
    /// Forwarded typography scale; `None` uses ambient desktop typography.
    pub typography: Option<enzui_theme::TypographyScale>,
    /// Optional pagination hook receiving the full text and returning legal ending byte offsets.
    /// Invalid offsets are discarded, valid offsets are clamped down to grapheme
    /// boundaries, and the result is sorted, deduplicated and extended with `text.len()`.
    /// `None` uses UAX#14 opportunities. Hook panics propagate to the caller.
    pub boundaries: Option<fn(&str) -> Vec<usize>>,
    /// Explicit family overriding the kind default.
    /// `None` uses the ambient UI role for body text or generic monospace.
    /// [`Self::font_from_theme`] snapshots the theme catalog instead.
    pub font_family: Option<Family>,
    /// Absolute line-height override in logical pixels, taking precedence over size-derived height.
    /// Direct field assignment is not validated; [`Self::pin_line_height_px`] accepts
    /// only finite positive values.
    pub line_height_px: Option<f32>,
    /// Font-size override in logical pixels, taking precedence over forwarded or ambient typography.
    /// Without a line-height override, its line box is derived by
    /// [`crate::enz_text::line_box_px`]. Direct field assignment is not validated.
    pub size_px: Option<f32>,
}
impl PartialEq for MeasureTypography {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
            && self.kind == other.kind
            && self.weight == other.weight
            && self.italic == other.italic
            && self.shaping == other.shaping
            && self.wrapping == other.wrapping
            && self.typography == other.typography
            && self.boundaries.is_some() == other.boundaries.is_some()
            && self.font_family == other.font_family
            && self.line_height_px == other.line_height_px
            && self.size_px == other.size_px
    }
}

impl Default for MeasureTypography {
    fn default() -> Self {
        Self {
            size: TextSize::Body,
            kind: TextMeasureKind::Body,
            weight: None,
            italic: false,
            shaping: TextShaping::default(),
            wrapping: Wrapping::Word,
            typography: None,
            boundaries: None,
            font_family: None,
            line_height_px: None,
            size_px: None,
        }
    }
}

impl MeasureTypography {
    /// Construct from a [`TextSize`] with body font and size-implied weight.
    pub fn new(size: TextSize) -> Self {
        Self {
            size,
            ..Self::default()
        }
    }

    /// Set the default family kind; an existing family override remains in effect.
    pub fn kind(mut self, kind: TextMeasureKind) -> Self {
        self.kind = kind;
        self
    }

    /// Override the weight (mirrors `EnzText::weight`).
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Set italic.
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// Set the shaping strategy.
    ///
    /// Defaults to [`TextShaping::Auto`], matching `EnzText`. Pass the same
    /// explicit policy used by the rendered run so exact measurement keeps the
    /// same advances and wrap points. Rich text uses Advanced. The raw iced
    /// shaping enum is accepted for source compatibility.
    pub fn shaping(mut self, shaping: impl Into<TextShaping>) -> Self {
        self.shaping = shaping.into();
        self
    }

    /// Set the line-wrapping strategy used by measurement.
    ///
    /// Defaults to [`Wrapping::Word`], preserving the pagination profile.
    /// Forward the painted surface's mode when measuring its visual rows.
    pub fn wrapping(mut self, wrapping: Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    /// Forward a typography scale for size, line height and weight resolution.
    /// Explicit pins and weight overrides retain precedence.
    pub fn typography(mut self, scale: enzui_theme::TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// Supply legal pagination endings for the full text.
    /// See [`Self::boundaries`] for normalization and panic behavior.
    pub fn boundaries(mut self, f: fn(&str) -> Vec<usize>) -> Self {
        self.boundaries = Some(f);
        self
    }

    /// Override the glyph family with a resolved family.
    /// Use the same family as the painted text to preserve wrap points.
    pub fn font_family(mut self, family: Family) -> Self {
        self.font_family = Some(family);
        self
    }

    /// Pin absolute line height in logical pixels, independently of size.
    /// Non-finite or nonpositive inputs preserve the existing pin.
    pub fn pin_line_height_px(mut self, px: f32) -> Self {
        if is_positive(px) {
            self.line_height_px = Some(px);
        }
        self
    }

    /// Pin font size in logical pixels, overriding the scale.
    /// Line height follows [`crate::enz_text::line_box_px`] unless separately pinned.
    /// Non-finite or nonpositive inputs preserve the existing pin.
    pub fn pin_size_px(mut self, px: f32) -> Self {
        if is_positive(px) {
            self.size_px = Some(px);
        }
        self
    }

    /// Snapshot the theme catalog family for the current kind.
    /// Body selects the Document role; monospace selects the Monospace role.
    /// Changing the kind afterward does not re-resolve this override. Share the
    /// resolved family with the painted text to preserve pagination.
    pub fn font_from_theme(mut self, theme: &enzui_theme::EnzTheme) -> Self {
        let role = match self.kind {
            TextMeasureKind::Body => enzui_theme::FontRole::Document,
            TextMeasureKind::Monospace => enzui_theme::FontRole::Monospace,
        };
        self.font_family = Some(theme.font_catalog.family(role).to_iced());
        self
    }

    fn break_offsets(&self, text: &str) -> Vec<usize> {
        match self.boundaries {
            Some(f) => normalize_boundaries(text, f(text)),
            None => default_break_offsets(text),
        }
    }

    /// Resolve font size in logical pixels: explicit pin, then forwarded or ambient typography.
    pub fn size_px(&self) -> f32 {
        if let Some(px) = self.size_px {
            return px;
        }
        self.size.resolve_size(self.typography.as_ref())
    }

    /// Resolve absolute line height in logical pixels.
    /// An explicit line-height pin wins, followed by a pinned size's proportional
    /// line box, then the size rung resolved through forwarded or ambient typography.
    pub fn line_height_px(&self) -> f32 {
        if let Some(px) = self.line_height_px {
            return px;
        }
        if let Some(px) = self.size_px {
            return crate::enz_text::line_box_px(px);
        }
        self.size.resolve_line_height(self.typography.as_ref())
    }

    /// Resolve weight: explicit override, then the size rung through forwarded or ambient typography.
    pub fn effective_weight(&self) -> Weight {
        self.weight
            .unwrap_or_else(|| self.size.resolve_weight(self.typography.as_ref()))
    }

    pub(crate) fn font(&self) -> Font {
        let family = match self.font_family {
            Some(f) => f,
            None => match self.kind {
                TextMeasureKind::Body => {
                    enzui_theme::text_defaults::family(enzui_theme::FontRole::Ui).to_iced()
                }
                TextMeasureKind::Monospace => {
                    crate::monospace_fallback::ensure_monospace_fallback();
                    Family::Monospace
                }
            },
        };
        Font {
            family,
            weight: self.effective_weight(),
            style: if self.italic {
                Style::Italic
            } else {
                Style::Normal
            },
            ..Font::DEFAULT
        }
    }

    fn as_text<'a>(&self, content: &'a str, measure_width: f32) -> Text<&'a str> {
        Text {
            content,
            bounds: Size::new(measure_width, UNBOUNDED_HEIGHT),
            size: Pixels(self.size_px()),
            line_height: LineHeight::Absolute(Pixels(self.line_height_px())),
            font: self.font(),
            align_x: alignment::Horizontal::Left.into(),
            align_y: alignment::Vertical::Top,
            shaping: self.shaping.into(),
            wrapping: self.wrapping,
            ellipsis: iced::advanced::text::Ellipsis::None,
            hint_factor: None,
        }
    }
}

/// Font-backed paragraph bounds and a height-derived visual-line estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMeasurement {
    /// Paragraph height in logical pixels; zero for empty text or an invalid width.
    pub measured_height: f32,
    /// Resolved absolute line height in logical pixels, including for empty or invalid-width inputs.
    pub line_height: f32,
    /// Height divided by line height, rounded and at least one for nonempty valid-width inputs.
    /// This is an estimate rather than a count of the shaper's layout runs.
    pub line_count: usize,
}

/// Measure paragraph height at a logical-pixel width using body typography.
/// Newlines force breaks; default wrapping is by word. Empty text or a width
/// that is non-finite or nonpositive yields zero height and zero line count.
pub fn enz_text_measure(text: &str, measure_width: f32, size: TextSize) -> TextMeasurement {
    enz_text_measure_with(text, measure_width, MeasureTypography::new(size))
}

/// Measure paragraph height using an explicit descriptor.
/// Empty text or a non-finite or nonpositive width yields zero height and line
/// count while retaining the resolved line height. Measurement uses a finite
/// vertical bound of 10 million logical pixels.
pub fn enz_text_measure_with(
    text: &str,
    measure_width: f32,
    typo: MeasureTypography,
) -> TextMeasurement {
    let line_height = typo.line_height_px();

    if text.is_empty() || !is_positive(measure_width) {
        return TextMeasurement {
            measured_height: 0.0,
            line_height,
            line_count: 0,
        };
    }

    let paragraph = shape(typo.as_text(text, measure_width));
    let bounds = paragraph.min_bounds();
    let line_count = if line_height > 0.0 {
        (bounds.height / line_height).round().max(1.0) as usize
    } else {
        1
    };

    TextMeasurement {
        measured_height: bounds.height,
        line_height,
        line_count,
    }
}

/// Return the source byte offset where the first visual line after
/// `max_lines` begins.
///
/// The offset comes from the same graphics [`GraphicsParagraph`] layout runs
/// that iced's official renderers paint; it is not a character-advance
/// estimate. A return value of `text.len()` means all content fits. Empty text,
/// a zero line budget, or an invalid width returns `0`.
///
/// The result is a UTF-8 byte offset and always lies on a character boundary.
/// Build `typo` with [`MeasureTypography::pin_size_px`] and
/// [`MeasureTypography::pin_line_height_px`] when a reader exposes independent
/// font-size and line-spacing settings.
pub fn enz_text_break_offset_with(
    text: &str,
    measure_width: f32,
    max_lines: usize,
    typo: MeasureTypography,
) -> usize {
    if text.is_empty() || max_lines == 0 || !is_positive(measure_width) {
        return 0;
    }

    let paragraph = shape(typo.as_text(text, measure_width));
    let buffer = paragraph.buffer();
    let Some(next_run) = buffer.layout_runs().nth(max_lines) else {
        return text.len();
    };
    // Glyph offsets are hard-line-local; minimum logical offset handles visual bidi order.
    let mut source_base = 0usize;
    for line in buffer.lines.iter().take(next_run.line_i) {
        source_base += line.text().len() + line.ending().as_str().len();
    }
    let local_offset = next_run
        .glyphs
        .iter()
        .map(|glyph| glyph.start)
        .min()
        .unwrap_or(0);

    (source_base + local_offset).min(text.len())
}

/// Measure the widest paragraph line in logical pixels.
/// Empty text returns zero. Newlines produce separate lines; the width and height
/// bounds are 10 million logical pixels, so exceptionally long content can wrap.
///
/// Widths are cached per thread by text and resolved typography, and dropped
/// when a font load changes what text measures: views measure the same
/// labels on every rebuild.
pub fn enz_text_measure_width(text: &str, typo: MeasureTypography) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let request = typo.as_text(text, UNBOUNDED_HEIGHT);
    let key = WidthKey {
        size: request.size.0.to_bits(),
        line_height: match request.line_height {
            LineHeight::Absolute(px) => px.0.to_bits(),
            LineHeight::Relative(factor) => !factor.to_bits(),
        },
        font: request.font,
        shaping: request.shaping,
        wrapping: request.wrapping,
    };
    let fonts = iced::advanced::graphics::text::font_system()
        .read()
        .map(|system| system.version())
        .ok();
    let cached = MEASURED_WIDTHS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.fonts != fonts {
            cache.fonts = fonts;
            cache.widths.clear();
            cache.len = 0;
        }
        cache
            .widths
            .get(&key)
            .and_then(|widths| widths.get(text))
            .copied()
    });
    if let Some(width) = cached {
        return width;
    }
    let width = shape(request).min_bounds().width;
    MEASURED_WIDTHS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len >= MEASURED_WIDTHS_CAPACITY {
            cache.widths.clear();
            cache.len = 0;
        }
        cache.len += 1;
        cache
            .widths
            .entry(key)
            .or_default()
            .insert(text.into(), width);
    });
    width
}

/// How many widths a thread keeps before starting over.
const MEASURED_WIDTHS_CAPACITY: usize = 4096;

/// What a measured width depends on besides the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct WidthKey {
    size: u32,
    line_height: u32,
    font: Font,
    shaping: iced::advanced::text::Shaping,
    wrapping: Wrapping,
}

#[derive(Default)]
struct MeasuredWidths {
    /// The font system version the widths were measured against.
    fonts: Option<iced::advanced::graphics::text::Version>,
    widths: std::collections::HashMap<WidthKey, std::collections::HashMap<Box<str>, f32>>,
    /// Widths held across every typography.
    len: usize,
}

thread_local! {
    static MEASURED_WIDTHS: std::cell::RefCell<MeasuredWidths> =
        std::cell::RefCell::new(MeasuredWidths::default());
}

/// Split text into contiguous, lossless byte ranges using body typography.
/// See [`paginate_by_height_with`] for height overflow and boundary rules.
pub fn paginate_by_height(
    text: &str,
    measure_width: f32,
    max_height: f32,
    size: TextSize,
) -> Vec<(usize, usize)> {
    paginate_by_height_with(
        text,
        measure_width,
        max_height,
        MeasureTypography::new(size),
    )
}

/// Split text into contiguous, lossless byte ranges at legal pagination boundaries.
///
/// Each page selects the largest fitting legal prefix at the supplied
/// logical-pixel width and height. If even the first legal span exceeds the
/// height, that span is emitted to guarantee progress without dropping bytes.
/// Default boundaries use grapheme-clamped UAX#14 opportunities; a supplied hook
/// is normalized as described by [`MeasureTypography::boundaries`].
///
/// Empty text or a non-finite or nonpositive dimension returns an empty vector.
/// Panics from a supplied boundary hook propagate.
pub fn paginate_by_height_with(
    text: &str,
    measure_width: f32,
    max_height: f32,
    typo: MeasureTypography,
) -> Vec<(usize, usize)> {
    if text.is_empty() || !is_positive(measure_width) || !is_positive(max_height) {
        return Vec::new();
    }
    let boundaries = typo.break_offsets(text);

    let mut pages: Vec<(usize, usize)> = Vec::new();
    let mut page_start = 0usize;
    let mut cursor = 0usize;

    let fits = |page_start: usize, end: usize| -> bool {
        let slice = &text[page_start..end];
        let m = enz_text_measure_with(slice, measure_width, typo);
        m.measured_height <= max_height + f32::EPSILON
    };

    while page_start < text.len() {
        while cursor < boundaries.len() && boundaries[cursor] <= page_start {
            cursor += 1;
        }
        // Gallop over this page before binary search to limit repeated shaping of long suffixes.
        let n = boundaries.len() - cursor;
        debug_assert!(n > 0, "cursor invariant: at least one candidate remains");
        let mut last_fit: Option<usize> = None;
        let mut lo_off = 0usize;
        let hi_off;
        let mut step = 1usize;
        let mut probe = 0usize;
        loop {
            let end = boundaries[cursor + probe];
            if fits(page_start, end) {
                last_fit = Some(end);
                lo_off = probe;
                if probe + 1 == n {
                    hi_off = n;
                    break;
                }
                let next = (probe + step).min(n - 1);
                if next == probe {
                    hi_off = n;
                    break;
                }
                probe = next;
                step *= 2;
            } else {
                hi_off = probe;
                break;
            }
        }
        if hi_off > lo_off + 1 {
            let (mut blo, mut bhi) = (lo_off + 1, hi_off);
            while blo < bhi {
                let mid = (blo + bhi) / 2;
                let end = boundaries[cursor + mid];
                if fits(page_start, end) {
                    last_fit = Some(end);
                    blo = mid + 1;
                } else {
                    bhi = mid;
                }
            }
        }

        let page_end = match last_fit {
            Some(end) => end,
            // Preserve losslessness when even the first legal span exceeds the budget.
            None => boundaries[cursor],
        };

        pages.push((page_start, page_end));
        page_start = page_end;
    }

    pages
}

fn default_break_offsets(text: &str) -> Vec<usize> {
    let len = text.len();
    if len == 0 {
        return vec![0];
    }
    let mut offsets: Vec<usize> = unicode_linebreak::linebreaks(text)
        .map(|(offset, _opportunity)| clamp_to_grapheme(text, offset))
        .collect();
    if offsets.last() != Some(&len) {
        offsets.push(len);
    }
    offsets.dedup();
    offsets
}

fn clamp_to_grapheme(text: &str, offset: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    if offset == 0 || offset >= text.len() {
        return offset;
    }
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= offset)
        .last()
        .unwrap_or(0)
}

fn normalize_boundaries(text: &str, mut offsets: Vec<usize>) -> Vec<usize> {
    let len = text.len();
    offsets.retain(|&b| b > 0 && b <= len && text.is_char_boundary(b));
    for b in &mut offsets {
        *b = clamp_to_grapheme(text, *b);
    }
    offsets.retain(|&b| b > 0);
    offsets.sort_unstable();
    if offsets.last() != Some(&len) {
        offsets.push(len);
    }
    offsets.dedup();
    offsets
}

/// Line-height ratio used by the rough estimator.
pub const ESTIMATE_LINE_HEIGHT_RATIO: f32 = 1.3;

/// Character-advance ratio (× size) used by the rough estimator per kind.
pub fn estimate_advance_ratio(kind: TextMeasureKind) -> f32 {
    match kind {
        TextMeasureKind::Body => 0.52,
        TextMeasureKind::Monospace => 0.60,
    }
}

/// Estimate height without accessing the font system.
/// Uses the fixed [`TextSize::px`] ramp, scalar counts and a 1.3 line-height
/// ratio, ignoring ambient typography. Newlines force rows; estimated wrapping
/// can split words or grapheme clusters. Empty text or invalid width returns zero.
pub fn estimate_text_height(
    text: &str,
    measure_width: f32,
    size: TextSize,
    kind: TextMeasureKind,
) -> f32 {
    let px = size.px();
    let line_height = px * ESTIMATE_LINE_HEIGHT_RATIO;
    if text.is_empty() || !is_positive(measure_width) {
        return 0.0;
    }
    let advance = px * estimate_advance_ratio(kind);
    if advance <= 0.0 {
        return 0.0;
    }
    let mut lines = 0usize;
    for logical in text.split('\n') {
        let n = logical.chars().count();
        if n == 0 {
            lines += 1;
        } else {
            let w = n as f32 * advance;
            lines += ((w / measure_width).ceil() as usize).max(1);
        }
    }
    lines as f32 * line_height
}
