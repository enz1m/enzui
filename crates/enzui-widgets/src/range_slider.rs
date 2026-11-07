//! A two-thumb slider choosing a range: a price band, a time window, a
//! frequency cut (GTK `GtkRange` with two marks, Qt Quick `RangeSlider`).
//!
//! The application owns the pair and receives the new pair on every change.
//! The thumbs never cross: each moves between the range's end and the other
//! thumb. Each thumb is its own Tab stop and its own `Slider` node for
//! assistive technology, named `<label> minimum` and `<label> maximum`.
//! Arrows step (Right and Up increase; mirrored in right-to-left text),
//! Page Up and Page Down move a tenth of the range, Home and End go to the
//! thumb's limits. A press on the track moves the nearer thumb there and
//! drags it.
//!
//! Colours and stepping are the [`slider`](crate::slider)'s: the same tokens,
//! snapping and default step.

use crate::slider::{
    SliderTokens, clamp_and_snap, implicit_step, slider_decrement_value, slider_increment_value,
};
use crate::value_action::{ValueIntent, ValueRequest};
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, Nav, navigation_from_key_in};
use enzui_core::placement::{Direction, resolve_direction};
use enzui_core::semantics::{NumericValue, Orientation, Role, Semantics};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::{
    Background, Border, Color, Element, Event, Length, Point, Rectangle, Shadow, Size, Vector,
    mouse,
};
use std::ops::RangeInclusive;
use std::rc::Rc;

/// The thumb's extent along the track: the same 28 x 18 capsule as
/// [`crate::slider`], so the two read as one family.
pub const THUMB_WIDTH: f32 = 28.0;

/// The thumb's height across the track.
pub const THUMB_HEIGHT: f32 = 18.0;

/// The track thickness, as the single slider's.
pub const TRACK_THICKNESS: f32 = 6.0;

/// The narrowest the control lays out.
const MIN_LENGTH: f32 = 140.0;

/// Builder for [`enz_range_slider`].
#[must_use]
pub struct EnzRangeSlider<'a, Message> {
    range: RangeInclusive<f32>,
    values: (f32, f32),
    on_change: OnChange<'a, Message>,
    step: Option<f32>,
    label: Option<String>,
    value_format: Option<Rc<dyn Fn(f32) -> String + 'a>>,
    direction: Option<Direction>,
    enabled: bool,
    tokens: Option<SliderTokens>,
    id: Option<String>,
}

/// A range slider over `range` showing `values` (low, high); `on_change`
/// receives the new pair.
pub fn enz_range_slider<'a, Message: Clone + 'a>(
    range: RangeInclusive<f32>,
    values: (f32, f32),
    on_change: impl Fn((f32, f32)) -> Message + 'a,
) -> EnzRangeSlider<'a, Message> {
    EnzRangeSlider {
        range,
        values,
        on_change: Box::new(on_change),
        step: None,
        label: None,
        value_format: None,
        direction: None,
        enabled: true,
        tokens: None,
        id: None,
    }
}

impl<'a, Message: Clone + 'a> EnzRangeSlider<'a, Message> {
    /// Snap both thumbs to multiples of `step` from the range's start; by
    /// default arrows move 1% of the range without snapping.
    pub fn step(mut self, step: f32) -> Self {
        self.step = (step.is_finite() && step > 0.0).then_some(step);
        self
    }

    /// What the range is of, for assistive technology ("Price").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// How a value reads ("€40"); assistive technology announces it.
    pub fn value_format(mut self, format: impl Fn(f32) -> String + 'a) -> Self {
        self.value_format = Some(Rc::new(format));
        self
    }

    /// The writing direction; by default the locale's. In right-to-left text
    /// the minimum is at the right.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Set availability; a disabled slider dims, leaves the Tab order and
    /// publishes no changes.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Pin the colours.
    pub fn tokens(mut self, tokens: SliderTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// A stable identity: the thumbs are `<id>-low` and `<id>-high`, for
    /// programmatic focus and routed accessibility actions. By default it
    /// derives from the label.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Build the control.
    pub fn into_element(self) -> Element<'a, Message> {
        let (min, max) = enzui_core::metrics::OrderedRangeF32::new_sorted(
            *self.range.start(),
            *self.range.end(),
        )
        .map_or((0.0, 1.0), |range| (range.lo(), range.hi()));
        let low = clamp_and_snap(self.values.0, min, max, self.step);
        let high = clamp_and_snap(self.values.1, min, max, self.step);
        let (low, high) = if low <= high {
            (low, high)
        } else {
            (high, low)
        };
        let base = self.id.clone().or_else(|| {
            self.label
                .as_ref()
                .map(|label| format!("enz-range-slider-{label}"))
        });
        let ids = base.map(|base| {
            [
                Id::from(format!("{base}-low")),
                Id::from(format!("{base}-high")),
            ]
        });
        let pinned = self.tokens;
        Element::new(RangeSliderControl {
            values: [low, high],
            min,
            max,
            step: self.step,
            on_change: self.enabled.then_some(self.on_change),
            label: self.label,
            value_format: self.value_format,
            mirrored: resolve_direction(self.direction) == Direction::RightToLeft,
            tokens: Box::new(move |theme: &iced::Theme| {
                pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, SliderTokens::from_palette)
                })
            }),
            ids,
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzRangeSlider<'a, Message>> for Element<'a, Message> {
    fn from(slider: EnzRangeSlider<'a, Message>) -> Self {
        slider.into_element()
    }
}

/// Which thumb: `0` the low one, `1` the high one.
type Thumb = usize;

/// Receives the new (low, high) pair.
type OnChange<'a, Message> = Box<dyn Fn((f32, f32)) -> Message + 'a>;

#[derive(Debug, Default)]
struct State {
    focus: [FocusState; 2],
    /// The thumb a drag holds, and the value it last published.
    dragging: Option<(Thumb, f32)>,
    pending: [Option<ValueIntent>; 2],
    window_key: u64,
    /// The thumbs' painted fractions, gliding to values set by a key or the
    /// app; a drag moves them directly.
    glide: crate::motion::StateFade<(f32, f32)>,
}

struct RangeSliderControl<'a, Message> {
    values: [f32; 2],
    min: f32,
    max: f32,
    step: Option<f32>,
    on_change: Option<OnChange<'a, Message>>,
    label: Option<String>,
    value_format: Option<Rc<dyn Fn(f32) -> String + 'a>>,
    mirrored: bool,
    tokens: Box<dyn Fn(&iced::Theme) -> SliderTokens + 'a>,
    ids: Option<[Id; 2]>,
}

impl<Message> RangeSliderControl<'_, Message> {
    fn fraction(&self, value: f32) -> f32 {
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        ((value - self.min) / span).clamp(0.0, 1.0)
    }

    fn thumb_center_x(&self, bounds: Rectangle, thumb: Thumb) -> f32 {
        self.center_x_at(bounds, self.fraction(self.values[thumb]))
    }

    /// The fractions both thumbs are at.
    fn fractions(&self) -> (f32, f32) {
        (self.fraction(self.values[0]), self.fraction(self.values[1]))
    }

    /// Where a thumb at `fraction` of the range is centred.
    fn center_x_at(&self, bounds: Rectangle, fraction: f32) -> f32 {
        let usable = (bounds.width - THUMB_WIDTH).max(0.0);
        let fraction = if self.mirrored {
            1.0 - fraction
        } else {
            fraction
        };
        bounds.x + THUMB_WIDTH / 2.0 + fraction * usable
    }

    fn thumb_rect(&self, bounds: Rectangle, thumb: Thumb) -> Rectangle {
        Self::rect_at(bounds, self.thumb_center_x(bounds, thumb))
    }

    /// The thumb centred at `x`.
    fn rect_at(bounds: Rectangle, x: f32) -> Rectangle {
        Rectangle::new(
            Point::new(
                x - THUMB_WIDTH / 2.0,
                bounds.center_y() - THUMB_HEIGHT / 2.0,
            ),
            Size::new(THUMB_WIDTH, THUMB_HEIGHT),
        )
    }

    fn value_at(&self, bounds: Rectangle, x: f32) -> f32 {
        let usable = (bounds.width - THUMB_WIDTH).max(f32::EPSILON);
        let fraction = ((x - bounds.x - THUMB_WIDTH / 2.0) / usable).clamp(0.0, 1.0);
        let fraction = if self.mirrored {
            1.0 - fraction
        } else {
            fraction
        };
        clamp_and_snap(
            self.min + fraction * (self.max - self.min),
            self.min,
            self.max,
            self.step,
        )
    }

    /// The thumb a press at `x` takes: the nearer one; on a tie, the one on
    /// the side the press is on.
    fn nearest(&self, bounds: Rectangle, x: f32) -> Thumb {
        let low = self.thumb_center_x(bounds, 0);
        let high = self.thumb_center_x(bounds, 1);
        let (to_low, to_high) = ((x - low).abs(), (x - high).abs());
        if (to_low - to_high).abs() < 0.5 {
            let toward_max = if self.mirrored { x < low } else { x > low };
            usize::from(toward_max)
        } else {
            usize::from(to_high < to_low)
        }
    }

    /// The thumb's limits: the range's end and the other thumb.
    fn limits(&self, thumb: Thumb) -> (f32, f32) {
        if thumb == 0 {
            (self.min, self.values[1])
        } else {
            (self.values[0], self.max)
        }
    }

    fn moved(&self, thumb: Thumb, nav: Nav) -> f32 {
        let value = self.values[thumb];
        let (low, high) = self.limits(thumb);
        let page = (self.max - self.min) / 10.0;
        let next = match nav {
            Nav::Increment => slider_increment_value(value, self.min, self.max, self.step),
            Nav::Decrement => slider_decrement_value(value, self.min, self.max, self.step),
            Nav::PageUp => clamp_and_snap(value + page, self.min, self.max, self.step),
            Nav::PageDown => clamp_and_snap(value - page, self.min, self.max, self.step),
            Nav::Home => low,
            Nav::End => high,
        };
        next.clamp(low, high)
    }

    fn routed(&self, thumb: Thumb, intent: &ValueIntent) -> Option<f32> {
        let (low, high) = self.limits(thumb);
        let value = match intent {
            ValueIntent::Increment => self.moved(thumb, Nav::Increment),
            ValueIntent::Decrement => self.moved(thumb, Nav::Decrement),
            ValueIntent::Set(text) => {
                let parsed: f32 = text.trim().parse().ok()?;
                if !parsed.is_finite() {
                    return None;
                }
                clamp_and_snap(parsed, self.min, self.max, self.step).clamp(low, high)
            }
        };
        Some(value)
    }

    /// Publish the pair with `thumb` at `value`, when that changes it.
    fn publish(&self, shell: &mut Shell<'_, Message>, thumb: Thumb, value: f32) {
        let Some(on_change) = &self.on_change else {
            return;
        };
        if value == self.values[thumb] {
            return;
        }
        let mut values = self.values;
        values[thumb] = value;
        shell.publish(on_change((values[0], values[1])));
    }

    fn semantics(&self, thumb: Thumb, focused: bool) -> Semantics {
        let (low, high) = self.limits(thumb);
        let name = match (&self.label, thumb) {
            (Some(label), 0) => {
                enzui_core::localized!("enzui-range-slider-low", "label" => label.as_str())
            }
            (Some(label), _) => {
                enzui_core::localized!("enzui-range-slider-high", "label" => label.as_str())
            }
            (None, 0) => enzui_core::localization::text("enzui-range-slider-low-unnamed"),
            (None, _) => enzui_core::localization::text("enzui-range-slider-high-unnamed"),
        };
        let routable = self.ids.is_some() && self.on_change.is_some();
        Semantics::new(Role::Slider)
            .label(name)
            .orientation(Orientation::Horizontal)
            .focused(focused)
            .disabled(self.on_change.is_none())
            .adjustable(routable)
            .value_opt(
                self.value_format
                    .as_ref()
                    .map(|format| format(self.values[thumb])),
            )
            .numeric(NumericValue {
                value: f64::from(self.values[thumb]),
                min: f64::from(low),
                max: f64::from(high),
                step: Some(f64::from(
                    self.step
                        .unwrap_or_else(|| implicit_step(self.min, self.max)),
                )),
            })
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for RangeSliderControl<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let height = THUMB_HEIGHT.max(24.0);
        let size = limits
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .resolve(Length::Fill, height, Size::new(MIN_LENGTH, height));
        layout::Node::new(size).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        for thumb in 0..2 {
            let rect = self.thumb_rect(bounds, thumb);
            let id = self.ids.as_ref().map(|ids| &ids[thumb]);
            if self.on_change.is_some() {
                operation.focusable(id, rect, &mut state.focus[thumb]);
            } else {
                state.focus[thumb].unfocus();
            }
            let mut node =
                SemanticsNode::new(self.semantics(thumb, state.focus[thumb].is_focused()));
            operation.custom(id, rect, &mut node);
            if self.on_change.is_some()
                && let Some(id) = id
            {
                let mut request = ValueRequest::new(id.clone());
                operation.custom(Some(id), rect, &mut request);
                if let Some(intent) = request.take() {
                    state.pending[thumb] = Some(intent);
                }
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
        let state = tree.state.downcast_mut::<State>();
        state.window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            let fractions = self.fractions();
            let dragging = state.dragging.is_some();
            if state
                .glide
                .frame(*now, None, dragging, |_: &iced::Theme| fractions)
            {
                shell.request_redraw();
            }
        }
        for thumb in 0..2 {
            if let Some(intent) = state.pending[thumb].take()
                && let Some(value) = self.routed(thumb, &intent)
            {
                self.publish(shell, thumb, value);
            }
        }
        if self.on_change.is_none() {
            // Disabled mid-drag, the release never arrives here: drop the
            // drag so re-enabling does not resume it.
            if state.dragging.take().is_some() {
                shell.request_redraw();
            }
            return;
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position() else {
                    return;
                };
                if !bounds.contains(position) {
                    for focus in &mut state.focus {
                        focus.unfocus();
                    }
                    return;
                }
                let thumb = self.nearest(bounds, position.x);
                state.focus[thumb].focus();
                state.focus[1 - thumb].unfocus();
                let (low, high) = self.limits(thumb);
                let value = self.value_at(bounds, position.x).clamp(low, high);
                state.dragging = Some((thumb, value));
                self.publish(shell, thumb, value);
                shell.request_redraw();
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some((thumb, last)) = state.dragging {
                    let (low, high) = self.limits(thumb);
                    let value = self.value_at(bounds, position.x).clamp(low, high);
                    if value != last {
                        state.dragging = Some((thumb, value));
                        self.publish(shell, thumb, value);
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let handles_event = state.dragging.take().is_some();
                if handles_event {
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            Event::Keyboard(key) => {
                let direction = if self.mirrored {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                };
                let Some(thumb) = (0..2).find(|thumb| state.focus[*thumb].is_focused()) else {
                    return;
                };
                if let Some(nav) = navigation_from_key_in(key, direction) {
                    let value = self.moved(thumb, nav);
                    self.publish(shell, thumb, value);
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
        let state = tree.state.downcast_ref::<State>();
        let tokens = (self.tokens)(theme);
        let alpha = if self.on_change.is_some() {
            1.0
        } else {
            crate::color::disabled_alpha(tokens.disabled_alpha)
        };
        let dim = |color: Color| crate::color::with_opacity(color, alpha);
        let hairline =
            enzui_theme::StrokeToken::Hairline.resolve(&crate::theme_lookup::stroke(theme));
        // The rail runs edge to edge, as the single slider's does; the thumbs
        // travel inside it.
        let track = Rectangle::new(
            Point::new(bounds.x, bounds.center_y() - TRACK_THICKNESS / 2.0),
            Size::new(bounds.width.max(0.0), TRACK_THICKNESS),
        );
        let radius = TRACK_THICKNESS / 2.0;
        renderer.fill_quad(
            renderer::Quad {
                bounds: track,
                border: Border {
                    color: dim(tokens.track_border),
                    width: tokens.track_border_width.unwrap_or(hairline),
                    radius: radius.into(),
                },
                ..renderer::Quad::default()
            },
            Background::Color(dim(tokens.track)),
        );
        let painted = state.glide.paint(theme, self.fractions());
        let (a, b) = (
            self.center_x_at(bounds, painted.0),
            self.center_x_at(bounds, painted.1),
        );
        let fill = Rectangle::new(
            Point::new(a.min(b), track.y),
            Size::new((a - b).abs(), TRACK_THICKNESS),
        );
        let fill_color = if state.dragging.is_some() {
            tokens.fill_hover
        } else {
            tokens.fill
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: fill,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..renderer::Quad::default()
            },
            Background::Color(dim(fill_color)),
        );
        for thumb in 0..2 {
            let rect = Self::rect_at(bounds, if thumb == 0 { a } else { b });
            if enzui_core::focus::focus_ring_visible_for(
                state.focus[thumb].is_focused(),
                state.window_key,
            ) {
                let outset = tokens.focus_ring_outset.max(0.0);
                crate::focus_button::paint_focus_ring(
                    renderer,
                    rect,
                    crate::focus_button::FocusRingQuad {
                        ink: tokens.focus_ring,
                        width: tokens.focus_ring_width.max(0.0),
                        radius: (THUMB_HEIGHT / 2.0 + outset).into(),
                        outset,
                        snap: true,
                    },
                );
            }
            let color = if state.dragging.is_some_and(|(held, _)| held == thumb) {
                tokens.thumb_dragging
            } else {
                tokens.thumb
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: rect,
                    border: Border {
                        color: dim(tokens.thumb_border),
                        width: tokens.thumb_border_width.unwrap_or(hairline),
                        radius: (THUMB_HEIGHT / 2.0).into(),
                    },
                    shadow: Shadow {
                        color: dim(tokens.thumb_shadow),
                        offset: Vector::new(
                            tokens.thumb_shadow_offset.x,
                            tokens.thumb_shadow_offset.y,
                        ),
                        blur_radius: tokens.thumb_shadow_blur,
                    },
                    snap: true,
                },
                Background::Color(dim(color)),
            );
        }
    }
}
