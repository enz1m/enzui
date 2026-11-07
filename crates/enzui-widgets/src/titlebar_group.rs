//! EnzTitlebarGroup: basic horizontal cluster of titlebar widgets.
//!
//! A back-compat alias for a plain horizontal titlebar cluster (icons / chips /
//! badges): the transparent, borderless, unpadded `Ghost` variant of
//! `EnzTitlebarIconGroup`.
//!
//! # Thin delegation to the canonical Ghost path
//!
//! This name is kept for source compatibility, but it is not a second
//! `Row`/`container` implementation — it configures and delegates to
//! [`crate::titlebar_icon_group::enz_titlebar_icon_group`] with the
//! [`Ghost`](crate::titlebar_icon_group::TitlebarIconGroupVariant::Ghost)
//! variant. A single code path prevents the default height and clipping policy
//! from drifting between the two names.
//!
//! Both the height default (32 px =
//! `titlebar-control-height = max(button-height − 4, field-md-height) =
//! max(28, 32)`) and the `clip: true` therefore come from
//! the shared Ghost implementation.

use crate::titlebar_icon_group::{self, TitlebarIconGroupVariant, enz_titlebar_icon_group};
use iced::Element;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default group height in pixels — sourced from the canonical constant in
/// [`crate::titlebar_icon_group::DEFAULT_GROUP_HEIGHT`] so the alias and
/// canonical path always agree.
pub const DEFAULT_GROUP_HEIGHT: f32 = titlebar_icon_group::DEFAULT_GROUP_HEIGHT;

/// Default inter-widget spacing (`spacing-sm`, `8.0` px).
pub const DEFAULT_GROUP_SPACING: f32 = 8.0;

/// Sanitise an f32 dimension to finite, non-negative (non-finite → 0).
///
/// Used for spacing (where zero is meaningful intent). For height, use
/// [`enzui_core::metrics::FinitePositive`] instead — zero height is
/// not expressible intent and must fall back to the default.
fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_titlebar_group(children)`.
pub fn enz_titlebar_group<'a, Message>(
    children: Vec<Element<'a, Message>>,
) -> EnzTitlebarGroup<'a, Message>
where
    Message: 'a,
{
    EnzTitlebarGroup {
        children,
        group_height: DEFAULT_GROUP_HEIGHT,
        group_spacing: DEFAULT_GROUP_SPACING,
    }
}

/// Builder returned by [`enz_titlebar_group`]. A thin compatibility façade over
/// the Ghost variant of [`crate::titlebar_icon_group::EnzTitlebarIconGroup`].
#[must_use]
pub struct EnzTitlebarGroup<'a, Message> {
    children: Vec<Element<'a, Message>>,
    group_height: f32,
    group_spacing: f32,
}

impl<'a, Message> EnzTitlebarGroup<'a, Message>
where
    Message: 'a,
{
    /// Set the group height in pixels (default [`DEFAULT_GROUP_HEIGHT`]).
    ///
    /// Non-finite, negative, and zero values are not expressible intent for a
    /// height and fall back to `DEFAULT_GROUP_HEIGHT` (32). This matches the
    /// canonical [`crate::titlebar_icon_group::EnzTitlebarIconGroup::group_height`]
    /// policy.
    pub fn group_height(mut self, h: f32) -> Self {
        self.group_height =
            enzui_core::metrics::FinitePositive::new_or(h, DEFAULT_GROUP_HEIGHT).get();
        self
    }

    /// Set the inter-widget spacing in pixels (default `8.0`).
    /// Non-finite/negative → 0.
    pub fn group_spacing(mut self, s: f32) -> Self {
        self.group_spacing = finite_nonneg(s);
        self
    }
}

impl<'a, Message> EnzTitlebarGroup<'a, Message>
where
    Message: 'a,
{
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

impl<'a, Message> From<EnzTitlebarGroup<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzTitlebarGroup<'a, Message>) -> Self {
        // Configure the canonical Ghost path — one implementation, so height
        // default, cross-axis centring, and clipping all come from there.
        enz_titlebar_icon_group(w.children)
            .variant(TitlebarIconGroupVariant::Ghost)
            .group_height(w.group_height)
            .group_spacing(w.group_spacing)
            .into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
