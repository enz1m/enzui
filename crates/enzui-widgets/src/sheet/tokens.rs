//! Tokens the sheet paints with: radii, padding, handle geometry, and
//! the palette-resolved `SheetTokens` colours.

use super::*;

/// Corner radius of a sheet surface: the theme's `dialog` radius, which
/// follows the window radius by default.
#[must_use]
pub fn sheet_radius() -> f32 {
    enzui_theme::text_defaults::shape_radius(|s| s.dialog)
}

/// Inner padding of a sheet surface.
pub const SHEET_PADDING: f32 = 24.0;

/// Width of the drag handle pill.
pub const HANDLE_WIDTH: f32 = 36.0;

/// Height of the drag handle pill.
pub const HANDLE_HEIGHT: f32 = 5.0;

/// Corner radius of the drag handle pill.
pub const HANDLE_RADIUS: f32 = 3.0;

/// Colour tokens for a sheet surface.
#[derive(Debug, Clone, Copy)]
pub struct SheetTokens {
    /// The surface fill. Flat and opaque.
    pub surface: Color,
    /// The hairline around the surface.
    pub border: Color,
    /// The drag handle pill.
    pub handle: Color,
}

impl SheetTokens {
    /// Tokens resolved from a runtime palette.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            surface: p.surface_elevated,
            // A shade of the sheet's own surface.
            border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            // `border_strong` at 1.78:1 on `surface_elevated` in both themes
            // falls below the WCAG 1.4.11 3:1 floor for non-text UI
            // components. `text_muted` clears it (dark 5.58:1, light 5.04:1)
            // and reads as a subdued-but-visible grip rather than a near-
            // invisible hairline.
            handle: p.text_muted,
        }
    }
}

/// Whether a sheet is attached to an edge or floating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SheetShape {
    /// Attached to the bottom edge: only the top corners are rounded.
    #[default]
    EdgeAttached,
    /// A centred, fully rounded surface.
    Floating,
}

/// The semantics a sheet publishes.
///
/// A sheet is a dialog: it is a modal surface with a name, and AT should
/// announce it as one. The detent is *not* published as a value — a sheet at
/// "medium" is not a slider at 50%, and announcing a geometry choice as a
/// value would be noise to a screen-reader user.
pub fn sheet_semantics(label: Option<&str>, modal: bool) -> Semantics {
    let mut semantics = Semantics::new(Role::Dialog).modal(modal);
    if let Some(label) = label {
        semantics = semantics.label(label);
    }
    semantics
}

/// The tokens a sheet paints with: the caller's pin, or the runtime theme.
///
/// One function so the handle and the surface cannot resolve differently —
/// two places deciding the same thing is a mutation nothing can catch.
pub(crate) fn resolve_tokens(pinned: Option<SheetTokens>, theme: &iced::Theme) -> SheetTokens {
    crate::theme_lookup::resolve_pinned(
        crate::theme_lookup::PIN_SITE_SHEET,
        pinned,
        theme,
        SheetTokens::from_palette,
    )
}
