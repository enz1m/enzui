//! EnzPane: content-clipping pane.
//!
//! A pane that owns its assigned size, clips overshoot, and never leaks its
//! content's preferred width back up.
//!
//! # Scope
//!
//! - Builder `enz_pane(content)`.
//! - `.fill_height(bool)` — vertical `Length::Fill` (default `true`) vs.
//!   `Shrink`. Width is always `Length::Fill` — the pane's defining
//!   invariant is that it owns its horizontal allocation.
//! - `.inset(SpaceToken)` — uniform padding.
//! - `.clip(bool)` — clip overflow (default `true`).
//! - `.spacing_scale(SpacingScale)` — forward a custom density so the inset
//!   resolves against it instead of the stock ladder.
//!
//! # Deferred
//!
//! - Per-axis / per-side `inset-*` overrides (uniform only).

use crate::surface_stack::SpaceToken;
use iced::widget::container;
use iced::{Element, Length, Padding};

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for a simple content pane (fill / inset / clip wrapper). See
/// [`enz_pane`].
#[must_use]
pub struct EnzPane<'a, Message> {
    content: Element<'a, Message>,
    fill_height: bool,
    inset: SpaceToken,
    clip: bool,
    /// Explicit spacing-scale override for the layout-time inset padding.
    /// `None` resolves the stock ladder.
    spacing_scale: Option<enzui_theme::SpacingScale>,
}

/// Construct a pane wrapping `content`.
pub fn enz_pane<'a, Message: 'a>(content: Element<'a, Message>) -> EnzPane<'a, Message> {
    EnzPane {
        content,
        fill_height: true,
        inset: SpaceToken::None,
        clip: true,
        spacing_scale: None,
    }
}

impl<'a, Message: 'a> EnzPane<'a, Message> {
    /// Fill the available height instead of hugging content. Defaults to
    /// `true`. Width is always `Length::Fill` — the pane's defining
    /// invariant is that it owns its horizontal allocation.
    pub fn fill_height(mut self, f: bool) -> Self {
        self.fill_height = f;
        self
    }

    /// Uniform inner padding. Defaults to [`SpaceToken::None`].
    pub fn inset(mut self, i: SpaceToken) -> Self {
        self.inset = i;
        self
    }
    /// Clip content to the pane bounds. Defaults to `true`.
    pub fn clip(mut self, c: bool) -> Self {
        self.clip = c;
        self
    }

    /// Forward a custom [`SpacingScale`](enzui_theme::SpacingScale) so the
    /// inset padding resolves against it instead of the stock ladder.
    ///
    /// `Widget::layout` has no `&Theme` parameter — the inset is consumed as
    /// `Container::padding`, which iced resolves during layout and never hands
    /// a theme — so a custom density must be threaded in explicitly at build
    /// time. Without this setter the pane uses the compile-time default ladder.
    /// Pass `theme.spacing` from the app's `EnzTheme` to make the inset follow a
    /// custom density.
    pub fn spacing_scale(mut self, spacing: enzui_theme::SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let height = if self.fill_height {
            Length::Fill
        } else {
            Length::Shrink
        };
        // Layout-time resolution: the forwarded ladder if the caller supplied
        // one, else the stock ladder (`SpaceToken::px()`'s behaviour). A `0`
        // inset stays 0 under any ladder — `space0` is 0 in every scale — so
        // "no inset" is honoured literally rather than re-derived.
        let inset_px = self
            .spacing_scale
            .map_or_else(|| self.inset.px(), |s| self.inset.resolve(&s));
        let c = container(self.content)
            .padding(Padding::from(inset_px))
            .width(Length::Fill)
            .height(height);
        // Native Container clipping only affects paint. EnzPane's clip is a
        // content boundary, so pointer hit-testing, focus traversal, and AT
        // collection must use the same rectangle. Intentional overlays remain
        // forwarded by InertClip and may escape the pane.
        crate::inert_clip::InertClip::new(c, self.clip).into()
    }
}

impl<'a, Message: 'a> From<EnzPane<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzPane<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
