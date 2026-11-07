//! Shared scroll-offset calculations for revealing rows and correcting heights.
//!
//! [`reveal_offset_at`] maps an index to a clamped pixel offset; each virtual
//! surface handles its own stable-key lookup. [`reconcile_measurements`]
//! captures the current index anchor before updating heights, preserving its
//! viewport position. Sequence mutations require a stable-key anchor instead.

use enzui_core::virtual_sequence::{
    Anchor, MeasurementCorrection, SequenceGeometry, VariableHeightModel,
};

/// Where a revealed row should sit in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RevealAlignment {
    /// Move the minimum amount: no movement when the row is fully visible,
    /// otherwise bring the nearest edge into view.
    #[default]
    Nearest,
    /// Put the row's top at the viewport top.
    Start,
    /// Centre the row in the viewport.
    Center,
    /// Put the row's bottom at the viewport bottom.
    End,
}

fn sane_nonnegative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

fn sane_positive(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

/// Compute the clamped absolute offset revealing `index` under `alignment`.
///
/// Returns `None` when the index is outside the geometry. Nonpositive or
/// nonfinite viewport heights use zero; negative or nonfinite current offsets
/// use zero. [`RevealAlignment::Nearest`] preserves an already fully visible
/// row's current offset, subject to the final scrollable-extent clamp.
#[must_use]
pub fn reveal_offset_at(
    geometry: &(impl SequenceGeometry + ?Sized),
    index: usize,
    viewport_height: f32,
    current_scroll_top: f32,
    alignment: RevealAlignment,
) -> Option<f32> {
    if index >= geometry.len() {
        return None;
    }

    let viewport_height = sane_positive(viewport_height, 0.0);
    let current = sane_nonnegative(current_scroll_top, 0.0);
    let start = geometry.offset_of(index);
    let end = geometry.offset_of(index.saturating_add(1));
    let max_scroll = (geometry.total_height() - viewport_height).max(0.0);

    let target = match alignment {
        RevealAlignment::Start => start,
        RevealAlignment::Center => start - (viewport_height - (end - start)) / 2.0,
        RevealAlignment::End => end - viewport_height,
        RevealAlignment::Nearest => {
            if start < current {
                start
            } else if end > current + viewport_height {
                end - viewport_height
            } else {
                current
            }
        }
    };
    Some(target.clamp(0.0, max_scroll))
}

/// Update measured heights and return the offset preserving the current index anchor.
///
/// Captures the anchor in the old geometry before applying measurements.
/// Only height deltas strictly above the anchor shift the returned offset;
/// measurements at or below it still update the model. Each changed height
/// updates the subsequent prefix offsets in O(rows below the change).
/// An empty batch preserves `scroll_top`; the result is not clamped or
/// normalized. Keep sequence ordering unchanged during this operation.
///
/// [`crate::row_measure::RowMeasurement`] converts into the accepted tuples;
/// [`crate::chat_list::EnzChatList::on_measure`] wires that producer into rows.
/// Callers retain the model and apply the returned offset to both the pixel
/// scroll position and the virtual surface's realisation window.
#[must_use]
pub fn reconcile_measurements(
    model: &mut VariableHeightModel,
    scroll_top: f32,
    measurements: impl IntoIterator<Item = (usize, f32)>,
) -> f32 {
    let anchor_index = Anchor::from_scroll_top(model, scroll_top).index();
    let delta = model.apply_measurements(measurements, anchor_index);
    MeasurementCorrection { delta }.apply(scroll_top)
}
