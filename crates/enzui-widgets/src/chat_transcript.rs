//! [`enz_chat_transcript`] — scrolling surface for chat bubbles.
//!
//! ## Layout tokens
//!
//! - Transcript padding (default [`SpaceToken::Lg`] = `spacing-lg`, resolves
//!   to 16 px on the stock [`SpacingScale`] ladder).
//! - Bubble spacing (default [`SpaceToken::Md`] = `spacing-md`, resolves
//!   to 12 px on the stock ladder).
//!
//! A view holding the live [`enzui_theme::EnzTheme`] forwards its scale
//! via [`EnzChatTranscript::spacing_scale`], so a rescaled `spacing-lg`
//! actually drives the transcript padding. Without the
//! forwarded scale, the tokens resolve against [`SpacingScale::default`].
//! - Background: the transcript owns a full-bounds background fill that
//!   resolves against the runtime `iced::Theme`, not delegated to a parent.
//!   Opt out with [`EnzChatTranscript::background_owned`]`(false)` when nesting
//!   the transcript inside a surface that already paints the background.
//! - Scroll position is observed through an `iced::widget::Id` + [`EnzChatTranscript::on_scroll`]
//!   [`Viewport`] callback threaded into the underlying [`enz_scroll_column`].
//!   Those are observation hooks; the transcript does not need a host to
//!   drive scroll-to-bottom (see below).
//!
//! ## The transcript opens at the NEWEST message
//!
//! A chat transcript follows the conversation by default: it opens
//! scrolled to the newest message and stays pinned as turns are appended.
//!
//! It releases that pin the moment the reader scrolls up past
//! [`RELEASE_DISTANCE`](crate::tail_follow::RELEASE_DISTANCE), and re-acquires
//! it when they scroll back to the bottom — so a message arriving while the reader
//! is in the history does not yank the view back down. The behaviour and the
//! [`Follow`](enzui_core::chat_stream::Follow) state machine behind it
//! are documented in [`crate::tail_follow`], which [`crate::chat_list`] shares.
//!
//! Opt out with [`EnzChatTranscript::follow_tail`]`(false)` for the rare
//! archival surface that should be read from the start. A host that wants raw
//! offset control should also opt out, then drive
//! `iced::widget::operation::snap_to(id, RelativeOffset::END)` against its own
//! [`Id`] as before — two owners of one offset will fight.
//!
//! ## `max-content-width`
//!
//! The bubble column applies the declared `max-content-width: 720px`; the
//! surrounding `EnzScrollColumn` may still fill 100% width. Therefore the bubble
//! column is capped at `Length::Fixed(max_content_width)` and centred with
//! flexible gutters, so an ultra-wide window still reads like a chat panel and
//! not a stretched paragraph. iced's `iced::widget::Container` has no
//! `max_width`, so the cap + centring is a `[Space | column | Space]` sandwich.
//! `max_content_width(0.0)` disables the cap and lets the column fill the
//! transcript. This deliberate divergence keeps scroll ownership unambiguous.

use crate::scroll_column::enz_scroll_column;
use crate::surface_stack::SpaceToken;
use enzui_core::metrics::FiniteNonNeg;
use enzui_theme::{Palette, SpacingScale};
use iced::widget::scrollable::Viewport;
use iced::widget::{Id, Space, container, row};
use iced::{Background, Element, Length};

/// Default outer padding token (`transcript-padding: spacing-lg`;
/// resolves to 16 px on the stock [`SpacingScale`] ladder, follows a
/// forwarded scale via [`EnzChatTranscript::spacing_scale`]).
pub const DEFAULT_TRANSCRIPT_PADDING: SpaceToken = SpaceToken::Lg;
/// Default inter-bubble spacing token (`bubble-spacing: spacing-md`;
/// resolves to 12 px on the stock ladder, follows a forwarded scale via
/// [`EnzChatTranscript::spacing_scale`]).
pub const DEFAULT_BUBBLE_SPACING: SpaceToken = SpaceToken::Md;
/// Default max content width.
pub const DEFAULT_MAX_CONTENT_WIDTH: f32 = 720.0;

/// Resolve the centre-column width from `max_content_width`. A finite,
/// positive value caps the column at `Length::Fixed`; anything else
/// (`0.0`, negative, `NaN`, `±INFINITY`) falls back to `Length::Fill`.
/// The `.is_finite()` check is essential: `INFINITY > 0.0` is `true`, so a
/// bare `> 0.0` guard would emit `Length::Fixed(Infinity)` and poison the
/// layout pass.
#[inline]
fn resolve_content_width(max_content_width: f32) -> Length {
    if max_content_width.is_finite() && max_content_width > 0.0 {
        Length::Fixed(max_content_width)
    } else {
        Length::Fill
    }
}

/// One padding/spacing dimension in the transcript: either a semantic
/// [`SpaceToken`] (resolved against a [`SpacingScale`] at build time, so it
/// follows a rescaled theme) or an explicit pixel escape hatch (wins
/// regardless of forwarded scale).
#[derive(Debug, Clone, Copy)]
enum Dim {
    Token(SpaceToken),
    Px(f32),
}

/// Builder for an EnzChatTranscript element.
#[must_use]
pub struct EnzChatTranscript<'a, Message> {
    children: Vec<Element<'a, Message>>,
    /// Storage that retains semantic identity so `.spacing_scale()` set later
    /// can still re-resolve the token against a rescaled ladder. Materialised
    /// in `into_element` via [`Self::resolve_dim`].
    transcript_padding: Dim,
    bubble_spacing: Dim,
    max_content_width: f32,
    width: Length,
    height: Length,
    background_owned: bool,
    /// Active runtime scale forwarded by the view owner. `None` falls back to
    /// [`SpacingScale::default`] — same three-tier precedence as
    /// [`crate::status_bar`].
    spacing_scale: Option<SpacingScale>,
    id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    /// open at the newest message and follow appended turns until the
    /// reader scrolls away. Default `true` — see the module header.
    follow_tail: bool,
}

/// Create an EnzChatTranscript wrapping chat-bubble children.
pub fn enz_chat_transcript<'a, Message>(
    children: Vec<Element<'a, Message>>,
) -> EnzChatTranscript<'a, Message>
where
    Message: 'a,
{
    EnzChatTranscript {
        children,
        transcript_padding: Dim::Token(DEFAULT_TRANSCRIPT_PADDING),
        bubble_spacing: Dim::Token(DEFAULT_BUBBLE_SPACING),
        max_content_width: DEFAULT_MAX_CONTENT_WIDTH,
        width: Length::Fill,
        height: Length::Fill,
        background_owned: true,
        spacing_scale: None,
        id: None,
        on_scroll: None,
        // following the tail is the default. A transcript that opens at
        // the oldest message is the one behaviour no chat surface wants.
        follow_tail: true,
    }
}

impl<'a, Message> EnzChatTranscript<'a, Message>
where
    Message: 'a,
{
    /// Outer transcript padding from a semantic [`SpaceToken`]
    /// (`transcript-padding: spacing-lg`, default [`SpaceToken::Lg`]).
    /// Under [`Self::spacing_scale`] this resolves against the forwarded
    /// ladder; without it, against [`SpacingScale::default`] (16 px).
    pub fn transcript_padding_token(mut self, token: SpaceToken) -> Self {
        self.transcript_padding = Dim::Token(token);
        self
    }

    /// Raw-pixel escape hatch for [`Self::transcript_padding_token`].
    ///
    /// Non-expressible input (`NaN`, `±INFINITY`, negative) falls back to the
    /// token default ([`DEFAULT_TRANSCRIPT_PADDING`] resolved against
    /// [`SpacingScale::default`] = 16 px); `0.0` is expressible intent ("no
    /// padding") and is honoured literally. The former `px.max(0.0)` let
    /// `+inf` through untouched and silently mapped `NaN` to `0.0` (unlike
    /// AGENTS "valid extreme values keep degrading into their opposite").
    ///
    /// Precedence: this explicit pin wins over a forwarded
    /// [`Self::spacing_scale`] for this dimension.
    pub fn transcript_padding(mut self, px: f32) -> Self {
        self.transcript_padding =
            Dim::Px(FiniteNonNeg::new_or(px, DEFAULT_TRANSCRIPT_PADDING.px()).get());
        self
    }

    /// Inter-bubble spacing from a semantic [`SpaceToken`]
    /// (`bubble-spacing: spacing-md`, default [`SpaceToken::Md`]).
    /// Under [`Self::spacing_scale`] this resolves against the forwarded
    /// ladder; without it, against [`SpacingScale::default`] (12 px).
    pub fn bubble_spacing_token(mut self, token: SpaceToken) -> Self {
        self.bubble_spacing = Dim::Token(token);
        self
    }

    /// Raw-pixel escape hatch for [`Self::bubble_spacing_token`].
    ///
    /// Non-expressible input (`NaN`, `±INFINITY`, negative) falls back to the
    /// token default ([`DEFAULT_BUBBLE_SPACING`] resolved against
    /// [`SpacingScale::default`] = 12 px); `0.0` is expressible intent ("no
    /// gap") and is honoured literally. The former `px.max(0.0)` let `+inf`
    /// through and mapped `NaN` to `0.0`.
    ///
    /// Precedence: this explicit pin wins over a forwarded
    /// [`Self::spacing_scale`] for this dimension.
    pub fn bubble_spacing(mut self, px: f32) -> Self {
        self.bubble_spacing = Dim::Px(FiniteNonNeg::new_or(px, DEFAULT_BUBBLE_SPACING.px()).get());
        self
    }

    /// Forward the active runtime [`SpacingScale`] so `transcript-padding` and
    /// `bubble-spacing` semantic tokens resolve against a rescaled theme's
    /// ladder rather than the stock one. Matches the
    /// pattern [`crate::status_bar::EnzStatusBar::spacing_scale`] uses.
    ///
    /// Precedence: an explicit `_px` setter on a given dimension wins over
    /// this forwarded scale, which wins over the stock [`SpacingScale::default`].
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Resolve a stored [`Dim`] against the effective scale (forwarded or
    /// stock). NaN / ±inf / negative from an escape hatch already sanitised
    /// at set-time, so a token round trip must not re-introduce them.
    fn resolve_dim(dim: Dim, scale: &SpacingScale) -> f32 {
        let v = match dim {
            Dim::Token(t) => t.resolve(scale),
            Dim::Px(p) => p,
        };
        if v.is_finite() && v >= 0.0 { v } else { 0.0 }
    }

    /// Cap the inner bubble column at `px` and centre it horizontally
    /// (`max-content-width`, default 720 px). Set to `0.0` to
    /// disable the cap and let the column fill the transcript width.
    ///
    /// Non-expressible input (`NaN`, `±INFINITY`, negative) falls back to the
    /// default ([`DEFAULT_MAX_CONTENT_WIDTH`] = 720 px); `0.0` is expressible
    /// intent ("disable the cap") and is honoured literally. The former
    /// `px.max(0.0)` let `+inf` through into the `Length` cap and mapped `NaN`
    /// to `0.0`; all three setters share the same finite-positive policy.
    pub fn max_content_width(mut self, px: f32) -> Self {
        self.max_content_width = FiniteNonNeg::new_or(px, DEFAULT_MAX_CONTENT_WIDTH).get();
        self
    }

    /// Whether the transcript paints its own full-bounds theme background
    /// fill (default `true`). Pass `false` when
    /// the transcript is nested inside a surface that already owns the
    /// background, to avoid double-painting.
    pub fn background_owned(mut self, owned: bool) -> Self {
        self.background_owned = owned;
        self
    }

    /// Attach a stable [`Id`] to the underlying scroll region so a host can
    /// drive scroll-to-bottom / restore offset via
    /// `iced::widget::operation::snap_to(id, …)` — the iced-native equivalent
    /// of the two-way `scroll-y` bind.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Observe scroll-offset changes (delivers the [`Viewport`]), e.g. to
    /// drive a "jump to latest" affordance or persist the reader's position.
    ///
    /// This is not needed to pin the transcript to the newest
    /// message — it does that itself. See [`Self::follow_tail`].
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Whether the transcript opens at the newest message and follows
    /// appended turns (default `true`).
    ///
    /// While following, content appended after first layout keeps the newest
    /// message on screen. The pin is released as soon as the reader scrolls up
    /// past [`RELEASE_DISTANCE`](crate::tail_follow::RELEASE_DISTANCE) and is
    /// re-acquired when they scroll back down, so an arriving message never
    /// yanks a reader out of history.
    ///
    /// Pass `false` for an archival transcript meant to be read from the
    /// start, or when the host drives the offset itself via
    /// [`Self::id`] + `snap_to` (two owners of one offset will fight).
    pub fn follow_tail(mut self, follow: bool) -> Self {
        self.follow_tail = follow;
        self
    }

    /// Override the outer width (default `Length::Fill`).
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Override the outer height (default `Length::Fill`).
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Build the element.
    pub fn into_element(self) -> Element<'a, Message> {
        // Resolution invariant: resolve semantic tokens against the FORWARDED scale
        // at build time, then hand pre-resolved pixel values to the scroll
        // column. Pinned by `forwarded_spacing_scale_moves_padding_and_spacing`.
        let default = SpacingScale::default();
        let scale = self.spacing_scale.as_ref().unwrap_or(&default);
        let transcript_padding = Self::resolve_dim(self.transcript_padding, scale);
        let bubble_spacing = Self::resolve_dim(self.bubble_spacing, scale);

        // The scroll column carries the padding and spacing, plus the stable
        // Id and viewport observation threaded from this builder.
        let mut scroll = enz_scroll_column(self.children)
            .spacing(bubble_spacing)
            .inset(transcript_padding)
            // Transcript padding is semantic content padding, not scrollbar
            // clearance, so a supported 0/small padding must not place
            // right-aligned user bubbles beneath the floating overlay
            // scrollbar.
            //
            // The gutter is opt-in because enabling it for every scroll region
            // would make unrelated layouts asymmetric.
            // ChatTranscript is one of the two places that genuinely needs it:
            // a right-aligned user bubble is laid flush to the trailing edge,
            // so at `transcript_padding(0)` it would sit under the thumb.
            // Composition is `max`, so a transcript with real padding
            // (the 16 px default) is unaffected and stays symmetric — only the
            // small-padding case is lifted to the 8 px clearance.
            .scrollbar_gutter(true);
        if let Some(id) = self.id {
            scroll = scroll.id(id);
        }
        if let Some(on_scroll) = self.on_scroll {
            scroll = scroll.on_scroll(on_scroll);
        }

        // Cap + centre the bubble column within max_content_width using
        // flexible spacers on both sides (iced Container has no max-width).
        // `resolve_content_width` guards against Length::Fixed(Infinity)
        // poisoning the layout pass (the same invariant as EnzCard).
        // wrap the scroll region so it opens at the newest message and
        // follows appended turns until the reader scrolls away. Applied to the
        // scroll column itself (not the outer container) so the wrapper's
        // `Operation` probe finds this transcript's own `Scrollable` first.
        let wrap = |el: Element<'a, Message>| -> Element<'a, Message> {
            if self.follow_tail {
                crate::tail_follow::enz_tail_follow(el)
            } else {
                el
            }
        };

        let inner: Element<'a, Message> = match resolve_content_width(self.max_content_width) {
            Length::Fill => wrap(scroll.width(Length::Fill).into()),
            capped => {
                let column_element: Element<'a, Message> = wrap(scroll.width(capped).into());
                row![
                    Space::new().width(Length::Fill).height(Length::Fill),
                    column_element,
                    Space::new().width(Length::Fill).height(Length::Fill),
                ]
                .into()
            }
        };

        let mut surface = container(inner).width(self.width).height(self.height);

        // Own the runtime-themed background fill unless the caller opted out
        // for nesting.
        if self.background_owned {
            surface = surface.style(|theme: &iced::Theme| {
                let palette = Palette::from_iced(theme);
                container::Style {
                    background: Some(Background::Color(palette.background)),
                    ..container::Style::default()
                }
            });
        }

        surface.into()
    }
}

impl<'a, Message> From<EnzChatTranscript<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzChatTranscript<'a, Message>) -> Self {
        w.into_element()
    }
}
