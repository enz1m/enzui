//! The fill-width, single-line row preset of [`EnzLabel`].
//!
//! [`enz_icon_label`] uses an intrinsic icon frame, a Body title rung and an
//! icon following the row tone. The title elides at the end by default.
//! Compared with [`enz_label`](crate::label::enz_label), empty titles collapse
//! the text column, names are not whitespace-trimmed, catalog icons accept
//! zero size, and a named accessibility node can remain when no branch renders.
//! All label capabilities are available through this alias; see [`crate::label`]
//! for colour precedence, metrics and accessibility contracts.

use crate::label::{self, EnzLabel, LabelA11y, LabelDisplay, LabelTone};

/// The canonical label builder, preset for the icon + title row. See
/// [`enz_icon_label`] and [`EnzLabel`].
pub type EnzIconLabel = EnzLabel<'static>;

/// Semantic tone applied to the icon and (by default) the label — the canonical
/// [`LabelTone`].
pub type IconLabelTone = LabelTone;

/// Which parts of the row to show — the canonical
/// [`LabelDisplay`]. Note the row spelling's `Both`, and
/// [`LabelStyle`](crate::label::LabelStyle)'s `TitleAndIcon`, are
/// the same presentation.
pub type IconLabelDisplay = LabelDisplay;

/// How this row names itself to assistive technology — the canonical
/// [`LabelA11y`].
pub type IconLabelA11y = LabelA11y;

/// The stock metric and weight defaults, re-exported at their compatibility path.
pub use crate::label::{DEFAULT_ICON_SIZE, DEFAULT_TEXT_SIZE, DEFAULT_WEIGHT};

/// Default spacing between icon and label (`spacing-sm` = 8 px). Stock
/// fallback; the active `SpacingScale::sm` rung wins when
/// [`EnzLabel::spacing_scale`](crate::label::EnzLabel::spacing_scale) is
/// forwarded.
pub const DEFAULT_SPACING: f32 = label::DEFAULT_GAP;

/// Construct a new themed icon + label row. `icon` is a bundled icon name
/// (canonical Lucide or semantic alias); use
/// [`EnzLabel::glyph`](crate::label::EnzLabel::glyph) to pass an arbitrary text
/// glyph instead.
pub fn enz_icon_label<I: Into<String>, L: Into<String>>(icon: I, label: L) -> EnzIconLabel {
    label::icon_label_preset(icon.into(), label.into())
}
