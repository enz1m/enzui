//! A focusable on/off switch: a track and a sliding thumb with exact,
//! independent dimensions.
//!
//! Most apps want [`toggle`](crate::toggle), which wraps this with a label,
//! description and a row that toggles when clicked. Use `switch` directly for
//! a bare indicator or a self-composed row. Unlike iced's `Toggler`, it keeps
//! the specified 32x18 geometry and supports keyboard focus, Enter and Space,
//! and assistive technology.
//!
//! The thumb slides over the theme's `motion-normal` duration with ease-out.
//! Under reduced motion (theme or [`reduced_motion`](Switch::reduced_motion))
//! it lands on the first frame. A settled switch requests no redraws.
//!

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{
    FocusState, PressFocus, activation_from_key, press_focus_from_cursor, release_focus_if_inert,
};
use enzui_core::metrics::{FiniteF32, FiniteNonNeg};
use enzui_core::placement::{Direction, resolve_direction};
use enzui_core::semantics::{Role, Semantics, Toggle};
use enzui_theme::Palette;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::animation::Animation;
use iced::time::Instant;
use iced::{
    Background, Border, Color, Element, Event, Length, Rectangle, Shadow, Size, Vector, keyboard,
    mouse, touch, window,
};
use std::cell::Cell;

/// Exact track + thumb geometry for one switch size .
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwitchGeometry {
    /// Track width in px.
    pub track_width: f32,
    /// Track height in px.
    pub track_height: f32,
    /// Thumb diameter in px.
    pub thumb: f32,
    /// Inset of the thumb from the track edge in px.
    pub inset: f32,
}

impl SwitchGeometry {
    /// Switch sizes: sm 32×18 / md 40×22 / lg 50×28,
    /// with 14 / 18 / 24 px thumbs (2px inset each side).
    pub const SM: Self = Self {
        track_width: 32.0,
        track_height: 18.0,
        thumb: 14.0,
        inset: 2.0,
    };
    /// Medium switch (default).
    pub const MD: Self = Self {
        track_width: 40.0,
        track_height: 22.0,
        thumb: 18.0,
        inset: 2.0,
    };
    /// Large switch.
    pub const LG: Self = Self {
        track_width: 50.0,
        track_height: 28.0,
        thumb: 24.0,
        inset: 2.0,
    };
}

/// Resolved colours for the switch, per interaction state.
#[derive(Debug, Clone, Copy)]
pub struct SwitchColors {
    /// Track fill (already state-resolved: on/off, hover, disabled).
    pub track: Color,
    /// Thumb fill; its alpha also controls disabled thumb-icon opacity.
    pub thumb: Color,
    /// Focus-ring colour.
    pub focus_ring: Color,
    /// Focus-ring width.
    pub focus_ring_width: f32,
    /// Thumb drop-shadow colour .
    ///
    /// Sanitized with `crate::tooltip::finite_color` at the paint boundary,
    /// so a poisoned channel cannot reach the renderer quad.
    pub thumb_shadow: Color,
    /// Thumb drop-shadow blur radius, px .
    /// Sanitized with [`FiniteNonNeg`] at the paint boundary.
    pub thumb_shadow_blur: f32,
    /// Thumb drop-shadow vertical offset, px (Original `drop-shadow-offset-y:
    /// 1px`). May legitimately be negative (an upward shadow), so it is
    /// sanitized with [`FiniteF32`], not [`FiniteNonNeg`].
    pub thumb_shadow_offset_y: f32,
}

/// Drop-shadow blur on the switch thumb, in px: small enough that the
/// shadow stays inside the track's rim instead of smudging its end.
pub const THUMB_SHADOW_BLUR: f32 = 2.0;

/// Vertical drop-shadow offset on the switch thumb, in px.
pub const THUMB_SHADOW_OFFSET_Y: f32 = 1.0;

/// The thumb's drop-shadow colour: the palette's shadow hue at a fixed 0.18
/// alpha, lighter than the elevation shadow used by cards and dialogs.
pub fn thumb_shadow_color(p: &Palette) -> Color {
    Color {
        a: 0x2e as f32 / 255.0,
        ..p.shadow
    }
}

/// The interaction state passed to the colour resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchState {
    /// Whether the switch is on.
    pub on: bool,
    /// Whether the pointer is hovering.
    pub hovered: bool,
    /// Whether the switch is disabled.
    pub disabled: bool,
    /// Whether the switch has keyboard focus.
    pub focused: bool,
}

/// Per-widget state: focus, the Space latch, and the thumb's slide
/// animation, which `diff` restarts when `on` changes.
#[derive(Debug)]
struct WidgetState {
    focus: FocusState,
    space_held: bool,
    /// The thumb slide. `false` = thumb left (off), `true` = thumb right (on).
    anim: Animation<bool>,
    /// Previous `on` value — edge-detect for `go_mut` in `diff`.
    prev_on: bool,
    /// A toggle seen in `diff` whose slide has not started yet.
    ///
    /// `diff` deliberately does not start the transition. It runs in
    /// `UserInterface::build`, outside the frame loop, where the only
    /// instants available are the wall clock and the last latched frame. Both
    /// are wrong: in an idle app the last frame can be seconds old, so the
    /// transition would already be finished before the next frame is drawn
    /// and the thumb would teleport (measured as a 42 px → 64 px jump between
    /// two captures 28 ms apart with no frame in between). `diff` records the
    /// intent only, and the next `RedrawRequested` starts the slide at the
    /// frame instant iced supplies: the true current time in a real app
    /// (`Instant::now()` stamped in `iced/winit/src/lib.rs`) and a
    /// controllable one in tests.
    pending_transition: bool,
    /// The instant of the most recent `RedrawRequested` frame.
    ///
    /// Read (not `Instant::now()`) so the animation phase is a function of the
    /// frame timestamp iced hands over, exactly as `SpinnerState::frame_at`
    /// (in the `spinner` module) does. That is what makes the motion deterministic
    /// under test and identical for two switches mounted on different frames.
    frame_at: Cell<Option<Instant>>,
    /// Shared raw-duration, unknown-policy, easing, and applied-duration latch.
    motion: crate::motion::MotionLatch,
    /// An assistive-technology `Click` routed in `operate`, consumed by the
    /// next `update`, which toggles exactly as a pointer or key press does.
    activate_pending: bool,
    /// Per-window modality key: captured from `shell.window()` in
    /// `update`, read in `draw`, so a commit gesture in another window does not
    /// hide this switch's keyboard ring.
    window_key: u64,
}

impl WidgetState {
    fn new(on: bool) -> Self {
        Self {
            focus: FocusState::default(),
            space_held: false,
            anim: Self::make_anim(on, DEFAULT_NORMAL_MS, enzui_theme::MotionEasing::default()),
            prev_on: on,
            pending_transition: false,
            frame_at: Cell::new(None),
            motion: crate::motion::MotionLatch::new(
                DEFAULT_NORMAL_MS,
                DEFAULT_NORMAL_MS,
                enzui_theme::MotionEasing::default(),
            ),
            activate_pending: false,
            window_key: enzui_core::focus::KEY_UNKEYED,
        }
    }

    /// The thumb slides over `motion-normal` with the Material standard curve,
    /// `cubic-bezier(0.4, 0.0, 0.2, 1.0)`, whose nearest `lilt` equivalent is `EaseOut` (the same one `expander::anim`
    /// picked for its body transition).
    fn make_anim(
        initial: bool,
        normal_ms: f32,
        easing: enzui_theme::MotionEasing,
    ) -> Animation<bool> {
        crate::motion::build_anim(initial, normal_ms, easing)
    }

    /// Rebuild the animation when the effective duration changes (which
    /// includes collapsing to 0 ms under a reduced-motion theme, and expanding
    /// back when the policy is lifted).
    fn sync_duration(&mut self, effective_ms: f32) {
        self.motion.sync_animation(&mut self.anim, effective_ms);
    }

    /// The frame instant to evaluate the animation at.
    fn now(&self) -> Instant {
        self.frame_at.get().unwrap_or_else(Instant::now)
    }

    /// The slide duration in ms: zero under reduced motion (a per-instance
    /// override wins over the theme), else the theme's `motion-normal`. Before
    /// the first draw the theme's preference is unknown, so it is zero too.
    fn effective_normal_ms(&self, override_reduced: Option<bool>) -> f32 {
        self.motion.effective_ms(override_reduced)
    }
}

/// `MotionScale::default().normal_ms` — the duration used before any theme has
/// been seen (state is constructed before the first `draw`).
const DEFAULT_NORMAL_MS: f32 = 180.0;

impl iced::advanced::widget::operation::Focusable for WidgetState {
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

/// Runtime-theme color resolver for a switch: maps the live `iced::Theme` and
/// the switch's current [`SwitchState`] to its [`SwitchColors`].
type SwitchColorFn<'a> = Box<dyn Fn(&iced::Theme, SwitchState) -> SwitchColors + 'a>;

/// A focusable on/off switch with exact geometry.
#[must_use]
pub struct Switch<'a, Message> {
    on: bool,
    geometry: SwitchGeometry,
    on_toggle: Option<Box<dyn Fn(bool) -> Message + 'a>>,
    /// Read-only: focusable and announced, but refuses to toggle. This
    /// differs from *disabled* (`on_toggle == None`) exactly as
    /// [`read_only`](crate::text_field::EnzTextField::read_only) does for text
    /// fields: a disabled switch is inert (no focus, no semantic actions,
    /// dimmed chrome), while a read-only one stays focusable and keyboard
    /// reachable, paints its ring with undimmed chrome and is announced as
    /// read-only, and every toggle path (pointer, touch, Enter, Space,
    /// routed AccessKit) is refused. The `on_toggle` callback is kept so
    /// focus and semantics behave as live. Set by
    /// [`read_only`](Self::read_only).
    read_only: bool,
    colors: SwitchColorFn<'a>,
    id: Option<iced::advanced::widget::Id>,
    label: Option<String>,
    description: Option<String>,
    thumb_icon: Option<iced::advanced::svg::Handle>,
    direction: Option<Direction>,
    /// Per-instance reduced-motion override (`None` = follow the theme).
    reduced_motion: Option<bool>,
}

/// Default switch colors follow the live theme and share the composed toggle's
/// raised thumb surface and contrasting tracks. Disabled controls dim together.
pub fn default_colors(theme: &iced::Theme, state: SwitchState) -> SwitchColors {
    crate::toggle::switch_colors(
        crate::theme_lookup::resolve_live(theme, crate::toggle::ToggleTokens::from_palette),
        state,
    )
}

/// Create a [`Switch`]. `on_toggle` receives the *new* value; pass `None` via
/// [`Switch::on_toggle_maybe`] to disable.
pub fn switch<'a, Message>(
    on: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> Switch<'a, Message> {
    Switch {
        on,
        geometry: SwitchGeometry::MD,
        on_toggle: Some(Box::new(on_toggle)),
        read_only: false,
        colors: Box::new(default_colors),
        id: None,
        label: None,
        description: None,
        thumb_icon: None,
        direction: None,
        reduced_motion: None,
    }
}

impl<'a, Message> Switch<'a, Message> {
    /// Set the exact geometry.
    pub fn geometry(mut self, geometry: SwitchGeometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// Enable/disable by supplying `Some`/`None` toggle callback.
    pub fn on_toggle_maybe(mut self, cb: Option<impl Fn(bool) -> Message + 'a>) -> Self {
        self.on_toggle = cb.map(|c| Box::new(c) as Box<dyn Fn(bool) -> Message + 'a>);
        self
    }

    /// Mark this switch read-only: focusable and announced, edits refused.
    ///
    /// See the [`Self::read_only`](Switch#structfield.read_only) field docs for
    /// the read-only-vs-disabled contract. Pass a live `on_toggle` alongside
    /// this (do not set `on_toggle == None`, which is *disabled*).
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Whether this switch participates in focus / announces itself as an
    /// interactive control. True when live or read-only — both are
    /// keyboard-reachable; only a disabled (`on_toggle == None`, non-read-only)
    /// switch is not.
    fn is_focusable(&self) -> bool {
        self.on_toggle.is_some() || self.read_only
    }

    /// Whether a toggle action may actually fire. True only when live and not
    /// read-only. A read-only switch is focusable but every mutating path is
    /// refused, mirroring `text_input` built without an `on_input` handler.
    fn is_actionable(&self) -> bool {
        self.on_toggle.is_some() && !self.read_only
    }

    /// Colour resolver: maps the runtime `iced::Theme` + interaction state to
    /// concrete track/thumb/ring colours, so custom-widget colours follow the
    /// runtime theme (dark/light switch) just like the built-in widgets.
    pub fn colors(mut self, f: impl Fn(&iced::Theme, SwitchState) -> SwitchColors + 'a) -> Self {
        self.colors = Box::new(f);
        self
    }

    /// Stable id for programmatic focus.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Accessible name (for the switch semantics).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Longer accessible description (e.g. a subtitle explaining what the
    /// switch controls). Conveyed to assistive tech as the node's description,
    /// distinct from the short accessible name set by [`Switch::label`].
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Paint a bundled symbolic icon inside the moving thumb.
    ///
    /// The icon is decorative: this switch remains the only input target and
    /// the only checked semantic node. Its ink is resolved against the painted
    /// thumb on every draw, so it follows live theme changes. The glyph is
    /// inset two pixels from the thumb on each side.
    pub fn thumb_icon(mut self, name: &str) -> Self {
        self.thumb_icon = Some(crate::icon::handle_for(name));
        self
    }

    /// Override the writing direction used for the thumb endpoints.
    ///
    /// By default the switch follows the ambient locale. In right-to-left
    /// layout, off is on the right and on is on the left.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Force the thumb-slide animation on (`false`) or off (`true`),
    /// overriding the theme's `motion.reduced_motion` policy for this instance.
    ///
    /// `None` (the default) follows the theme, which is what an accessibility
    /// preference should normally do; the override exists for hosts that
    /// already know the answer (a screenshot harness, a still preview).
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// The thumb's left edge at a logical off-to-on progress and writing direction.
    pub(crate) fn thumb_x_at(&self, bounds_x: f32, progress: f32, direction: Direction) -> f32 {
        let g = self.geometry;
        let leading_x = bounds_x + g.inset;
        let trailing_x = bounds_x + g.track_width - g.thumb - g.inset;
        let logical_progress = if direction == Direction::RightToLeft {
            1.0 - progress
        } else {
            progress
        };
        leading_x + (trailing_x - leading_x) * logical_progress
    }

    fn resolved_thumb_icon_size(&self) -> f32 {
        (self.geometry.thumb - 4.0).max(0.0)
    }

    /// The accessibility [`Semantics`] this switch publishes.
    pub fn semantics(&self, focused: bool) -> Semantics {
        let mut sem = Semantics::new(Role::Switch)
            .label_opt(self.label.clone())
            .toggled(if self.on { Toggle::True } else { Toggle::False })
            // Disabled only when truly inert: no callback and not read-only. A
            // read-only switch is a live, reachable control whose edits are
            // refused, so it is announced as read-only, not disabled.
            .disabled(self.on_toggle.is_none() && !self.read_only)
            .read_only(self.read_only)
            .focused(focused)
            .default_action();
        if let Some(desc) = &self.description {
            sem = sem.description(desc.clone());
        }
        sem
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for Switch<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<WidgetState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(WidgetState::new(self.on))
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<WidgetState>();
        // Release stale focus at the rebuild, on the same
        // `on_toggle.is_some()` condition `operate` gates on.
        //
        // `switch` is the one widget in the family carrying a second
        // `&& !disabled` guard on its ring paint, so a pixel probe for the ring
        // cannot fail here. Its `semantics()` has no such guard and publishes
        // the raw flag, so this release is what stops a disabled switch
        // announcing itself focused. The ring guard stays: defence in depth on
        // a focus ring is harmless, and removing it is caught.
        let _ = release_focus_if_inert(&mut state.focus, self.is_focusable());
        // Start the slide when `on` changes, with the duration resynced from
        // the theme first so a reduced-motion theme applies to this
        // transition.
        let effective_ms = state.effective_normal_ms(self.reduced_motion);
        state.sync_duration(effective_ms);
        if self.on != state.prev_on {
            state.prev_on = self.on;
            // Record the intent only; the next frame starts the slide at its
            // own instant. See `pending_transition` for why stamping it here
            // silently restores the teleport.
            state.pending_transition = true;
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fixed(self.geometry.track_width),
            height: Length::Fixed(self.geometry.track_height),
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::Node::new(limits.resolve(
            Length::Fixed(self.geometry.track_width),
            Length::Fixed(self.geometry.track_height),
            Size::new(self.geometry.track_width, self.geometry.track_height),
        ))
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<WidgetState>();
        // Only an enabled switch registers for focus, so Tab and
        // `focus(id)` skip a disabled one.
        // A switch disabled while focused drops its focus: `draw` hides the
        // ring when disabled, so it would otherwise stay invisibly focused
        // with no way to activate it.
        if self.is_focusable() {
            operation.focusable(self.id.as_ref(), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        // Publish the switch role, checked, disabled and focus state.
        let focused = state.focus.is_focused();
        let mut node = SemanticsNode::new(self.semantics(focused));
        operation.custom(self.id.as_ref(), layout.bounds(), &mut node);

        // Offer the routed `Click` only when the switch can act on it, so an
        // assistive-technology activation is refused exactly when a pointer or
        // key toggle would be (disabled or read-only).
        if self.is_actionable()
            && let Some(id) = &self.id
        {
            let mut request = crate::activation::ActivationRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if request.armed() {
                tree.state.downcast_mut::<WidgetState>().activate_pending = true;
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

        // Feed this window's input modality from every raw event, so a
        // pointer-last frame suppresses the keyboard ring even in an app with
        // no `focus_button` to keep the modality current. Keyed per window; the
        // key is stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<WidgetState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        // A routed activation toggles through the same `on_toggle(!self.on)`
        // call as a pointer or key press. The flag is cleared before
        // publishing, so it fires once, and a switch rebuilt disabled discards
        // it.
        {
            let state = tree.state.downcast_mut::<WidgetState>();
            if state.activate_pending {
                state.activate_pending = false;
                // `is_actionable`, not `on_toggle.is_some()`: a switch armed
                // while live and then rebuilt read-only must DISCARD the
                // pending toggle, the same rule the disabled path relies on.
                if self.is_actionable()
                    && let Some(on_toggle) = &self.on_toggle
                {
                    shell.publish(on_toggle(!self.on));
                    shell.request_redraw();
                }
            }
        }

        // Advance the thumb slide on each frame, and ask for the next frame
        // only while it is still moving. Requesting one unconditionally spins
        // the compositor forever, so the request is
        // strictly gated on `is_animating`, and `toggle_thumb_motion.rs`
        // asserts a settled switch asks for nothing.
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<WidgetState>();
            state.frame_at.set(Some(*now));
            let effective_ms = state.effective_normal_ms(self.reduced_motion);
            state.sync_duration(effective_ms);
            // Start a slide `diff` asked for, at this frame's instant.
            if state.pending_transition {
                state.pending_transition = false;
                state.anim.go_mut(self.on, *now);
            }
            if state.anim.is_animating(*now) {
                shell.request_redraw();
            }
        }

        // Pointer focus is EXCLUSIVE. Every focusable decides, from
        // the one press position, whether it was the control hit — at most one
        // is, so the press that focuses this control is the same press that
        // unfocuses its siblings. This runs before the disabled early-return
        // (a disabled switch must still drop focus when the press lands
        // elsewhere) and takes no focus of its own when disabled.
        if let Some(press) = press_focus_from_cursor(event, cursor, bounds) {
            // `is_focusable`: a read-only switch is a valid focus target (it is
            // reachable, just not editable), so a click on it takes focus even
            // though the toggle itself is refused below.
            let enabled = self.is_focusable();
            let state = tree.state.downcast_mut::<WidgetState>();
            let was_focused = state.focus.is_focused();
            match press {
                PressFocus::Take if enabled => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }

        let Some(on_toggle) = &self.on_toggle else {
            // A disabled switch drops focus and any held Space or drag, so
            // re-enabling it cannot replay a stale press.
            let state = tree.state.downcast_mut::<WidgetState>();
            if state.focus.is_focused() || state.space_held {
                state.focus.unfocus();
                state.space_held = false;
                shell.request_redraw();
            }
            return;
        };

        // Read-only: focusable and announced, but every mutating path is
        // refused; the input is still consumed so it does not fall through.
        if self.read_only {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                    if cursor.is_over(bounds) =>
                {
                    shell.capture_event();
                }
                Event::Touch(touch::Event::FingerPressed { position, .. })
                    if bounds.contains(*position) =>
                {
                    shell.capture_event();
                }
                Event::Keyboard(kb) if activation_from_key(kb) => {
                    let state = tree.state.downcast_mut::<WidgetState>();
                    if state.focus.is_focused() {
                        state.space_held = false;
                        shell.capture_event();
                    }
                }
                _ => {}
            }
            return;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let handles_event = cursor.is_over(bounds);
                if handles_event {
                    // Focus was already taken above by the exclusive sweep.
                    shell.publish(on_toggle(!self.on));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            // Touch carries its own position — use it, not the cursor.
            Event::Touch(touch::Event::FingerPressed { position, .. }) => {
                let handles_event = bounds.contains(*position);
                if handles_event {
                    shell.publish(on_toggle(!self.on));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Keyboard(kb) if activation_from_key(kb) => {
                let state = tree.state.downcast_mut::<WidgetState>();
                if state.focus.is_focused() {
                    // Enter fires immediately; Space fires on release.
                    if let keyboard::Event::KeyPressed {
                        key: keyboard::Key::Named(named),
                        ..
                    } = kb
                    {
                        match named {
                            keyboard::key::Named::Enter => {
                                shell.publish(on_toggle(!self.on));
                                shell.capture_event();
                                shell.request_redraw();
                            }
                            keyboard::key::Named::Space => {
                                state.space_held = true;
                                shell.capture_event();
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyReleased {
                key: keyboard::Key::Named(keyboard::key::Named::Space),
                ..
            }) => {
                let state = tree.state.downcast_mut::<WidgetState>();
                if state.space_held {
                    state.space_held = false;
                    if state.focus.is_focused() {
                        shell.publish(on_toggle(!self.on));
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
        let state = tree.state.downcast_ref::<WidgetState>();

        // Latch the theme's motion policy so `update`/`diff` (neither of which
        // receives a `&Theme`) can honour it. Same latch-in-draw arrangement as
        // `expander::anim` (its easing/duration latch in `draw`) and `spinner`.
        let motion = crate::theme_lookup::motion(theme);
        // `Some(..)` so `update`/`diff` can tell "policy known" from the
        // pre-draw `None`.
        state.motion.observe(
            motion.normal_ms,
            motion.reduced_motion,
            motion.easing_or(None),
        );

        let hovered = cursor.is_over(bounds) && self.on_toggle.is_some();
        let focused = state.focus.is_focused();
        let disabled = self.on_toggle.is_none();

        let colors = (self.colors)(
            theme,
            SwitchState {
                on: self.on,
                hovered,
                disabled,
                focused,
            },
        );

        let g = self.geometry;
        let radius = g.track_height / 2.0;

        // Focus ring: a slightly larger rounded rect behind the track.
        //
        // Gate on the shared focus-visible policy, not raw focus, so a
        // pointer click focuses the switch without painting the keyboard ring;
        // keyed per window.
        if enzui_core::focus::focus_ring_visible_for(focused, state.window_key) && !disabled {
            // Shared ring painter. The outset is this widget's own
            // `focus_ring_width + 1` gap, so a thicker themed ring still
            // clears the track by exactly 1 px.
            let pad = colors.focus_ring_width + 1.0;
            crate::focus_button::paint_focus_ring(
                renderer,
                bounds,
                crate::focus_button::FocusRingQuad {
                    ink: colors.focus_ring,
                    width: colors.focus_ring_width,
                    radius: (radius + pad).into(),
                    outset: pad,
                    snap: true,
                },
            );
        }

        // Track.
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: radius.into(),
                },
                shadow: Shadow::default(),
                snap: false,
            },
            Background::Color(colors.track),
        );

        // Track and thumb retain fractional coordinates so their centres agree
        // at fractional display scales and while the thumb slides.
        let thumb_y = bounds.y + (g.track_height - g.thumb) / 2.0;
        let progress = state
            .anim
            .interpolate(0.0, 1.0, state.now())
            .clamp(0.0, 1.0);
        let thumb_x = self.thumb_x_at(bounds.x, progress, resolve_direction(self.direction));

        // Sanitise the shadow at the paint boundary: `SwitchColors` fields
        // are public. Blur is finite and non-negative; the offset may be
        // negative (an upward shadow); colours are clamped.
        let shadow = Shadow {
            color: crate::tooltip::finite_color(colors.thumb_shadow),
            offset: Vector::new(
                0.0,
                FiniteF32::new_or(colors.thumb_shadow_offset_y, THUMB_SHADOW_OFFSET_Y).get(),
            ),
            blur_radius: FiniteNonNeg::new_or(colors.thumb_shadow_blur, THUMB_SHADOW_BLUR).get(),
        };

        let thumb_bounds = Rectangle {
            x: thumb_x,
            y: thumb_y,
            width: g.thumb,
            height: g.thumb,
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: thumb_bounds,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: (g.thumb / 2.0).into(),
                },
                shadow,
                snap: false,
            },
            Background::Color(colors.thumb),
        );

        if let Some(handle) = &self.thumb_icon {
            use iced::advanced::svg::Renderer as _;

            let icon_size = self.resolved_thumb_icon_size();
            let icon_bounds = Rectangle {
                x: thumb_bounds.center_x() - icon_size / 2.0,
                y: thumb_bounds.center_y() - icon_size / 2.0,
                width: icon_size,
                height: icon_size,
            };
            let palette = crate::theme_lookup::palette(theme);
            let painted_thumb =
                crate::accessibility_metrics::composite_over(colors.thumb, colors.track);
            let icon_ink = crate::tokens::ink_for(painted_thumb, &palette);
            renderer.draw_svg(
                iced::advanced::svg::Svg {
                    handle: handle.clone(),
                    color: Some(icon_ink),
                    rotation: iced::Radians(0.0),
                    opacity: if disabled { colors.thumb.a } else { 1.0 },
                },
                icon_bounds,
                thumb_bounds,
            );
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) && self.on_toggle.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message> From<Switch<'a, Message>> for Element<'a, Message>
where
    Message: 'a + Clone,
{
    fn from(s: Switch<'a, Message>) -> Self {
        Self::new(s)
    }
}
