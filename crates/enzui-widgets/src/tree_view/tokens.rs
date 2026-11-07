//! `tree_view`: tokens.
//!
//! Owns the token model for the tree-view implementation.

use enzui_theme::Palette;
use iced::Color;

/// The opaque backdrops actually visible behind a settled tree row.
///
/// A tree selection uses [`Palette::accent_soft`], not the generic
/// [`Palette::surface_selected`]. Callers that put their own content in a row
/// can read these fields and resolve ink against the same backdrop the tree
/// paints instead of guessing from a similarly named palette role.
///
/// Obtain this from [`EnzTreeView::resolved_row_fills`](super::EnzTreeView::resolved_row_fills)
/// so per-instance `surface_background`, `item_background`, and
/// `selected_background` overrides are included. [`Self::from_palette`] is the
/// default-token form when no view exists yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeViewRowFills {
    /// Backdrop visible behind a settled, selected row.
    pub selected: Color,
    /// Backdrop visible behind a settled, unselected row.
    pub unselected: Color,
}

impl TreeViewRowFills {
    /// Resolve the default tree-row backdrops from `palette`.
    pub fn from_palette(palette: &Palette) -> Self {
        Self::resolved(palette, None, None, None)
    }

    pub(crate) fn resolved(
        palette: &Palette,
        surface: Option<Color>,
        item: Option<Color>,
        selected: Option<Color>,
    ) -> Self {
        let surface = surface.unwrap_or(palette.surface);
        Self {
            selected: crate::surface_interaction::composite(
                surface,
                selected.unwrap_or(palette.accent_soft),
            ),
            unselected: crate::surface_interaction::composite(
                surface,
                item.unwrap_or(Color::TRANSPARENT),
            ),
        }
    }

    /// Pick the visible backdrop for one row state.
    pub fn for_selected(self, selected: bool) -> Color {
        if selected {
            self.selected
        } else {
            self.unselected
        }
    }
}

/// Default row height. `list-row-height`, shared with
/// [`ListRowSurfaceTokens`](crate::ListRowSurfaceTokens) — a tree row and a list row are the same
/// affordance and must not be two different heights.
pub const ROW_HEIGHT: f32 = 34.0;

/// The smallest row surface a
/// [`tree_row_gap`](crate::tree_view::EnzTreeView::tree_row_gap) may leave
/// behind, in px.
///
/// A gap is taken out of the row's own pitch, so an unclamped one would
/// shrink the surface to nothing and leave a rail that paints no row and
/// catches no pointer. 24 px is the platform minimum pointer target the
/// framework holds elsewhere, so a clamped gap still leaves something a
/// user can hit rather than something merely visible.
pub const MIN_ROW_SURFACE: f32 = 24.0;

/// Default indentation added per level.
pub const INDENT_WIDTH: f32 = 18.0;

/// Default chevron slot width.
pub const DISCLOSURE_SIZE: f32 = 16.0;

/// Default horizontal padding at both ends of a row.
pub const ROW_PADDING_X: f32 = 8.0;

/// Default gap between the chevron slot and the label.
pub const LABEL_GAP: f32 = 4.0;

/// Default label size (`text-size-md`).
pub const BODY_SIZE: f32 = 13.0;

/// Default panel corner radius.
pub const PANEL_RADIUS: f32 = 8.0;

/// Default panel border width.
pub const PANEL_BORDER_WIDTH: f32 = 1.0;

/// Default overscan for the virtualized path: extra rows realized on
/// each side of the visible range. Matches `shortcut_list.rs`.
pub const DEFAULT_OVERSCAN: usize = 5;
