//! Bounded integer or decimal spinbox with minus and plus segments and one focus target.
//!
//! [`enz_stepper`] counts whole numbers (`i32`, `u32`) or decimals (`f32`,
//! `f64`), keeping a decimal value to its shown places and showing them in
//! the locale's digits.
//!
//! The caller owns the value and receives changes only when an action produces
//! a different bounded value. Arrow keys step, PageUp/PageDown move by a tenth
//! of the range or one configured step, and Home/End choose the bounds.
//! Wrapping applies to ordinary steps; page moves clamp.
//!
//! An optional label appears above the control and supplies its default ID.
//! Use `.aria_label(..)` to name a control whose caption is already provided by its row.
//! Use distinct IDs or namespaces for repeated labels. Anonymous controls
//! publish numeric semantics but advertise no routed value actions.
//! A nonblank value-text override changes display and announced text while
//! retaining numeric semantics. The value can be typed in place unless
//! [`EnzStepper::editable`] turns it off; [`EnzStepper::range_error`] marks a
//! typed value outside the range before it is clamped.

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{
    FocusState, Nav, PressFocus, navigation_from_key, press_focus_from_cursor,
    release_focus_if_inert,
};
use enzui_core::semantics::{NumericValue, Role, Semantics};

use crate::value_action::{ValueIntent, ValueRequest};
use enzui_theme::Palette;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer, text};
use iced::widget::{column, text as text_widget};
use iced::{
    Alignment, Background, Border, Color, Element, Event, Length, Point, Rectangle, Shadow, Size,
    mouse, touch, window,
};

/// Geometry presets sharing the text-field family's height and text-size tiers.
/// Applying a size preserves token colours and sets semicircular caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepperSize {
    /// Compact: 26 px height. Maps to [`FieldSizeTokens::SM`](crate::tokens::FieldSizeTokens::SM).
    Sm,
    /// Medium: 32 px height. Maps to [`FieldSizeTokens::MD`](crate::tokens::FieldSizeTokens::MD).
    #[default]
    Md,
    /// Large: 40 px height. Maps to [`FieldSizeTokens::LG`](crate::tokens::FieldSizeTokens::LG).
    Lg,
}

impl StepperSize {
    fn field(self) -> crate::tokens::FieldSizeTokens {
        self.field_tier().tokens()
    }

    pub(crate) fn field_tier(self) -> crate::text_field::TextFieldSize {
        use crate::text_field::TextFieldSize;
        match self {
            Self::Sm => TextFieldSize::Sm,
            Self::Md => TextFieldSize::Md,
            Self::Lg => TextFieldSize::Lg,
        }
    }

    fn apply(self, t: StepperTokens) -> StepperTokens {
        let f = self.field();
        StepperTokens {
            height: f.height,
            body_size: f.text_size,
            glyph_size: f.text_size + 2.0,
            label_size: (f.text_size - 1.0).max(0.0),
            radius: f.height / 2.0,
            button_width: match self {
                Self::Sm => 24.0,
                Self::Md => 28.0,
                Self::Lg => 34.0,
            },
            value_min_width: match self {
                Self::Sm => 40.0,
                Self::Md => 48.0,
                Self::Lg => 56.0,
            },
            ..t
        }
    }
}

/// Colour + geometry tokens for the stepper.
#[derive(Debug, Clone, Copy)]
pub struct StepperTokens {
    /// Control fill (default `field`, the shared field body).
    pub background: Color,
    /// Outer border colour. Under the pill default this is
    /// [`Color::TRANSPARENT`]; a caller can restore an outline by setting it to
    /// a visible colour (e.g. `field_border`).
    pub border: Color,
    /// Inner segment-divider ink; defaults to transparent independently of the border.
    pub divider: Color,
    /// Hover fill for an enabled segment.
    pub segment_hover: Color,
    /// Pressed fill for an enabled segment.
    pub segment_pressed: Color,
    /// Value/label ink colour (default `text`).
    pub text: Color,
    /// Muted ink for the secondary label (default `text_muted`).
    pub muted: Color,
    /// Palette disabled ink retained in the token set.
    /// Control drawing dims `text` and `muted`; the caption uses `disabled_text`.
    pub disabled: Color,
    /// Disabled-state text ink for the label, resolved against the default
    /// page backdrop. See [`crate::color::disabled_text_ink`].
    pub disabled_text: Color,
    /// Keyboard focus-ring colour (default `focus_ring`).
    pub focus_ring: Color,
    /// Edge of a typed value outside the range, with
    /// [`EnzStepper::range_error`] (default `error`).
    pub error: Color,
    /// Highlight behind a typed value about to be replaced (default
    /// `accent_soft`).
    pub selection: Color,
    /// The caret while typing (default `text`).
    pub caret: Color,

    /// Overall control height in px (default 32).
    pub height: f32,
    /// Width of each ± button in px (default 28).
    pub button_width: f32,
    /// Minimum width of the value readout in px (default 48).
    pub value_min_width: f32,
    /// Total horizontal text inset in logical pixels, split equally on both sides.
    /// Defaults to 8, giving 4 px per side.
    pub value_inset: f32,
    /// Corner radius in logical pixels; defaults to 16 for the 32 px pill.
    pub radius: f32,
    /// Outer border width in logical pixels; defaults to 1, edged like a field.
    pub border_width: f32,
    /// Width of the outline drawn in `focus_ring` while the stepper shows
    /// keyboard focus, in px (default 2). Zero hides it.
    pub focus_ring_width: f32,
    /// Width of the inner dividers beside the value, in px (default 1). The
    /// pill default paints them transparent; see `divider`.
    pub divider_width: f32,
    /// Value text size in px (default 13).
    pub body_size: f32,
    /// Secondary-label text size in px (default 12).
    pub label_size: f32,
    /// Minus/plus glyph size in logical pixels; defaults to 15.
    pub glyph_size: f32,
}

impl StepperTokens {
    /// Resolve palette colours: a 32 px pill filled and edged like a text field, with transparent dividers.
    /// Defaults to 28 px segments, a 48 px value-cell floor and 16 px caps.
    /// Body and caption sizes use the shared UI text scale.
    pub fn from_palette(p: &Palette) -> Self {
        let height = 32.0;
        Self {
            background: p.field,
            border: p.field_border,
            divider: Color::TRANSPARENT,
            segment_hover: crate::surface_interaction::composite(p.field, p.overlay_hover()),
            segment_pressed: crate::surface_interaction::composite(p.field, p.overlay_pressed()),
            text: p.text,
            muted: p.text_muted,
            disabled: p.disabled,
            disabled_text: crate::color::disabled_text_ink(p, p.background),
            focus_ring: p.focus_ring,
            error: p.error,
            selection: p.accent_soft,
            caret: p.text,

            height,
            button_width: 28.0,
            value_min_width: 48.0,
            value_inset: 8.0,
            radius: height / 2.0,
            border_width: 1.0,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            divider_width: 1.0,
            body_size: enzui_theme::text_scale::ui_px(13.0),
            label_size: enzui_theme::text_scale::ui_px(12.0),
            glyph_size: 15.0,
        }
    }

    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replace negative or non-finite geometry with fixed defaults; preserve zero and colours.
    /// Fallbacks include 32 px height, 16 px radius and zero border width.
    /// Changing height alone does not recompute radius.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        Self {
            height: FiniteNonNeg::new_or(self.height, 32.0).get(),
            button_width: FiniteNonNeg::new_or(self.button_width, 28.0).get(),
            value_min_width: FiniteNonNeg::new_or(self.value_min_width, 48.0).get(),
            value_inset: FiniteNonNeg::new_or(self.value_inset, 8.0).get(),
            radius: FiniteNonNeg::new_or(self.radius, 16.0).get(),
            border_width: FiniteNonNeg::new_or(self.border_width, 0.0).get(),
            focus_ring_width: FiniteNonNeg::new_or(self.focus_ring_width, 2.0).get(),
            divider_width: FiniteNonNeg::new_or(self.divider_width, 1.0).get(),
            body_size: FiniteNonNeg::new_or(self.body_size, 13.0).get(),
            label_size: FiniteNonNeg::new_or(self.label_size, 12.0).get(),
            glyph_size: FiniteNonNeg::new_or(self.glyph_size, 15.0).get(),
            ..*self
        }
    }
}

/// Return ordered inclusive bounds, swapping a reversed range.
pub fn normalize_bounds(min: i32, max: i32) -> (i32, i32) {
    let r = enzui_core::metrics::OrderedRangeI32::new_sorted(min, max);
    (r.lo(), r.hi())
}

/// Add `delta` in the full i32 domain and resolve against ordered bounds.
/// With wrap enabled, any overshoot jumps to the opposite endpoint, regardless
/// of delta magnitude. Otherwise clamp. The widget disables wrap for page moves.
pub fn stepper_adjust(value: i32, delta: i32, min: i32, max: i32, wrap: bool) -> i32 {
    wide_adjust(
        i64::from(value),
        i64::from(delta),
        i64::from(min),
        i64::from(max),
        wrap,
    ) as i32
}

fn effective_value(value: i32, min: i32, max: i32) -> i32 {
    let (min, max) = normalize_bounds(min, max);
    value.clamp(min, max)
}

/// Parse an assistive-technology value string into the ordered integer range.
/// Trims surrounding whitespace and rounds floating-point values to the nearest
/// integer, with ties away from zero. Out-of-range values and infinities clamp.
/// Empty, unparsable and NaN payloads return `None`; this helper emits no message.
pub fn stepper_set_value(text: &str, min: i32, max: i32) -> Option<i32> {
    Some(effective_value(
        wide_set_value(text, i64::from(min), i64::from(max))? as i32,
        min,
        max,
    ))
}

fn wide_bounds(min: i64, max: i64) -> (i64, i64) {
    (min.min(max), min.max(max))
}

fn wide_adjust(value: i64, delta: i64, min: i64, max: i64, wrap: bool) -> i64 {
    let (min, max) = wide_bounds(min, max);
    let target = value + delta;
    if wrap && target > max {
        min
    } else if wrap && target < min {
        max
    } else {
        target.clamp(min, max)
    }
}

fn wide_set_value(text: &str, min: i64, max: i64) -> Option<i64> {
    let parsed: f64 = text.trim().parse().ok()?;
    if parsed.is_nan() {
        return None;
    }
    let (min, max) = wide_bounds(min, max);
    Some(parsed.round().clamp(min as f64, max as f64) as i64)
}

/// `text` as a value counted in units of `10^-decimals`; a decimal comma is
/// read as a point.
fn scaled_set_value(text: &str, min: i64, max: i64, decimals: u32) -> Option<i64> {
    let (min, max) = wide_bounds(min, max);
    Some(scaled_parse(text, decimals)?.clamp(min, max))
}

/// `text` in units of `10^-decimals`, not yet clamped to a range.
fn scaled_parse(text: &str, decimals: u32) -> Option<i64> {
    let text = text.trim();
    let parsed: f64 = text
        .parse()
        .ok()
        .or_else(|| text.replace(',', ".").parse().ok())?;
    if !parsed.is_finite() {
        return None;
    }
    Some(
        (parsed * scale(decimals))
            .round()
            .clamp(i64::MIN as f64, i64::MAX as f64) as i64,
    )
}

fn scale(decimals: u32) -> f64 {
    10_f64.powi(decimals.min(MAX_DECIMALS).cast_signed())
}

fn unscale(value: i64, decimals: u32) -> f64 {
    value as f64 / scale(decimals)
}

/// `value` units of `10^-decimals`, in the locale's digits with every
/// decimal place shown.
fn format_scaled(value: i64, decimals: u32) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let digits = format!("{:.*}", decimals as usize, unscale(value, decimals));
    digits
        .parse::<enzui_core::localization::Decimal>()
        .ok()
        .and_then(|number| {
            enzui_core::localization::current()
                .locale_for(enzui_core::localization::LocaleCategory::Numeric)
                .format_decimal(&number)
                .ok()
        })
        .unwrap_or(digits)
}

/// The most decimal places a decimal stepper keeps.
pub const MAX_DECIMALS: u32 = 6;

fn page_step(step: i64, min: i64, max: i64) -> i64 {
    let (min, max) = wide_bounds(min, max);
    step.max(((max - min) / 10).min(i64::from(i32::MAX)))
}

mod value_domain {
    /// A stepper value converted to and from whole units of `10^-decimals`.
    pub trait Sealed: Copy {
        /// Whether the type holds fractions, so decimal places apply.
        const FRACTIONAL: bool;
        fn to_units(self, decimals: u32) -> i64;
        fn from_units(units: i64, decimals: u32) -> Self;
        /// The places a step of this size needs to be shown exactly.
        fn places(self) -> u32 {
            0
        }
    }

    impl Sealed for i32 {
        const FRACTIONAL: bool = false;
        fn to_units(self, _decimals: u32) -> i64 {
            i64::from(self)
        }
        fn from_units(units: i64, _decimals: u32) -> Self {
            units.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as Self
        }
    }

    impl Sealed for u32 {
        const FRACTIONAL: bool = false;
        fn to_units(self, _decimals: u32) -> i64 {
            i64::from(self)
        }
        fn from_units(units: i64, _decimals: u32) -> Self {
            units.clamp(0, i64::from(u32::MAX)) as Self
        }
    }

    impl Sealed for f64 {
        const FRACTIONAL: bool = true;
        fn to_units(self, decimals: u32) -> i64 {
            if self.is_finite() {
                (self * super::scale(decimals))
                    .round()
                    .clamp(i64::MIN as f64, i64::MAX as f64) as i64
            } else {
                0
            }
        }
        fn from_units(units: i64, decimals: u32) -> Self {
            super::unscale(units, decimals)
        }
        fn places(self) -> u32 {
            (0..=super::MAX_DECIMALS)
                .find(|&places| {
                    let scaled = self.abs() * super::scale(places);
                    (scaled - scaled.round()).abs() < 1e-6
                })
                .unwrap_or(super::MAX_DECIMALS)
        }
    }

    impl Sealed for f32 {
        const FRACTIONAL: bool = true;
        fn to_units(self, decimals: u32) -> i64 {
            f64::from(self).to_units(decimals)
        }
        fn from_units(units: i64, decimals: u32) -> Self {
            super::unscale(units, decimals) as Self
        }
        fn places(self) -> u32 {
            // Through the shortest decimal text, so 0.1_f32 is one place.
            self.to_string()
                .parse::<f64>()
                .map_or(0, <f64 as Sealed>::places)
        }
    }
}

/// The value types a stepper counts in: `i32`, `u32`, `f32` and `f64`.
/// Sealed so values fit exactly in the control's arithmetic and numeric
/// semantics; floating-point values are kept to their shown decimal places.
pub trait StepperValue: value_domain::Sealed {}
impl StepperValue for i32 {}
impl StepperValue for u32 {}
impl StepperValue for f32 {}
impl StepperValue for f64 {}

/// Bounded spinbox builder for whole or decimal numbers; see [`enz_stepper`].
#[must_use]
pub struct EnzStepper<'a, Message, Value = i32> {
    value: Value,
    min: Option<Value>,
    max: Option<Value>,
    step: Option<Value>,
    decimals: Option<u32>,
    chrome: StepperChrome,
    on_change: Box<dyn Fn(Value) -> Message + 'a>,
}

/// The namespace stepper ids are derived under when no [`EnzStepper::id`] or
/// [`EnzStepper::id_prefix`] is supplied. The key is the stepper's label.
pub const DEFAULT_ID_PREFIX: &str = "enz-stepper";

pub(crate) const VALUE_ELLIPSIS: iced::advanced::text::Ellipsis =
    iced::advanced::text::Ellipsis::End;

/// Build a controlled spinbox over `value`, an `i32`, `u32`, `f32` or `f64`,
/// with an event-time change callback.
///
/// Defaults to range 0 through 100, step 1, enabled, no wrapping or label.
/// A floating-point value shows the decimal places its step needs (a step
/// of `0.05` shows two), or [`EnzStepper::decimals`]; values are kept to
/// those places, so `0.1` stepped ten times is exactly `1.0`. The value can
/// be typed: click it, type, then Enter or click away; Escape cancels. The
/// displayed value clamps to the ordered range; callbacks do not mutate it.
pub fn enz_stepper<'a, Message: Clone + 'a, Value: StepperValue + 'a>(
    value: Value,
    on_change: impl Fn(Value) -> Message + 'a,
) -> EnzStepper<'a, Message, Value> {
    EnzStepper {
        value,
        min: None,
        max: None,
        step: None,
        decimals: None,
        chrome: StepperChrome {
            wrap: false,
            enabled: true,
            editable: true,
            range_error: false,
            label: String::new(),
            label_visible: false,
            value_text: String::new(),
            tokens: None,
            size: None,
            id: None,
            id_prefix: DEFAULT_ID_PREFIX.to_owned(),
            value_width: None,
        },
        on_change: Box::new(on_change),
    }
}

/// Build a controlled unsigned spinbox; the same as [`enz_stepper`] with a
/// `u32` value. Set `.max(u32::MAX)` for the full domain.
pub fn enz_stepper_u32<'a, Message: Clone + 'a>(
    value: u32,
    on_change: impl Fn(u32) -> Message + 'a,
) -> EnzStepper<'a, Message, u32> {
    enz_stepper(value, on_change)
}

impl<'a, Message: Clone + 'a, Value: StepperValue + 'a> EnzStepper<'a, Message, Value> {
    /// Lower bound of the value range (inclusive). Defaults to 0. A range with
    /// `min > max` is swapped rather than panicking.
    pub fn min(mut self, v: Value) -> Self {
        self.min = Some(v);
        self
    }
    /// Upper bound of the value range (inclusive). Defaults to 100.
    pub fn max(mut self, v: Value) -> Self {
        self.max = Some(v);
        self
    }
    /// Increment applied per ± press or arrow key; defaults to 1, and is at
    /// least one unit in the last shown place.
    pub fn step(mut self, v: Value) -> Self {
        self.step = Some(v);
        self
    }
    /// Decimal places a floating-point value shows and keeps, at most
    /// [`MAX_DECIMALS`]. Defaults to the places the step needs. Ignored for
    /// whole-number values.
    pub fn decimals(mut self, places: u8) -> Self {
        self.decimals = Some(u32::from(places).min(MAX_DECIMALS));
        self
    }
    /// Wrap ordinary step actions to the opposite endpoint on overshoot.
    /// Defaults to false. Page moves clamp and Home/End choose their endpoint.
    pub fn wrap(mut self, w: bool) -> Self {
        self.chrome.wrap = w;
        self
    }
    /// Set availability; defaults to true.
    /// Disabled controls publish disabled semantics, leave the focus ring and emit
    /// no value changes. Control colours use theme disabled opacity; the caption
    /// uses contrast-resolved disabled text ink.
    pub fn enabled(mut self, e: bool) -> Self {
        self.chrome.enabled = e;
        self
    }
    /// Whether the value can be typed: clicking it (or typing a digit while
    /// the stepper has focus) edits it in place; Enter or clicking away
    /// commits, clamped to the range, and Escape cancels. Defaults to true.
    pub fn editable(mut self, editable: bool) -> Self {
        self.chrome.editable = editable;
        self
    }
    /// Paint the error style while a typed value lies outside the range
    /// (it is clamped when committed). Defaults to false.
    pub fn range_error(mut self, show: bool) -> Self {
        self.chrome.range_error = show;
        self
    }
    /// Set the caption above the control and its accessible name; defaults to empty.
    /// Without an explicit ID, a nonempty label also determines routed identity.
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.chrome.label = l.into();
        self.chrome.label_visible = true;
        self
    }
    /// Name the spinbox without adding a caption widget, deriving its routed ID from the name.
    /// Like [`Self::label`], this replaces the name; the last label setter controls caption presence.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.chrome.label = label.into();
        self.chrome.label_visible = false;
        self
    }
    /// Override the centred readout and announced value text, retaining numeric semantics.
    /// Empty or whitespace-only text falls back to the clamped value. Nonblank
    /// text keeps its original whitespace; a constrained readout elides at the end.
    pub fn value_text(mut self, v: impl Into<String>) -> Self {
        self.chrome.value_text = v.into();
        self
    }
    /// Pin colour and geometry tokens after sanitizing invalid dimensions.
    /// Unset tokens resolve colours from the runtime theme. [`Self::size`] overrides
    /// its preset geometry fields regardless of setter order.
    pub fn tokens(mut self, t: StepperTokens) -> Self {
        self.chrome.tokens = Some(t.sanitized());
        self
    }
    /// Apply a geometry preset while preserving runtime or pinned colours.
    /// Overrides height, segment width, value-cell floor, radius and text sizes.
    /// Inset, border and focus-ring widths remain from the token set.
    pub fn size(mut self, size: StepperSize) -> Self {
        self.chrome.size = Some(size);
        self
    }
    /// Set routed and programmatic-focus identity, overriding the label-derived ID.
    /// Use distinct IDs for controls sharing a label.
    pub fn id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.chrome.id = Some(id.into());
        self
    }

    /// Namespace label-derived IDs; defaults to [`DEFAULT_ID_PREFIX`].
    /// Ignored when [`Self::id`] is set. Label changes also change derived identity.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.chrome.id_prefix = prefix.into();
        self
    }

    /// Set an additional minimum width for the value cell in logical pixels.
    /// The cell also fits its token floor or measured readout plus inset and 1 px
    /// rounding slack. Parent limits may shrink it. Invalid values leave any previous
    /// override intact; changing this option does not pin colours.
    pub fn value_width(mut self, w: f32) -> Self {
        if w.is_finite() && w >= 0.0 {
            self.chrome.value_width = Some(w);
        }
        self
    }

    /// Build the spinbox, optionally with a caption above it.
    /// The caller rebuilds with a new value after handling the change callback.
    pub fn into_element(self) -> Element<'a, Message> {
        let decimals = if Value::FRACTIONAL {
            self.decimals
                .unwrap_or_else(|| self.step.map_or(0, Value::places))
                .min(MAX_DECIMALS)
        } else {
            0
        };
        let units = |value: Value| value.to_units(decimals);
        let (min, max) = wide_bounds(
            self.min.map_or(0, units),
            self.max.map_or_else(|| 100 * scale(decimals) as i64, units),
        );
        let step = self
            .step
            .map_or_else(|| scale(decimals) as i64, units)
            .max(1);
        let on_change = self.on_change;
        self.chrome.assemble(
            units(self.value).clamp(min, max),
            (min, max),
            step,
            decimals,
            Box::new(move |value| on_change(Value::from_units(value, decimals))),
        )
    }
}

impl<'a, Message: Clone + 'a, Value: StepperValue + 'a> From<EnzStepper<'a, Message, Value>>
    for Element<'a, Message>
{
    fn from(w: EnzStepper<'a, Message, Value>) -> Self {
        w.into_element()
    }
}

/// The settings both steppers share, once their numbers are resolved to the
/// integer domain the control works in.
struct StepperChrome {
    wrap: bool,
    enabled: bool,
    editable: bool,
    range_error: bool,
    label: String,
    label_visible: bool,
    value_text: String,
    tokens: Option<StepperTokens>,
    size: Option<StepperSize>,
    id: Option<iced::advanced::widget::Id>,
    id_prefix: String,
    value_width: Option<f32>,
}

impl StepperChrome {
    fn effective_id(&self) -> Option<iced::advanced::widget::Id> {
        self.id.clone().or_else(|| {
            (!self.label.is_empty())
                .then(|| enzui_core::identity::derive_id(&self.id_prefix, &self.label))
        })
    }

    /// The control over `eff` in `bounds`, counted in units of
    /// `10^-decimals`.
    fn assemble<'a, Message: Clone + 'a>(
        self,
        eff: i64,
        (min, max): (i64, i64),
        step: i64,
        decimals: u32,
        on_change: Box<dyn Fn(i64) -> Message + 'a>,
    ) -> Element<'a, Message> {
        let pinned = self.tokens;
        let size = self.size;
        let geo_pinned = match size {
            Some(s) => Some(s.apply(pinned.unwrap_or_else(StepperTokens::dark))),
            None => pinned,
        };
        let enabled = self.enabled;
        let effective_id = self.effective_id();

        let resolve = move |theme: &iced::Theme| -> StepperTokens {
            let base = pinned.unwrap_or_else(|| {
                crate::theme_lookup::resolve(theme, StepperTokens::from_palette)
            });
            match size {
                Some(s) => s.apply(base),
                None => base,
            }
        };

        let has_override = !self.value_text.trim().is_empty();
        let display = if has_override {
            self.value_text.clone()
        } else {
            format_scaled(eff, decimals)
        };
        let announced_value = has_override.then(|| display.clone());

        let control = SpinBoxControl {
            value: eff,
            min,
            max,
            step,
            decimals,
            wrap: self.wrap,
            editable: self.editable,
            range_error: self.range_error,
            display,
            announced_value,
            label: if self.label.is_empty() {
                None
            } else {
                Some(self.label.clone())
            },
            on_change: enabled.then_some(on_change),
            tokens_fn: Box::new(resolve),
            pinned: geo_pinned,
            id: effective_id,
            value_width: self.value_width,
        };

        if self.label.is_empty() || !self.label_visible {
            Element::new(control)
        } else {
            let lbl = self.label;
            let cap_enabled = enabled;
            let caption_size = geo_pinned.map_or(12.0, |t| t.label_size);
            let caption = text_widget(lbl)
                .size(caption_size)
                .style(move |theme: &iced::Theme| text_widget::Style {
                    color: Some({
                        let t = resolve(theme);
                        if cap_enabled {
                            t.muted
                        } else {
                            t.disabled_text
                        }
                    }),
                });
            column![caption, Element::new(control)]
                .spacing(4)
                .align_x(Alignment::Start)
                .into()
        }
    }
}

fn dim_if(c: Color, enabled: bool, theme: &iced::Theme) -> Color {
    if enabled {
        c
    } else {
        crate::color::with_opacity(c, crate::theme_lookup::opacity(theme).disabled())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Segment {
    #[default]
    None,
    Minus,
    Plus,
}

#[derive(Debug, Clone, Default)]
struct ControlState {
    focus: FocusState,
    /// The value being typed, while editing.
    edit: Option<Edit>,
    pressed: Segment,
    pressed_finger: Option<touch::Finger>,
    pending_value: Option<ValueIntent>,
    hovered_painted: Option<Segment>,
    window_key: u64,
}

/// A value being typed in place.
#[derive(Debug, Clone, Default)]
struct Edit {
    draft: String,
    /// The draft is the old value, shown selected: the next character
    /// replaces it.
    fresh: bool,
}

/// Whether `c` can be part of a typed value.
fn is_value_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ',')
}

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

struct SpinBoxControl<'a, Message> {
    value: i64,
    min: i64,
    max: i64,
    step: i64,
    wrap: bool,
    editable: bool,
    range_error: bool,
    display: String,
    announced_value: Option<String>,
    label: Option<String>,
    /// The value counts units of `10^-decimals`.
    decimals: u32,
    on_change: Option<Box<dyn Fn(i64) -> Message + 'a>>,
    tokens_fn: Box<dyn Fn(&iced::Theme) -> StepperTokens + 'a>,
    pinned: Option<StepperTokens>,
    id: Option<iced::advanced::widget::Id>,
    value_width: Option<f32>,
}

pub(crate) fn measure_value_text(content: &str, size: f32, weight: iced::font::Weight) -> f32 {
    use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
    use iced::advanced::text::Paragraph;
    if content.is_empty() {
        return 0.0;
    }
    GraphicsParagraph::with_text(iced::advanced::text::Text {
        content,
        bounds: Size::new(f32::INFINITY, size.max(1.0)),
        size: size.into(),
        line_height: iced::advanced::text::LineHeight::default(),
        // Tabular figures: the value keeps its width as it steps.
        font: iced::Font {
            weight,
            ..iced::Font::default()
        }
        .tabular(),
        align_x: iced::advanced::text::Alignment::Left,
        align_y: iced::alignment::Vertical::Top,
        shaping: iced::advanced::text::Shaping::Advanced,
        wrapping: iced::advanced::text::Wrapping::None,
        ellipsis: iced::advanced::text::Ellipsis::None,
        hint_factor: None,
    })
    .min_bounds()
    .width
}

impl<'a, Message> SpinBoxControl<'a, Message> {
    fn geometry(&self) -> StepperTokens {
        self.pinned.unwrap_or_else(StepperTokens::dark)
    }

    fn layout_size(&self, limits: &layout::Limits) -> Size {
        let t = self.geometry();
        let measured = measure_value_text(&self.display, t.body_size, iced::font::Weight::Medium);
        let content_w = measured + t.value_inset + 1.0;
        let mut eff_value_w = t.value_min_width.max(content_w);
        if let Some(explicit) = self.value_width {
            eff_value_w = eff_value_w.max(explicit);
        }
        let w = t.button_width * 2.0 + eff_value_w;
        let size = limits.resolve(
            Length::Fixed(w),
            Length::Fixed(t.height),
            Size::new(w, t.height),
        );
        let avail_w = limits.max().width;
        let floor_w = if avail_w.is_finite() {
            w.min(avail_w)
        } else {
            w
        };
        let avail_h = limits.max().height;
        let floor_h = if avail_h.is_finite() {
            t.height.min(avail_h)
        } else {
            t.height
        };
        Size::new(size.width.max(floor_w), size.height.max(floor_h))
    }

    fn can_decrement(&self) -> bool {
        self.wrap || self.value > self.min
    }
    fn can_increment(&self) -> bool {
        self.wrap || self.value < self.max
    }

    fn semantics(&self, focused: bool) -> Semantics {
        // Anonymous controls have no value-request carrier, so advertise no routed actions.
        let routable = self.id.is_some();
        Semantics::new(Role::SpinButton)
            .label_opt(self.label.clone())
            .disabled(self.on_change.is_none())
            .focused(focused)
            .adjustable(routable)
            .editable(routable)
            .value_opt(self.announced_value.clone())
            .numeric(NumericValue {
                value: unscale(self.value, self.decimals),
                min: unscale(self.min, self.decimals),
                max: unscale(self.max, self.decimals),
                step: Some(unscale(self.step, self.decimals)),
            })
    }

    fn finish_press(
        &self,
        tree: &mut Tree,
        bounds: Rectangle,
        t: &StepperTokens,
        position: Option<Point>,
        on_change: &dyn Fn(i64) -> Message,
        shell: &mut Shell<'_, Message>,
    ) {
        let seg = position.map_or(Segment::None, |p| self.segment_at(p, bounds, t));
        let state = tree.state.downcast_mut::<ControlState>();
        let pressed = state.pressed;
        state.pressed = Segment::None;
        state.pressed_finger = None;

        if pressed == seg && pressed != Segment::None {
            let new = match seg {
                Segment::Minus if self.can_decrement() => Some(wide_adjust(
                    self.value, -self.step, self.min, self.max, self.wrap,
                )),
                Segment::Plus if self.can_increment() => Some(wide_adjust(
                    self.value, self.step, self.min, self.max, self.wrap,
                )),
                _ => None,
            };
            if let Some(v) = new
                && v != self.value
            {
                shell.publish(on_change(v));
                shell.capture_event();
            }
        }
        shell.request_redraw();
    }

    /// The value typed so far, clamped to the range, or the current value
    /// when the draft is not a number.
    fn draft_value(&self, edit: &Edit) -> i64 {
        if edit.fresh {
            return self.value;
        }
        scaled_set_value(&edit.draft, self.min, self.max, self.decimals).unwrap_or(self.value)
    }

    /// Whether the draft is a number outside the range.
    fn draft_out_of_range(&self, edit: &Edit) -> bool {
        !edit.fresh
            && scaled_parse(&edit.draft, self.decimals)
                .is_some_and(|value| value < self.min || value > self.max)
    }

    /// Ends editing, publishing the typed value when it changes the value.
    fn commit(
        &self,
        tree: &mut Tree,
        on_change: &dyn Fn(i64) -> Message,
        shell: &mut Shell<'_, Message>,
    ) {
        let state = tree.state.downcast_mut::<ControlState>();
        if let Some(edit) = state.edit.take() {
            let value = self.draft_value(&edit);
            if value != self.value {
                shell.publish(on_change(value));
            }
            shell.request_redraw();
        }
    }

    /// Typing keys: characters of a value, Backspace, Enter and Escape.
    /// Returns whether the key was taken.
    fn edit_key(
        &self,
        tree: &mut Tree,
        event: &iced::keyboard::Event,
        on_change: &dyn Fn(i64) -> Message,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        use iced::keyboard::{Event as Key, key::Named};
        let Key::KeyPressed {
            key,
            text,
            modifiers,
            ..
        } = event
        else {
            return false;
        };
        if !self.editable || modifiers.control() || modifiers.alt() || modifiers.logo() {
            return false;
        }
        let editing = tree.state.downcast_ref::<ControlState>().edit.is_some();
        match key.as_ref() {
            iced::keyboard::Key::Named(Named::Enter) if editing => {
                self.commit(tree, on_change, shell);
                true
            }
            iced::keyboard::Key::Named(Named::Escape) if editing => {
                tree.state.downcast_mut::<ControlState>().edit = None;
                shell.request_redraw();
                true
            }
            iced::keyboard::Key::Named(Named::Backspace) if editing => {
                if let Some(edit) = tree.state.downcast_mut::<ControlState>().edit.as_mut() {
                    if edit.fresh {
                        edit.draft.clear();
                        edit.fresh = false;
                    } else {
                        let _ = edit.draft.pop();
                    }
                }
                shell.request_redraw();
                true
            }
            _ => {
                let typed: String = text
                    .as_deref()
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| is_value_char(*c))
                    .collect();
                if typed.is_empty() {
                    return false;
                }
                let state = tree.state.downcast_mut::<ControlState>();
                let edit = state.edit.get_or_insert_with(Edit::default);
                if edit.fresh {
                    edit.draft.clear();
                    edit.fresh = false;
                }
                edit.draft.push_str(&typed);
                shell.request_redraw();
                true
            }
        }
    }

    /// The value cell between the two segments.
    fn value_rect(&self, bounds: Rectangle, t: &StepperTokens) -> Rectangle {
        let (minus, plus) = self.segment_rects(bounds, t);
        let half_inset = t.value_inset / 2.0;
        let x = minus.x + minus.width + half_inset;
        Rectangle {
            x,
            y: bounds.y,
            width: (plus.x - half_inset - x).max(1.0),
            height: bounds.height,
        }
    }

    fn routed_value(&self, intent: &ValueIntent) -> Option<i64> {
        let v = match intent {
            ValueIntent::Increment if self.can_increment() => {
                wide_adjust(self.value, self.step, self.min, self.max, self.wrap)
            }
            ValueIntent::Decrement if self.can_decrement() => {
                wide_adjust(self.value, -self.step, self.min, self.max, self.wrap)
            }
            ValueIntent::Increment | ValueIntent::Decrement => return None,
            ValueIntent::Set(text) => scaled_set_value(text, self.min, self.max, self.decimals)?,
        };
        (v != self.value).then_some(v)
    }

    fn segment_rects(&self, bounds: Rectangle, t: &StepperTokens) -> (Rectangle, Rectangle) {
        let minus = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: t.button_width,
            height: bounds.height,
        };
        let plus = Rectangle {
            x: bounds.x + bounds.width - t.button_width,
            y: bounds.y,
            width: t.button_width,
            height: bounds.height,
        };
        (minus, plus)
    }

    fn segment_at(&self, pos: Point, bounds: Rectangle, t: &StepperTokens) -> Segment {
        let (minus, plus) = self.segment_rects(bounds, t);
        if minus.contains(pos) {
            Segment::Minus
        } else if plus.contains(pos) {
            Segment::Plus
        } else {
            Segment::None
        }
    }

    fn hover_segment(&self, bounds: Rectangle, cursor: mouse::Cursor) -> Segment {
        if self.on_change.is_none() {
            return Segment::None;
        }
        let t = self.geometry();
        let seg = cursor
            .position()
            .map_or(Segment::None, |p| self.segment_at(p, bounds, &t));
        let available = match seg {
            Segment::Minus => self.can_decrement(),
            Segment::Plus => self.can_increment(),
            Segment::None => false,
        };
        if available { seg } else { Segment::None }
    }

    fn nav_from(&self, base: i64, nav: Nav) -> i64 {
        match nav {
            Nav::Increment => wide_adjust(base, self.step, self.min, self.max, self.wrap),
            Nav::Decrement => wide_adjust(base, -self.step, self.min, self.max, self.wrap),
            Nav::PageUp => wide_adjust(
                base,
                page_step(self.step, self.min, self.max),
                self.min,
                self.max,
                false,
            ),
            Nav::PageDown => wide_adjust(
                base,
                -page_step(self.step, self.min, self.max),
                self.min,
                self.max,
                false,
            ),
            Nav::Home => self.min,
            Nav::End => self.max,
        }
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for SpinBoxControl<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ControlState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ControlState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<ControlState>();
        // Reconcile before paint; an accessibility operation may never run.
        let _ = release_focus_if_inert(&mut state.focus, self.on_change.is_some());
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::Node::new(self.layout_size(limits)).apply(tree);
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
        if self.on_change.is_some() {
            operation.focusable(self.id.as_ref(), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        let mut node = SemanticsNode::new(self.semantics(state.focus.is_focused()));
        operation.custom(self.id.as_ref(), layout.bounds(), &mut node);

        if self.on_change.is_some()
            && let Some(id) = &self.id
        {
            let mut request = ValueRequest::new(id.clone());
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

        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<ControlState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        // Cache survives view rebuilds and tracks the hover state committed on redraw.
        if let Event::Window(window::Event::RedrawRequested(_)) = event {
            let painted = self.hover_segment(bounds, cursor);
            tree.state.downcast_mut::<ControlState>().hovered_painted = Some(painted);
        } else {
            let hovered = self.hover_segment(bounds, cursor);
            let state = tree.state.downcast_ref::<ControlState>();
            if state.hovered_painted.is_some_and(|was| was != hovered) {
                shell.request_redraw();
            }
        }

        // Consume before the disabled return so re-enabling cannot revive a queued action.
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

        // Disabled controls still participate in exclusive pointer-focus release.
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

        // Focus moving away commits what was typed; a disabled control
        // drops it.
        if !tree.state.downcast_ref::<ControlState>().focus.is_focused()
            && tree.state.downcast_ref::<ControlState>().edit.is_some()
        {
            match &self.on_change {
                Some(on_change) => self.commit(tree, on_change.as_ref(), shell),
                None => tree.state.downcast_mut::<ControlState>().edit = None,
            }
        }

        let Some(on_change) = &self.on_change else {
            // Disabled mid-press, the release never arrives here: drop the
            // press so re-enabling cannot complete it.
            let state = tree.state.downcast_mut::<ControlState>();
            state.edit = None;
            if state.pressed != Segment::None {
                state.pressed = Segment::None;
                state.pressed_finger = None;
                shell.request_redraw();
            }
            return;
        };
        let t = self.geometry();

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let handles_event = cursor.is_over(bounds);
                if handles_event {
                    let pos = cursor.position().unwrap_or_default();
                    let seg = self.segment_at(pos, bounds, &t);
                    // A press on a ± segment while typing commits first, so
                    // the step applies to the typed value.
                    if seg != Segment::None
                        && tree.state.downcast_ref::<ControlState>().edit.is_some()
                    {
                        self.commit(tree, on_change.as_ref(), shell);
                    }
                    let state = tree.state.downcast_mut::<ControlState>();
                    state.pressed = seg;
                    state.pressed_finger = None;
                    // A press on the value starts typing it, the old value
                    // selected so the first character replaces it.
                    if seg == Segment::None && self.editable && state.edit.is_none() {
                        state.edit = Some(Edit {
                            draft: format_scaled(self.value, self.decimals),
                            fresh: true,
                        });
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                let handles_event = bounds.contains(*position);
                if handles_event {
                    let seg = self.segment_at(*position, bounds, &t);
                    let state = tree.state.downcast_mut::<ControlState>();
                    if state.pressed == Segment::None {
                        state.pressed = seg;
                        state.pressed_finger = Some(*id);
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let owned_by_finger = tree
                    .state
                    .downcast_ref::<ControlState>()
                    .pressed_finger
                    .is_some();
                if !owned_by_finger {
                    self.finish_press(tree, bounds, &t, cursor.position(), on_change, shell);
                }
            }
            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                let owner = tree.state.downcast_ref::<ControlState>().pressed_finger;
                if owner == Some(*id) {
                    self.finish_press(tree, bounds, &t, Some(*position), on_change, shell);
                }
            }
            Event::Touch(touch::Event::FingerLost { id, .. }) => {
                let state = tree.state.downcast_mut::<ControlState>();
                if state.pressed_finger == Some(*id) {
                    state.pressed = Segment::None;
                    state.pressed_finger = None;
                    shell.request_redraw();
                }
            }
            Event::Keyboard(kb) => {
                let state = tree.state.downcast_ref::<ControlState>();
                if !state.focus.is_focused() {
                    return;
                }
                if self.edit_key(tree, kb, on_change.as_ref(), shell) {
                    shell.capture_event();
                    return;
                }
                let state = tree.state.downcast_ref::<ControlState>();
                if let Some(nav) = navigation_from_key(kb) {
                    // While typing, an arrow steps from the typed value.
                    let base = state
                        .edit
                        .as_ref()
                        .map_or(self.value, |edit| self.draft_value(edit));
                    tree.state.downcast_mut::<ControlState>().edit = None;
                    let v = self.nav_from(base, nav);
                    if v != self.value {
                        shell.publish(on_change(v));
                    }
                    shell.request_redraw();
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
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::text::Renderer as _;
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<ControlState>();
        let focused = state.focus.is_focused();
        let enabled = self.on_change.is_some();
        let ring_visible = enzui_core::focus::focus_ring_visible_for(focused, state.window_key);
        let t = (self.tokens_fn)(theme);
        let g = self.geometry();

        let dim = |c: Color| dim_if(c, enabled, theme);

        let out_of_range = self.range_error
            && enabled
            && state
                .edit
                .as_ref()
                .is_some_and(|edit| self.draft_out_of_range(edit));
        let border_color = if out_of_range {
            t.error
        } else if ring_visible && enabled {
            t.focus_ring
        } else {
            dim(t.border)
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: border_color,
                    width: if out_of_range || (ring_visible && enabled) {
                        g.focus_ring_width.max(g.border_width)
                    } else {
                        g.border_width
                    },
                    radius: g.radius.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            },
            Background::Color(dim(t.background)),
        );

        let (minus_r, plus_r) = self.segment_rects(bounds, &g);
        let hovered_seg = self.hover_segment(bounds, cursor);

        let seg_fill = |seg: Segment| -> Option<Color> {
            let avail = match seg {
                Segment::Minus => self.can_decrement(),
                Segment::Plus => self.can_increment(),
                Segment::None => false,
            };
            if !enabled || !avail {
                return None;
            }
            if state.pressed == seg {
                Some(t.segment_pressed)
            } else if hovered_seg == seg {
                Some(t.segment_hover)
            } else {
                None
            }
        };
        // Segment fills share outer caps; square outer corners would overpaint the pill.
        let radius = iced::border::Radius::default();
        for (seg, r, corner) in [
            (
                Segment::Minus,
                minus_r,
                radius.top_left(g.radius).bottom_left(g.radius),
            ),
            (
                Segment::Plus,
                plus_r,
                radius.top_right(g.radius).bottom_right(g.radius),
            ),
        ] {
            if let Some(c) = seg_fill(seg) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: r,
                        border: Border {
                            radius: corner,
                            ..Border::default()
                        },
                        shadow: Shadow::default(),
                        snap: true,
                    },
                    Background::Color(c),
                );
            }
        }

        let divider = dim(t.divider);
        for x in [minus_r.x + minus_r.width, plus_r.x] {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: x - g.divider_width / 2.0,
                        y: bounds.y,
                        width: g.divider_width,
                        height: bounds.height,
                    },
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Color(divider),
            );
        }

        for (content, r, avail) in [
            ("\u{2212}", minus_r, self.can_decrement()),
            ("+", plus_r, self.can_increment()),
        ] {
            let color = if enabled && avail {
                dim(t.text)
            } else {
                dim(t.muted)
            };
            let txt = text::Text {
                content: content.to_owned(),
                bounds: Size::new(r.width, r.height),
                size: g.glyph_size.into(),
                line_height: text::LineHeight::Absolute(r.height.into()),
                font: iced::Font::default(),
                align_x: text::Alignment::Center,
                align_y: iced::alignment::Vertical::Center,
                shaping: text::Shaping::Advanced,
                wrapping: text::Wrapping::None,
                ellipsis: text::Ellipsis::None,
                hint_factor: None,
            };
            let pos = Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0);
            let clip = r.intersection(viewport).unwrap_or(r);
            renderer.fill_text(txt, pos, color, clip);
        }

        // Split the inset so centred text stays clear of both segment fills.
        let value_rect = self.value_rect(bounds, &g);
        let value_w = value_rect.width;
        let editing = state.edit.as_ref().filter(|_| enabled);
        let content = editing.map_or_else(|| self.display.clone(), |edit| edit.draft.clone());
        if let Some(edit) = editing {
            // The old value selected, or a caret after what is typed.
            let width =
                measure_value_text(&content, g.body_size, iced::font::Weight::Medium).min(value_w);
            let centre = value_rect.center_x();
            let line = g.body_size * 1.3;
            let top = bounds.center_y() - line / 2.0;
            let mark = if edit.fresh {
                Rectangle::new(
                    Point::new(centre - width / 2.0 - 1.0, top),
                    Size::new(width + 2.0, line),
                )
            } else {
                Rectangle::new(
                    Point::new((centre + width / 2.0 + 0.5).round(), top),
                    Size::new(1.0, line),
                )
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: mark,
                    border: Border::default().rounded(if edit.fresh { 3.0 } else { 0.0 }),
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Color(if edit.fresh { t.selection } else { t.caret }),
            );
        }
        let value_txt = text::Text {
            content,
            bounds: Size::new(value_w, bounds.height),
            size: g.body_size.into(),
            line_height: text::LineHeight::Absolute(bounds.height.into()),
            font: iced::Font {
                weight: iced::font::Weight::Medium,
                ..iced::Font::default()
            }
            .tabular(),
            align_x: text::Alignment::Center,
            align_y: iced::alignment::Vertical::Center,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
            ellipsis: VALUE_ELLIPSIS,
            hint_factor: None,
        };
        let value_pos = Point::new(
            value_rect.x + value_w / 2.0,
            value_rect.y + bounds.height / 2.0,
        );
        let value_clip = value_rect.intersection(viewport).unwrap_or(value_rect);
        let ink = if out_of_range { t.error } else { dim(t.text) };
        renderer.fill_text(value_txt, value_pos, ink, value_clip);
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        if self.on_change.is_some()
            && self.editable
            && cursor.is_over(self.value_rect(bounds, &self.geometry()))
        {
            mouse::Interaction::Text
        } else if self.on_change.is_some() && cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message> From<SpinBoxControl<'a, Message>> for Element<'a, Message>
where
    Message: 'a + Clone,
{
    fn from(c: SpinBoxControl<'a, Message>) -> Self {
        Element::new(c)
    }
}
