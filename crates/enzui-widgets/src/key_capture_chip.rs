//! Controlled keyboard-chord recording with typed [`KeyCapture`] reports.
//!
//! The application owns binding, arming and candidate validation, optionally
//! through [`enzui_core::capture_session::CaptureSession`]. It must enforce
//! one active recorder per window. Reports never mutate the supplied binding.
//! A focused, enabled, armed chip consumes uncaptured keyboard events, including repeats,
//! Tab, Enter and Space; unmodified Escape cancels and Backspace requests clearing.
//! Focus loss, window deactivation and reactivation request cancellation.
//!
//! The accessible Button name and derived identity use the required label.
//! Repeated labels need distinct namespaces or explicit IDs. The chord is the
//! accessible value; enabled recording adds instructions as a description, and
//! rejection adds invalid semantics. Capture is not a selected or toggled state.
//! Default geometry is 28 px high with a 96 px minimum width, subject to parent
//! constraints, 8 px horizontal padding and the theme's `key` corner radius
//! (5 px by default). Text uses UI-scaled 11 px.

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, PressFocus, press_focus_from_cursor};
use enzui_core::metrics::FiniteNonNeg;
use enzui_core::semantics::{Role, Semantics};
use enzui_core::{Modifier, Shortcut, ShortcutKey};
use enzui_theme::Palette;
use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{Paragraph as _, Renderer as _};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Renderer as _, Shell, Widget, renderer};
use iced::{
    Background, Border, Color, Element, Event, Length, Point, Rectangle, Shadow, Size, keyboard,
    mouse, touch,
};

/// Chip height (px), the button height.
pub const DEFAULT_HEIGHT: f32 = 28.0;
/// Minimum chip width (px). `chip-min-width: 96px`.
pub const DEFAULT_MIN_WIDTH: f32 = 96.0;
/// Horizontal padding (px). `spacing-sm`.
pub const DEFAULT_PADDING_X: f32 = 8.0;
/// Base text size before UI scaling in logical pixels: 11.
pub const DEFAULT_TEXT_SIZE: f32 = 11.0;
/// Border width (px) when idle or focused.
pub const DEFAULT_BORDER_WIDTH: f32 = 1.0;
/// Border width (px) while armed: strictly heavier than the focus ring, so
/// "listening" differs from "selected" by shape as well as colour.
pub const DEFAULT_CAPTURE_BORDER_WIDTH: f32 = 2.0;

// Built-in focus and capture inks coincide; a heavier armed border distinguishes them.
const _: () = assert!(
    DEFAULT_CAPTURE_BORDER_WIDTH > DEFAULT_BORDER_WIDTH,
    "the capture border must read thicker than the resting one"
);

/// Chip ink and geometry overrides; unpinned colours follow the live palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyCaptureChipTokens {
    /// Idle chip fill.
    pub background: Color,
    /// Complete hover fill; defaults to the hover wash composited over the idle surface.
    pub hover_background: Color,
    /// Fill while armed (recording).
    pub capture_background: Color,
    /// Fill while disabled.
    pub disabled_background: Color,
    /// Idle border.
    pub border: Color,
    /// Border while hovered.
    pub hover_border: Color,
    /// Border while keyboard-focused but not armed.
    pub focus_border: Color,
    /// Armed outline ink; defaults to the palette accent.
    pub capture_border: Color,
    /// Border while the host reports the current candidate invalid.
    pub invalid_border: Color,
    /// Ink for an assigned binding.
    pub text_color: Color,
    /// Ink for the "unassigned" placeholder.
    pub placeholder_color: Color,
    /// Ink for the "press keys…" hint while armed.
    pub capture_text_color: Color,
    /// Ink while disabled.
    pub disabled_text_color: Color,
    /// Chip height (px).
    pub height: f32,
    /// Minimum chip width (px).
    pub min_width: f32,
    /// Horizontal padding (px).
    pub padding_x: f32,
    /// Corner radius (px).
    pub radius: f32,
    /// Label size (px).
    pub text_size: f32,
    /// Border width when idle / hovered / focused.
    pub border_width: f32,
    /// Border width while armed.
    pub capture_border_width: f32,
}

impl KeyCaptureChipTokens {
    /// Derive tokens from `theme`: its palette, and its `key` shape for the
    /// corners.
    #[must_use]
    pub fn from_theme(theme: &iced::Theme) -> Self {
        Self {
            radius: crate::theme_lookup::shape_radius(theme, |s| s.key),
            ..crate::theme_lookup::resolve(theme, Self::from_palette)
        }
    }

    /// Derive tokens from a palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_alt,
            hover_background: crate::surface_interaction::composite(
                p.surface_alt,
                p.overlay_hover(),
            ),
            capture_background: p.accent_soft,
            disabled_background: p.surface,
            border: crate::surface_edge::edge_color(
                p.surface_alt,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            hover_border: crate::surface_edge::hover_edge_color(p.surface_alt, p.text),
            focus_border: p.focus_ring,
            capture_border: p.accent,
            invalid_border: p.error,
            text_color: p.text,
            placeholder_color: p.text_muted,
            capture_text_color: p.accent,
            disabled_text_color: p.disabled,
            height: DEFAULT_HEIGHT,
            min_width: DEFAULT_MIN_WIDTH,
            padding_x: DEFAULT_PADDING_X,
            radius: enzui_theme::text_defaults::shape_radius(|s| s.key),
            text_size: enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
            border_width: DEFAULT_BORDER_WIDTH,
            capture_border_width: DEFAULT_CAPTURE_BORDER_WIDTH,
        }
    }

    /// Dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replace non-finite or negative geometry with each field's default.
    /// All geometry, including text size, accepts zero. Invalid text size falls back
    /// to unscaled 11 px; colours pass through unchanged.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            height: FiniteNonNeg::new_or(self.height, DEFAULT_HEIGHT).get(),
            min_width: FiniteNonNeg::new_or(self.min_width, DEFAULT_MIN_WIDTH).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, DEFAULT_PADDING_X).get(),
            radius: FiniteNonNeg::new_or(
                self.radius,
                enzui_theme::text_defaults::shape_radius(|s| s.key),
            )
            .get(),
            text_size: FiniteNonNeg::new_or(self.text_size, DEFAULT_TEXT_SIZE).get(),
            border_width: FiniteNonNeg::new_or(self.border_width, DEFAULT_BORDER_WIDTH).get(),
            capture_border_width: FiniteNonNeg::new_or(
                self.capture_border_width,
                DEFAULT_CAPTURE_BORDER_WIDTH,
            )
            .get(),
            ..*self
        }
    }
}

/// Capture reports for the host to validate and apply; the binding is never mutated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyCapture {
    /// Request recording after pointer release, routed activation, or idle Enter/Space.
    /// The host arms this chip and disarms any other recorder in its window.
    Started,
    /// The user pressed a complete chord while armed. The host validates it.
    Candidate(Shortcut),
    /// Unmodified Backspace while armed: the user asked to unassign.
    Cleared,
    /// Request cancellation after unmodified Escape, reactivation, focus loss or deactivation.
    Cancelled,
}

/// Decision for one logical key press; the widget consumes repeats while armed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureAction {
    /// Emit a candidate chord. Not a commit — the host validates it.
    Candidate(Shortcut),
    /// Unassign the binding (unmodified Backspace).
    Clear,
    /// End capture with no change (Escape).
    Cancel,
    /// Wait without a candidate for bare modifiers or unidentified keys.
    Wait,
    /// Ignore an auto-repeat without emitting a report; the armed widget still consumes it.
    Ignore,
}

/// Recognize modifier and lock keys that cannot form a candidate by themselves.
#[must_use]
pub fn is_bare_modifier(key: &keyboard::Key) -> bool {
    use keyboard::key::Named;
    matches!(
        key,
        keyboard::Key::Named(
            Named::Control
                | Named::Shift
                | Named::Alt
                | Named::AltGraph
                | Named::Meta
                | Named::Super
                | Named::Hyper
                | Named::Fn
                | Named::FnLock
                | Named::CapsLock
                | Named::NumLock
                | Named::ScrollLock
                | Named::Symbol
                | Named::SymbolLock
        )
    )
}

/// Classify an unmodified logical key with its exact modifier set.
/// Use iced's `key`, not `modified_key`, text input or a physical scancode.
/// Repeats return Ignore. Unmodified Escape cancels; unmodified Backspace clears.
/// Modified forms are candidates. Bare modifiers and unidentified keys wait;
/// other keys, including Tab, Enter and Space, become candidates.
#[must_use]
pub fn capture_action(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    repeat: bool,
) -> CaptureAction {
    use keyboard::key::Named;

    if repeat {
        return CaptureAction::Ignore;
    }
    let enz_modifiers = Modifier::from_iced(modifiers);

    match key {
        keyboard::Key::Named(Named::Escape) if enz_modifiers == Modifier::NONE => {
            CaptureAction::Cancel
        }
        keyboard::Key::Named(Named::Backspace) if enz_modifiers == Modifier::NONE => {
            CaptureAction::Clear
        }
        keyboard::Key::Unidentified => CaptureAction::Wait,
        key if is_bare_modifier(key) => CaptureAction::Wait,
        keyboard::Key::Named(named) => {
            CaptureAction::Candidate(Shortcut::new(enz_modifiers, *named))
        }
        keyboard::Key::Character(text) => CaptureAction::Candidate(Shortcut::new(
            enz_modifiers,
            ShortcutKey::Character(text.clone()),
        )),
    }
}

/// Fill and glyph-ink state, with disabled before capture, hover, focus and idle.
/// The keyboard focus outline is independent of hover and is hidden while armed
/// or disabled. Enabled invalid feedback overrides the state outline ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipVisual {
    /// Not enabled. Inert; no capture, no focus ring.
    Disabled,
    /// Armed and listening for a chord.
    Capturing,
    /// Pointer over the chip (and not armed).
    Hovered,
    /// Keyboard-focused, not armed, not hovered.
    Focused,
    /// At rest.
    Idle,
}

/// Controlled key-capture chip. Build with [`enz_key_capture_chip`].
pub struct EnzKeyCaptureChip<'a, Message> {
    binding: Option<Shortcut>,
    armed: bool,
    enabled: bool,
    invalid: bool,
    capture_hint: String,
    empty_hint: String,
    label: String,
    tokens: Option<KeyCaptureChipTokens>,
    on_capture: Box<dyn Fn(KeyCapture) -> Message + 'a>,
    id: Option<iced::advanced::widget::Id>,
    id_prefix: String,
}

/// Default namespace for label-derived chip identities.
pub const DEFAULT_ID_PREFIX: &str = "enz-key-capture-chip";

/// Create an enabled, unarmed recorder with no binding.
/// The required label supplies its accessible name and derived identity key.
/// Labels must be unique within the namespace; use [`EnzKeyCaptureChip::id_prefix`]
/// or an explicit ID to disambiguate repeated labels.
pub fn enz_key_capture_chip<'a, Message>(
    label: impl Into<String>,
    on_capture: impl Fn(KeyCapture) -> Message + 'a,
) -> EnzKeyCaptureChip<'a, Message> {
    EnzKeyCaptureChip {
        binding: None,
        armed: false,
        enabled: true,
        invalid: false,
        capture_hint: enzui_core::localization::text("enzui-key-capture-chip-press-keys"),
        empty_hint: enzui_core::localization::text("enzui-key-capture-chip-unassigned"),
        label: label.into(),
        tokens: None,
        on_capture: Box::new(on_capture),
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
    }
}

impl<'a, Message> EnzKeyCaptureChip<'a, Message> {
    /// Current binding; `None` is unassigned.
    #[must_use]
    pub fn binding(mut self, binding: Option<Shortcut>) -> Self {
        self.binding = binding;
        self
    }

    /// Whether the host armed this chip.
    #[must_use]
    pub fn armed(mut self, armed: bool) -> Self {
        self.armed = armed;
        self
    }

    /// Whether the chip is enabled.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Rejected candidate; drives the invalid border and `Semantics::invalid`.
    #[must_use]
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Text shown while armed.
    #[must_use]
    pub fn capture_hint(mut self, hint: impl Into<String>) -> Self {
        self.capture_hint = hint.into();
        self
    }

    /// Text shown when unassigned and not armed.
    #[must_use]
    pub fn empty_hint(mut self, hint: impl Into<String>) -> Self {
        self.empty_hint = hint.into();
        self
    }

    /// Token override, sanitized so no non-finite geometry reaches the
    /// renderer.
    #[must_use]
    pub fn tokens(mut self, tokens: KeyCaptureChipTokens) -> Self {
        self.tokens = Some(tokens.sanitized());
        self
    }

    /// Override the namespace-and-label-derived identity with a stable, unique ID.
    #[must_use]
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Distinct namespace for same-labelled chips on one screen; ignored when
    /// `id` is set. Only needed for deliberate duplicate names.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }
    fn resolved_id(&self) -> iced::advanced::widget::Id {
        self.id
            .clone()
            .unwrap_or_else(|| enzui_core::identity::derive_id(&self.id_prefix, &self.label))
    }
    fn activation_report(&self) -> KeyCapture {
        if self.armed {
            KeyCapture::Cancelled
        } else {
            KeyCapture::Started
        }
    }
    fn text(&self) -> String {
        if self.armed {
            self.capture_hint.clone()
        } else {
            match &self.binding {
                Some(shortcut) => shortcut.localized_label(),
                None => self.empty_hint.clone(),
            }
        }
    }

    fn resolved_tokens(&self, theme: &iced::Theme) -> KeyCaptureChipTokens {
        self.tokens
            .unwrap_or_else(|| KeyCaptureChipTokens::from_theme(theme))
    }
    fn visual(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> ChipVisual {
        if !self.enabled {
            ChipVisual::Disabled
        } else if self.armed {
            ChipVisual::Capturing
        } else if cursor.is_over(bounds) {
            ChipVisual::Hovered
        } else if self.focus_ring_visible(state) {
            ChipVisual::Focused
        } else {
            ChipVisual::Idle
        }
    }
    fn focus_ring_visible(&self, state: &State) -> bool {
        self.enabled
            && !self.armed
            && enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key)
    }
    fn semantics(&self, focused: bool) -> Semantics {
        let value = match &self.binding {
            Some(shortcut) => shortcut.localized_label(),
            None => self.empty_hint.clone(),
        };
        let mut semantics = Semantics::new(Role::Button)
            .label(self.label.clone())
            .value(value)
            .default_action()
            .disabled(!self.enabled)
            .invalid(self.invalid)
            .focused(focused);
        if self.armed && self.enabled {
            semantics = semantics.description(self.capture_hint.clone());
        }
        semantics
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct State {
    focus: FocusState,
    is_pressed: bool,
    // Hover fill and keyboard outline can change independently.
    visual: Option<(ChipVisual, bool)>,
    // Operations latch requests; the event pass consumes each request once.
    activate_pending: bool,
    window_key: u64,
}

impl iced::advanced::widget::operation::Focusable for State {
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

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzKeyCaptureChip<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let tokens = self
            .tokens
            .unwrap_or_else(|| KeyCaptureChipTokens::from_palette(&Palette::dark()));
        let text_width = GraphicsParagraph::with_text(iced::advanced::text::Text {
            content: self.text().as_str(),
            bounds: Size::new(f32::INFINITY, tokens.height.max(1.0)),
            size: tokens.text_size.into(),
            line_height: iced::widget::text::LineHeight::default(),
            font: iced::Font::DEFAULT,
            align_x: iced::advanced::text::Alignment::Center,
            align_y: iced::alignment::Vertical::Center,
            shaping: iced::widget::text::Shaping::Advanced,
            wrapping: iced::widget::text::Wrapping::None,
            ellipsis: iced::widget::text::Ellipsis::End,
            hint_factor: None,
        })
        .min_bounds()
        .width;
        let width = (text_width + tokens.padding_x * 2.0).max(tokens.min_width);
        layout::Node::new(limits.resolve(
            Length::Shrink,
            Length::Shrink,
            Size::new(width, tokens.height),
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
        let id = self.resolved_id();
        let state = tree.state.downcast_mut::<State>();
        if self.enabled {
            operation.focusable(Some(&id), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        let focused = state.focus.is_focused();
        let mut node = SemanticsNode::new(self.semantics(focused));
        operation.custom(Some(&id), layout.bounds(), &mut node);
        if self.enabled {
            let mut request = crate::activation::ActivationRequest::new(id.clone());
            operation.custom(Some(&id), layout.bounds(), &mut request);
            if request.armed() {
                tree.state.downcast_mut::<State>().activate_pending = true;
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
        tree.state.downcast_mut::<State>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);
        // Drop focus before capture and disabled exits so outside presses can cancel recording.
        if let Some(decision) = press_focus_from_cursor(event, cursor, bounds) {
            let state = tree.state.downcast_mut::<State>();
            let was_focused = state.focus.is_focused();
            match decision {
                PressFocus::Take if self.enabled => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
            if was_focused && !state.focus.is_focused() && self.armed {
                shell.publish((self.on_capture)(KeyCapture::Cancelled));
            }
        }
        if self.armed
            && let Event::Window(iced::window::Event::Unfocused) = event
        {
            shell.publish((self.on_capture)(KeyCapture::Cancelled));
            shell.request_redraw();
        }
        if let Event::Window(iced::window::Event::RedrawRequested(_)) = event {
            let painted = {
                let state = tree.state.downcast_ref::<State>();
                (
                    self.visual(state, bounds, cursor),
                    self.focus_ring_visible(state),
                )
            };
            tree.state.downcast_mut::<State>().visual = Some(painted);
        }
        {
            let state = tree.state.downcast_mut::<State>();
            if state.activate_pending {
                state.activate_pending = false;
                shell.publish((self.on_capture)(self.activation_report()));
                shell.request_redraw();
            }
        }

        if shell.is_event_captured() || !self.enabled {
            if !self.enabled {
                let state = tree.state.downcast_mut::<State>();
                if state.is_pressed {
                    state.is_pressed = false;
                    shell.request_redraw();
                }
            }
            self.request_redraw_on_visual_change(tree, event, bounds, cursor, shell);
            return;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let handles_event = cursor.is_over(bounds);
                if handles_event {
                    let state = tree.state.downcast_mut::<State>();
                    state.is_pressed = true;
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Touch(touch::Event::FingerPressed { position, .. }) => {
                let handles_event = bounds.contains(*position);
                if handles_event {
                    let state = tree.state.downcast_mut::<State>();
                    state.is_pressed = true;
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let state = tree.state.downcast_mut::<State>();
                if state.is_pressed {
                    state.is_pressed = false;
                    if cursor.is_over(bounds) {
                        shell.publish((self.on_capture)(self.activation_report()));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Touch(touch::Event::FingerLifted { position, .. }) => {
                let state = tree.state.downcast_mut::<State>();
                if state.is_pressed {
                    state.is_pressed = false;
                    if bounds.contains(*position) {
                        shell.publish((self.on_capture)(self.activation_report()));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Touch(touch::Event::FingerLost { .. }) => {
                tree.state.downcast_mut::<State>().is_pressed = false;
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                repeat,
                ..
            }) => {
                let focused = tree.state.downcast_ref::<State>().focus.is_focused();
                if !focused {
                } else if self.armed {
                    match capture_action(key, *modifiers, *repeat) {
                        CaptureAction::Candidate(shortcut) => {
                            shell.publish((self.on_capture)(KeyCapture::Candidate(shortcut)));
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        CaptureAction::Clear => {
                            shell.publish((self.on_capture)(KeyCapture::Cleared));
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        CaptureAction::Cancel => {
                            shell.publish((self.on_capture)(KeyCapture::Cancelled));
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        CaptureAction::Wait => shell.capture_event(),
                        // Repeats must not leak to app accelerators during recording.
                        CaptureAction::Ignore => shell.capture_event(),
                    }
                } else if !*repeat {
                    use keyboard::key::Named;
                    if matches!(key, keyboard::Key::Named(Named::Enter | Named::Space))
                        && Modifier::from_iced(*modifiers) == Modifier::NONE
                    {
                        shell.publish((self.on_capture)(KeyCapture::Started));
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Keyboard(
                keyboard::Event::KeyReleased { .. } | keyboard::Event::ModifiersChanged(_),
            ) if self.armed && tree.state.downcast_ref::<State>().focus.is_focused() => {
                shell.capture_event();
            }
            _ => {}
        }

        self.request_redraw_on_visual_change(tree, event, bounds, cursor, shell);
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
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();
        let tokens = self.resolved_tokens(theme);
        let visual = self.visual(state, bounds, cursor);

        let (background, mut border_color, border_width, text_color) = match visual {
            ChipVisual::Disabled => (
                tokens.disabled_background,
                tokens.border,
                tokens.border_width,
                tokens.disabled_text_color,
            ),
            ChipVisual::Capturing => (
                tokens.capture_background,
                tokens.capture_border,
                tokens.capture_border_width,
                tokens.capture_text_color,
            ),
            ChipVisual::Hovered => (
                tokens.hover_background,
                tokens.hover_border,
                tokens.border_width,
                self.ink(&tokens),
            ),
            ChipVisual::Focused => (
                tokens.background,
                tokens.focus_border,
                tokens.border_width,
                self.ink(&tokens),
            ),
            ChipVisual::Idle => (
                tokens.background,
                tokens.border,
                tokens.border_width,
                self.ink(&tokens),
            ),
        };
        // Keyboard focus remains visible under hover; invalid feedback takes precedence below.
        if self.focus_ring_visible(state) {
            border_color = tokens.focus_border;
        }
        if self.invalid && self.enabled {
            border_color = tokens.invalid_border;
        }

        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: border_color,
                    width: border_width,
                    radius: tokens.radius.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            },
            Background::Color(background),
        );

        renderer.fill_text(
            iced::advanced::text::Text {
                content: self.text(),
                bounds: bounds.size(),
                size: tokens.text_size.into(),
                line_height: iced::widget::text::LineHeight::default(),
                font: iced::Font::DEFAULT,
                align_x: iced::advanced::text::Alignment::Center,
                align_y: iced::alignment::Vertical::Center,
                shaping: iced::widget::text::Shaping::Advanced,
                wrapping: iced::widget::text::Wrapping::None,
                ellipsis: iced::widget::text::Ellipsis::End,
                hint_factor: None,
            },
            Point::new(bounds.center_x(), bounds.center_y()),
            text_color,
            bounds,
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
        if self.enabled && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<Message> EnzKeyCaptureChip<'_, Message> {
    fn request_redraw_on_visual_change<M>(
        &self,
        tree: &Tree,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, M>,
    ) {
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            return;
        }
        let state = tree.state.downcast_ref::<State>();
        let current = (
            self.visual(state, bounds, cursor),
            self.focus_ring_visible(state),
        );
        if state.visual.is_some_and(|painted| painted != current) {
            shell.request_redraw();
        }
    }
    fn ink(&self, tokens: &KeyCaptureChipTokens) -> Color {
        if self.binding.is_some() {
            tokens.text_color
        } else {
            tokens.placeholder_color
        }
    }
}

impl<'a, Message: 'a> EnzKeyCaptureChip<'a, Message> {
    /// Build the recorder as an iced element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzKeyCaptureChip<'a, Message>> for Element<'a, Message> {
    fn from(chip: EnzKeyCaptureChip<'a, Message>) -> Self {
        Self::new(chip)
    }
}
