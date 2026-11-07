//! Horizontal item reveal targets from caller-supplied geometry.

/// Re-clamp a scroll offset into the valid `[0, max]` range, where
/// `max = content_width - band_width` (0 when the content fits). Guards a
/// stale offset after a resize / content shrink from leaving blank space on the
/// right.
///
/// Each non-finite or negative width is treated as zero. A non-finite offset
/// returns zero; a finite offset is clamped to the resulting range.
pub fn clamp_offset(offset: f32, band_width: f32, content_width: f32) -> f32 {
    let band = if band_width.is_finite() {
        band_width.max(0.0)
    } else {
        0.0
    };
    let content = if content_width.is_finite() {
        content_width.max(0.0)
    } else {
        0.0
    };
    let max = (content - band).max(0.0);
    if offset.is_finite() {
        offset.clamp(0.0, max)
    } else {
        0.0
    }
}

/// Minimum-movement reveal for the item spanning
/// `[item_x, item_x + item_width]`:
///
/// * If the item's left edge is off the left of the band, scroll so its
///   left edge is flush with the band's left edge.
/// * Else if the item's right edge is off the right of the band, scroll
///   so its right edge is flush with the band's right edge.
/// * Otherwise the item is already fully visible — return the current
///   offset unchanged (do nothing, preserving the user's manual
///   scroll).
///
/// The result is always re-clamped into `[0, content_width - band_width]`,
/// so a shrunk content set cannot overscroll. Feed the returned offset into
/// a `scroll_to` on the `content_id`.
pub fn reveal_offset(
    current_offset: f32,
    band_width: f32,
    content_width: f32,
    item_x: f32,
    item_width: f32,
) -> f32 {
    let target = if item_x < current_offset {
        item_x
    } else if item_x + item_width > current_offset + band_width {
        item_x + item_width - band_width
    } else {
        current_offset
    };
    clamp_offset(target, band_width, content_width)
}

/// Convenience for uniform items of width `item_width` laid out every
/// `item_width + item_spacing` (tab strips, chip trays).
///
/// A negative `index` is a no-op (returns `current_offset` unchanged).
pub fn reveal_index_offset(
    current_offset: f32,
    band_width: f32,
    content_width: f32,
    index: i32,
    item_width: f32,
    item_spacing: f32,
) -> f32 {
    if index < 0 {
        return current_offset;
    }
    let item_x = index as f32 * (item_width + item_spacing);
    reveal_offset(
        current_offset,
        band_width,
        content_width,
        item_x,
        item_width,
    )
}
