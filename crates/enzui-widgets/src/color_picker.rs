//! `EnzColorPicker`: a continuous colour picker — hue strip, a pointer- and
//! keyboard-driven saturation/brightness plane, an optional alpha strip, a live
//! swatch and a precise hex field.
//!
//! # Niche in the catalogue
//!
//! The catalogue's other colour affordance is
//! [`color_swatch_grid`](crate::color_swatch_grid): a *palette inventory* that
//! chooses from a fixed set of authored swatches, and `iced` ships no
//! picker at all. `color picker`, `colour picker`, `color wheel` and
//! `eyedropper` are this widget's search terms; the grid answers only the
//! fixed-palette question. Without a continuous editor, an app whose whole job
//! is editing colours (a theme editor) leaves the user typing `#rrggbbaa` by
//! hand.
//!
//! # The working value is `Hsva`, not `Color`
//!
//! The picker is driven by [`Hsva`] and publishes [`Hsva`]. That is deliberate:
//! RGB cannot represent the *working* state of a colour editor. Drag the plane
//! to saturation 0 and the RGB result is a grey that has forgotten its hue, so
//! a `Color`-driven picker snaps the hue strip back to red the instant the user
//! reaches the left edge — the single most reported defect in hand-rolled
//! pickers. Callers convert at their own boundary with [`Hsva::from_color`] /
//! [`Hsva::to_color`], and keep the `Hsva` as their edit state.
//!
//! # No silent drift
//!
//! [`Hsva::to_color`] quantises to 8 bits, the precision `#rrggbb[aa]`
//! carries, so what the swatch paints is exactly what a hex round-trip
//! preserves: `Hsva::from_color(c).to_color() == c` for every 8-bit colour
//! (gated by `color_picker::hsva_round_trips_every_8_bit_colour`). Alpha
//! rides through the same channel and is only *shown* when the caller opts in
//! with [`EnzColorPicker::alpha`].
//!
//! # Keyboard and assistive technology
//!
//! Every continuous axis is keyboard operable:
//!
//! * the plane (saturation ×  brightness) is a focus stop that answers
//!   Left/Right (saturation), Up/Down (brightness), PageUp/PageDown (coarse
//!   steps) and Home/End (that axis' extremes), with a visible focus ring;
//!   Shift-only variants remain plane navigation, while Ctrl-, Alt-, and
//!   Logo-modified variants are left for application shortcuts;
//! * hue and alpha are shipped [`crate::slider`]s, so they carry the slider
//!   role, `NumericValue` and the framework's routed `Increment`/`Decrement`;
//! * the hex field is an ordinary editable text field, which is the precise
//!   route both for a person who knows the value and for assistive technology.
//!
//! The plane publishes `Role::Slider` with a spoken value naming *both* axes
//! ("Saturation 45%, brightness 90%"). It declares `adjustable(false)`: a
//! routed AccessKit `Increment` has no unambiguous meaning on a two-axis
//! control, and advertising an action nothing can route is the unrouted-action
//! defect this repo has filed repeatedly. The hue slider and the hex field are
//! the routable adjustments.

pub use crate::slider::CheckerboardTokens;

use crate::enz_text::{TextSize, enz_text};
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, PressFocus, press_focus_from_cursor, release_focus_if_inert};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::SpaceToken;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::keyboard::key::Named;
use iced::{
    Background, Border, Color, Degrees, Element, Event, Gradient, Length, Rectangle, Shadow, Size,
    keyboard, mouse, touch,
};

/// Default height of the saturation/brightness plane, in logical px.
pub const DEFAULT_PLANE_HEIGHT: f32 = 148.0;

/// Smallest plane a parent may squeeze the picker into before it stops
/// shrinking. Below this the 2D gesture stops being usable at all.
pub const MIN_PLANE_SIZE: f32 = 72.0;

/// Radius of the plane's draggable handle ring, in logical px.
///
/// 11 px gives a 22 px ring plus its 2 px stroke — a 24 px touch target, the
/// framework's global minimum hit target, on a control whose *whole* box is
/// also pressable.
pub const HANDLE_RADIUS: f32 = 11.0;

/// One arrow-key step on the plane's 0..=1 axes (1%).
pub const PLANE_STEP: f32 = 0.01;

/// One PageUp/PageDown step on the plane's 0..=1 axes (10%).
pub const PLANE_PAGE: f32 = 0.10;

/// One arrow-key step on the hue strip, in degrees.
pub const HUE_STEP: f32 = 1.0;

/// Track thickness of the hue and alpha rails, in logical px.
///
/// Thicker than a plain slider's 8 px: these rails carry a *spectrum*, and 8 px
/// of a 360° ramp is too thin to read a hue off.
pub const HUE_TRACK_HEIGHT: f32 = 14.0;

/// Default gap between the plane handle and its focus ring, in logical px.
pub const HANDLE_FOCUS_OUTSET: f32 = 3.0;

/// The smallest handle radius that still fits its two contrast rings.
const MIN_HANDLE_RADIUS: f32 = 4.0;

/// Fixed dimensions of the colour picker's parts, in logical pixels.
///
/// The plane's height has its own setter, [`EnzColorPicker::plane_height`].
/// The handle's two contrast rings (a 1 px dark ring outside a 2 px white
/// one) are not here: they keep the handle visible on both the white and
/// the black corner of the plane, whatever the theme.
///
/// Invalid values (`NaN`, infinities, and negative or zero values where a
/// positive size is required) use the default. A handle radius below 4 is
/// raised to 4 so both rings still fit. A zero focus width hides the ring;
/// a zero outset draws it on the handle's edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorPickerSizeTokens {
    /// Plane handle radius; default [`HANDLE_RADIUS`] (11).
    pub handle_radius: f32,
    /// Plane handle focus ring width; default the theme's `stroke.focus_ring`.
    pub handle_focus_width: f32,
    /// Gap between the handle and its focus ring; default
    /// [`HANDLE_FOCUS_OUTSET`] (3).
    pub handle_focus_outset: f32,
    /// Track thickness of the hue and alpha rails; default
    /// [`HUE_TRACK_HEIGHT`] (14).
    pub track_height: f32,
    /// Live swatch width; default [`SWATCH_WIDTH`] (44).
    pub swatch_width: f32,
    /// Live swatch height; default [`SWATCH_HEIGHT`] (28).
    pub swatch_height: f32,
}

impl Default for ColorPickerSizeTokens {
    fn default() -> Self {
        Self {
            handle_radius: HANDLE_RADIUS,
            handle_focus_width: enzui_theme::text_defaults::focus_ring_width(),
            handle_focus_outset: HANDLE_FOCUS_OUTSET,
            track_height: HUE_TRACK_HEIGHT,
            swatch_width: SWATCH_WIDTH,
            swatch_height: SWATCH_HEIGHT,
        }
    }
}

impl ColorPickerSizeTokens {
    /// Replace each invalid field with its default, as documented on the type.
    #[must_use]
    pub fn sanitized(self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        Self {
            handle_radius: FinitePositive::new_or(self.handle_radius, HANDLE_RADIUS)
                .get()
                .max(MIN_HANDLE_RADIUS),
            handle_focus_width: FiniteNonNeg::new_or(
                self.handle_focus_width,
                enzui_theme::text_defaults::focus_ring_width(),
            )
            .get(),
            handle_focus_outset: FiniteNonNeg::new_or(
                self.handle_focus_outset,
                HANDLE_FOCUS_OUTSET,
            )
            .get(),
            track_height: FinitePositive::new_or(self.track_height, HUE_TRACK_HEIGHT).get(),
            swatch_width: FinitePositive::new_or(self.swatch_width, SWATCH_WIDTH).get(),
            swatch_height: FinitePositive::new_or(self.swatch_height, SWATCH_HEIGHT).get(),
        }
    }
}

/// The six primaries of the hue wheel plus the wrap back to red — the stops
/// that make the hue rail a real continuous spectrum.
///
/// Seven stops, inside iced's eight-stop linear-gradient limit, so none is
/// silently dropped.
#[must_use]
pub fn hue_spectrum_stops() -> Vec<(f32, Color)> {
    (0..=6u8)
        .map(|sixth| {
            let hue = f32::from(sixth) * 60.0;
            (
                f32::from(sixth) / 6.0,
                Hsva::new(hue, 1.0, 1.0).to_color_unquantised(),
            )
        })
        .collect()
}

/// The alpha rail's stops: this colour fully transparent, then fully opaque.
#[must_use]
pub fn alpha_ramp_stops(value: Hsva) -> Vec<(f32, Color)> {
    let opaque = Hsva {
        alpha: 1.0,
        ..value.sanitised()
    }
    .to_color_unquantised();
    vec![(0.0, Color { a: 0.0, ..opaque }), (1.0, opaque)]
}

/// The on-screen numeric readout of every axis the picker edits.
#[must_use]
pub fn axis_readout(value: Hsva, with_alpha: bool) -> String {
    let v = value.sanitised();
    let pct = |unit: f32| (unit * 100.0).round() as i32;
    let base = enzui_core::localized!("enzui-color-picker-h-s-b", "hue" => v.hue.round() as i32, "saturation" => pct(v.saturation), "brightness" => pct(v.value));
    if with_alpha {
        enzui_core::localized!("enzui-color-picker-a", "base" => base, "alpha" => pct(v.alpha))
    } else {
        base
    }
}

/// The default widget-id prefix for the picker's parts.
pub const DEFAULT_ID_PREFIX: &str = "enz-color-picker";

// ---------------------------------------------------------------------------
// Hsva
// ---------------------------------------------------------------------------

/// A colour in the picker's working space: hue (degrees), saturation, value
/// (brightness) and alpha, each of the last three in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsva {
    /// Hue in degrees, `0.0..360.0`.
    pub hue: f32,
    /// Saturation, `0.0..=1.0`.
    pub saturation: f32,
    /// Value / brightness, `0.0..=1.0`.
    pub value: f32,
    /// Alpha, `0.0..=1.0`.
    pub alpha: f32,
}

impl Default for Hsva {
    fn default() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            value: 0.0,
            alpha: 1.0,
        }
    }
}

impl Hsva {
    /// A fully opaque colour from its three axes.
    #[must_use]
    pub fn new(hue: f32, saturation: f32, value: f32) -> Self {
        Self {
            hue,
            saturation,
            value,
            alpha: 1.0,
        }
    }

    /// Clamp every axis into range, mapping non-finite input to a usable value
    /// rather than letting it reach layout, paint or the AT bus.
    #[must_use]
    pub fn sanitised(self) -> Self {
        let unit = |v: f32, fallback: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                fallback
            }
        };
        let hue = if self.hue.is_finite() {
            self.hue.rem_euclid(360.0)
        } else {
            0.0
        };
        Self {
            hue,
            saturation: unit(self.saturation, 0.0),
            value: unit(self.value, 0.0),
            alpha: unit(self.alpha, 1.0),
        }
    }

    /// Derive the working axes from an RGBA colour.
    ///
    /// An achromatic colour has no recoverable hue; `0.0` is reported. That is
    /// exactly why the picker is *driven* by `Hsva` — see the module docs.
    #[must_use]
    pub fn from_color(color: Color) -> Self {
        let Color { r, g, b, a } = color;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;
        let hue = if chroma <= 0.0 {
            0.0
        } else if (max - r).abs() < f32::EPSILON {
            60.0 * (((g - b) / chroma) % 6.0)
        } else if (max - g).abs() < f32::EPSILON {
            60.0 * ((b - r) / chroma + 2.0)
        } else {
            60.0 * ((r - g) / chroma + 4.0)
        };
        Self {
            hue: hue.rem_euclid(360.0),
            saturation: if max <= 0.0 { 0.0 } else { chroma / max },
            value: max,
            alpha: a,
        }
        .sanitised()
    }

    /// The RGBA colour these axes denote, quantised to 8 bits per channel
    /// so a `#rrggbb[aa]` round trip is exact (see the module docs).
    pub fn to_color(self) -> Color {
        quantise8(self.to_color_unquantised())
    }

    /// The unquantised conversion, used for paint where the extra precision
    /// costs nothing and banding is undesirable.
    pub fn to_color_unquantised(self) -> Color {
        let Self {
            hue,
            saturation,
            value,
            alpha,
        } = self.sanitised();
        let c = value * saturation;
        let h = hue / 60.0;
        let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = value - c;
        Color {
            r: r + m,
            g: g + m,
            b: b + m,
            a: alpha,
        }
    }

    /// The opaque colour of this hue at full saturation and brightness — what
    /// the plane's horizontal gradient runs to and the hue strip paints.
    pub fn pure_hue(self) -> Color {
        Self {
            hue: self.sanitised().hue,
            saturation: 1.0,
            value: 1.0,
            alpha: 1.0,
        }
        .to_color_unquantised()
    }

    /// The `#rrggbb` / `#rrggbbaa` text for this colour, lower case.
    ///
    /// The alpha suffix appears only when the colour is not fully opaque, which
    /// is the same rule `enzui_core::theme_file::registry::format_color`
    /// applies — so a theme value edited through the picker serialises to the
    /// text it already had.
    #[must_use]
    pub fn to_hex(self) -> String {
        let c = self.to_color();
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let (r, g, b, a) = (q(c.r), q(c.g), q(c.b), q(c.a));
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

/// Round every channel to the 8-bit precision hex text carries.
pub fn quantise8(color: Color) -> Color {
    let q = |v: f32| f32::from((v.clamp(0.0, 1.0) * 255.0).round() as u8) / 255.0;
    Color {
        r: q(color.r),
        g: q(color.g),
        b: q(color.b),
        a: q(color.a),
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// A continuous colour picker. Build it with [`enz_color_picker`].
pub struct EnzColorPicker<'a, Message> {
    value: Hsva,
    on_change: Box<dyn Fn(Hsva) -> Message + 'a>,
    on_hex_input: Option<Box<dyn Fn(String) -> Message + 'a>>,
    hex_text: Option<String>,
    hex_error: Option<String>,
    label: Option<String>,
    show_alpha: bool,
    checker_tokens: CheckerboardTokens,
    geometry: ColorPickerSizeTokens,
    plane_height: f32,
    enabled: bool,
    id_prefix: String,
}

/// Construct a continuous colour picker over an [`Hsva`] working value.
pub fn enz_color_picker<'a, Message: Clone + 'a>(
    value: Hsva,
    on_change: impl Fn(Hsva) -> Message + 'a,
) -> EnzColorPicker<'a, Message> {
    EnzColorPicker {
        value: value.sanitised(),
        on_change: Box::new(on_change),
        on_hex_input: None,
        hex_text: None,
        hex_error: None,
        label: None,
        show_alpha: false,
        checker_tokens: CheckerboardTokens::default(),
        geometry: ColorPickerSizeTokens::default(),
        plane_height: DEFAULT_PLANE_HEIGHT,
        enabled: true,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
    }
}

impl<'a, Message: Clone + 'a> EnzColorPicker<'a, Message> {
    /// The accessible name every part derives from ("Accent" → "Accent hue").
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Show the alpha strip. Off by default: a token whose format carries no
    /// alpha must not grow a control that edits one.
    #[must_use]
    pub const fn alpha(mut self, show: bool) -> Self {
        self.show_alpha = show;
        self
    }

    /// Pin the shared transparency backdrop for the alpha rail and live swatch.
    /// It is visible only when [`Self::alpha`] is enabled.
    #[must_use]
    pub fn checker_tokens(mut self, tokens: CheckerboardTokens) -> Self {
        self.checker_tokens = tokens.sanitized();
        self
    }

    /// Pin the picker's fixed dimensions: the plane handle, its focus ring,
    /// the rails and the swatch.
    ///
    /// Colours continue to follow the runtime theme. See
    /// [`ColorPickerSizeTokens`] for units, defaults and invalid-value handling.
    #[must_use]
    pub fn size_tokens(mut self, tokens: ColorPickerSizeTokens) -> Self {
        self.geometry = tokens.sanitized();
        self
    }

    /// Override the hex field's text with the caller's in-progress draft, so a
    /// half-typed or unparseable value survives a rebuild.
    #[must_use]
    pub fn hex_text(mut self, text: impl Into<String>) -> Self {
        self.hex_text = Some(text.into());
        self
    }

    /// The validation message shown under the hex field.
    #[must_use]
    pub fn hex_error(mut self, error: impl Into<String>) -> Self {
        self.hex_error = Some(error.into());
        self
    }

    /// Route hex edits. Without this the field is present but read-only, so a
    /// caller cannot accidentally ship a text box that silently discards input.
    #[must_use]
    pub fn on_hex_input(mut self, on_input: impl Fn(String) -> Message + 'a) -> Self {
        self.on_hex_input = Some(Box::new(on_input));
        self
    }

    /// The plane's height in logical px (default [`DEFAULT_PLANE_HEIGHT`]).
    ///
    /// Finite positive values are clamped to at least [`MIN_PLANE_SIZE`].
    /// `NaN`, either infinity, zero, and negative values restore the default,
    /// so malformed caller input cannot collapse or poison the interactive
    /// plane's layout.
    #[must_use]
    pub fn plane_height(mut self, height: f32) -> Self {
        self.plane_height =
            enzui_core::metrics::FinitePositive::new_or(height, DEFAULT_PLANE_HEIGHT).get();
        self
    }

    /// Disable every control in the picker.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The id prefix each part derives its widget id from.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// The widget id of the saturation/brightness plane under `prefix`.
    #[must_use]
    pub fn plane_id_for(prefix: &str) -> iced::advanced::widget::Id {
        iced::advanced::widget::Id::from(format!("{prefix}-plane"))
    }

    /// The widget id of the hex field under `prefix`, so two pickers with
    /// distinct prefixes never publish the same field id.
    #[must_use]
    pub fn hex_id_for(prefix: &str) -> iced::advanced::widget::Id {
        iced::advanced::widget::Id::from(format!("{prefix}-hex"))
    }

    fn part_label(&self, plain: &str, named: &str) -> String {
        match &self.label {
            Some(label) => enzui_core::localized!(named, "label" => label.as_str()),
            None => enzui_core::localization::text(plain),
        }
    }

    /// Compose the picker.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let value = self.value.sanitised();
        let plane_label =
            self.part_label("enzui-color-picker-plane", "enzui-color-picker-plane-named");
        let hue_label = self.part_label("enzui-color-picker-hue", "enzui-color-picker-hue-named");
        let alpha_label =
            self.part_label("enzui-color-picker-alpha", "enzui-color-picker-alpha-named");
        let swatch_label = self.part_label(
            "enzui-color-picker-preview",
            "enzui-color-picker-preview-named",
        );
        let hex_label = self.part_label("enzui-color-picker-hex", "enzui-color-picker-hex-named");

        let on_change: std::rc::Rc<dyn Fn(Hsva) -> Message + 'a> = self.on_change.into();

        let geometry = self.geometry;
        let plane = SaturationValuePlane {
            value,
            geometry,
            height: self.plane_height,
            label: plane_label,
            id: Self::plane_id_for(&self.id_prefix),
            on_change: self.enabled.then(|| {
                let f = std::rc::Rc::clone(&on_change);
                Box::new(move |v: Hsva| f(v)) as Box<dyn Fn(Hsva) -> Message + 'a>
            }),
        };

        let hue = {
            let f = std::rc::Rc::clone(&on_change);
            crate::slider::enz_slider(0.0..=360.0, value.hue, move |hue| f(Hsva { hue, ..value }))
                .step(HUE_STEP)
                .label(&hue_label)
                .value_format(|hue| format!("{}°", hue.round() as i32))
                .show_value(true)
                .enabled(self.enabled)
                // The axis has to *be* the spectrum: a default rail paints one
                // accent fill, leaving "hue" a number the user has to guess at.
                // This paints the real continuous wheel across the rail.
                .track_paint(crate::slider::TrackPaint::spectrum(hue_spectrum_stops()))
                .track_height(geometry.track_height)
                .id(iced::advanced::widget::Id::from(format!(
                    "{}-hue",
                    self.id_prefix
                )))
                .into_element()
        };

        let mut rows: Vec<Element<'a, Message>> = vec![plane.into(), hue];

        if self.show_alpha {
            let f = std::rc::Rc::clone(&on_change);
            rows.push(
                crate::slider::enz_slider(0.0..=1.0, value.alpha, move |alpha| {
                    f(Hsva { alpha, ..value })
                })
                .step(1.0 / 255.0)
                .label(&alpha_label)
                .value_format(|alpha| format!("{}%", (alpha * 100.0).round() as i32))
                .show_value(true)
                .enabled(self.enabled)
                .track_paint(
                    crate::slider::TrackPaint::translucent(alpha_ramp_stops(value))
                        .checker_tokens(self.checker_tokens),
                )
                .track_height(geometry.track_height)
                .id(iced::advanced::widget::Id::from(format!(
                    "{}-alpha",
                    self.id_prefix
                )))
                .into_element(),
            );
        }

        // The live swatch. It is a terminal semantics leaf, not a bare styled
        // container: the meaningful datum of a swatch is the colour, and AT can
        // only speak text — the same rule `color_swatch_grid` states for its
        // inventory cells (that widget itself cannot serve here: its
        // constructor pins `'static`, which would force every host message
        // type to be `'static`).
        let hex_now = value.to_hex();
        let painted = value.to_color();
        let swatch: Element<'a, Message> = crate::semantics_container::SemanticsContainer::leaf(
            Element::new(Swatch {
                color: painted,
                checker: self.show_alpha,
                checker_tokens: self.checker_tokens,
                width: geometry.swatch_width,
                height: geometry.swatch_height,
            }),
            Semantics::new(Role::Image)
                .label(swatch_label)
                .description(hex_now.clone()),
        )
        .into();

        let hex_value = self.hex_text.unwrap_or(hex_now);
        let mut field = match self.on_hex_input {
            Some(on_input) => {
                crate::text_field::enz_text_field_owned(hex_label, "#rrggbb", hex_value, on_input)
            }
            None => crate::text_field::enz_text_field_owned(
                hex_label,
                "#rrggbb",
                hex_value,
                |_: String| unreachable!("a read-only field publishes no input"),
            )
            .read_only(true),
        }
        .id(Self::hex_id_for(&self.id_prefix))
        .enabled(self.enabled);
        if let Some(error) = self.hex_error {
            field = field.error(error);
        }

        rows.push(
            // Aligned to the field's box, not to its label above it: the
            // swatch is the field's preview and shares its height and line.
            crate::enz_hstack(vec![swatch, field.into_element()])
                .hug_content(false)
                .gap(SpaceToken::Sm)
                .cross(crate::CrossAlign::End)
                .into_element(),
        );

        // The numbers the axes hold, on screen. A picker whose
        // only visible value is the hex text cannot be checked against what a
        // drag did to saturation or brightness — both collapse into the same
        // six characters.
        rows.push(
            enz_text(axis_readout(value, self.show_alpha))
                .size(TextSize::Footnote)
                .into_element(),
        );

        crate::enz_vstack(rows).gap(SpaceToken::Sm).into_element()
    }
}

impl<'a, Message: Clone + 'a> From<EnzColorPicker<'a, Message>> for Element<'a, Message> {
    fn from(picker: EnzColorPicker<'a, Message>) -> Self {
        picker.into_element()
    }
}

// ---------------------------------------------------------------------------
// The saturation/brightness plane
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct PlaneState {
    focus: FocusState,
    dragging: bool,
    dragging_finger: Option<touch::Finger>,
    window_key: u64,
}

impl iced::advanced::widget::operation::Focusable for PlaneState {
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

/// A keyboard intent on the two-axis plane, in axis deltas.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlaneNav {
    saturation: f32,
    value: f32,
    absolute: bool,
}

#[derive(Debug, Clone, Copy)]
struct PlaneNavClaim {
    key: Named,
    nav: PlaneNav,
}

/// The complete keyboard surface owned by the saturation/value plane.
///
/// Both decoding and the modifier matrix consume this table, so adding a new
/// plane key automatically adds all sixteen modifier classes to the proof.
const PLANE_NAV_CLAIMS: &[PlaneNavClaim] = &[
    PlaneNavClaim {
        key: Named::ArrowRight,
        nav: PlaneNav {
            saturation: PLANE_STEP,
            value: 0.0,
            absolute: false,
        },
    },
    PlaneNavClaim {
        key: Named::ArrowLeft,
        nav: PlaneNav {
            saturation: -PLANE_STEP,
            value: 0.0,
            absolute: false,
        },
    },
    PlaneNavClaim {
        key: Named::ArrowUp,
        nav: PlaneNav {
            saturation: 0.0,
            value: PLANE_STEP,
            absolute: false,
        },
    },
    PlaneNavClaim {
        key: Named::ArrowDown,
        nav: PlaneNav {
            saturation: 0.0,
            value: -PLANE_STEP,
            absolute: false,
        },
    },
    PlaneNavClaim {
        key: Named::PageUp,
        nav: PlaneNav {
            saturation: 0.0,
            value: PLANE_PAGE,
            absolute: false,
        },
    },
    PlaneNavClaim {
        key: Named::PageDown,
        nav: PlaneNav {
            saturation: 0.0,
            value: -PLANE_PAGE,
            absolute: false,
        },
    },
    PlaneNavClaim {
        key: Named::Home,
        nav: PlaneNav {
            saturation: 0.0,
            value: 0.0,
            absolute: true,
        },
    },
    PlaneNavClaim {
        key: Named::End,
        nav: PlaneNav {
            saturation: 1.0,
            value: 1.0,
            absolute: true,
        },
    },
];

/// Decode a key press into a plane movement.
///
/// Deliberately not `enzui_core::focus::navigation_from_key`: that
/// helper collapses Up and Right onto one `Increment`, which is right for a
/// one-axis control and wrong here — the two axes must move independently or
/// the keyboard cannot reach most of the plane.
fn plane_nav_from_key(event: &keyboard::Event) -> Option<PlaneNav> {
    let keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.intersects(
        keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT | keyboard::Modifiers::LOGO,
    ) {
        return None;
    }
    PLANE_NAV_CLAIMS
        .iter()
        .find(|claim| claim.key == *named)
        .map(|claim| claim.nav)
}

struct SaturationValuePlane<'a, Message> {
    value: Hsva,
    geometry: ColorPickerSizeTokens,
    height: f32,
    label: String,
    id: iced::advanced::widget::Id,
    /// `None` == disabled: all input is ignored.
    on_change: Option<Box<dyn Fn(Hsva) -> Message + 'a>>,
}

impl<Message> SaturationValuePlane<'_, Message> {
    /// The spoken value: both axes, because both are what this control edits.
    fn semantics(&self, focused: bool) -> Semantics {
        let v = self.value.sanitised();
        Semantics::new(Role::Slider)
            .label(self.label.clone())
            .value(enzui_core::localized!("enzui-color-picker-saturation-brightness", "saturation" => (v.saturation * 100.0).round() as i32, "brightness" => (v.value * 100.0).round() as i32))
            .description(
                "Left and right adjust saturation; up and down adjust brightness. \
                 The hex field sets an exact value.",
            )
            .adjustable(false)
            .disabled(self.on_change.is_none())
            .focused(focused)
    }

    /// Map a pointer position inside `bounds` onto the two axes.
    fn value_at(&self, position: iced::Point, bounds: Rectangle) -> Hsva {
        let w = bounds.width.max(1.0);
        let h = bounds.height.max(1.0);
        let saturation = ((position.x - bounds.x) / w).clamp(0.0, 1.0);
        // Brightness runs bottom-to-top: the top edge is the bright one, which
        // is what the vertical black overlay paints.
        let value = 1.0 - ((position.y - bounds.y) / h).clamp(0.0, 1.0);
        Hsva {
            saturation,
            value,
            ..self.value.sanitised()
        }
    }

    /// Apply a keyboard intent.
    fn nav_value(&self, nav: PlaneNav) -> Hsva {
        let current = self.value.sanitised();
        if nav.absolute {
            return Hsva {
                saturation: nav.saturation,
                value: nav.value,
                ..current
            };
        }
        Hsva {
            saturation: (current.saturation + nav.saturation).clamp(0.0, 1.0),
            value: (current.value + nav.value).clamp(0.0, 1.0),
            ..current
        }
    }

    /// Where the handle's centre sits for the current value.
    fn handle_centre(&self, bounds: Rectangle) -> iced::Point {
        let v = self.value.sanitised();
        iced::Point::new(
            bounds.x + v.saturation * bounds.width,
            bounds.y + (1.0 - v.value) * bounds.height,
        )
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SaturationValuePlane<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PlaneState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(PlaneState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<PlaneState>();
        let _ = release_focus_if_inert(&mut state.focus, self.on_change.is_some());
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fixed(self.height.max(MIN_PLANE_SIZE)),
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let hint = self.size();
        let resolved = limits.resolve(
            hint.width,
            hint.height,
            Size::new(MIN_PLANE_SIZE, self.height.max(MIN_PLANE_SIZE)),
        );
        // A narrow parent wins: painting a plane wider than the box the parent
        // granted is this repo's recurring "minimum under a bounded region
        // defeats the bound" defect, and an opaque interactive surface
        // overlapping its neighbour is strictly worse than a small one.
        let width = resolved.width.max(1.0);
        let height = resolved.height.max(1.0);
        layout::Node::new(Size::new(width, height)).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<PlaneState>();
        if self.on_change.is_some() {
            operation.focusable(Some(&self.id), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        let mut node = SemanticsNode::new(self.semantics(state.focus.is_focused()));
        operation.custom(Some(&self.id), layout.bounds(), &mut node);
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
        tree.state.downcast_mut::<PlaneState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        if let Some(decision) = press_focus_from_cursor(event, cursor, bounds) {
            let enabled = self.on_change.is_some();
            let state = tree.state.downcast_mut::<PlaneState>();
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
            let state = tree.state.downcast_mut::<PlaneState>();
            if state.dragging {
                state.dragging = false;
                state.dragging_finger = None;
                shell.request_redraw();
            }
            return;
        };

        let publish = |shell: &mut Shell<'_, Message>, next: Hsva| {
            if next != self.value.sanitised() {
                shell.publish(on_change(next));
                shell.request_redraw();
            }
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let handles_event = cursor.is_over(bounds);
                if handles_event {
                    let position = cursor.position().unwrap_or_default();
                    let state = tree.state.downcast_mut::<PlaneState>();
                    state.dragging = true;
                    state.dragging_finger = None;
                    publish(shell, self.value_at(position, bounds));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                let handles_event = bounds.contains(*position);
                if handles_event {
                    let state = tree.state.downcast_mut::<PlaneState>();
                    if !state.dragging {
                        state.dragging = true;
                        state.dragging_finger = Some(*id);
                        publish(shell, self.value_at(*position, bounds));
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let handles_event = tree.state.downcast_ref::<PlaneState>().dragging;
                if handles_event {
                    let position = cursor.position().unwrap_or_default();
                    publish(shell, self.value_at(position, bounds));
                    shell.capture_event();
                }
            }
            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                let state = tree.state.downcast_ref::<PlaneState>();
                if state.dragging && state.dragging_finger == Some(*id) {
                    publish(shell, self.value_at(*position, bounds));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let state = tree.state.downcast_mut::<PlaneState>();
                if state.dragging && state.dragging_finger.is_none() {
                    state.dragging = false;
                    shell.request_redraw();
                }
            }
            Event::Touch(
                touch::Event::FingerLifted { id, .. } | touch::Event::FingerLost { id, .. },
            ) => {
                let state = tree.state.downcast_mut::<PlaneState>();
                if state.dragging && state.dragging_finger == Some(*id) {
                    state.dragging = false;
                    state.dragging_finger = None;
                    shell.request_redraw();
                }
            }
            Event::Keyboard(key_event) => {
                let focused = tree.state.downcast_ref::<PlaneState>().focus.is_focused();
                if focused && let Some(nav) = plane_nav_from_key(key_event) {
                    publish(shell, self.nav_value(nav));
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
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        let state = tree.state.downcast_ref::<PlaneState>();
        let palette = crate::theme_lookup::palette(theme);
        let radius = crate::theme_lookup::radius(theme).sm;

        // Saturation: white on the left, the pure hue on the right.
        // `Degrees(90)` runs the gradient left→right in iced's quad space.
        let saturation = Gradient::Linear(
            iced::gradient::Linear::new(Degrees(90.0))
                .add_stop(0.0, Color::WHITE)
                .add_stop(1.0, self.value.pure_hue()),
        );
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
            Background::Gradient(saturation),
        );

        // Brightness: transparent at the top, black at the bottom.
        let brightness = Gradient::Linear(
            iced::gradient::Linear::new(Degrees(180.0))
                .add_stop(0.0, Color::TRANSPARENT)
                .add_stop(1.0, Color::BLACK),
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: palette.border,
                    width: enzui_theme::StrokeToken::Hairline
                        .resolve(&crate::theme_lookup::stroke(theme)),
                    radius: radius.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            },
            Background::Gradient(brightness),
        );

        // The handle: a ring whose interior shows the chosen colour, so it is
        // legible on both a white and a black corner of the plane.
        let centre = self.handle_centre(bounds);
        let radius = self.geometry.handle_radius;
        let handle = Rectangle {
            x: centre.x - radius,
            y: centre.y - radius,
            width: radius * 2.0,
            height: radius * 2.0,
        };
        if enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key) {
            let pad = self.geometry.handle_focus_outset;
            crate::focus_button::paint_focus_ring(
                renderer,
                handle,
                crate::focus_button::FocusRingQuad {
                    ink: palette.focus_ring,
                    width: self.geometry.handle_focus_width,
                    radius: (radius + pad).into(),
                    outset: pad,
                    snap: false,
                },
            );
        }
        // Outer dark ring, then the white ring, then the fill: two rings mean
        // the handle keeps a visible edge against both extremes of the plane.
        renderer.fill_quad(
            renderer::Quad {
                bounds: handle,
                border: Border {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                    width: 1.0,
                    radius: radius.into(),
                },
                shadow: Shadow::default(),
                snap: false,
            },
            Background::Color(Color::TRANSPARENT),
        );
        let inner = Rectangle {
            x: handle.x + 1.0,
            y: handle.y + 1.0,
            width: handle.width - 2.0,
            height: handle.height - 2.0,
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: inner,
                border: Border {
                    color: Color::WHITE,
                    width: 2.0,
                    radius: (radius - 1.0).into(),
                },
                shadow: Shadow::default(),
                snap: false,
            },
            Background::Color(self.value.to_color()),
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if self.on_change.is_some() && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message> From<SaturationValuePlane<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(plane: SaturationValuePlane<'a, Message>) -> Self {
        Self::new(plane)
    }
}

// ---------------------------------------------------------------------------
// The live swatch
// ---------------------------------------------------------------------------

/// Width of the picker's live swatch, in logical px.
pub const SWATCH_WIDTH: f32 = 44.0;

/// Height of the picker's live swatch, in logical px.
pub const SWATCH_HEIGHT: f32 = 28.0;

// Alpha previews need a stable transparency backdrop independent of the host surface.
#[derive(Debug, Clone, Copy)]
struct Swatch {
    color: Color,
    checker: bool,
    checker_tokens: CheckerboardTokens,
    width: f32,
    height: f32,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Swatch {
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fixed(self.width),
            height: Length::Fixed(self.height),
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::Node::new(limits.resolve(
            Length::Fixed(self.width),
            Length::Fixed(self.height),
            Size::new(self.width, self.height),
        ))
        .apply(tree);
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        // The hex field's corners, so swatch and field read as one pair.
        let radius = crate::text_field::TextFieldSize::Md.resolve_radius(theme, None);
        if self.checker {
            crate::slider::paint_checkerboard_with_tokens(
                renderer,
                bounds,
                radius,
                self.checker_tokens,
            );
        }
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: crate::theme_lookup::palette(theme).border,
                    width: enzui_theme::StrokeToken::Hairline
                        .resolve(&crate::theme_lookup::stroke(theme)),
                    radius: radius.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            },
            Background::Color(self.color),
        );
    }
}
