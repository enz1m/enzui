//! Internal cap-band centring from the shaped H glyph's font metrics.
//!
//! Offsets use logical pixels, positive downward, and are clamped to a quarter
//! of the requested font size. Invalid dimensions or unavailable metrics yield
//! zero. The process cache uses font and size/line-height bits; it has no size
//! bound or font-load invalidation.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::Font;
use iced::advanced::graphics::text::cosmic_text;
use iced::widget::text::LineHeight;
pub(crate) fn cap_centre_offset(font: Font, size: f32, line_height: LineHeight) -> f32 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    let line = line_height.to_absolute(iced::Pixels(size)).0;
    if !line.is_finite() || line <= 0.0 {
        return 0.0;
    }
    type HeightCache = Mutex<HashMap<(Font, u32, u32), f32>>;
    static CACHE: OnceLock<HeightCache> = OnceLock::new();
    let key = (font, size.to_bits(), line.to_bits());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return v;
    }
    let v = measure(font, size, line)
        .filter(|v| v.is_finite())
        .map_or(0.0, |v| v.clamp(-size / 4.0, size / 4.0));
    if let Ok(mut c) = cache.lock() {
        c.insert(key, v);
    }
    v
}

fn measure(font: Font, size: f32, line: f32) -> Option<f32> {
    // Resolve generic families through the same fallback setup used for painting.
    crate::sans_serif_fallback::ensure_sans_serif_fallback();
    let mut system = iced::advanced::graphics::text::font_system().write().ok()?;
    let system = system.raw();
    let mut buffer = cosmic_text::Buffer::new(system, cosmic_text::Metrics::new(size, line));
    buffer.set_text(
        "H",
        &iced::advanced::graphics::text::to_attributes(font),
        cosmic_text::Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(system, false);
    let run = buffer.layout_runs().next()?;
    let glyph = run.glyphs.first()?;
    let (baseline, top) = (run.line_y, run.line_top);
    let face = system.get_font(glyph.font_id, glyph.font_weight)?;
    let metrics = face.as_swash().metrics(&[]);
    let cap = metrics.cap_height / f32::from(metrics.units_per_em) * size;
    if cap <= 0.0 {
        return None;
    }
    Some((top + line / 2.0) - (baseline - cap / 2.0))
}

/// [`cap_centre_offset`] for a single line centred in a box `box_height` tall
/// whose top sits on a whole pixel, adjusted so the baseline lands exactly on a
/// whole pixel.
///
/// The renderer rounds glyph baselines to whole pixels, so the plain offset
/// could leave the cap band up to half a pixel high or low depending on the
/// box height. This picks the pixel row itself and, between two rows, prefers
/// the lower one: a label without descenders over a button's contact shadow
/// otherwise reads as sitting high.
pub(crate) fn snapped_cap_centre_offset(
    font: Font,
    size: f32,
    line_height: LineHeight,
    box_height: f32,
) -> f32 {
    let offset = cap_centre_offset(font, size, line_height);
    let line = line_height.to_absolute(iced::Pixels(size)).0;
    let Some(baseline) = baseline_from_top(font, size, line) else {
        return offset;
    };
    if !box_height.is_finite() || box_height <= 0.0 {
        return offset;
    }
    // Box padding of twice the offset on one side moves the centred line by
    // the offset, so the baseline sits here, relative to the box top.
    let ideal = offset + (box_height - line) / 2.0 + baseline;
    let row = (ideal - LOWER_ROW_BIAS).ceil();
    (offset + row - ideal).clamp(-size / 4.0, size / 4.0)
}

/// How far below the ideal baseline the chosen row may be before the row above
/// wins: up to 0.7 px lower, at most 0.3 px higher.
const LOWER_ROW_BIAS: f32 = 0.3;

fn baseline_from_top(font: Font, size: f32, line: f32) -> Option<f32> {
    if !size.is_finite() || size <= 0.0 || !line.is_finite() || line <= 0.0 {
        return None;
    }
    type BaselineCache = Mutex<HashMap<(Font, u32, u32), Option<f32>>>;
    static CACHE: OnceLock<BaselineCache> = OnceLock::new();
    let key = (font, size.to_bits(), line.to_bits());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return v;
    }
    let v = (|| {
        crate::sans_serif_fallback::ensure_sans_serif_fallback();
        let mut system = iced::advanced::graphics::text::font_system().write().ok()?;
        let system = system.raw();
        let mut buffer = cosmic_text::Buffer::new(system, cosmic_text::Metrics::new(size, line));
        buffer.set_text(
            "H",
            &iced::advanced::graphics::text::to_attributes(font),
            cosmic_text::Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(system, false);
        let run = buffer.layout_runs().next()?;
        Some(run.line_y - run.line_top).filter(|v| v.is_finite())
    })();
    if let Ok(mut c) = cache.lock() {
        c.insert(key, v);
    }
    v
}
