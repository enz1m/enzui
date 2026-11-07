//! An eight-dot indeterminate loader with viewport-gated self-animation.
//!
//! Defaults to a 24 px active accent spinner rotating once per second. Colours
//! follow the live palette unless pinned; inactive dots use the control-border
//! track ink at 28% of its alpha. Drawing uses the actual constrained bounds,
//! and extents below 2 px paint no dots while retaining the layout and semantics.
//!
//! The dots are painted once and turned by the compositor: the spinner
//! publishes its angle as a track and its next turns as a lane animation, so
//! it costs the UI thread no frames while it spins.
//!
//! Explicit phase, inactivity and explicit reduced motion disable scheduling.
//! Theme-driven reduced motion rests at phase zero unless a phase is pinned;
//! while visible it requests a probe two seconds ahead so a theme change can resume
//! animation. Theme policy is observed during drawing and used on the next
//! update. Offscreen animation and probes are suppressed by [`crate::viewport_gate`].
//!
//! The optional label names a ProgressIndicator accessibility leaf. Hosts
//! remove the spinner when loading ends; inactivity does not remove that role.

use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::time::Instant;
use iced::widget::canvas::{self, Event, Frame, Path, Program};
use iced::window;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size};
use std::cell::Cell;
/// Palette role for active-dot ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpinnerTone {
    /// Accent ink; the default tone.
    #[default]
    Accent,
    /// Success foreground ink.
    Success,
    /// Warning foreground ink.
    Warning,
    /// Error foreground ink.
    Danger,
}

/// Active-dot and inactive-track colour overrides.
#[derive(Debug, Clone, Copy)]
pub struct SpinnerTokens {
    /// Default active-dot ink, resolved from palette accent.
    pub accent_color: Color,
    /// Success ink, resolved from the contrast-tuned success foreground.
    pub success_color: Color,
    /// Warning ink, resolved from the contrast-tuned warning foreground.
    pub warning_color: Color,
    /// Destructive ink, resolved from the contrast-tuned error foreground.
    pub danger_color: Color,
    /// Inactive-dot ink, resolved from palette control_border and painted at 28% of its alpha.
    pub track_color: Color,
}

impl SpinnerTokens {
    /// Resolve accent, contrast-tuned status foregrounds and control-border track ink.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            accent_color: p.accent,
            success_color: p.tone_success_fg,
            warning_color: p.tone_warning_fg,
            danger_color: p.tone_error_fg,
            track_color: p.control_border,
        }
    }

    /// Use the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Use the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Resolve active-dot ink for a semantic tone.
    pub fn color_for(&self, tone: SpinnerTone) -> Color {
        match tone {
            SpinnerTone::Accent => self.accent_color,
            SpinnerTone::Success => self.success_color,
            SpinnerTone::Warning => self.warning_color,
            SpinnerTone::Danger => self.danger_color,
        }
    }
}

/// Default requested width and height in logical pixels: 24.
pub const DEFAULT_SIZE: f32 = 24.0;
/// Number of painted dots around the ring: 8.
pub const DOT_COUNT: usize = 8;
/// Seconds per full turn: 1.
pub const ROTATION_PERIOD_SECS: f32 = 1.0;
/// Requested delay for visible, theme-driven reduced-motion probes, in seconds: 2.
/// Explicit static configurations and hidden spinners schedule no probes.
pub const REDUCED_MOTION_PROBE_SECS: u64 = 2;
const INACTIVE_OPACITY: f32 = 0.28;
fn sane_size(size: f32) -> f32 {
    if size.is_finite() && size > 0.0 {
        size
    } else {
        DEFAULT_SIZE
    }
}
fn sane_phase(phase: f32) -> f32 {
    if phase.is_finite() { phase } else { 0.0 }
}

/// Minimum actual canvas extent that paints dots, in logical pixels: 2.
pub const MIN_RENDERABLE_EXTENT: f32 = 2.0;
/// Return max(2, 0.16 × extent), capped by the extent, in logical pixels.
/// Non-positive or non-finite extents return zero.
pub fn dot_size_for(size: f32) -> f32 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    (size * 0.16).max(2.0).min(size)
}

/// Return half the extent remaining after [`dot_size_for`], never negative.
/// Non-positive or non-finite extents return zero.
pub fn ring_radius_for(size: f32) -> f32 {
    if !size.is_finite() || size <= 0.0 {
        return 0.0;
    }
    ((size - dot_size_for(size)) / 2.0).max(0.0)
}

/// Return (1 − index/8) × 0.85 + 0.15 for a dot index.
/// The painted indices are 0–7, fading from 1 to 0.25625; indices are not clamped.
pub fn dot_opacity(i: usize) -> f32 {
    (1.0 - (i as f32) / (DOT_COUNT as f32)) * 0.85 + 0.15
}
#[derive(Debug, Default, PartialEq, Eq)]
struct SpinnerState {
    started_at: Option<Instant>,
    frame_at: Option<Instant>,
    theme_motion_reduced: Cell<Option<bool>>,
}

impl SpinnerState {
    fn theme_says_reduced(&self) -> bool {
        self.theme_motion_reduced.get().unwrap_or(false)
    }
    fn elapsed_secs(&self) -> f32 {
        match (self.started_at, self.frame_at) {
            (Some(start), Some(now)) => now.saturating_duration_since(start).as_secs_f32(),
            _ => 0.0,
        }
    }
}

/// Convert elapsed seconds to radians, wrapping once per [`ROTATION_PERIOD_SECS`].
/// Non-positive or non-finite durations return zero.
pub fn phase_for_elapsed(elapsed_secs: f32) -> f32 {
    if !elapsed_secs.is_finite() || elapsed_secs <= 0.0 {
        return 0.0;
    }
    (elapsed_secs % ROTATION_PERIOD_SECS) / ROTATION_PERIOD_SECS * std::f32::consts::TAU
}

#[derive(Debug, Clone, Copy)]
struct SpinnerProgram {
    /// A [`Rotor`] turns the painted dots; the program paints phase zero and
    /// keeps no clock of its own.
    turned: bool,
    active: bool,
    tone: SpinnerTone,
    fixed_phase: Option<f32>,
    pinned_tokens: Option<SpinnerTokens>,
    fill_override: Option<Color>,
    track_override: Option<Color>,
    reduced_motion: Option<bool>,
}

impl SpinnerProgram {
    fn is_static(&self) -> bool {
        self.fixed_phase.is_some() || self.reduced_motion == Some(true) || !self.active
    }
    fn is_still(&self, state: &SpinnerState) -> bool {
        if self.is_static() {
            return true;
        }
        match self.reduced_motion {
            Some(explicit) => explicit,
            None => state.theme_says_reduced(),
        }
    }
    fn motion_reduced(&self, theme: &iced::Theme) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme))
    }
    fn observe_theme(&self, state: &SpinnerState, theme: &iced::Theme) {
        state
            .theme_motion_reduced
            .set(Some(crate::theme_lookup::reduced_motion(theme)));
    }
    fn phase(&self, state: &SpinnerState, theme: &iced::Theme) -> f32 {
        match self.fixed_phase {
            Some(p) => p,
            None if self.turned || self.motion_reduced(theme) => 0.0,
            None => phase_for_elapsed(state.elapsed_secs()),
        }
    }
    /// Each dot's centre, diameter and colour for a spinner filling `area`
    /// at `phase`; none when `area` is too small to paint.
    fn dots(&self, theme: &iced::Theme, area: Rectangle, phase: f32) -> Vec<(Point, f32, Color)> {
        let extent = area.width.min(area.height).max(0.0);
        if extent < MIN_RENDERABLE_EXTENT {
            return Vec::new();
        }
        let center = area.center();
        let dot_size = dot_size_for(extent);
        let radius = ring_radius_for(extent);
        let (active_color, track_color) = self.colors(theme);
        (0..DOT_COUNT)
            .map(|i| {
                let theta = phase + std::f32::consts::TAU * (i as f32) / (DOT_COUNT as f32);
                let (mut color, opacity) = if self.active {
                    (active_color, dot_opacity(i))
                } else {
                    (track_color, INACTIVE_OPACITY)
                };
                color.a *= opacity;
                (
                    Point::new(
                        center.x + radius * theta.cos(),
                        center.y + radius * theta.sin(),
                    ),
                    dot_size,
                    color,
                )
            })
            .collect()
    }

    fn colors(&self, theme: &iced::Theme) -> (Color, Color) {
        let tokens = crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_SPINNER,
            self.pinned_tokens,
            theme,
            SpinnerTokens::from_palette,
        );
        let active = self
            .fill_override
            .unwrap_or_else(|| tokens.color_for(self.tone));
        let track = self.track_override.unwrap_or(tokens.track_color);
        (active, track)
    }
}

impl<Message> Program<Message> for SpinnerProgram {
    type State = SpinnerState;

    fn frame_interest(&self, _state: &Self::State) -> bool {
        !self.turned && !self.is_static()
    }

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if self.turned || self.is_static() {
            return None;
        }
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        // Theme changes can resume motion; update sees only the policy latched by draw.
        if self.is_still(state) {
            return Some(canvas::Action::request_redraw_at(
                *now + iced::time::Duration::from_secs(REDUCED_MOTION_PROBE_SECS),
            ));
        }
        // Frame timestamps keep each instance clock independent of wall-clock sampling.
        if state.started_at.is_none() {
            state.started_at = Some(*now);
        }
        state.frame_at = Some(*now);
        Some(canvas::Action::request_redraw())
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        // Latch policy even when constrained bounds suppress all geometry.
        self.observe_theme(state, theme);
        let area = Rectangle::with_size(bounds.size());
        for (center, size, color) in self.dots(theme, area, self.phase(state, theme)) {
            frame.fill(&Path::circle(center, size / 2.0), color);
        }
        vec![frame.into_geometry()]
    }
}

/// Builder for a circular loading indicator and its accessibility leaf.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzSpinner {
    size: f32,
    tone: SpinnerTone,
    active: bool,
    fixed_phase: Option<f32>,
    tokens: Option<SpinnerTokens>,
    fill_override: Option<Color>,
    track_override: Option<Color>,
    reduced_motion: Option<bool>,
    label: Option<String>,
}

impl EnzSpinner {
    /// Set requested width and height in logical pixels; defaults to 24.
    /// Non-positive or non-finite values fall back to 24. Parent constraints still
    /// determine the bounds used for drawing.
    pub fn size(mut self, size: f32) -> Self {
        self.size = sane_size(size);
        self
    }

    /// Choose active-dot ink; defaults to Accent and yields to an explicit fill.
    pub fn tone(mut self, tone: SpinnerTone) -> Self {
        self.tone = tone;
        self
    }

    /// Pin the phase in radians and disable animation and probes.
    /// Non-finite phases become zero; finite phases are used without normalization.
    /// The pin wins over reduced-motion phase selection.
    pub fn phase(mut self, phase: f32) -> Self {
        self.fixed_phase = Some(sane_phase(phase));
        self
    }

    /// Pin phase zero and disable animation and probes.
    pub fn static_frame(self) -> Self {
        self.phase(0.0)
    }

    /// Override the live theme motion policy for this instance.
    /// True disables scheduling and uses phase zero unless a phase is pinned. False
    /// permits animation under a reduced-motion theme but does not override an
    /// explicit phase pin or inactivity. Rebuild to change an explicit override.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Choose active dots or dim track-coloured dots; defaults to true.
    /// False disables animation and probes without removing the accessibility role.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Pin active-dot ink, overriding the tone and any token pin.
    /// Inactive dots are unaffected.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill_override = Some(color);
        self
    }

    /// Pin inactive-dot ink, overriding both live palette and pinned tokens.
    /// Active dots are unaffected.
    pub fn track(mut self, color: Color) -> Self {
        self.track_override = Some(color);
        self
    }

    /// Pin the complete colour bundle; direct fill and track overrides still win.
    pub fn tokens(mut self, tokens: SpinnerTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set the accessible name of the ProgressIndicator leaf.
    /// Text is retained verbatim, including an empty string.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    fn semantics(&self) -> Semantics {
        Semantics::new(SemRole::ProgressIndicator).label_opt(self.label.clone())
    }
    fn program(&self) -> SpinnerProgram {
        SpinnerProgram {
            turned: false,
            active: self.active,
            tone: self.tone,
            fixed_phase: self.fixed_phase,
            reduced_motion: self.reduced_motion,
            pinned_tokens: self.tokens,
            fill_override: self.fill_override,
            track_override: self.track_override,
        }
    }

    /// Build the constrained canvas, viewport gate and ProgressIndicator leaf.
    /// The leaf remains present even when animation is inactive or offscreen.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let size = sane_size(self.size);
        let semantics = self.semantics();
        let mut program = self.program();
        // A spinner that can turn is turned by the compositor.
        program.turned = program.fixed_phase.is_none() && program.active;
        let canvas: Element<'a, Message> = canvas::Canvas::new(program)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into();
        let canvas = if program.turned {
            Element::new(Rotor {
                child: canvas,
                program,
                reduced_motion: program.reduced_motion,
            })
        } else {
            canvas
        };
        // Keep the frame gate inside the semantic leaf so hiding does not hide the role.
        let gated: Element<'a, Message> = crate::viewport_gate::ViewportGate::new(canvas).into();
        crate::semantics_container::SemanticsContainer::leaf(gated, semantics).into()
    }
}

impl<'a, Message: 'a> From<EnzSpinner> for Element<'a, Message> {
    fn from(s: EnzSpinner) -> Self {
        s.into_element()
    }
}

/// Create a 24 px active accent spinner with no label, colour pins or motion override.
pub fn enz_spinner() -> EnzSpinner {
    EnzSpinner {
        size: DEFAULT_SIZE,
        tone: SpinnerTone::default(),
        active: true,
        fixed_phase: None,
        tokens: None,
        fill_override: None,
        track_override: None,
        reduced_motion: None,
        label: None,
    }
}

/// How far ahead one lane animation turns a spinner, in turns. The UI thread
/// wakes once per this many turns to hand the lane the next stretch.
const TURNS_PER_ANIMATION: f32 = 8.0;

/// Turns the spinner's dots: paints them where they are as round quads,
/// which a replay turns on by moving them (round quads look the same at any
/// angle), so nothing is composited. Its child paints the spinner when it
/// does not turn.
struct Rotor<'a, Message> {
    child: Element<'a, Message>,
    program: SpinnerProgram,
    /// The spinner's explicit motion policy, if any; the theme's otherwise.
    reduced_motion: Option<bool>,
}

struct RotorState {
    node: iced::advanced::renderer::NodeId,
    started_at: Option<Instant>,
    frame_at: Option<Instant>,
    theme_reduced: Cell<bool>,
    /// Whether the last draw painted through the turning composite, which a
    /// replay turns.
    recorded: Cell<bool>,
    /// The angle the last draw painted the dots at; tracks turn them on
    /// from there.
    recorded_angle: Cell<f32>,
}

impl RotorState {
    /// The angle at the latest frame, within one turn.
    fn angle(&self) -> f32 {
        match (self.started_at, self.frame_at) {
            (Some(start), Some(now)) => {
                phase_for_elapsed(now.saturating_duration_since(start).as_secs_f32())
            }
            _ => 0.0,
        }
    }
}

impl<Message> Rotor<'_, Message> {
    fn reduced(&self, state: &RotorState) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| state.theme_reduced.get())
    }

    fn track(
        node: iced::advanced::renderer::NodeId,
        center: Point,
        angle: f32,
    ) -> iced::advanced::renderer::Track {
        iced::advanced::renderer::Track {
            scale_origin: center,
            rotation: angle,
            ..iced::advanced::renderer::Track::identity(node)
        }
    }
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, Renderer> for Rotor<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<RotorState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(RotorState {
            node: crate::motion::next_scene_node(),
            started_at: None,
            frame_at: None,
            theme_reduced: Cell::new(false),
            recorded: Cell::new(false),
            recorded_angle: Cell::new(0.0),
        })
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        _cursor: iced::mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let iced::Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let reduced = self.reduced(tree.state.downcast_ref::<RotorState>());
        let state = tree.state.downcast_mut::<RotorState>();
        let started = *state.started_at.get_or_insert(*now);
        state.frame_at = Some(*now);
        if reduced {
            // A theme that allows motion again resumes the turn.
            if self.reduced_motion.is_none() {
                shell.schedule_redraw_at(
                    *now + iced::time::Duration::from_secs(REDUCED_MOTION_PROBE_SECS),
                );
            }
            return;
        }
        let bounds = layout.bounds();
        if !crate::viewport_gate::is_visible(bounds, viewport) {
            return;
        }
        crate::motion::request_motion_frame(shell, state.recorded.get());
        if !state.recorded.get() {
            return;
        }
        let node = state.node;
        let center = bounds.center();
        // The dots were painted at the recorded angle; the track turns them
        // the rest of the way.
        let angle = state.angle() - state.recorded_angle.get();
        shell.publish_track(Self::track(node, center, angle));
        let duration =
            iced::time::Duration::from_secs_f32(TURNS_PER_ANIMATION * ROTATION_PERIOD_SECS);
        shell.publish_track_animation(iced::advanced::renderer::TrackAnimation {
            node,
            start: *now,
            duration,
            keyframes: vec![
                (0.0, Self::track(node, center, angle)),
                (
                    1.0,
                    Self::track(
                        node,
                        center,
                        angle + TURNS_PER_ANIMATION * std::f32::consts::TAU,
                    ),
                ),
            ],
            // A new stretch is a new travel for the lane.
            generation: (now.saturating_duration_since(started).as_secs_f32()
                / (TURNS_PER_ANIMATION * ROTATION_PERIOD_SECS)) as u64,
        });
        // With the lane turning it, the UI thread sleeps until the stretch
        // nearly runs out.
        shell.schedule_redraw_at(*now + duration.mul_f32(0.9));
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<RotorState>();
        state
            .theme_reduced
            .set(crate::theme_lookup::reduced_motion(theme));
        let bounds = layout.bounds();
        let draw_child = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        };
        if self.reduced(state) {
            draw_child(renderer);
            state.recorded.set(false);
            return;
        }
        // The dots are painted where they are now, as round quads; a replay
        // turns them on by moving them, so neither paint nor replay
        // composites anything.
        let angle = state.angle();
        let dots = self.program.dots(theme, bounds, angle);
        renderer.with_layer(bounds, |renderer| {
            renderer.mark_recording(state.node);
            for (center, size, color) in dots {
                renderer.fill_quad(
                    iced::advanced::renderer::Quad {
                        bounds: Rectangle::new(
                            Point::new(center.x - size / 2.0, center.y - size / 2.0),
                            Size::new(size, size),
                        ),
                        border: iced::border::rounded(size / 2.0),
                        // Turning dots move by fractions of a pixel.
                        snap: false,
                        ..iced::advanced::renderer::Quad::default()
                    },
                    color,
                );
            }
            renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
        });
        state.recorded_angle.set(angle);
        state.recorded.set(true);
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> iced::mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
}
