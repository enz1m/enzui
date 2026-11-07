//! Trigger and popup geometry tokens.

use super::*;

/// Size tier for `EnzComboBox`. Re-exports `TextFieldSize` so combo fields
/// and text fields visually pair one-to-one.
pub type ComboBoxSize = TextFieldSize;

/// Gap between the trigger's bottom edge and the popup's top edge.
pub const POPUP_GAP: f32 = 4.0;

/// Maximum nominal visible row count before scrolling; placement may shrink further.
pub const VISIBLE_ROWS: usize = 6;

pub(crate) const POPUP_PADDING: f32 = 4.0;

pub(crate) const POPUP_RADIUS_FALLBACK: f32 = 10.0;

pub(crate) const ROW_RADIUS_FALLBACK: f32 = 6.0;

pub(crate) const CHECK_COLUMN: f32 = 18.0;

/// Bundled checkmark alias used by the selected option.
pub const SELECTED_CHECK_ICON: &str = "selection.check";

/// Popup height cap in logical pixels: row heights plus top and bottom padding.
#[must_use]
pub fn popup_max_height(size: ComboBoxSize) -> f32 {
    size.combo_tokens().height * VISIBLE_ROWS as f32 + POPUP_PADDING * 2.0
}
