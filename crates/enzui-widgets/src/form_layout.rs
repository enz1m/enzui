//! EnzFormLayout: label-aligned form container for settings and data entry.
//!
//! A vertical stack of form rows (typically `EnzLabeledContent`) with uniform
//! inter-row spacing and a form-wide inset: a thin specialisation of the
//! [`EnzVStack`](crate::stack::EnzVStack): **`spacing-sm` row gap,
//! `spacing-md` inset, cross-axis `Stretch`** so every row fills the form
//! width. It composes that stack rather than re-implementing a bare `Column`.
//!
//! Callers control label/control column alignment per-row on each
//! `enz_labeled_content` (design: label width is per-row,
//! not cascaded from the form).
//!
//! # Spacing tokens
//!
//! Defaults come from the shared [`SpaceToken`] vocabulary, not local
//! literals:
//!
//! * row gap = [`SpaceToken::Sm`] (`spacing-sm`, 8 px on the stock ladder),
//! * inset = [`SpaceToken::Md`] (`spacing-md`, 12 px on the stock ladder).
//!
//! The fluent `row_spacing(f32)` / `form_padding(f32)` setters remain as a
//! finite custom escape hatch — they map to [`SpaceToken::Custom`] and
//! normalise negative / non-finite values to `0`.
//!
//! ## Theme reactivity
//!
//! The tokens ultimately feed `Row::spacing` / `Container::padding` inside
//! [`crate::stack::EnzVStack`], both consumed in `layout()` where iced
//! supplies no `&Theme`. So the ladder cannot be resolved *inside* layout;
//! the form threads a scale through at build time via
//! [`EnzFormLayout::spacing_scale`], following the same three-tier precedence
//! [`crate::status_bar::EnzStatusBar`] uses:
//!
//! 1. an explicit `row_spacing(px)` / `form_padding(px)` value wins for that
//!    dimension,
//! 2. otherwise a forwarded [`SpacingScale`] resolves each token,
//! 3. otherwise the stock [`SpacingScale::default`] ladder resolves each
//!    token (via `SpaceToken::px()`).

use crate::stack::{CrossAlign, enz_vstack};
use crate::surface_stack::SpaceToken;
use enzui_theme::SpacingScale;
use iced::{Element, Length};

// ---------------------------------------------------------------------------
// Defaults (semantic tokens)
// ---------------------------------------------------------------------------

/// Default row gap token — `spacing-sm` (8 px).
pub const DEFAULT_ROW_GAP: SpaceToken = SpaceToken::Sm;

/// Default form inset token — `spacing-md` (12 px).
pub const DEFAULT_FORM_INSET: SpaceToken = SpaceToken::Md;

/// Default row gap in pixels (`spacing-sm` resolved against the default
/// ladder = 8).
pub const DEFAULT_ROW_SPACING: f32 = 8.0;

/// Default form padding in pixels (`spacing-md` resolved against the default
/// ladder = 12).
pub const DEFAULT_FORM_PADDING: f32 = 12.0;

/// Normalise a caller-supplied pixel escape-hatch value to a finite,
/// non-negative [`SpaceToken::Custom`].
fn custom_token(px: f32) -> SpaceToken {
    // `Custom` stores f32 so finite fractional logical pixels survive exactly.
    // NaN/±inf/negative are not expressible and degrade to 0; literal 0 is
    // expressible intent and remains 0.
    SpaceToken::Custom(enzui_core::metrics::FiniteNonNeg::new_or(px, 0.0).get())
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_form_layout(rows)`.
pub fn enz_form_layout<'a, Message>(rows: Vec<Element<'a, Message>>) -> EnzFormLayout<'a, Message>
where
    Message: 'a,
{
    EnzFormLayout {
        rows,
        row_gap: DEFAULT_ROW_GAP,
        inset: DEFAULT_FORM_INSET,
        width: Length::Fill,
        spacing_scale: None,
    }
}

/// Builder returned by [`enz_form_layout`].
#[must_use]
pub struct EnzFormLayout<'a, Message> {
    rows: Vec<Element<'a, Message>>,
    row_gap: SpaceToken,
    inset: SpaceToken,
    width: Length,
    /// Active runtime spacing scale forwarded by the view owner. `None`
    /// falls back to [`SpacingScale::default`] — same three-tier precedence
    /// as [`crate::status_bar`].
    spacing_scale: Option<SpacingScale>,
}

impl<'a, Message> EnzFormLayout<'a, Message>
where
    Message: 'a,
{
    /// Set the vertical spacing between rows using a semantic
    /// [`SpaceToken`] role (default [`SpaceToken::Sm`]).
    pub fn row_gap(mut self, token: SpaceToken) -> Self {
        self.row_gap = token;
        self
    }

    /// Set the form inset using a semantic [`SpaceToken`] role (default
    /// [`SpaceToken::Md`]).
    pub fn inset(mut self, token: SpaceToken) -> Self {
        self.inset = token;
        self
    }

    /// Custom pixel escape hatch for the row gap. Negative / non-finite
    /// values normalise to `0`. Prefer [`Self::row_gap`] with a token role.
    pub fn row_spacing(mut self, s: f32) -> Self {
        self.row_gap = custom_token(s);
        self
    }

    /// Custom pixel escape hatch for the form inset. Negative / non-finite
    /// values normalise to `0`. Prefer [`Self::inset`] with a token role.
    pub fn form_padding(mut self, p: f32) -> Self {
        self.inset = custom_token(p);
        self
    }

    /// Override the outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Forward the active runtime [`SpacingScale`] so `row_gap` and `inset`
    /// tokens resolve against a rescaled theme's ladder rather than the
    /// stock one. Matches the pattern
    /// [`crate::status_bar::EnzStatusBar::spacing_scale`] uses.
    ///
    /// Precedence: an explicit `row_spacing(px)` / `form_padding(px)` value
    /// wins for that dimension (via [`SpaceToken::Custom`]), the forwarded
    /// scale wins over the stock ladder, and the stock ladder is the final
    /// fallback.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
}

impl<'a, Message: 'a> EnzFormLayout<'a, Message> {
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzFormLayout<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzFormLayout<'a, Message>) -> Self {
        // Resolve semantic tokens against the forwarded scale
        // at build time, then hand pre-resolved pixel values to EnzVStack via
        // `SpaceToken::Custom(px)` -- EnzVStack itself only resolves tokens
        // via `SpaceToken::px()` (stock ladder), so a themed rung would be
        // dropped there. `Custom(px)` short-circuits that lookup and carries
        // the resolved value straight through to `Row::spacing` /
        // `Container::padding`.
        let default = SpacingScale::default();
        let scale = w.spacing_scale.as_ref().unwrap_or(&default);
        let row_gap = resolve_to_custom(w.row_gap, scale);
        let inset = resolve_to_custom(w.inset, scale);

        // Thin EnzVStack specialisation: Sm gap, Md inset, cross Stretch so
        // rows fill the form width; natural height (fill defaults false).
        let stack: Element<'a, Message> = enz_vstack(w.rows)
            .gap(row_gap)
            .inset(inset)
            .cross(CrossAlign::Stretch)
            .into();
        // The VStack's own Stretch already fills width; wrap only to honour a
        // caller width override (e.g. a fixed-width form panel).
        match w.width {
            Length::Fill => stack,
            other => iced::widget::container(stack).width(other).into(),
        }
    }
}

/// Resolve a [`SpaceToken`] against `scale` and repackage the result as a
/// [`SpaceToken::Custom`] so it bypasses the downstream `.px()` lookup (which
/// would re-resolve against `SpacingScale::default()`, losing the forwarded
/// scale). Fractional theme rungs survive exactly; non-finite / negative
/// values clamp to `0.0`.
fn resolve_to_custom(token: SpaceToken, scale: &SpacingScale) -> SpaceToken {
    let px = token.resolve(scale);
    SpaceToken::Custom(enzui_core::metrics::FiniteNonNeg::new_or(px, 0.0).get())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
