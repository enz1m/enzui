//! EnzScrollCard: flat-by-default scrolling surface with an opt-in border.
//!
//! A thin wrapper over `enz_scroll_column` that supplies the border,
//! surface tone, and corner radius so a dialog / settings pane can hand
//! the surface responsibility to one primitive instead of every screen
//! re-styling the outer container.
//!
//! # Tones
//!
//! - `Sunken` — `surface_alt`, the dialog background.
//! - `Surface` (default) — plain `surface`.
//! - `Flat` — transparent fill, border only (rely on parent bg).
//!
//! # Rounded corners
//!
//! The card paints a rounded chrome quad, but `Container::clip(true)` is a
//! rectangular scissor, so an opaque, zero-inset child (a full-bleed row
//! with its own background) would otherwise paint straight through the corner
//! arc. [`enz_scroll_column`] insets nothing on the top/left/bottom by default,
//! so full-bleed content requires the rounded clip.
//!
//! The card wraps its scrolling child in the shared
//! [`crate::rounded_clip::RoundedClip`] primitive rather than growing
//! its own geometry — the same choice `EnzSurface`, `EnzCard` and `EnzTreeView`
//! share — so the child is masked to the card's arc. *Requiring* a non-zero
//! inset would delete the legitimate full-bleed layout, and rounding each
//! child's own surface cannot be guaranteed by the caller. A nested plain
//! clip inherits the enclosing rounded clip by the renderer, which is what
//! makes the mask reach through the scrollable.
//!
//! A separate fill-loop safeguard is unnecessary because iced's layout model
//! does not have the bare-`Rectangle` fill-binding loop.

use crate::scroll_column::enz_scroll_column;
use crate::theme_lookup;
use enzui_theme::Palette;
use iced::widget::Id;
use iced::widget::container;
use iced::widget::scrollable::Viewport;
use iced::{Border, Color, Element, Length};

/// Surface tone of a scroll card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollCardTone {
    /// `surface_alt` fill — an explicitly sunken dialog background.
    Sunken,
    /// Plain `surface` fill (the flat default).
    #[default]
    Surface,
    /// Transparent fill (border only; inherits the parent background).
    Flat,
}

/// Corner-radius preset of a scroll card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollCardRadius {
    /// 4px radius.
    Sm,
    /// 8px radius.
    Md,
    /// 12px radius (the default).
    #[default]
    Lg,
}

impl ScrollCardRadius {
    fn value(self) -> f32 {
        match self {
            ScrollCardRadius::Sm => 4.0,
            ScrollCardRadius::Md => 8.0,
            ScrollCardRadius::Lg => 12.0,
        }
    }
}

/// Builder for a scrolling surface with optional card chrome. See [`enz_scroll_card`].
#[must_use]
pub struct EnzScrollCard<'a, Message> {
    children: Vec<Element<'a, Message>>,
    tone: ScrollCardTone,
    bordered: bool,
    radius: ScrollCardRadius,
    gap: f32,
    inset: f32,
    max_content_height: Option<f32>,
    id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
}

/// Construct a scroll card wrapping `children`. Defaults: [`ScrollCardTone::Surface`],
/// borderless, [`ScrollCardRadius::Lg`], 8px gap, no inset, no height cap.
pub fn enz_scroll_card<'a, Message: 'a>(
    children: Vec<Element<'a, Message>>,
) -> EnzScrollCard<'a, Message> {
    EnzScrollCard {
        children,
        tone: ScrollCardTone::default(),
        bordered: false,
        radius: ScrollCardRadius::default(),
        gap: 8.0, // "sm"
        inset: 0.0,
        max_content_height: None,
        id: None,
        on_scroll: None,
    }
}

impl<'a, Message: 'a> EnzScrollCard<'a, Message> {
    /// Set the surface tone. Defaults to [`ScrollCardTone::Surface`].
    pub fn tone(mut self, t: ScrollCardTone) -> Self {
        self.tone = t;
        self
    }
    /// Draw the 1px border. Defaults to `false`.
    pub fn bordered(mut self, b: bool) -> Self {
        self.bordered = b;
        self
    }
    /// Set the corner-radius preset. Defaults to [`ScrollCardRadius::Lg`].
    pub fn radius_size(mut self, r: ScrollCardRadius) -> Self {
        self.radius = r;
        self
    }
    /// Vertical gap between children in px (default 8).
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = g;
        self
    }
    /// Inner padding around the content in px. Non-finite or negative input
    /// falls back to 0. Defaults to 0.
    pub fn inset(mut self, i: f32) -> Self {
        self.inset = enzui_core::metrics::FiniteNonNeg::new_or(i, 0.0).get();
        self
    }
    /// Cap the scrollable content height in px; taller content scrolls. Unset
    /// by default (grows to fit).
    ///
    /// An invalid cap — non-finite (`NaN`, `±INFINITY`) or `<= 0` — is
    /// normalised to `None` at the setter and is indistinguishable from an
    /// uncapped card at every observation point. This matches the
    /// `EnzScrollColumn` discipline (its uncapped-max-height handling) and the
    /// `MaxHeight` wrapper (in the `max_height` module) so all three agree on what
    /// "has a cap" means. The alternative — storing the raw request and
    /// deferring sanitation to build time — lets the outer chrome and the
    /// inner column disagree, which collapses an empty invalid-capped card to
    /// zero outer height (chrome sees `Some(_)` and picks `Shrink`; inner
    /// column sanitises to uncapped and produces zero content height).
    pub fn max_content_height(mut self, h: f32) -> Self {
        self.max_content_height = sanitize_cap(h);
        self
    }

    /// Attach a stable [`Id`] so the inner scroll region can be targeted by
    /// `scroll_to` / `scroll_by` / `snap_to` operations and — critically —
    /// by [`enzui_core::focus::reveal_focused`], which scrolls a focused
    /// descendant into view.
    ///
    /// Without an `Id` neither programmatic scrolling nor focus-reveal is
    /// possible; that is the same constraint every other `iced` scroll
    /// consumer lives under.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Observe scroll offset changes (delivers the [`Viewport`] on every
    /// scroll). Forwarded to the inner
    /// [`EnzScrollColumn`](crate::scroll_column::EnzScrollColumn), which
    /// routes it to the shared
    /// [`EnzScrollView`](crate::scroll_view::EnzScrollView).
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    fn tone_fill(tone: ScrollCardTone, p: &Palette) -> Color {
        match tone {
            ScrollCardTone::Sunken => p.surface_alt,
            ScrollCardTone::Surface => p.surface,
            ScrollCardTone::Flat => Color::TRANSPARENT,
        }
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // Fill + border colours resolve from the runtime theme so the
        // card follows a light theme instead of being locked to Palette::dark().
        let tone = self.tone;
        let bordered = self.bordered;
        // Border visibility is paint-only: retain the one-pixel geometry so
        // the flat default cannot resize the scrolling region.
        let border_width = 1.0;
        let radius = self.radius.value();

        // The field is already sanitised at the setter (see
        // `max_content_height`) so both consumers below -- the inner scroll
        // column and the outer chrome's Shrink/Fill switch -- agree on what
        // "has a cap" means. Without setter-side sanitation the two would
        // diverge for invalid inputs (0/negative/NaN/±INF), which is exactly
        // the shared sanitation prevents.
        let cap: Option<f32> = self.max_content_height;

        let mut sc = enz_scroll_column(self.children).spacing(self.gap);
        if self.inset > 0.0 {
            sc = sc.inset(self.inset);
        }
        if let Some(h) = cap {
            sc = sc.max_content_height(h);
        }
        if let Some(id) = self.id {
            sc = sc.id(id);
        }
        if let Some(cb) = self.on_scroll {
            sc = sc.on_scroll(cb);
        }
        let inner: Element<'a, Message> = sc.into();

        // Mask the scrolling child to the card's rounded outline.
        //
        // `Container::clip(true)` (kept below) is a *rectangular* scissor
        // (`iced/widget/src/container.rs`): it hands the child a plain
        // `Rectangle` viewport, so the radius only rounds the painted chrome
        // quad while an opaque, zero-inset child would paint straight through
        // the corner arc. Because `enz_scroll_column` insets nothing on the
        // top/left/bottom by default, a full-bleed row's own background would
        // reach the ~3.5 px cutout of the default 12 px `Lg` radius.
        //
        // Reuse the shared primitive rather than growing local
        // geometry, exactly as `EnzSurface`/`EnzCard` and -- over an
        // `enz_scroll_column` specifically -- `EnzTreeView` (`tree_view.rs`) do:
        // `RoundedClip` records the child in a renderer layer whose mask
        // follows the arc (`Renderer::with_layer_rounded`), and a
        // nested plain clip *inherits* that rounded clip,
        // which is what makes it effective through the scrollable. The outer
        // container carries no padding, so the mask needs no `outset`: its
        // bounds already equal the child's laid-out bounds. Composing the
        // primitive keeps the full-bleed layout expressible -- nothing is
        // forced to inset -- while closing the corner leak.
        let masked: Element<'a, Message> =
            crate::rounded_clip::rounded_clip(inner, iced::border::Radius::new(radius)).into();

        container(masked)
            .width(Length::Fill)
            // When a content cap is set, hug the content (Shrink) and let the
            // inner MaxHeight impose the ceiling -- the chrome must not stretch
            // past the capped content. When uncapped, Fill the parent (an
            // uncapped scroll card filling its pane is intended). Mirrors
            // scroll_column.rs's height handling.
            .height(match cap {
                Some(_) => Length::Shrink,
                None => Length::Fill,
            })
            // Rectangular half of the clip contract: stops overflow at the
            // straight edges. The rounded half is the `RoundedClip` above.
            .clip(true)
            .style(move |theme: &iced::Theme| {
                let p = theme_lookup::palette(theme);
                container::Style {
                    background: Some(Self::tone_fill(tone, &p).into()),
                    border: Border {
                        color: if bordered {
                            crate::surface_edge::edge_color(
                                Self::tone_fill(tone, &p),
                                crate::surface_edge::DEFAULT_EDGE_DEPTH,
                            )
                        } else {
                            Color::TRANSPARENT
                        },
                        width: border_width,
                        radius: radius.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a, Message: 'a> From<EnzScrollCard<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzScrollCard<'a, Message>) -> Self {
        w.into_element()
    }
}

/// Normalise a requested cap to `Some(finite positive)` or `None` (uncapped).
///
/// Mirrors `scroll_column::sanitize_cap` and `max_height::sanitize_cap` --
/// the three must agree. The card resolves the cap once into this shape
/// and shares the result with both the inner column and the outer chrome, so
/// an invalid input (`0`, negative, `NaN`, `±INF`) degrades to *uncapped* on
/// both sides. Without this, an invalid cap makes the inner column fill while
/// the outer chrome selects `Shrink` -- the empty case collapses the card to
/// zero height and short content sits at 2-row content-hug (~49.6 px) instead
/// of filling the parent (~2000 px).
///
/// `0` is treated as invalid here for parity with `scroll_column`: a zero
/// cap is not expressible intent for a scroll region (a scroll surface with
/// height 0 is invisible, matching what the `max-content-height: 0px`
/// documents as "uncapped"). See `EnzScrollColumn`'s max-content-height handling.
fn sanitize_cap(cap: f32) -> Option<f32> {
    if cap.is_finite() && cap > 0.0 {
        Some(cap)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
