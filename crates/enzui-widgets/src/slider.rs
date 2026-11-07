//! `EnzSlider` — a custom, focusable slider primitive.
//!
//! # Where this belongs
//!
//! [`media_controls`](crate::media_controls) uses this as its scrubber — never
//! a hand-rolled progress bar — so the transport strip carries the focus ring,
//! step/page keys and drag-scrub. Reach for `slider` directly wherever a
//! bounded value needs a focusable primitive (volume, coarse-tune).
//!
//! The stock `iced::widget::Slider` defects this exists to avoid: disabled
//! only dims (drag/wheel/keys still fire); the 28×18 capsule never renders;
//! keyboard is hover-only Up/Down (no Tab focus, no Left/Right/Home/End/Page,
//! no ring); `.step()` takes zero/negative/NaN/∞; endpoints unvalidated;
//! caption reads the raw wrapper value; zero slider semantics.
//!
//! This module paints track/fill/pill itself and owns interaction + focus +
//! keyboard + semantics — answered at the primitive level. It renders only the
//! control; label/value-caption composition lives in [`EnzSlider`].
//!
//! # Orientation
//!
//! Typed axis [`SliderOrientation`], default `Horizontal`. Deliberately one
//! implementation, no `EnzVSlider` fork: sanitization, focus identity, routed
//! a11y and the numeric contract stay shared (a fork's a11y decays without
//! reddening the original's tests).
//!
//! Three rules, each with a plausible opposite the next reader will assume:
//!
//! 1. Vertical maps minimum to the BOTTOM (mixer-fader): value increases
//!    as `y` DECREASES. The one sign flip; lives in `value_at` and
//!    `paint_rects` only.
//!
//! 2. Up/Right increment, Down/Left decrement, in both orientations. The
//!    axis never selects the live arrow pair — the alternative would change
//!    the horizontal contract (which answers Up/Down), strand vertical-fader
//!    Up/Down users and keyboard Left/Right users. Platform convention (ARIA
//!    `slider`, GTK, AppKit).
//!
//! 3. RTL touches the horizontal axis only. Never by flipping the value
//!    map (that inverts vertical as a side effect): the vertical `value_at`
//!    branch consults no direction at all
//!    (`vertical_value_mapping_is_unaffected_by_ambient_rtl` pins it).

use crate::enz_text::enz_text;
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{
    FocusState, Nav, PressFocus, navigation_from_key_in, press_focus_from_cursor,
    release_focus_if_inert,
};
use enzui_core::semantics::{NumericValue, Orientation, Role, Semantics};
use enzui_theme::{Palette, SpacingScale, TypographyScale};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::widget::{Space, column};
use iced::{
    Background, Border, Color, Degrees, Element, Event, Gradient, Length, Rectangle, Shadow, Size,
    mouse, touch, window,
};
use std::ops::RangeInclusive;

use crate::color::darken;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Slider paint roles, seeded from the palette and default geometry.
///
/// Dimensions are logical pixels. Invalid nonnegative dimensions use their
/// defaults; zero hides an outline, removes an outset or disables shadow blur.
/// Unpinned outlines follow the live hairline stroke, even with a colour snapshot.
#[derive(Debug, Clone, Copy)]
pub struct SliderTokens {
    /// Track (inactive portion) background color.
    pub track: Color,
    /// Rail outline colour for part-to-part contrast.
    /// See [`Palette::meter_track_border`].
    pub track_border: Color,
    /// Active (filled) portion color.
    pub fill: Color,
    /// Fill color when hovered.
    pub fill_hover: Color,
    /// Neutral thumb fill: text ink in dark palettes, surface fill in light palettes.
    pub thumb: Color,
    /// Optional thumb outline; transparent by default.
    pub thumb_border: Color,
    /// Elevation shadow cast by the thumb.
    pub thumb_shadow: Color,
    /// Thumb color while a drag is live.
    pub thumb_dragging: Color,
    /// Label text color.
    pub text_color: Color,
    /// Value text color (muted).
    pub value_text_color: Color,
    /// Focus-visible ring color.
    pub focus_ring: Color,
    /// Rail outline width; `None` follows the live hairline. Invalid pins also use it.
    pub track_border_width: Option<f32>,
    /// Thumb outline width; `None` follows the live hairline. Invalid pins also use it.
    pub thumb_border_width: Option<f32>,
    /// Focus outline width; default 2. Zero hides the ring.
    pub focus_ring_width: f32,
    /// Focus outline outset from the thumb; default 3.
    pub focus_ring_outset: f32,
    /// Shadow displacement; default (0, 2). Each non-finite axis uses its default.
    pub thumb_shadow_offset: iced::Vector,
    /// Shadow blur radius; default 4.
    pub thumb_shadow_blur: f32,
    /// Disabled opacity multiplier; default 0.62. Finite values clamp to [0, 1],
    /// non-finite values use the default. Transparent colours stay transparent.
    pub disabled_alpha: f32,
    /// Stop-dot diameter; default 2. Zero hides the dots without changing snapping.
    pub step_mark_size: f32,
    /// Stop dots on the empty rail.
    pub step_mark: Color,
    /// Stop dots on the filled part, in the fill's ink.
    pub step_mark_on_fill: Color,
    /// Minimum centre-to-centre stop separation; default 8. Zero allows dense dots,
    /// subject to the 64-interval paint limit.
    pub step_mark_min_spacing: f32,
}

impl SliderTokens {
    /// Derive slider tokens from a theme palette.
    pub fn from_palette(p: &Palette) -> Self {
        // A thumb is opaque wherever it sits, glass included.
        let thumb = if p.is_dark() {
            p.text
        } else {
            crate::color::with_alpha(p.surface, 1.0)
        };
        let drag_overlay = if p.is_dark() {
            Color {
                a: p.overlay_pressed().a,
                ..p.surface
            }
        } else {
            p.overlay_pressed()
        };
        Self {
            // A soft groove; the accent fill clears 3:1 against it (see
            // `Palette::meter_track`), and it draws no separate outline.
            track: p.meter_track(),
            track_border: p.meter_track_border(),
            fill: p.accent,
            fill_hover: darken(p.accent, 0.08),
            thumb,
            // A shade of the thumb's own fill: a white thumb on a light page
            // keeps a crisp rim without a harsh outline.
            thumb_border: crate::surface_edge::edge_color(
                thumb,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            thumb_shadow: p.shadow,
            thumb_dragging: crate::surface_interaction::composite(thumb, drag_overlay),
            text_color: p.text,
            value_text_color: p.text_muted,
            focus_ring: p.focus_ring,
            track_border_width: None,
            thumb_border_width: None,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            focus_ring_outset: 3.0,
            thumb_shadow_offset: iced::Vector::new(0.0, 2.0),
            thumb_shadow_blur: 4.0,
            disabled_alpha: 0.62,
            step_mark_size: 2.0,
            step_mark: Color { a: 0.38, ..p.text },
            step_mark_on_fill: Color {
                a: 0.7,
                ..p.filled_color(p.accent).text
            },
            step_mark_min_spacing: 8.0,
        }
    }

    /// Convenience: tokens for dark theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: tokens for light theme.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Orientation
// ---------------------------------------------------------------------------

/// The axis an [`EnzSlider`] is laid out and driven along.
///
/// [`Horizontal`](Self::Horizontal) is the default: untouched `enz_slider(..)`
/// is horizontal. A WIDGET vocabulary, not a re-export of
/// [`Orientation`]: the axis is a
/// layout/interaction property also published to AT;
/// [`to_semantic`](Self::to_semantic) is the single meeting point, so the AT
/// axis cannot drift from the interaction axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum SliderOrientation {
    /// Left-to-right track: the minimum sits at the leading edge, the maximum
    /// at the trailing edge. The compatibility default.
    #[default]
    Horizontal,
    /// Bottom-to-top track: the minimum sits at the bottom and the maximum
    /// at the top, which is the mixer-fader convention every equalizer uses.
    ///
    /// Note what that implies arithmetically: the value *increases* as the
    /// pointer's `y` *decreases*, so the vertical pointer map carries a sign
    /// flip the horizontal one does not.
    Vertical,
}

impl SliderOrientation {
    /// The semantic axis published to assistive technology.
    ///
    /// Derived from the same value driving layout, paint and the pointer map —
    /// never a hardcoded `Horizontal` at the semantics site. A vertical slider
    /// announcing horizontal would pass every horizontal-only test: one source
    /// or nothing.
    #[must_use]
    pub const fn to_semantic(self) -> Orientation {
        match self {
            Self::Horizontal => Orientation::Horizontal,
            Self::Vertical => Orientation::Vertical,
        }
    }

    /// Whether this is the vertical axis.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Vertical)
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Minimum interactive extent along the interaction axis: the width of a
/// horizontal slider, the height of a vertical one.
///
/// Named for the axis, not for `width`: `width` only means the right thing on
/// one of the two axes.
const MIN_LENGTH: f32 = 140.0;

/// Minimum pointer target height (WCAG 2.2 target size, 24px floor).
///
/// The PAINTED row is only thumb-tall (20 px default) — below the a11y floor.
/// Hit row decouples from painted row: layout reserves
/// [`hit_thickness`](SliderGeometry::hit_thickness), `draw` keeps centring
/// track+thumb inside it. Visual unchanged; only the hit region grows.
const MIN_HIT_HEIGHT: f32 = 24.0;

/// Resolved geometry for the slider control, in axis-relative terms.
///
/// `track_height`/`thumb_width`/`thumb_height` name the right thing on one
/// axis only: a vertical slider reading `thumb_width` for travel is a latent
/// bug. Every field is ALONG or across the interaction axis; the public
/// width/height mapping happens exactly once, in
/// [`resolved_geometry`](EnzSlider::resolved_geometry).
#[derive(Debug, Clone, Copy)]
struct SliderGeometry {
    /// Which axis the value travels along.
    orientation: SliderOrientation,
    /// Track extent across the interaction axis (its "thinness"): the
    /// height of a horizontal track, the width of a vertical one.
    track_thickness: f32,
    /// Thumb extent along the interaction axis — the direction it travels.
    thumb_along: f32,
    /// Thumb extent across the interaction axis.
    thumb_across: f32,
    /// A horizontal slider in a right-to-left locale: the minimum is at the
    /// right edge.
    mirrored: bool,
}

impl SliderGeometry {
    /// Painted cross-axis extent: the thicker of thumb and track.
    fn row_thickness(&self) -> f32 {
        self.thumb_across.max(self.track_thickness)
    }

    /// Full interaction cross-axis extent: never below the accessibility floor.
    ///
    /// `max` no-ops when the painted row already exceeds the floor (taller
    /// custom thumb) — intended. On vertical this is the control's WIDTH: the
    /// 24 px WCAG floor holds on whichever axis is narrow.
    fn hit_thickness(&self) -> f32 {
        self.row_thickness().max(MIN_HIT_HEIGHT)
    }

    /// The control's `(width, height)` for a given along-axis length.
    fn size_for(&self, along: f32) -> Size {
        match self.orientation {
            SliderOrientation::Horizontal => Size::new(along, self.hit_thickness()),
            SliderOrientation::Vertical => Size::new(self.hit_thickness(), along),
        }
    }

    /// The bounds extent along the interaction axis.
    fn along_of(&self, bounds: Rectangle) -> f32 {
        self.along_of_size(bounds.size())
    }

    /// The size extent along the interaction axis.
    fn along_of_size(&self, size: Size) -> f32 {
        match self.orientation {
            SliderOrientation::Horizontal => size.width,
            SliderOrientation::Vertical => size.height,
        }
    }

    /// The bounds origin along the interaction axis.
    fn along_origin_of(&self, bounds: Rectangle) -> f32 {
        match self.orientation {
            SliderOrientation::Horizontal => bounds.x,
            SliderOrientation::Vertical => bounds.y,
        }
    }

    fn painted_thumb_along(&self, bounds: Rectangle) -> f32 {
        self.thumb_along.min(self.along_of(bounds).max(0.0))
    }

    fn usable_along(&self, bounds: Rectangle) -> f32 {
        (self.along_of(bounds) - self.painted_thumb_along(bounds)).max(0.0)
    }

    /// Pill radius: half the thumb's shorter side, so the thumb reads as the
    /// same 28×18 capsule whichever way it is turned. Equals `thumb_height / 2`
    /// for a default horizontal slider.
    fn thumb_radius(&self, bounds: Rectangle) -> f32 {
        self.painted_thumb_along(bounds).min(self.thumb_across) / 2.0
    }

    /// Every rectangle the slider paints, for a given `bounds` and fill
    /// `frac`tion.
    ///
    /// Out of `draw` so both orientations' GEOMETRY is a pure unit-testable
    /// function, and so pointer map and paint share [`usable_along`](Self::usable_along)
    /// instead of each computing thumb travel. Vertical places
    /// the thumb at `(1 - frac)` of travel (minimum at the BOTTOM): fill grows
    /// upward from the bottom edge — what a fader looks like.
    fn paint_rects(&self, bounds: Rectangle, frac: f32) -> SliderPaint {
        let usable = self.usable_along(bounds);
        let thumb_along = self.painted_thumb_along(bounds);
        let track_t = self.track_thickness;
        match self.orientation {
            SliderOrientation::Horizontal => {
                // Mirrored, the fill grows from the right edge leftwards.
                let travel = if self.mirrored { 1.0 - frac } else { frac };
                let thumb_x = bounds.x + travel * usable;
                let track_y = bounds.y + (bounds.height - track_t) / 2.0;
                let fill_w = if self.mirrored {
                    (bounds.x + bounds.width - (thumb_x + thumb_along / 2.0))
                        .clamp(0.0, bounds.width)
                } else {
                    (thumb_x + thumb_along / 2.0 - bounds.x).clamp(0.0, bounds.width)
                };
                SliderPaint {
                    track: Rectangle {
                        x: bounds.x,
                        y: track_y,
                        width: bounds.width,
                        height: track_t,
                    },
                    fill: Rectangle {
                        x: if self.mirrored {
                            bounds.x + bounds.width - fill_w
                        } else {
                            bounds.x
                        },
                        y: track_y,
                        width: fill_w,
                        height: track_t,
                    },
                    thumb: Rectangle {
                        x: thumb_x,
                        y: bounds.y + (bounds.height - self.thumb_across) / 2.0,
                        width: thumb_along,
                        height: self.thumb_across,
                    },
                }
            }
            SliderOrientation::Vertical => {
                // `1.0 - frac`: the maximum is at the TOP, so a full slider has
                // its thumb at y = bounds.y.
                let thumb_y = bounds.y + (1.0 - frac) * usable;
                let track_x = bounds.x + (bounds.width - track_t) / 2.0;
                let fill_top = thumb_y + thumb_along / 2.0;
                let bottom = bounds.y + bounds.height;
                let fill_h = (bottom - fill_top).clamp(0.0, bounds.height);
                SliderPaint {
                    track: Rectangle {
                        x: track_x,
                        y: bounds.y,
                        width: track_t,
                        height: bounds.height,
                    },
                    fill: Rectangle {
                        x: track_x,
                        y: bottom - fill_h,
                        width: track_t,
                        height: fill_h,
                    },
                    thumb: Rectangle {
                        x: bounds.x + (bounds.width - self.thumb_across) / 2.0,
                        y: thumb_y,
                        width: self.thumb_across,
                        height: thumb_along,
                    },
                }
            }
        }
    }
}

/// The three rectangles [`SliderGeometry::paint_rects`] resolves.
#[derive(Debug, Clone, Copy)]
struct SliderPaint {
    /// The full inactive track.
    track: Rectangle,
    /// The active portion, growing from the minimum end.
    fill: Rectangle,
    /// The pill thumb.
    thumb: Rectangle,
}

// ---------------------------------------------------------------------------
// Value math (pure, unit-tested)
// ---------------------------------------------------------------------------

/// Sanitize a caller-supplied range into a valid `(min, max)` with `min <= max`
/// and both finite. Falls back to `0.0..=1.0` if the endpoints are unusable.
fn sanitize_range(range: &RangeInclusive<f32>) -> (f32, f32) {
    // The centralized ordered-range policy: `new_sorted` repairs a
    // reversed pair and rejects non-finite endpoints, which is exactly this
    // widget's contract; the 0..=1 fallback stays widget-local.
    enzui_core::metrics::OrderedRangeF32::new_sorted(*range.start(), *range.end())
        .map_or((0.0, 1.0), |r| (r.lo(), r.hi()))
}

// Only a finite, strictly positive step snaps; anything else disables
// snapping instead of reaching division-based snapping.
fn sanitize_step(step: Option<f32>) -> Option<f32> {
    step.and_then(enzui_core::metrics::FinitePositive::new)
        .map(enzui_core::metrics::FinitePositive::get)
}

/// Clamp `value` to `[min, max]` and snap to `step` (if any), then re-clamp.
///
/// `f32::clamp` PROPAGATES NaN: a NaN `value` survived into formatting,
/// semantics, fill and renderer coordinates. Non-finite input normalizes to
/// `min` first (in range for every range; 0.0 is not). `pub` because the a11y
/// router drives the same stepping rule from `Increment` rather than
/// a second one. Keyboard handler and AT router both route here: one landing
/// place per change.
pub fn clamp_and_snap(value: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    let value = if value.is_finite() { value } else { min };
    let value = value.clamp(min, max);
    let min = f64::from(min);
    let max = f64::from(max);
    let snapped = match step {
        Some(step) => {
            let step = f64::from(step);
            let steps = ((f64::from(value) - min) / step).round();
            min + steps * step
        }
        None => f64::from(value),
    };
    let snapped = snapped.clamp(min, max);
    if max - f64::from(value) <= (snapped - f64::from(value)).abs() {
        max as f32
    } else {
        snapped as f32
    }
}

fn range_span(min: f32, max: f32) -> f64 {
    f64::from(max) - f64::from(min)
}

fn offset_and_snap(value: f32, offset: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    let value = if value.is_finite() { value } else { min };
    let shifted =
        (f64::from(value) + f64::from(offset)).clamp(f64::from(min), f64::from(max)) as f32;
    clamp_and_snap(shifted, min, max, step)
}

fn value_at_fraction(fraction: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    let raw = (f64::from(min) + f64::from(fraction) * range_span(min, max))
        .clamp(f64::from(min), f64::from(max)) as f32;
    clamp_and_snap(raw, min, max, step)
}

/// Increment used by arrow keys when no explicit step is set: 1% of the range.
///
/// `pub` for the same reason as [`clamp_and_snap`]: the a11y router's
/// end-to-end test drives this same rule so `Increment` on a slider without an
/// explicit step advances by 1% of the range.
pub fn implicit_step(min: f32, max: f32) -> f32 {
    (range_span(min, max) / 100.0).max(f64::from(f32::EPSILON)) as f32
}

/// Apply an `AccessibilityAction::Increment` to a slider's numeric contract:
/// bump by `step` (or the 1%-of-range default for `None`), then clamp+snap.
/// Symmetric [`slider_decrement_value`] for the other direction.
///
/// The keyboard handler's rule through `nav_value` (`Increment => current +
/// base_step`) — which is why `implicit_step` and `clamp_and_snap` are `pub`:
/// one helper for both call sites, so "an Increment" means one thing whether
/// from an arrow key or an AT action request.
pub fn slider_increment_value(value: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    let base_step = step.unwrap_or_else(|| implicit_step(min, max));
    offset_and_snap(value, base_step, min, max, step)
}

/// Symmetric counterpart of [`slider_increment_value`] for
/// `AccessibilityAction::Decrement`. See that function's docs.
pub fn slider_decrement_value(value: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    if value == max
        && let Some(step) = step
    {
        let last = (f64::from(min)
            + (range_span(min, max) / f64::from(step)).floor() * f64::from(step))
            as f32;
        if last < max {
            return last.max(min);
        }
    }
    let base_step = step.unwrap_or_else(|| implicit_step(min, max));
    offset_and_snap(value, -base_step, min, max, step)
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for an EnzSlider element.
#[must_use]
pub struct EnzSlider<'a, Message> {
    range: RangeInclusive<f32>,
    value: f32,
    on_change: Box<dyn Fn(f32) -> Message + 'a>,
    on_drag_start: Option<Message>,
    on_release: Option<Message>,
    step: Option<f32>,
    step_marks: bool,
    show_value: bool,
    value_format: Option<std::rc::Rc<dyn Fn(f32) -> String + 'a>>,
    hover_preview: bool,
    thumb_grow: bool,
    label: String,
    /// Whether [`label`](Self::label) is painted above the control, or only
    /// names it to assistive technology ([`aria_label`](Self::aria_label)).
    label_visible: bool,
    caption_typography: Option<TypographyScale>,
    caption_spacing: Option<SpacingScale>,
    orientation: SliderOrientation,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    track_height: f32,
    thumb_width: f32,
    thumb_height: f32,
    /// Along-axis extent of the control when vertical. Horizontal sliders fill
    /// their parent's width; a vertical slider has no equivalent "fill the
    /// column" default that is safe (a bare `Length::Fill` height inside an
    /// unbounded column resolves to the whole viewport), so the vertical length
    /// is an explicit, defaulted number.
    vertical_length: f32,
    enabled: bool,
    tokens: Option<SliderTokens>,
    track_paint: Option<TrackPaint>,
    id: Option<iced::advanced::widget::Id>,
    /// Namespace for the label-derived default id. Defaults to
    /// [`DEFAULT_ID_PREFIX`].
    id_prefix: String,
}

/// An opt-in rail painting for sliders whose *track itself* carries the
/// meaning of the axis — a hue spectrum, an alpha ramp.
///
/// Default `None`: ordinary sliders paint the flat track + accent fill,
/// byte-for-byte as always. When set, the rail paints 1. an optional alpha
/// checkerboard (a translucent ramp reads VISIBLE, not guessed), then 2. the
/// caller's gradient across the whole track — and the accent fill suppresses
/// (it would repaint the caller's stops). The thumb stays the value indicator.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackPaint {
    /// Gradient stops in track order, offsets in `0.0..=1.0`. iced's linear
    /// gradient carries at most eight; extras are ignored by the renderer.
    pub stops: Vec<(f32, Color)>,
    /// Paint an alpha checkerboard under the gradient.
    pub checker: bool,
    /// Shared transparency backdrop; used only when `checker` is true.
    pub checker_tokens: CheckerboardTokens,
}

impl TrackPaint {
    /// A spectrum rail: opaque stops, no checkerboard.
    #[must_use]
    pub fn spectrum(stops: Vec<(f32, Color)>) -> Self {
        Self {
            stops,
            checker: false,
            checker_tokens: CheckerboardTokens::default(),
        }
    }

    /// A translucency rail: the checkerboard is painted beneath the stops.
    #[must_use]
    pub fn translucent(stops: Vec<(f32, Color)>) -> Self {
        Self {
            stops,
            checker: true,
            checker_tokens: CheckerboardTokens::default(),
        }
    }

    /// Pin the transparency backdrop independently of gradient stops.
    #[must_use]
    pub fn checker_tokens(mut self, tokens: CheckerboardTokens) -> Self {
        self.checker_tokens = tokens.sanitized();
        self
    }

    fn gradient(&self, angle: Degrees, opacity: f32) -> Gradient {
        let mut linear = iced::gradient::Linear::new(angle);
        for (offset, color) in &self.stops {
            linear = linear.add_stop(
                offset.clamp(0.0, 1.0),
                crate::color::with_opacity(*color, opacity),
            );
        }
        Gradient::Linear(linear)
    }
}

/// One square of the alpha checkerboard, in logical px.
pub const CHECKER_SQUARE: f32 = 5.0;

/// The checkerboard's light square.
pub const CHECKER_LIGHT: Color = Color::from_rgb(0.85, 0.85, 0.85);

/// The checkerboard's dark square.
pub const CHECKER_DARK: Color = Color::from_rgb(0.55, 0.55, 0.55);

/// Shared transparency backdrop for slider ramps and colour-picker previews.
///
/// Neutral defaults distinguish transparency from the selected colour and do
/// not follow the page palette. Pass the same snapshot to both components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckerboardTokens {
    /// Cell edge in logical pixels; default 5. Invalid or nonpositive values
    /// use the default. Subpixel cells are clamped to 1 to bound paint work.
    pub square_size: f32,
    /// Base cell colour; defaults to neutral RGB (0.85, 0.85, 0.85).
    pub light: Color,
    /// Alternating cell colour, painted over the base; defaults to RGB (0.55, 0.55, 0.55).
    pub dark: Color,
}

impl Default for CheckerboardTokens {
    fn default() -> Self {
        Self {
            square_size: CHECKER_SQUARE,
            light: CHECKER_LIGHT,
            dark: CHECKER_DARK,
        }
    }
}

impl CheckerboardTokens {
    /// Normalize the cell edge; colour channels retain their supplied values.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            square_size: enzui_core::metrics::FinitePositive::new_or(
                self.square_size,
                CHECKER_SQUARE,
            )
            .get()
            .max(1.0),
            ..self
        }
    }
}

/// Paint a transparency backdrop using the shared default appearance.
pub fn paint_checkerboard(renderer: &mut iced::Renderer, bounds: Rectangle, radius: f32) {
    paint_checkerboard_with_tokens(renderer, bounds, radius, CheckerboardTokens::default());
}

/// Paint a transparency backdrop using a supplied appearance snapshot.
/// Bounds and corner radius are logical pixels; the caller supplies clipping.
pub fn paint_checkerboard_with_tokens(
    renderer: &mut iced::Renderer,
    bounds: Rectangle,
    radius: f32,
    tokens: CheckerboardTokens,
) {
    paint_checkerboard_with_opacity(renderer, bounds, radius, tokens, 1.0);
}

fn paint_checkerboard_with_opacity(
    renderer: &mut impl iced::advanced::Renderer,
    bounds: Rectangle,
    radius: f32,
    tokens: CheckerboardTokens,
    opacity: f32,
) {
    let tokens = tokens.sanitized();
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    renderer.fill_quad(
        renderer::Quad {
            bounds,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: radius.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        },
        Background::Color(crate::color::with_opacity(tokens.light, opacity)),
    );
    let cols = (bounds.width / tokens.square_size).ceil() as i32;
    let rows = (bounds.height / tokens.square_size).ceil() as i32;
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) % 2 == 0 {
                continue;
            }
            let x = bounds.x + col as f32 * tokens.square_size;
            let y = bounds.y + row as f32 * tokens.square_size;
            let square = Rectangle {
                x,
                y,
                width: tokens.square_size.min(bounds.x + bounds.width - x),
                height: tokens.square_size.min(bounds.y + bounds.height - y),
            };
            if square.width <= 0.0 || square.height <= 0.0 {
                continue;
            }
            renderer.fill_quad(
                renderer::Quad {
                    bounds: square,
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Color(crate::color::with_opacity(tokens.dark, opacity)),
            );
        }
    }
}

/// The namespace slider ids are derived under when no [`EnzSlider::id`] or
/// [`EnzSlider::id_prefix`] is supplied. The key is the slider's label.
pub const DEFAULT_ID_PREFIX: &str = "enz-slider";

/// Default along-axis length of a vertical slider, in px.
///
/// Horizontal takes `Length::Fill` along-axis (a row's width is parent-bounded
/// in every ordinary layout). Vertical has no such guarantee — `Fill` height
/// in an unbounded `column` resolves against the viewport — so vertical is a
/// fixed length by default; [`length`](EnzSlider::length) overrides.
pub const DEFAULT_VERTICAL_LENGTH: f32 = 160.0;

/// Create an EnzSlider element.
///
/// # Arguments
///
/// * `range` — the value range (e.g. `0.0..=100.0`).
/// * `value` — current value (clamped to `range` for render and caption).
/// * `on_change` — callback producing a message on value change.
pub fn enz_slider<'a, Message: 'a>(
    range: RangeInclusive<f32>,
    value: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> EnzSlider<'a, Message> {
    EnzSlider {
        range,
        value,
        on_change: Box::new(on_change),
        on_drag_start: None,
        on_release: None,
        step: None,
        step_marks: true,
        show_value: false,
        value_format: None,
        hover_preview: false,
        thumb_grow: false,
        label: String::new(),
        label_visible: true,
        caption_typography: None,
        caption_spacing: None,
        orientation: SliderOrientation::Horizontal,
        direction: None,
        track_height: 6.0,
        // A 28 x 18 capsule knob on a 6 px rail; `thumb_grow` swells it
        // while hovered or dragged (see `GROW_SCALE`).
        thumb_width: 28.0,
        thumb_height: 18.0,
        vertical_length: DEFAULT_VERTICAL_LENGTH,
        enabled: true,
        tokens: None,
        track_paint: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
    }
}

impl<'a, Message: Clone + 'a> EnzSlider<'a, Message> {
    /// Publish before value changes when a pointer or touch drag begins.
    pub fn on_drag_start(mut self, message: Message) -> Self {
        self.on_drag_start = Some(message);
        self
    }

    /// Publish when an owned drag ends, is lost, or is disabled.
    /// Keyboard, wheel and accessibility value changes do not publish this message.
    pub fn on_release(mut self, message: Message) -> Self {
        self.on_release = Some(message);
        self
    }

    /// Set the step increment. Non-finite / zero / negative steps are ignored
    /// (snapping disabled) rather than forwarded into division.
    /// Separated stop indicators appear on the rail when space permits;
    /// [`Self::step_marks`] controls their visibility without changing the step.
    pub fn step(mut self, step: f32) -> Self {
        self.step = Some(step);
        self
    }

    /// Show separated stop dots when space permits. Defaults to `true`.
    /// `false` hides only the dots; snapping and keyboard/AT stepping still
    /// use [`Self::step`]. Custom track painting suppresses the dots either way.
    pub fn step_marks(mut self, show: bool) -> Self {
        self.step_marks = show;
        self
    }

    /// Show a value label above the slider.
    pub fn show_value(mut self, show: bool) -> Self {
        self.show_value = show;
        self
    }

    /// Set a custom value formatter (e.g. `|v| format!("{v:.1}%")`).
    pub fn value_format(mut self, fmt: impl Fn(f32) -> String + 'a) -> Self {
        self.value_format = Some(std::rc::Rc::new(fmt));
        self
    }

    /// Show the value under the pointer in a small bubble above the track
    /// while the pointer hovers it, formatted by [`Self::value_format`]; a
    /// seek bar shows the time a click would jump to. Needs a value format.
    pub fn hover_preview(mut self, show: bool) -> Self {
        self.hover_preview = show;
        self
    }

    /// Swell the knob a little while the pointer is over the slider or
    /// dragging it, as Apple's capsule sliders do. Off by default.
    pub fn thumb_grow(mut self, grow: bool) -> Self {
        self.thumb_grow = grow;
        self
    }

    /// Set a label above the slider.
    pub fn label(mut self, label: &str) -> Self {
        label.clone_into(&mut self.label);
        self.label_visible = true;
        self
    }

    /// Name the slider for assistive technology without painting a caption:
    /// for a control whose purpose its surroundings already show (a seek
    /// bar under a video, a volume slider beside a speaker button). The name
    /// still derives the routable id, exactly as [`label`](Self::label)'s.
    pub fn aria_label(mut self, label: &str) -> Self {
        label.clone_into(&mut self.label);
        self.label_visible = false;
        self
    }

    /// Override the writing direction. By default a horizontal slider follows
    /// the locale: right-to-left puts the minimum at the right edge, fills
    /// from the right, and makes Left increase the value.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Set the interaction axis. Defaults to
    /// [`Horizontal`](SliderOrientation::Horizontal).
    ///
    /// Vertical maps its MINIMUM to the bottom, maximum to the top, runs
    /// [`DEFAULT_VERTICAL_LENGTH`] px unless [`length`](Self::length) says
    /// otherwise, and publishes [`Vertical`](Orientation::Vertical) to AT.
    /// Keyboard deliberately UNCHANGED: Up/Right increment, Down/Left decrement
    /// on both axes.
    pub const fn orientation(mut self, orientation: SliderOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Shorthand for `.orientation(SliderOrientation::Vertical)`.
    pub const fn vertical(self) -> Self {
        self.orientation(SliderOrientation::Vertical)
    }

    /// Set the along-axis length of a vertical slider in px (default:
    /// [`DEFAULT_VERTICAL_LENGTH`]).
    ///
    /// Ignored by horizontal (along-axis is parent-decided `Fill`).
    /// Non-finite / zero / negative falls back to the field's own previous
    /// value, never another field's — the builder stays order-independent.
    pub fn length(mut self, length: f32) -> Self {
        self.vertical_length =
            enzui_core::metrics::FinitePositive::new_or(length, self.vertical_length).get();
        self
    }

    /// Set the track thickness in pixels (default: 6).
    ///
    /// "Thickness" is the cross-axis extent: the height of a horizontal
    /// track and the width of a vertical one. The builder keeps the
    /// `track_height` name so horizontal-only callers stay source-compatible.
    pub fn track_height(mut self, h: f32) -> Self {
        self.track_height = enzui_core::metrics::FinitePositive::new_or(h, 6.0).get();
        self
    }

    /// Set the preferred on-screen thumb width in pixels (default: 28).
    ///
    /// This is along the track horizontally and across it vertically.
    /// The along-axis extent shrinks when the allocated track is shorter.
    pub fn thumb_width(mut self, w: f32) -> Self {
        self.thumb_width = enzui_core::metrics::FinitePositive::new_or(w, 28.0).get();
        self
    }

    /// Set the preferred on-screen thumb height in pixels (default: 18).
    ///
    /// This is across the track horizontally and along it vertically.
    /// The along-axis extent shrinks when the allocated track is shorter.
    pub fn thumb_height(mut self, h: f32) -> Self {
        self.thumb_height = enzui_core::metrics::FinitePositive::new_or(h, 18.0).get();
        self
    }

    /// Map the public width/height thumb builders onto the axis-relative
    /// geometry the control actually uses.
    ///
    /// The only meeting place of the two vocabularies: horizontal travels on
    /// x (along-extent = width), vertical on y (along-extent = height).
    /// Reading `thumb_width` in a vertical path is the bug this makes
    /// unrepresentable.
    fn resolved_geometry(&self) -> SliderGeometry {
        let (along, across) = match self.orientation {
            SliderOrientation::Horizontal => (self.thumb_width, self.thumb_height),
            SliderOrientation::Vertical => (self.thumb_height, self.thumb_width),
        };
        SliderGeometry {
            orientation: self.orientation,
            track_thickness: self.track_height,
            thumb_along: along,
            thumb_across: across,
            mirrored: self.orientation == SliderOrientation::Horizontal
                && enzui_core::placement::resolve_direction(self.direction)
                    == enzui_core::placement::Direction::RightToLeft,
        }
    }

    /// Set enabled/disabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Forward caption typography from the active theme.
    /// Label and value runs use `label_size` (default `11.0` logical pixels),
    /// with the scale's text weights. Invalid or non-positive label sizes use
    /// the default. Rebuild the view with the current scale when the theme changes.
    pub fn typography(mut self, scale: TypographyScale) -> Self {
        self.caption_typography = Some(scale);
        self
    }

    /// Forward caption spacing from the active theme.
    /// `space2` controls both the label/value gap and the caption/control gap
    /// (default `4.0` logical pixels). Invalid values use the default; zero is valid.
    /// Rebuild the view with the current scale when the theme changes.
    pub fn spacing_scale(mut self, scale: SpacingScale) -> Self {
        self.caption_spacing = Some(scale);
        self
    }

    /// Pin paint tokens; without a snapshot, colours follow the runtime palette.
    ///
    /// Outline widths set to `None` still follow the live hairline stroke.
    /// See [`SliderTokens`] for defaults and invalid-value handling.
    pub fn tokens(mut self, tokens: SliderTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Paint the rail itself as a spectrum / translucency ramp.
    ///
    /// See [`TrackPaint`]. Omitting this call leaves the ordinary track+fill
    /// paint untouched.
    pub fn track_paint(mut self, paint: TrackPaint) -> Self {
        self.track_paint = Some(paint);
        self
    }

    /// Assign a stable widget `Id` for programmatic focus
    /// (`focus(id)`) and a stable AT-tree identity. Forwarded to the inner
    /// control (owns focus state, `focusable` registration, semantics node).
    ///
    /// Wins over the label-derived default: set when two sliders share a
    /// label (else one id derived twice). Defaults derive from the LABEL,
    /// never an index — an index shifts on list reorder, routing requests
    /// onto whatever moved into the slot. Strictly worse than no id.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the label-derived default id.
    ///
    /// Two panels each offering a "Volume" slider on one screen would derive the
    /// same id; a distinct prefix per panel keeps both individually routable.
    /// Ignored when [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// The slider's routable identity: an explicit [`Self::id`], else one derived
    /// from the label.
    ///
    /// `None` with neither: the control stays anonymous, publishes no
    /// [`ValueRequest`](crate::value_action::ValueRequest) carrier, advertises
    /// no `Increment`/`Decrement` — but still announces its numeric value,
    /// which reads usefully.
    fn effective_id(&self) -> Option<iced::advanced::widget::Id> {
        self.id.clone().or_else(|| {
            (!self.label.is_empty())
                .then(|| enzui_core::identity::derive_id(&self.id_prefix, &self.label))
        })
    }

    // The rendered and reported value: clamped and snapped, so a caption
    // never shows 150 on 0..=100.
    fn effective_value(&self) -> f32 {
        let (min, max) = sanitize_range(&self.range);
        clamp_and_snap(self.value, min, max, sanitize_step(self.step))
    }

    /// Build the element. Tokens come from the runtime `iced::Theme` unless
    /// explicitly pinned.
    pub fn into_element(self) -> Element<'a, Message> {
        let typography = self.caption_typography.unwrap_or_default();
        let caption_size =
            enzui_core::metrics::FinitePositive::new_or(typography.label_size, caption_size())
                .get();
        let caption_gap = enzui_core::metrics::FiniteNonNeg::new_or(
            self.caption_spacing.unwrap_or_default().space2,
            caption_gap(),
        )
        .get();
        let pinned_tokens = self.tokens;
        let enabled = self.enabled;
        let geo = self.resolved_geometry();
        let along_length = self.vertical_length;
        let (min, max) = sanitize_range(&self.range);
        let step = sanitize_step(self.step);
        let eff_value = self.effective_value();
        // Resolved before `self.on_change` is moved into the control below.
        let effective_id = self.effective_id();

        let resolve = move |theme: &iced::Theme| -> SliderTokens {
            crate::theme_lookup::resolve_pinned_live(
                crate::theme_lookup::PIN_SITE_SLIDER,
                pinned_tokens,
                theme,
                SliderTokens::from_palette,
            )
        };

        // The formatted value is the value AT reads ("1:23", "40 %"), not
        // only the caption's: a bare number names seconds or a fraction.
        let value_text = self.value_format.as_ref().map(|format| format(eff_value));
        let control = SliderControl {
            value: eff_value,
            value_text,
            hover_format: self
                .value_format
                .clone()
                .filter(|_| self.hover_preview && enabled),
            thumb_grow: self.thumb_grow,
            min,
            max,
            step,
            step_marks: self.step_marks,
            geometry: geo,
            along_length,
            on_change: if enabled { Some(self.on_change) } else { None },
            on_drag_start: self.on_drag_start,
            on_release: self.on_release,
            tokens_fn: Box::new(resolve),
            track_paint: self.track_paint,
            label: if self.label.is_empty() {
                None
            } else {
                Some(self.label.clone())
            },
            // Label-derived when the caller supplies no id, so an ordinary
            // labelled slider is routable and the Increment/Decrement it
            // advertises actually reach `on_change` through the `ValueRequest`
            // carrier `operate` publishes.
            id: effective_id,
        };

        // No caption: return the bare control.
        let painted_label = self.label_visible && !self.label.is_empty();
        if !painted_label && !self.show_value {
            return Element::new(control);
        }

        // Vertical captions hug their content beneath the fader to keep bands narrow.
        let vertical = self.orientation.is_vertical();
        let mut caption = iced::widget::row![].spacing(caption_gap);
        if painted_label {
            let lbl = self.label.clone();
            let mut label_text = enz_text(lbl)
                .typography(typography)
                .size_px(caption_size)
                .single_line_elided()
                .color_resolved(move |theme: &iced::Theme| {
                    let tokens = resolve(theme);
                    dim_if(tokens.text_color, enabled, tokens.disabled_alpha)
                });
            if !vertical {
                // Horizontal: the label takes the slack so the value trails.
                // Vertical: nothing may stretch, or the tower gets wide.
                label_text = label_text.width(iced::Fill);
            }
            caption = caption.push(label_text.into_element());
        } else if self.show_value && !vertical {
            // Value-only caption: the `Fill` above rides on the *label*, so
            // with no label nothing stretched and the value hugged the leading
            // edge. Push a stretchable empty leading segment to keep it
            // trailing, exactly as `progress_bar.rs` does for the same shape.
            caption = caption.push(Space::new().width(iced::Fill));
        }
        if self.show_value {
            // Caption uses the *effective* (clamped+snapped) value.
            let value_str = if let Some(ref fmt) = self.value_format {
                fmt(eff_value)
            } else {
                format!("{}", eff_value.round() as i64)
            };
            caption = caption.push(
                enz_text(value_str)
                    .typography(typography)
                    .size_px(caption_size)
                    .tabular_figures(true)
                    .single_line_elided()
                    .color_resolved(move |theme: &iced::Theme| {
                        let tokens = resolve(theme);
                        dim_if(tokens.value_text_color, enabled, tokens.disabled_alpha)
                    })
                    .into_element(),
            );
        }

        if vertical {
            // Fader first, caption underneath, both centred on the tower's
            // cross axis, and the column sized to its content (`Shrink`) so a
            // row of eleven of them stays eleven narrow towers.
            return column![Element::new(control), caption]
                .spacing(caption_gap)
                .align_x(iced::Center)
                .width(Length::Shrink)
                .into();
        }

        column![caption, Element::new(control)]
            .spacing(caption_gap)
            .into()
    }
}

fn caption_size() -> f32 {
    TypographyScale::ambient().label_size
}

fn caption_gap() -> f32 {
    SpacingScale::default().space2
}

fn slider_border_width(value: Option<f32>, theme: &iced::Theme) -> f32 {
    let hairline = enzui_theme::StrokeToken::Hairline.resolve(&crate::theme_lookup::stroke(theme));
    enzui_core::metrics::FiniteNonNeg::new_or(value.unwrap_or(hairline), hairline).get()
}

fn dim_if(c: Color, enabled: bool, alpha: f32) -> Color {
    if enabled {
        c
    } else {
        crate::color::with_opacity(c, crate::color::disabled_alpha(alpha))
    }
}

impl<'a, Message: Clone + 'a> From<EnzSlider<'a, Message>> for Element<'a, Message> {
    fn from(slider: EnzSlider<'a, Message>) -> Self {
        slider.into_element()
    }
}

// ---------------------------------------------------------------------------
// The custom control widget
// ---------------------------------------------------------------------------

// Not `Copy`: `pending_value` carries a `ValueIntent`, whose `Set(String)`
// variant owns a heap payload.
#[derive(Debug, Default)]
struct ControlState {
    focus: FocusState,
    dragging: bool,
    /// The touch that owns the current drag, if the drag came from a finger
    /// rather than the mouse (the `stepper.rs` rule).
    ///
    /// Without an owner, `dragging` matches "a drag is live": a SECOND finger
    /// down mid-drag steals it, and either lift ends both. The owner makes a
    /// press belong to exactly one finger — second ignored, only the owner's
    /// move drives and only its lift ends. `None` = mouse drag (one mouse: no
    /// identity to track).
    dragging_finger: Option<touch::Finger>,
    /// A routed numeric action (`Increment` / `Decrement` / `SetValue`)
    /// is pending for this slider.
    ///
    /// Set by [`AdjustValue`](crate::value_action::AdjustValue) in `operate`
    /// via the published [`ValueRequest`](crate::value_action::ValueRequest)
    /// carrier; taken by `update` next event pass, computing through the
    /// slider's own arithmetic
    /// ([`slider_increment_value`]/[`slider_decrement_value`]/[`clamp_and_snap`])
    /// and publishing `on_change`. Lives in persistent tree state (the widget
    /// rebuilds every `view()`): the latch survives operation→event. The
    /// stepper's `pending_value` / `FocusButton`'s `activate_pending` shape, on
    /// purpose — a routed step is the ArrowRight arithmetic, another trigger.
    pending_value: Option<crate::value_action::ValueIntent>,
    /// The pointer over the track and the value under it, for the hover
    /// bubble.
    hover: Option<(iced::Point, f32)>,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide this slider's ring.
    window_key: u64,
    /// The knob's swell while the pointer is over the slider or dragging it.
    grow: crate::motion::Swell,
    /// The instant of the latest presented frame, which the swell is read at.
    frame_at: Option<iced::time::Instant>,
    /// The thumb's painted position, gliding to a value set by a key, a
    /// track click or the app; a drag moves it directly.
    glide: crate::motion::StateFade<f32>,
}

/// How long the knob takes to swell, in milliseconds.
const GROW_MS: f32 = 140.0;
/// How much larger a fully grown knob is, as the capsule knobs of Apple's
/// current sliders swell under the pointer and while dragged.
const GROW_SCALE: f32 = 0.18;

impl iced::advanced::widget::operation::Focusable for ControlState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
    }
}

struct SliderControl<'a, Message> {
    value: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
    step_marks: bool,
    geometry: SliderGeometry,
    /// Along-axis length in px, used only when the geometry is vertical
    /// (a horizontal slider fills its parent along x).
    along_length: f32,
    /// `None` == disabled: all input is ignored.
    on_drag_start: Option<Message>,
    on_release: Option<Message>,
    on_change: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    tokens_fn: Box<dyn Fn(&iced::Theme) -> SliderTokens + 'a>,
    /// Opt-in rail painting; `None` keeps the flat track + accent fill.
    track_paint: Option<TrackPaint>,
    label: Option<String>,
    /// The caller's formatted value, published as the textual value.
    value_text: Option<String>,
    /// Formats the value under the pointer for the hover bubble, when shown.
    hover_format: Option<std::rc::Rc<dyn Fn(f32) -> String + 'a>>,
    /// Whether the knob swells under the pointer and while dragged.
    thumb_grow: bool,
    id: Option<iced::advanced::widget::Id>,
}

impl<'a, Message> SliderControl<'a, Message> {
    /// The accessibility semantics this slider publishes into the AT tree via
    /// `operate()`, reflecting live focus.
    fn semantics(&self, focused: bool) -> Semantics {
        // A range control: reached via focus, adjusted via
        // Increment/Decrement (advertised from NumericValue) — not a synthetic
        // Click (an unrouted action AT cannot invoke; the same reasoning keeps
        // default actions off text fields). `adjustable` is the CAPABILITY,
        // declared not inferred: a slider routes Increment/Decrement so it
        // declares it; a progress bar carries the contract but routes nothing,
        // so it must not. The axis reads from `self.geometry` — the same value
        // sizing layout, pointer map and paint: a vertical slider cannot
        // announce horizontal without also laying out horizontally (loud, not
        // silent). Set in the contract node in every state (incl. disabled —
        // AT needs a disabled control's axis; AccessKit `set_orientation`).
        Semantics::new(Role::Slider)
            .label_opt(self.label.clone())
            .value_opt(self.value_text.clone())
            .disabled(self.on_change.is_none())
            .focused(focused)
            .adjustable(true)
            .orientation(self.geometry.orientation.to_semantic())
            .numeric(NumericValue {
                value: f64::from(self.finite_value()),
                min: f64::from(self.min),
                max: f64::from(self.max),
                step: self.step.map(f64::from),
            })
    }

    /// The value normalized against the (already sanitized) range, so no
    /// non-finite value can reach the AT tree, the fill fraction or the
    /// renderer coordinates -- even if a caller constructs the control
    /// directly rather than through [`EnzSlider::effective_value`].
    fn finite_value(&self) -> f32 {
        clamp_and_snap(self.value, self.min, self.max, self.step)
    }

    /// Map a pointer position to a clamped+snapped value on whichever axis this
    /// slider uses.
    ///
    /// Horizontal: the plain linear map. Vertical: the same map fraction
    /// INVERTED (minimum at the bottom — top-of-track, small y, is maximum).
    /// That single `1.0 -` is the whole sign flip, in one place: one thing to
    /// get wrong, one thing to test.
    fn value_at(&self, position: iced::Point, bounds: Rectangle) -> f32 {
        let g = self.geometry;
        let along = match g.orientation {
            SliderOrientation::Horizontal => position.x,
            SliderOrientation::Vertical => position.y,
        };
        let half = g.painted_thumb_along(bounds) / 2.0;
        let usable = g.usable_along(bounds);
        let t = if usable > 0.0 {
            (along - g.along_origin_of(bounds) - half) / usable
        } else {
            // A thumb filling its slot has no travel; pointer input uses the slot.
            (along - g.along_origin_of(bounds)) / g.along_of(bounds).max(1.0)
        }
        .clamp(0.0, 1.0);
        let t = if g.orientation.is_vertical() || g.mirrored {
            1.0 - t
        } else {
            t
        };
        value_at_fraction(t, self.min, self.max, self.step)
    }

    /// The `Length` this control takes along its interaction axis.
    ///
    /// Vertical is never `Fill` here: `Fill` height in an unbounded `column`
    /// resolves against the viewport (the recurring "Fill under a bound
    /// defeats the bound"), and eleven viewport-tall equalizer faders are that
    /// defect wearing a feature's clothes.
    fn along_axis_length(&self) -> Length {
        match self.geometry.orientation {
            SliderOrientation::Horizontal => Length::Fill,
            SliderOrientation::Vertical => Length::Fixed(self.along_length.max(MIN_LENGTH)),
        }
    }

    /// Apply a keyboard nav intent to the current value.
    ///
    /// `Increment`/`Decrement` route through [`slider_increment_value`] /
    /// [`slider_decrement_value`]: arrow-key path and a11y-router path run
    /// LITERALLY the same code — "one Increment" lands in one place, not two.
    fn nav_value(&self, nav: Nav) -> f32 {
        let base_step = self
            .step
            .unwrap_or_else(|| implicit_step(self.min, self.max));
        // A page is a *chunk of the range*, floored at one step: a bare
        // `10 * step` overshoots wildly for coarse steps (step 20 on 0..=100
        // would page by 200, five times the whole range).
        let page = base_step.max((range_span(self.min, self.max) / 10.0) as f32);
        let current = self.finite_value();
        match nav {
            Nav::Increment => slider_increment_value(current, self.min, self.max, self.step),
            Nav::Decrement => slider_decrement_value(current, self.min, self.max, self.step),
            Nav::PageUp => offset_and_snap(current, page, self.min, self.max, self.step),
            Nav::PageDown => offset_and_snap(current, -page, self.min, self.max, self.step),
            Nav::Home => self.min,
            Nav::End => self.max,
        }
    }

    fn fraction(&self) -> f32 {
        let span = range_span(self.min, self.max);
        if span.abs() < f64::from(f32::EPSILON) {
            0.0
        } else {
            ((f64::from(self.finite_value()) - f64::from(self.min)) / span).clamp(0.0, 1.0) as f32
        }
    }

    /// The value a routed [`ValueIntent`](crate::value_action::ValueIntent)
    /// produces, or `None` when it produces no change at all.
    ///
    /// Every arm through the keyboard path's helpers
    /// ([`slider_increment_value`], [`slider_decrement_value`],
    /// [`clamp_and_snap`]): a routed step and an ArrowRight are literally one
    /// arithmetic, not two agreeing today. `Set` carries UNVALIDATED AT text
    /// (empty, unparseable, out-of-range): parsed defensively, clamped into
    /// range; unparseable yields `None` (nothing published — never a panic or
    /// an invented value). A no-op reports `None`, not the unchanged value:
    /// incrementing a maxed slider must not redraw + re-emit `on_change` for
    /// a value that never moved.
    fn routed_value(&self, intent: &crate::value_action::ValueIntent) -> Option<f32> {
        use crate::value_action::ValueIntent;
        let current = self.finite_value();
        let next = match intent {
            ValueIntent::Increment => {
                slider_increment_value(current, self.min, self.max, self.step)
            }
            ValueIntent::Decrement => {
                slider_decrement_value(current, self.min, self.max, self.step)
            }
            // Parsed here, not in `value_action`: the widget is the only thing
            // that knows its value is an `f32` and what its range is.
            ValueIntent::Set(text) => {
                let parsed: f32 = text.trim().parse().ok()?;
                if !parsed.is_finite() {
                    return None;
                }
                clamp_and_snap(parsed, self.min, self.max, self.step)
            }
        };
        (next != current).then_some(next)
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for SliderControl<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ControlState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ControlState::default())
    }

    /// Release stale focus at the rebuild, not at the next `operate`.
    ///
    /// See `stepper.rs`'s `diff` for the full argument; slider is the sharpest
    /// case — `state.focus.is_focused()` feeds both the ring and the
    /// `SemanticsNode`, so a leak is simultaneously visible and audible (a
    /// disabled slider kept its ring and announced `focused: true` until some
    /// focus op happened to run). `dragging`/`dragging_finger` deliberately
    /// UNCLEARED: `update` owns the drag lifecycle (drops it on the disabled
    /// path). Focus only.
    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<ControlState>();
        let _ = release_focus_if_inert(&mut state.focus, self.on_change.is_some());
    }

    /// The along axis is `Fill` when horizontal and a fixed length when
    /// vertical; the cross axis is always the fixed hit thickness.
    ///
    /// Vertical is never along-`Fill`: `Fill` height in an unbounded `column`
    /// resolves against the viewport (the recurring "Fill under a bound
    /// defeats the bound") — eleven viewport-tall equalizer faders as a
    /// feature.
    fn size(&self) -> Size<Length> {
        let thickness = Length::Fixed(self.geometry.hit_thickness());
        match self.geometry.orientation {
            SliderOrientation::Horizontal => Size {
                width: self.along_axis_length(),
                height: thickness,
            },
            SliderOrientation::Vertical => Size {
                width: thickness,
                height: self.along_axis_length(),
            },
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let hint = self.size();
        let resolved = limits.resolve(hint.width, hint.height, self.geometry.size_for(MIN_LENGTH));
        // Enforce the 140px minimum interactive extent ALONG the interaction
        // axis (the width only for horizontal); cross pins to the hit
        // thickness so no parent squashes the WCAG floor. The floor CLAMPS to
        // what the parent offers: a bare `.max(MIN_LENGTH)` after
        // `limits.resolve` throws the allotment away — in a 64px region the
        // node reports 140px and `draw` paints all of it, 80px of opaque
        // INTERACTIVE control over its neighbour (measured). Honouring 140 in
        // a 64px box wrongs two controls instead of reaching one target:
        // shrinking is the strictly better failure. UNBOUNDED is not narrow:
        // a non-finite maximum keeps the full floor.
        let available = self.geometry.along_of_size(limits.max());
        let floor = if available.is_finite() {
            MIN_LENGTH.min(available)
        } else {
            MIN_LENGTH
        };
        let along_px = self.geometry.along_of_size(resolved).max(floor);
        layout::Node::new(self.geometry.size_for(along_px)).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<ControlState>();
        // Register in the focus ring so focus_next/prev/focus(id) reach us --
        // but only when enabled. A disabled slider (no `on_change`) must not
        // become a dead Tab stop that traps the user on a control which cannot
        // do anything; and if it still held focus from a previous frame, drop
        // it so Tab can move past. Same policy as `focus_button::widget`'s `operate`.
        if self.on_change.is_some() {
            operation.focusable(self.id.as_ref(), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        // Publish the slider's Slider + NumericValue semantics into the AT tree,
        // reflecting live focus.
        let mut node = SemanticsNode::new(self.semantics(state.focus.is_focused()));
        operation.custom(self.id.as_ref(), layout.bounds(), &mut node);

        // Offer this slider's value to `AdjustValue`, so a routed AccessKit
        // Increment/Decrement/SetValue moves it pointer- and key-free. Without
        // the carrier the request enqueues as `adjust_value(id, intent)`,
        // matches nothing, and AT reports success while the value never moves:
        // publishing here is what reaches the value. Gated on `on_change` (a
        // disabled slider offers nothing, matching the collector advertising
        // nothing on disabled nodes) and on `self.id` (id-less, the collector
        // strips the ad — a carrier would be unreachable anyway). Only OFFERED:
        // one widget arms per request; every other op fails the downcast and
        // ignores it.
        if self.on_change.is_some()
            && let Some(id) = &self.id
        {
            let mut request = crate::value_action::ValueRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if let Some(intent) = request.take() {
                tree.state.downcast_mut::<ControlState>().pending_value = Some(intent);
            }
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        // Keep this window's input modality current from every raw
        // event (a slider may be an app's only focusable), and stash the window
        // key for `draw` so its ring follows only this window's last input.
        // Ambient movement does not commit — only press/scroll/
        // touch — so a keyboard ring survives the cursor passing over.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<ControlState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);
        {
            let state = tree.state.downcast_mut::<ControlState>();
            let active = self.thumb_grow
                && self.on_change.is_some()
                && (state.dragging || cursor.is_over(bounds));
            let dragging = state.dragging;
            let ControlState {
                grow,
                frame_at,
                glide,
                ..
            } = state;
            grow.track(frame_at, event, active, GROW_MS, shell);
            if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
                let fraction = self.fraction();
                if glide.frame(*now, None, dragging, |_: &iced::Theme| fraction) {
                    shell.request_redraw();
                }
            }
        }

        if self.hover_format.is_some()
            && matches!(
                event,
                Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft)
            )
        {
            let hover = cursor
                .position_over(bounds)
                .map(|point| (point, self.value_at(point, bounds)));
            let state = tree.state.downcast_mut::<ControlState>();
            if state.hover != hover {
                state.hover = hover;
                // The bubble is an overlay, rebuilt with each new position.
                shell.invalidate_overlay();
                shell.request_redraw();
            }
        }

        // A routed numeric action armed in `operate`: apply here through the
        // same `routed_value` arithmetic as the arrow keys, publishing
        // `on_change` — a routed Increment is an ArrowRight, never drift.
        // First, above the disabled early-return's `on_change` binding, latch
        // taken unconditionally: armed-while-enabled then rebuilt-disabled
        // DISCARDS (never applies on some later enabled frame). Take-before-
        // publish is the widget half of the once-only guard (no second-event
        // re-publish in-frame). The identical rule/shape as `stepper.rs`.
        {
            let pending = tree
                .state
                .downcast_mut::<ControlState>()
                .pending_value
                .take();
            if let (Some(intent), Some(on_change)) = (pending, self.on_change.as_ref())
                && let Some(v) = self.routed_value(&intent)
            {
                shell.publish(on_change(v));
                shell.request_redraw();
            }
        }

        // Pointer focus is EXCLUSIVE. Runs before the disabled
        // early-return so a disabled slider still drops focus on a press that
        // lands elsewhere, and takes no focus of its own when disabled. This
        // only reads/writes the focus flag; it never touches `dragging`, so an
        // in-flight drag is undisturbed — the drag branches below run exactly
        // as before.
        if let Some(decision) = press_focus_from_cursor(event, cursor, bounds) {
            let enabled = self.on_change.is_some();
            let state = tree.state.downcast_mut::<ControlState>();
            let was_focused = state.focus.is_focused();
            match decision {
                PressFocus::Take if enabled => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }

        let Some(on_change) = &self.on_change else {
            // Disabled: ignore all input -- but a slider disabled *mid-drag*
            // would never see its release event, leaving `dragging` stuck true
            // for the lifetime of the widget state (a phantom drag resumes on
            // re-enable, and the thumb keeps the `thumb_dragging` colour).
            // Clear it first, and repaint so no stale drag frame survives.
            let state = tree.state.downcast_mut::<ControlState>();
            if state.dragging {
                state.dragging = false;
                state.dragging_finger = None;
                if let Some(message) = &self.on_release {
                    shell.publish(message.clone());
                }
                shell.request_redraw();
            }
            return;
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let finger_owned = {
                    let state = tree.state.downcast_ref::<ControlState>();
                    state.dragging && state.dragging_finger.is_some()
                };
                // A finger holding the drag keeps it: the mouse neither
                // steals nor restarts it.
                if cursor.is_over(bounds) && !finger_owned {
                    let pos = cursor.position().unwrap_or_default();
                    let state = tree.state.downcast_mut::<ControlState>();
                    // Focus was already taken above by the exclusive sweep.
                    if !state.dragging
                        && let Some(message) = &self.on_drag_start
                    {
                        shell.publish(message.clone());
                    }
                    state.dragging = true;
                    // The mouse owns no finger identity -- there is only one
                    // mouse, so a release cannot come from "another" one.
                    state.dragging_finger = None;
                    let v = self.value_at(pos, bounds);
                    if v != self.value {
                        shell.publish(on_change(v));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            // Touch carries its own position — use it, not the cursor.
            // The finger's identity is recorded, not discarded. A drag is
            // owned by exactly one finger; a *second* finger touching down while
            // one is already dragging must be ignored rather than stealing
            // ownership, or the first finger's release would end a drag the
            // second finger has been driving.
            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                let handles_event = bounds.contains(*position);
                if handles_event {
                    let state = tree.state.downcast_mut::<ControlState>();
                    if !state.dragging {
                        state.dragging = true;
                        if let Some(message) = &self.on_drag_start {
                            shell.publish(message.clone());
                        }
                        state.dragging_finger = Some(*id);
                        let v = self.value_at(*position, bounds);
                        if v != self.value {
                            shell.publish(on_change(v));
                        }
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let state = tree.state.downcast_ref::<ControlState>();
                // Only a mouse-owned drag follows the cursor.
                if state.dragging && state.dragging_finger.is_none() {
                    let pos = cursor.position().unwrap_or_default();
                    let v = self.value_at(pos, bounds);
                    if v != self.value {
                        shell.publish(on_change(v));
                        shell.request_redraw();
                    }
                    shell.capture_event();
                }
            }
            // Drag uses the finger position, not the cursor.
            // Only the finger that *started* the drag may move it. A
            // second finger's move is ignored while the owner holds the drag.
            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                let state = tree.state.downcast_ref::<ControlState>();
                if state.dragging && state.dragging_finger == Some(*id) {
                    let v = self.value_at(*position, bounds);
                    if v != self.value {
                        shell.publish(on_change(v));
                        shell.request_redraw();
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // A mouse release can only end a drag the mouse started. A drag
                // owned by a finger stays live: the mouse button coming up says
                // nothing about that finger.
                let state = tree.state.downcast_mut::<ControlState>();
                if state.dragging && state.dragging_finger.is_none() {
                    state.dragging = false;
                    if let Some(message) = &self.on_release {
                        shell.publish(message.clone());
                    }
                    shell.request_redraw();
                }
            }
            // Only the finger that *owns* the drag may end it. A
            // non-owning finger lifting (or being lost) says nothing about the
            // owner's gesture, so it must not end the drag.
            Event::Touch(
                touch::Event::FingerLifted { id, .. } | touch::Event::FingerLost { id, .. },
            ) => {
                let state = tree.state.downcast_mut::<ControlState>();
                if state.dragging && state.dragging_finger == Some(*id) {
                    state.dragging = false;
                    state.dragging_finger = None;
                    if let Some(message) = &self.on_release {
                        shell.publish(message.clone());
                    }
                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::Unfocused) => {
                let state = tree.state.downcast_mut::<ControlState>();
                if state.dragging {
                    state.dragging = false;
                    state.dragging_finger = None;
                    if let Some(message) = &self.on_release {
                        shell.publish(message.clone());
                    }
                    shell.request_redraw();
                }
            }
            Event::Keyboard(kb) => {
                let state = tree.state.downcast_ref::<ControlState>();
                if state.focus.is_focused()
                    && let Some(nav) = navigation_from_key_in(
                        kb,
                        if self.geometry.mirrored {
                            enzui_core::placement::Direction::RightToLeft
                        } else {
                            enzui_core::placement::Direction::LeftToRight
                        },
                    )
                {
                    let v = self.nav_value(nav);
                    if v != self.value {
                        shell.publish(on_change(v));
                        shell.request_redraw();
                    }
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<ControlState>();
        let focused = state.focus.is_focused();
        let dragging = state.dragging;
        let hovered = cursor.is_over(bounds);
        let tokens = (self.tokens_fn)(theme);
        let enabled = self.on_change.is_some();
        let opacity = if enabled {
            1.0
        } else {
            crate::color::disabled_alpha(tokens.disabled_alpha)
        };
        let dim = |color| crate::color::with_opacity(color, opacity);
        let track_border_width = slider_border_width(tokens.track_border_width, theme);
        let thumb_border_width = slider_border_width(tokens.thumb_border_width, theme);

        let g = self.geometry;
        let track_t = g.track_thickness;
        // One shared geometry function for both axes, also used by the
        // pointer map's `usable_along`, so "where the thumb is painted" and
        // "what value the cursor resolves to" cannot drift apart.
        let paint = g.paint_rects(bounds, state.glide.paint(theme, self.fraction()));

        if let Some(rail) = &self.track_paint {
            // A gradient owns the whole rail; an accent fill would overwrite its stops.
            if rail.checker {
                paint_checkerboard_with_opacity(
                    renderer,
                    paint.track,
                    track_t / 2.0,
                    rail.checker_tokens,
                    opacity,
                );
            }
            let angle = match g.orientation {
                // 90° runs left→right and 0° runs bottom→top in iced's quad
                // space, which is the direction each axis' value grows.
                SliderOrientation::Horizontal => Degrees(90.0),
                SliderOrientation::Vertical => Degrees(0.0),
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: paint.track,
                    border: Border {
                        color: dim(tokens.track_border),
                        width: track_border_width,
                        radius: (track_t / 2.0).into(),
                    },
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Gradient(rail.gradient(angle, opacity)),
            );
        } else {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: paint.track,
                    border: Border {
                        color: dim(tokens.track_border),
                        width: track_border_width,
                        radius: (track_t / 2.0).into(),
                    },
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Color(dim(tokens.track)),
            );

            // Fill (active portion).
            let fill_color = if hovered || dragging {
                tokens.fill_hover
            } else {
                tokens.fill
            };
            // The fill runs to the thumb's centre, so a disabled (translucent)
            // thumb shows it reaching halfway under, as the range slider does.
            renderer.fill_quad(
                renderer::Quad {
                    bounds: paint.fill,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: (track_t / 2.0).into(),
                    },
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Color(dim(fill_color)),
            );
        }

        let mark_size = enzui_core::metrics::FiniteNonNeg::new_or(tokens.step_mark_size, 2.0).get();
        let mark_spacing =
            enzui_core::metrics::FiniteNonNeg::new_or(tokens.step_mark_min_spacing, 8.0).get();
        if self.step_marks
            && self.track_paint.is_none()
            && mark_size > 0.0
            && let Some(step) = self.step
        {
            let intervals = range_span(self.min, self.max) / f64::from(step);
            // Dense stops merge into a second rail; retain snapping without painting them.
            if (1.0..=64.0).contains(&intervals)
                && g.usable_along(bounds) / intervals as f32 >= mark_spacing
            {
                let painted = state.glide.paint(theme, self.fraction());
                // Centred on the snapped track, across its thickness, so the
                // dots sit on the rail's middle line.
                let track = paint.track;
                for index in 0..=intervals.ceil() as usize {
                    let fraction = (index as f64 / intervals).min(1.0) as f32;
                    let stop = g.paint_rects(bounds, fraction).thumb;
                    let centre = match g.orientation {
                        SliderOrientation::Horizontal => {
                            iced::Point::new(stop.center_x(), track.center_y())
                        }
                        SliderOrientation::Vertical => {
                            iced::Point::new(track.center_x(), stop.center_y())
                        }
                    };
                    let on_fill = fraction <= painted;
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle::new(
                                iced::Point::new(
                                    (centre.x - mark_size / 2.0).round(),
                                    (centre.y - mark_size / 2.0).round(),
                                ),
                                Size::new(mark_size, mark_size),
                            ),
                            border: Border {
                                radius: (mark_size / 2.0).into(),
                                ..Border::default()
                            },
                            ..renderer::Quad::default()
                        },
                        dim(if on_fill {
                            tokens.step_mark_on_fill
                        } else {
                            tokens.step_mark
                        }),
                    );
                }
            }
        }

        // Focus is excluded from disabled opacity; inert controls release focus.
        if enzui_core::focus::focus_ring_visible_for(focused, state.window_key) {
            let pad =
                enzui_core::metrics::FiniteNonNeg::new_or(tokens.focus_ring_outset, 3.0).get();
            crate::focus_button::paint_focus_ring(
                renderer,
                paint.thumb,
                crate::focus_button::FocusRingQuad {
                    ink: tokens.focus_ring,
                    width: enzui_core::metrics::FiniteNonNeg::new_or(tokens.focus_ring_width, 2.0)
                        .get(),
                    radius: (g.thumb_radius(bounds) + pad).into(),
                    outset: pad,
                    snap: true,
                },
            );
        }

        // The capsule knob, swollen by `Grow` around its centre.
        let thumb_color = if dragging {
            tokens.thumb_dragging
        } else {
            tokens.thumb
        };
        let swell = state.grow.paint(state.frame_at, GROW_MS, theme);
        let scale = 1.0 + GROW_SCALE * swell;
        let knob = Rectangle {
            x: paint.thumb.center_x() - paint.thumb.width * scale / 2.0,
            y: paint.thumb.center_y() - paint.thumb.height * scale / 2.0,
            width: paint.thumb.width * scale,
            height: paint.thumb.height * scale,
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: knob,
                border: Border {
                    color: dim(tokens.thumb_border),
                    width: thumb_border_width,
                    radius: (g.thumb_radius(bounds) * scale).into(),
                },
                shadow: Shadow {
                    color: dim(tokens.thumb_shadow),
                    offset: iced::Vector::new(
                        enzui_core::metrics::FiniteF32::new_or(tokens.thumb_shadow_offset.x, 0.0)
                            .get(),
                        enzui_core::metrics::FiniteF32::new_or(tokens.thumb_shadow_offset.y, 2.0)
                            .get(),
                    ),
                    blur_radius: enzui_core::metrics::FiniteNonNeg::new_or(
                        tokens.thumb_shadow_blur,
                        4.0,
                    )
                    .get(),
                },
                snap: true,
            },
            Background::Color(dim(thumb_color)),
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // The ordinary arrow, as GTK and macOS sliders keep it; a grab hand
        // read as "drag the page" over a seek bar.
        mouse::Interaction::default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        _renderer: &iced::Renderer,
        _viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let (Some(format), Some((point, value))) = (
            self.hover_format.as_ref(),
            tree.state.downcast_ref::<ControlState>().hover,
        ) else {
            return Vec::new();
        };
        let bounds = layout.bounds();
        vec![iced::advanced::overlay::Element::new(Box::new(
            HoverBubble {
                text: format(value),
                anchor: iced::Point::new(point.x + translation.x, bounds.y + translation.y),
                window,
            },
        ))]
    }
}

/// The value under the pointer, in a small bubble above a slider's track.
///
/// Draw-only: it takes no input, so the pointer keeps reaching the slider.
struct HoverBubble {
    text: String,
    /// Window position of the pointer's x on the track's top edge.
    anchor: iced::Point,
    window: Size,
}

impl<Message> iced::advanced::Overlay<Message, iced::Theme, iced::Renderer> for HoverBubble {
    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        _cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::text::{Paragraph as _, Renderer as _};
        let palette = crate::theme_lookup::palette(theme);
        let size = enzui_theme::text_scale::ui_px(caption_size());
        fn text<T>(content: T, bounds: Size, size: f32) -> iced::advanced::Text<T> {
            iced::advanced::Text {
                content,
                bounds,
                size: iced::Pixels(size),
                line_height: iced::advanced::text::LineHeight::default(),
                font: iced::Font::DEFAULT.tabular(),
                align_x: iced::advanced::text::Alignment::Left,
                align_y: iced::alignment::Vertical::Top,
                // Tabular figures need the shaper; basic shaping skips features.
                shaping: iced::advanced::text::Shaping::Advanced,
                wrapping: iced::advanced::text::Wrapping::None,
                ellipsis: iced::advanced::text::Ellipsis::None,
                hint_factor: None,
            }
        }
        let measured = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_text(
            text(self.text.as_str(), Size::INFINITE, size),
        )
        .min_bounds();
        let (pad_x, pad_y, gap) = (8.0, 4.0, 6.0);
        let width = measured.width + 2.0 * pad_x;
        let height = measured.height + 2.0 * pad_y;
        let x =
            (self.anchor.x - width / 2.0).clamp(4.0, (self.window.width - width - 4.0).max(4.0));
        let y = (self.anchor.y - height - gap).max(4.0);
        let bubble = Rectangle::new(iced::Point::new(x, y), Size::new(width, height));
        renderer.with_layer(Rectangle::with_size(self.window), |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: bubble,
                    border: iced::Border {
                        radius: 6.0.into(),
                        ..iced::Border::default()
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(palette.surface_elevated),
            );
            renderer.fill_text(
                text(self.text.clone(), measured, size),
                iced::Point::new(x + pad_x, y + pad_y),
                palette.text,
                bubble,
            );
        });
    }

    fn index(&self) -> f32 {
        enzui_core::overlay_layer::OverlayLayer::Popup.z_index()
    }
}

impl<'a, Message> From<SliderControl<'a, Message>> for Element<'a, Message>
where
    Message: 'a + Clone,
{
    fn from(c: SliderControl<'a, Message>) -> Self {
        Self::new(c)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

impl crate::theme_lookup::FollowsThemeOpacity for SliderTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled(),
            ..self
        }
    }
}
