//! Document-tab reveal offsets and scroll operations.

use super::*;

/// Identify a strip’s session scrollable from its label.
///
/// Use a distinct label for each strip and the same label for reveal operations.
#[must_use]
pub fn session_scrollable_id(label: &str) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("enz-document-tabs-session::{label}"))
}

/// Scroll the session viewport to an absolute pixel offset along its orientation.
///
/// Run this operation with the strip’s label and update the application’s
/// [`EnzDocumentTabs::scroll_offset`] as well. Changing the filter offset alone
/// does not move the scrollable’s retained widget state. Pair with [`reveal_offset`].
pub fn reveal_scroll_operation<T: Send + 'static>(
    label: &str,
    offset: f32,
    orientation: Orientation,
) -> impl iced::advanced::widget::Operation<T> {
    let target = match orientation {
        Orientation::Horizontal => iced::widget::scrollable::AbsoluteOffset {
            x: Some(offset),
            y: None,
        },
        Orientation::Vertical => iced::widget::scrollable::AbsoluteOffset {
            x: None,
            y: Some(offset),
        },
    };
    iced::advanced::widget::operation::scrollable::scroll_to(
        session_scrollable_id(label),
        target,
        iced::advanced::widget::operation::Animation::Instant,
    )
}

/// Compute the minimum scroll needed to fully reveal a session tab.
///
/// Already visible, pinned or unknown tabs return `None`, as do viewports without
/// session room. Oversized tabs align at their leading edge. Other tabs align
/// at the nearest edge, and results are clamped to the metrics’ scroll range;
/// a result equal to the current offset returns `None`. Non-finite or negative
/// current offsets are treated as zero.
///
/// Use metrics matching the widget’s sizing and density, and an extent excluding
/// any fixed new-tab affordance. Feed a returned offset through both
/// [`EnzDocumentTabs::scroll_offset`] and [`reveal_scroll_operation`].
#[must_use]
pub fn reveal_offset(
    metrics: &StripMetrics,
    key: &enzui_core::document_tabs::DocumentKey,
    scroll_offset: f32,
    extent: f32,
) -> Option<f32> {
    let index = metrics.session_keys().iter().position(|k| k == key)?;

    let available = metrics.available_extent(extent);
    if available <= 0.0 {
        return None;
    }

    let lead = metrics.session_offset_of(index);
    let width = metrics.session_width_of(index);
    let trail = lead + width;

    let current = if scroll_offset.is_finite() {
        scroll_offset.max(0.0)
    } else {
        0.0
    };
    let view_end = current + available;

    let target = if width > available || lead < current {
        lead
    } else if trail > view_end {
        trail - available
    } else {
        return None;
    };

    let clamped = target.clamp(0.0, metrics.max_scroll_offset(extent));
    if clamped == current {
        None
    } else {
        Some(clamped)
    }
}
