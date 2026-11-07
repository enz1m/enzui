//! Edge fade indicators for scrolling regions.
//!
//! A scroll region clips its content at the viewport edge. Without a cue, a
//! clipped item looks like the end of the list rather than the start of
//! hidden content — the user never discovers the rest.
//!
//! [`scroll_fade_edges`] overlays a short gradient on the side(s) that are
//! actually clipped, fading the region's own background over the
//! content. Horizontal (left/right) and vertical (top/bottom) both.
//!
//! # Paints only where hidden content exists
//!
//! A fade paints only where hidden content exists:
//!
//! | strip state | left fade | right fade |
//! |---|---|---|
//! | fits in its container | no | no |
//! | overflows, at rest (offset 0) | no | yes |
//! | overflows, mid-scroll | yes | yes |
//! | overflows, scrolled to the end | yes | no |
//!
//! The same rule applies vertically (top/bottom). The two `no` cells at
//! rest and at end are the point: a fade that always paints while the strip
//! overflows is an
//! always-fire indicator wearing a gradient — it claims there is more to see
//! in a direction that has nothing, and looks deliberate enough to read as a
//! design choice rather than a bug. The per-side predicates live on
//! [`ScrollGeometry`](crate::scroll_view::ScrollGeometry) (`is_clipped_left`
//! / `_right` / `_top` / `_bottom`).
//!
//! # Hazards
//!
//! `Rectangle::chord` subtracts `FRAC_PI_2` before `(cos, sin)`, so the
//! angle `Radians(FRAC_PI_2)` — the intuitive "downwards" — produces a
//! horizontal ramp. Over a short strip that is one constant colour per
//! row, i.e. a solid band that reads as a deliberate divider rather than a
//! bug. The correct angles are `180°` for a top-edge fade (opaque at top,
//! clear downward) and `0°` for a bottom-edge fade. See
//! `vertical_fade_layer`.
//!
//! A fade must be the colour of the surface beneath it. In dark,
//! `surface_elevated` is also the dialog background, so a wrong colour
//! choice paints perfectly and changes nothing visible — a test that samples
//! only the faded region passes against a no-op. Gate on the ramp varying
//! along the axis, with a positive control that a hard clip reads as
//! non-varying.

use iced::widget::container;
use iced::{Background, Color, Element, Length, Theme};

/// Default fade width in px.
///
/// The approved proposal specifies 8–12px; 12 is the upper end, chosen because
/// a narrower fade over a *dense* tab strip is easy to mistake for antialiasing
/// on the adjacent pill edge.
pub const DEFAULT_FADE_WIDTH: f32 = 12.0;

/// Which edges of a strip are currently clipped, and therefore which fades to
/// paint.
///
/// Build this from a live
/// [`ScrollGeometry`](crate::scroll_view::ScrollGeometry) via
/// [`Self::from_geometry`] rather than assembling it by hand in production —
/// the geometry is the only thing that knows the real answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollFadeEdges {
    /// Content is hidden behind the left edge.
    pub left: bool,
    /// Content is hidden past the right edge.
    pub right: bool,
    /// Content is hidden above the top edge.
    pub top: bool,
    /// Content is hidden below the bottom edge.
    pub bottom: bool,
}

impl ScrollFadeEdges {
    /// Neither edge clipped — the strip fits, so no fade paints.
    pub const NONE: Self = Self {
        left: false,
        right: false,
        top: false,
        bottom: false,
    };

    /// Derive the clipped edges from live scroll geometry.
    pub fn from_geometry(geo: &crate::scroll_view::ScrollGeometry) -> Self {
        Self {
            left: geo.is_clipped_left(),
            right: geo.is_clipped_right(),
            top: geo.is_clipped_top(),
            bottom: geo.is_clipped_bottom(),
        }
    }

    /// Whether any fade paints at all.
    pub fn any(&self) -> bool {
        self.left || self.right || self.top || self.bottom
    }

    /// Whether any horizontal fade paints.
    pub fn any_horizontal(&self) -> bool {
        self.left || self.right
    }

    /// Whether any vertical fade paints.
    pub fn any_vertical(&self) -> bool {
        self.top || self.bottom
    }
}

/// Overlay fade gradients on the clipped edge(s) of `content`.
///
/// When neither edge is clipped the child is returned unwrapped — no
/// `stack`, no transparent overlay, nothing added to the tree. That keeps the
/// common (fitting) case free of a layer that could alter hit-testing, and it
/// makes "no fade" observable structurally rather than merely visually.
pub fn scroll_fade_edges<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    edges: ScrollFadeEdges,
) -> ScrollFadeEdge<'a, Message>
where
    Message: 'a,
{
    ScrollFadeEdge {
        content: content.into(),
        edges,
        width: DEFAULT_FADE_WIDTH,
        color: None,
    }
}

/// Builder returned by [`scroll_fade_edges`].
#[must_use]
pub struct ScrollFadeEdge<'a, Message> {
    content: Element<'a, Message>,
    edges: ScrollFadeEdges,
    width: f32,
    color: Option<Color>,
}

impl<'a, Message: 'a> ScrollFadeEdge<'a, Message> {
    /// Override the fade width in px (default [`DEFAULT_FADE_WIDTH`]).
    ///
    /// Non-finite or negative values are ignored. `0.0` is expressible intent
    /// — a zero-width fade — and is honoured literally.
    pub fn width(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.width = px;
        }
        self
    }

    /// Pin the colour the fade resolves to at its opaque end.
    ///
    /// Unset (the default) resolves from the runtime theme's `background`, so
    /// the fade dissolves into whatever the strip sits on and keeps tracking
    /// light/dark. Pin it only when the strip sits on a surface the palette
    /// does not describe.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    /// Whether this builder will actually paint anything — used by tests and
    /// by callers that want to skip building the overlay entirely.
    pub fn paints(&self) -> bool {
        self.edges.any() && self.width > 0.0
    }
}

/// One horizontal edge gradient: opaque at the strip edge, transparent inward.
fn fade_layer<'a, Message: 'a>(
    width: f32,
    pinned: Option<Color>,
    from_left: bool,
) -> Element<'a, Message> {
    container(iced::widget::Space::new().width(Length::Fixed(width)))
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .style(move |theme: &Theme| {
            let base = pinned.unwrap_or_else(|| crate::theme_lookup::palette(theme).background);
            // Opaque against the strip's outer edge, fully transparent inward,
            // so content dissolves rather than being cut off.
            let opaque = Color { a: 1.0, ..base };
            let clear = Color { a: 0.0, ..base };
            // `Linear` angle is measured clockwise from "up"; 90deg points
            // right, 270deg points left. A left-edge fade is opaque at its own
            // left, so it runs left->right: opaque at 0.0, clear at 1.0.
            let angle = if from_left { 90.0f32 } else { 270.0f32 };
            let grad = iced::gradient::Linear::new(iced::Radians(angle.to_radians()))
                .add_stop(0.0, opaque)
                .add_stop(1.0, clear);
            container::Style {
                background: Some(Background::Gradient(grad.into())),
                ..container::Style::default()
            }
        })
        .into()
}

/// One vertical edge gradient: opaque at the strip edge, transparent inward.
///
/// The angle convention is the subtle part; see this module's header
/// (`# Hazards`) for why `Radians(FRAC_PI_2)` is the wrong "downwards".
fn vertical_fade_layer<'a, Message: 'a>(
    height: f32,
    pinned: Option<Color>,
    from_top: bool,
) -> Element<'a, Message> {
    container(iced::widget::Space::new().height(Length::Fixed(height)))
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .style(move |theme: &Theme| {
            let base = pinned.unwrap_or_else(|| crate::theme_lookup::palette(theme).background);
            let opaque = Color { a: 1.0, ..base };
            let clear = Color { a: 0.0, ..base };
            // `Linear` angle is clockwise from "up": 180° = downward, 0° = upward.
            // A top-edge fade is opaque at the top, clear downward → 180°.
            // A bottom-edge fade is opaque at the bottom, clear upward → 0°.
            let angle = if from_top { 180.0f32 } else { 0.0f32 };
            let grad = iced::gradient::Linear::new(iced::Radians(angle.to_radians()))
                .add_stop(0.0, opaque)
                .add_stop(1.0, clear);
            container::Style {
                background: Some(Background::Gradient(grad.into())),
                ..container::Style::default()
            }
        })
        .into()
}

impl<'a, Message: 'a> From<ScrollFadeEdge<'a, Message>> for Element<'a, Message> {
    fn from(f: ScrollFadeEdge<'a, Message>) -> Self {
        // Nothing clipped (or a zero-width fade): return the child untouched.
        // Wrapping it in a `stack` with two invisible layers would add a
        // hit-testing surface for no visual gain.
        if !f.paints() {
            return f.content;
        }

        let has_horiz = f.edges.any_horizontal();
        let has_vert = f.edges.any_vertical();

        let mut layers: Vec<Element<'a, Message>> = vec![f.content];

        // Horizontal overlay: left fade | Fill spacer | right fade.
        if has_horiz {
            let mut overlay = iced::widget::row![];
            overlay = if f.edges.left {
                overlay.push(fade_layer::<Message>(f.width, f.color, true))
            } else {
                overlay.push(iced::widget::Space::new().width(Length::Fixed(0.0)))
            };
            overlay = overlay.push(iced::widget::Space::new().width(Length::Fill));
            overlay = if f.edges.right {
                overlay.push(fade_layer::<Message>(f.width, f.color, false))
            } else {
                overlay.push(iced::widget::Space::new().width(Length::Fixed(0.0)))
            };
            layers.push(overlay.into());
        }

        // Vertical overlay: top fade | Fill spacer | bottom fade.
        if has_vert {
            let mut overlay = iced::widget::column![];
            overlay = if f.edges.top {
                overlay.push(vertical_fade_layer::<Message>(f.width, f.color, true))
            } else {
                overlay.push(iced::widget::Space::new().height(Length::Fixed(0.0)))
            };
            overlay = overlay.push(iced::widget::Space::new().height(Length::Fill));
            overlay = if f.edges.bottom {
                overlay.push(vertical_fade_layer::<Message>(f.width, f.color, false))
            } else {
                overlay.push(iced::widget::Space::new().height(Length::Fixed(0.0)))
            };
            layers.push(overlay.into());
        }

        iced::widget::Stack::with_children(layers).into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
