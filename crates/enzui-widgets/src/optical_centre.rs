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
