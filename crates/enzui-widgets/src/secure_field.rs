//! A controlled password field with an in-frame reveal toggle.
//!
//! Enabled fields mask glyphs unless the caller sets `revealed`. Disabled
//! non-empty fields always show static bullets and expose no input focus target. Read-only
//! fields retain selection, focus and the reveal toggle while refusing edits.
//! The accessibility node stays a valueless [`Role::PasswordInput`] in either
//! reveal state. Masked inputs suppress keyboard Copy and Cut; revealed inputs
//! permit them. Edits still reach the caller through `on_input`; caller-supplied
//! labels and captions are published normally.
//!
//! [`restricted_context_menu`] supplies Paste and Select All only. Context-host
//! builders accept the caller's model unchanged, so the caller maintains this
//! restriction and dispatches recognised entries through [`edit_command`].

use crate::field_chrome::{ChromePaint, ChromeStatus, FieldChrome};
use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use enzui_core::menu_model::{MenuEntryModel, MenuModel};
use enzui_core::semantics::{Role, Semantics, Toggle};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::text::Style as TextStyle;
use iced::widget::text_input::{Status, Style as InputStyle};
use iced::widget::{Row, container, text, text_input};
use iced::{Background, Border, Color, Element, Length, Padding};
use std::borrow::Cow;
use std::rc::Rc;

pub use edit_command::{RunEditCommand, SecureFieldCommand, SecureFieldRequest, edit_command};

pub use crate::text_field::TextFieldSize as FieldSize;

/// Reveal-button geometry in logical pixels; colours resolve separately from the palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SecureFieldTokens {
    /// Eye glyph size; the default is 14 logical pixels.
    pub eye_icon_size: f32,
    /// Amount subtracted from field height to size the square eye button; defaults to 8 px.
    pub eye_inset: f32,
}

impl Default for SecureFieldTokens {
    fn default() -> Self {
        Self {
            eye_icon_size: 14.0,
            eye_inset: 8.0,
        }
    }
}

impl SecureFieldTokens {
    fn eye_side(self, field_height: f32) -> f32 {
        (field_height - self.eye_inset).max(0.0)
    }

    /// Replace non-finite or negative geometry with default values, preserving zero.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::default();
        Self {
            eye_icon_size: FiniteNonNeg::new_or(self.eye_icon_size, d.eye_icon_size).get(),
            eye_inset: FiniteNonNeg::new_or(self.eye_inset, d.eye_inset).get(),
        }
    }
}

/// Role suffix used to derive the reveal toggle's identity from its field.
pub const EYE_ID_SUFFIX: &str = "reveal";

/// Derive a reveal-toggle identity from a stable field identity.
/// An explicit field identity keeps the toggle stable through reorder and label changes.
#[must_use]
pub fn reveal_toggle_id(field_id: &iced::advanced::widget::Id) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(&format!("{field_id:?}"), EYE_ID_SUFFIX)
}

/// Entry identity for Paste in [`restricted_context_menu`].
pub const PASTE_ID: &str = "paste";

/// Entry identity for Select All in [`restricted_context_menu`].
pub const SELECT_ALL_ID: &str = "select-all";

/// Build Paste and Select All rows, omitting Cut and Copy.
/// Read-only disables Paste; an empty value disables Select All. The caller owns
/// the returned model and any later changes to its entries.
#[must_use]
pub fn restricted_context_menu(read_only: bool, value_is_empty: bool) -> MenuModel {
    MenuModel::new(vec![
        MenuEntryModel::command(
            PASTE_ID,
            enzui_core::localization::text("enzui-secure-field-paste"),
        )
        .icon("action.paste")
        .enabled(!read_only),
        MenuEntryModel::command(
            SELECT_ALL_ID,
            enzui_core::localization::text("enzui-secure-field-select-all"),
        )
        .icon("selection.check")
        .enabled(!value_is_empty),
    ])
}

/// Map the Paste and Select All entry identities to edit commands.
/// Unknown identities return `None`; enabled-state validation belongs to the menu host.
#[must_use]
pub fn command_for_entry(entry: &enzui_core::menu_model::EntryId) -> Option<SecureFieldCommand> {
    match entry.as_str() {
        PASTE_ID => Some(SecureFieldCommand::Paste),
        SELECT_ALL_ID => Some(SecureFieldCommand::SelectAll),
        _ => None,
    }
}
mod edit_command {
    use iced::Rectangle;
    use iced::advanced::widget::{Id, Operation, operation::Outcome};
    /// Edit commands supported by the secure-field operation carrier.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SecureFieldCommand {
        /// Request clipboard text through the input's paste path.
        Paste,
        /// Focus the input and select its complete value.
        SelectAll,
    }

    /// Operation carrier that holds one command for an identified secure field.
    #[derive(Debug, Clone)]
    pub struct SecureFieldRequest {
        id: Id,
        command: Option<SecureFieldCommand>,
    }

    impl SecureFieldRequest {
        /// Create an empty command carrier for the field identity.
        pub fn new(id: Id) -> Self {
            Self { id, command: None }
        }

        /// Identity of the field publishing this carrier.
        pub fn id(&self) -> &Id {
            &self.id
        }

        /// Remove and return the delivered command.
        pub fn take(&mut self) -> Option<SecureFieldCommand> {
            self.command.take()
        }

        /// Store a command, replacing any command already in this carrier.
        pub fn deliver(&mut self, command: SecureFieldCommand) {
            self.command = Some(command);
        }
    }

    /// Arm the first matching secure field for command execution on its next event pass.
    /// The latch persists across operation traversals and windows. Disabled or
    /// anonymous fields publish no carrier; an unknown target arms nothing.
    /// Multiple operations before an event replace the pending command; the last executes.
    #[derive(Debug, Clone)]
    pub struct RunEditCommand {
        target: Id,
        command: SecureFieldCommand,
        // One operation may traverse multiple windows; only the first match is armed.
        armed: bool,
    }

    impl RunEditCommand {
        /// Create an unarmed operation for one target and command.
        pub fn new(target: Id, command: SecureFieldCommand) -> Self {
            Self {
                target,
                command,
                armed: false,
            }
        }

        /// Field identity targeted by this operation.
        pub fn target(&self) -> &Id {
            &self.target
        }

        /// Command carried by this operation.
        pub fn command(&self) -> SecureFieldCommand {
            self.command
        }

        /// Whether a matching carrier accepted the command during traversal.
        pub fn armed(&self) -> bool {
            self.armed
        }
    }

    impl<T> Operation<T> for RunEditCommand {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
            operate(self);
        }

        fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
            if self.armed {
                return;
            }
            let Some(request) = state.downcast_mut::<SecureFieldRequest>() else {
                return;
            };
            if request.id() != &self.target {
                return;
            }
            self.armed = true;
            request.deliver(self.command);
        }

        fn finish(&self) -> Outcome<T> {
            Outcome::None
        }
    }

    /// Create the operation a host runs for a recognised restricted-menu entry.
    /// Select All focuses the input and selects its value on the next event pass.
    /// Paste focuses the input and requests clipboard text; a later clipboard event
    /// commits through the input callback. The operation does not validate menu-row
    /// enabled state, so the host must reject disabled activations.
    #[must_use]
    pub fn edit_command(id: Id, command: SecureFieldCommand) -> RunEditCommand {
        RunEditCommand::new(id, command)
    }
}
mod command_host {
    use super::edit_command::{SecureFieldCommand, SecureFieldRequest};
    use iced::advanced::widget::{Id, Operation, Tree, operation, tree};
    use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
    use iced::{Element, Event, Length, Rectangle, Size, Vector};
    #[derive(Debug, Default)]
    pub(super) struct State {
        // The operation-to-event latch must survive host view rebuilds.
        pending: Option<SecureFieldCommand>,
    }

    pub(super) struct CommandHost<'a, Message> {
        child: Element<'a, Message>,
        published_id: Option<Id>,
        probe_id: Id,
    }

    impl<'a, Message> CommandHost<'a, Message> {
        pub(super) fn new(
            child: impl Into<Element<'a, Message>>,
            published_id: Option<Id>,
            probe_id: Id,
        ) -> Self {
            Self {
                child: child.into(),
                published_id,
                probe_id,
            }
        }
        fn run(
            &mut self,
            tree: &mut Tree,
            layout: Layout,
            renderer: &iced::Renderer,
            op: &mut dyn Operation<()>,
        ) {
            let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(op));
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                &layout.bounds(),
                renderer,
                &mut *boxed,
            );
        }
    }
    fn command_modifier() -> iced::keyboard::Modifiers {
        #[cfg(target_os = "macos")]
        {
            iced::keyboard::Modifiers::LOGO
        }
        #[cfg(not(target_os = "macos"))]
        {
            iced::keyboard::Modifiers::CTRL
        }
    }

    impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CommandHost<'_, Message> {
        fn tag(&self) -> tree::Tag {
            tree::Tag::of::<State>()
        }

        fn state(&self) -> tree::State {
            tree::State::new(State::default())
        }

        fn diff(&mut self, tree: &mut Tree) {
            tree.diff_children(std::slice::from_mut(&mut self.child));
        }

        fn is_void(&self) -> bool {
            self.child.as_widget().is_void()
        }

        fn size(&self) -> Size<Length> {
            self.child.as_widget().size()
        }

        fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
            iced::advanced::layout::pass_through(tree, 0);
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout,
            viewport: &Rectangle,
            renderer: &iced::Renderer,
            operation: &mut dyn Operation,
        ) {
            if let Some(id) = self.published_id.as_ref() {
                let mut request = SecureFieldRequest::new(id.clone());
                operation.custom(Some(id), layout.bounds(), &mut request);
                if let Some(command) = request.take() {
                    tree.state.downcast_mut::<State>().pending = Some(command);
                }
            }
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        }

        fn update(
            &mut self,
            tree: &mut Tree,
            event: &Event,
            layout: Layout,
            cursor: mouse::Cursor,
            renderer: &iced::Renderer,
            shell: &mut Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            // Drain before child delivery so later events cannot replay the command.
            let pending = tree.state.downcast_mut::<State>().pending.take();
            if let Some(command) = pending {
                let mut focus = operation::focusable::focus::<()>(self.probe_id.clone());
                self.run(tree, layout, renderer, &mut focus);

                match command {
                    SecureFieldCommand::SelectAll => {
                        let mut select =
                            operation::text_input::select_all::<()>(self.probe_id.clone());
                        self.run(tree, layout, renderer, &mut select);
                    }
                    SecureFieldCommand::Paste => {
                        // Clipboard delivery is asynchronous; replay Paste and reset synthetic modifiers.
                        let restore = iced::keyboard::Modifiers::default();
                        let v = iced::keyboard::Key::Character("v".into());
                        let events = [
                            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(
                                command_modifier(),
                            )),
                            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                                key: v.clone(),
                                modified_key: v,
                                physical_key: iced::keyboard::key::Physical::Code(
                                    iced::keyboard::key::Code::KeyV,
                                ),
                                location: iced::keyboard::Location::Standard,
                                modifiers: command_modifier(),
                                text: None,
                                repeat: false,
                            }),
                            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(restore)),
                        ];
                        for synthetic in &events {
                            self.child.as_widget_mut().update(
                                &mut tree.children[0],
                                synthetic,
                                layout,
                                cursor,
                                renderer,
                                shell,
                                viewport,
                            );
                        }
                    }
                }
                shell.request_redraw();
            }

            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }

        fn draw(
            &self,
            tree: &Tree,
            renderer: &mut iced::Renderer,
            theme: &iced::Theme,
            style: &renderer::Style,
            layout: Layout,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        }

        fn mouse_interaction(
            &self,
            tree: &Tree,
            layout: Layout,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
            renderer: &iced::Renderer,
        ) -> mouse::Interaction {
            self.child.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        }

        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut Tree,
            layout: Layout,
            renderer: &iced::Renderer,
            viewport: &Rectangle,
            translation: Vector,
            window: Size,
        ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
            self.child.as_widget_mut().overlay(
                &mut tree.children[0],
                layout,
                renderer,
                viewport,
                translation,
                window,
            )
        }
    }

    impl<'a, Message: 'a> From<CommandHost<'a, Message>> for Element<'a, Message> {
        fn from(w: CommandHost<'a, Message>) -> Self {
            Element::new(w)
        }
    }
}

/// Controlled password input with a reveal toggle, captions and redacted accessibility.
#[must_use]
pub struct EnzSecureField<'a, Message> {
    value: &'a str,
    on_input: Box<dyn Fn(String) -> Message + 'a>,
    revealed: bool,
    on_toggle: Box<dyn Fn() -> Message + 'a>,
    on_submit: Option<Message>,
    on_submit_lazy: Option<Box<dyn Fn() -> Message + 'a>>,
    label: Cow<'a, str>,
    aria_label: Option<String>,
    placeholder: Cow<'a, str>,
    size: FieldSize,
    radius: Option<f32>,
    error_text: Option<Cow<'a, str>>,
    description: Option<Cow<'a, str>>,
    enabled: bool,
    read_only: bool,
    edit_menu: bool,
    id: Option<iced::advanced::widget::Id>,
    id_prefix: String,
    palette: Option<Palette>,
    tokens: SecureFieldTokens,
    variant: crate::tokens::FieldVariant,
}

/// Default namespace for identities derived from a visible label, aria label or placeholder.
pub const DEFAULT_ID_PREFIX: &str = "enz-secure-field";
/// Construct a controlled password input with a cloned reveal-toggle message.
/// The caller owns both value and reveal state. Defaults to enabled, editable,
/// medium-sized filled chrome, with no label, placeholder, captions or submit message.
pub fn enz_secure_field<'a, Message: Clone + 'a>(
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
    revealed: bool,
    on_toggle: Message,
) -> EnzSecureField<'a, Message> {
    enz_secure_field_with(value, on_input, revealed, move || on_toggle.clone())
}

/// Construct a password input whose reveal callback runs when the eye is activated.
/// Has the same defaults and controlled value/reveal state as [`enz_secure_field`].
pub fn enz_secure_field_with<'a, Message: Clone + 'a>(
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
    revealed: bool,
    on_toggle: impl Fn() -> Message + 'a,
) -> EnzSecureField<'a, Message> {
    EnzSecureField {
        value,
        on_input: Box::new(on_input),
        revealed,
        on_toggle: Box::new(on_toggle),
        on_submit: None,
        on_submit_lazy: None,
        label: Cow::Borrowed(""),
        aria_label: None,
        placeholder: Cow::Borrowed(""),
        size: FieldSize::default(),
        radius: None,
        error_text: None,
        description: None,
        enabled: true,
        edit_menu: true,
        read_only: false,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        palette: None,
        tokens: SecureFieldTokens::default(),
        variant: crate::tokens::FieldVariant::default(),
    }
}

impl<'a, Message: Clone + 'a> EnzSecureField<'a, Message> {
    /// Set the visible label above the field; empty by default.
    /// A non-empty label takes precedence for the accessible name and derived identity.
    pub fn label(mut self, l: impl Into<Cow<'a, str>>) -> Self {
        self.label = l.into();
        self
    }

    /// Set the empty-field placeholder; empty by default.
    /// Also supplies the name and derived identity when visible and aria labels are empty.
    pub fn placeholder(mut self, p: impl Into<Cow<'a, str>>) -> Self {
        self.placeholder = p.into();
        self
    }

    /// Set the field size tier; defaults to [`FieldSize::Md`].
    /// The tier controls height, padding, text size and unpinned radius.
    pub fn size(mut self, s: FieldSize) -> Self {
        self.size = s;
        self
    }

    /// Pin field and eye-button corner radius in logical pixels.
    /// Negative or non-finite pins become zero. Without a pin, radius follows the
    /// live size-tier radius even with an explicit palette.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }

    /// Set fill treatment; defaults to [`crate::tokens::FieldVariant::Filled`].
    /// Outlined fields use the palette surface instead of its alternate surface.
    pub fn variant(mut self, variant: crate::tokens::FieldVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the error caption and invalid semantics; an empty string clears both.
    /// A non-empty error replaces the description and takes border-colour precedence.
    pub fn error(mut self, t: impl Into<Cow<'a, str>>) -> Self {
        let t = t.into();
        self.error_text = if t.is_empty() { None } else { Some(t) };
        self
    }

    /// Set the helper caption and accessible description; an empty string clears it.
    /// A non-empty error takes precedence.
    pub fn description(mut self, t: impl Into<Cow<'a, str>>) -> Self {
        let t = t.into();
        self.description = if t.is_empty() { None } else { Some(t) };
        self
    }

    /// Enable input and the reveal toggle; enabled by default.
    /// Disabled non-empty fields show static bullets even when revealed, cannot focus or edit,
    /// and publish no menu-command carrier or context-menu invocation.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    /// Whether a right click opens the built-in edit menu (Paste and Select
    /// All; Cut and Copy too while revealed). On by default;
    /// [`into_context_menu_host`](Self::into_context_menu_host) turns it off.
    pub fn edit_menu(mut self, enabled: bool) -> Self {
        self.edit_menu = enabled;
        self
    }

    /// Retain normal chrome, input focus, selection and reveal while refusing edits.
    /// Defaults to false. Use a read-only restricted-menu model to disable Paste.
    pub fn read_only(mut self, r: bool) -> Self {
        self.read_only = r;
        self
    }

    /// Set the message emitted on Enter in a focused, editable input.
    /// A configured [`Self::on_submit_with`] factory takes precedence.
    pub fn on_submit(mut self, m: Message) -> Self {
        self.on_submit = Some(m);
        self
    }

    /// Set a submit factory called at Enter time for a focused, editable input.
    /// Suppresses the eager submit message, regardless of setter order.
    pub fn on_submit_with(mut self, f: impl Fn() -> Message + 'a) -> Self {
        self.on_submit_lazy = Some(Box::new(f));
        self
    }

    /// Replace the reveal callback with a factory called when the eye is activated.
    pub fn on_toggle_with(mut self, f: impl Fn() -> Message + 'a) -> Self {
        self.on_toggle = Box::new(f);
        self
    }

    /// Set a stable identity for focus and routed actions, overriding name-derived identity.
    /// Use distinct identities for fields sharing a name in the same view.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Set the namespace for a name-derived identity.
    /// Defaults to [`DEFAULT_ID_PREFIX`]; ignored when [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Set an accessible name without adding a visible label.
    /// A non-empty visible label wins; an empty aria label falls back to the placeholder.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }
    fn effective_id(&self) -> Option<iced::advanced::widget::Id> {
        self.id.clone().or_else(|| {
            let key: String = if !self.label.is_empty() {
                self.label.to_string()
            } else if let Some(aria) = self.aria_label.as_deref()
                && !aria.is_empty()
            {
                aria.to_owned()
            } else if !self.placeholder.is_empty() {
                self.placeholder.to_string()
            } else {
                return None;
            };
            Some(enzui_core::identity::derive_id(&self.id_prefix, &key))
        })
    }

    /// Pin colours to an explicit palette instead of resolving the runtime theme.
    /// Size-tier radius remains live unless pinned separately.
    pub fn palette(mut self, p: Palette) -> Self {
        self.palette = Some(p);
        self
    }

    /// Set eye geometry after replacing non-finite or negative values with defaults.
    /// Zero values are preserved; this setter does not change palette colours.
    pub fn tokens(mut self, t: SecureFieldTokens) -> Self {
        self.tokens = t.sanitized();
        self
    }
    fn semantics(&self) -> Semantics {
        let mut s = Semantics::new(Role::PasswordInput)
            .disabled(!self.enabled)
            .read_only(self.read_only)
            .editable(true)
            .invalid(self.error_text.is_some());
        if !self.label.is_empty() {
            s = s.label(self.label.to_string());
        } else if let Some(aria) = self.aria_label.as_deref()
            && !aria.is_empty()
        {
            s = s.label(aria.to_owned());
        } else if !self.placeholder.is_empty() {
            s = s.label(self.placeholder.to_string());
        }
        if let Some(err) = self.error_text.as_deref() {
            s = s.description(err.to_owned());
        } else if let Some(desc) = self.description.as_deref() {
            s = s.description(desc.to_owned());
        }
        s
    }

    /// Build the field, reveal toggle and optional captions.
    /// The input row publishes password semantics without the value. Optional label
    /// and caption changes preserve field state through keyed reconciliation.
    pub fn into_element(self) -> Element<'a, Message> {
        let size_tokens = self.size.tokens();
        let field_height = size_tokens.height;
        let size = self.size;
        let radius_pin = self.radius;
        let padding_x = size_tokens.padding_x;
        let text_size = size_tokens.text_size;
        let has_error = self.error_text.is_some();
        let enabled = self.enabled;
        let variant = self.variant;
        let read_only = self.read_only;
        let revealed = self.revealed;
        let eye_side = self.tokens.eye_side(field_height);
        let eye_icon_size = self.tokens.eye_icon_size;

        let field_semantics = self.semantics();
        let pinned = self.palette.clone();
        let user_id = self.effective_id();
        let probe_id = user_id
            .clone()
            .unwrap_or_else(iced::advanced::widget::Id::unique);

        let interactive = enabled && !read_only;
        // Native edits and routed SetValue must share the same commit callback.
        let on_input: Rc<dyn Fn(String) -> Message + 'a> = Rc::from(self.on_input);
        let middle: Element<'a, Message> = if enabled {
            let mut input = text_input(&self.placeholder, self.value)
                .size(text_size)
                .padding(Padding::from([size_tokens.padding_y(), 0.0]))
                .width(Length::Fill)
                .secure(!revealed)
                .id(probe_id.clone());
            if interactive {
                let typed = Rc::clone(&on_input);
                input = input.on_input(move |s| typed(s));
                if let Some(msg) = self.on_submit.filter(|_| self.on_submit_lazy.is_none()) {
                    input = input.on_submit(msg);
                }
            }
            let input_pinned = pinned.clone();
            input = input.style(move |theme: &iced::Theme, status: Status| {
                let p = input_pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                input_style(
                    &p,
                    status,
                    !enabled,
                    crate::theme_lookup::opacity(theme).disabled(),
                )
            });
            if self.edit_menu {
                crate::edit_menu::edit_menu(
                    input.into(),
                    crate::edit_menu::EditMenuOptions {
                        editable: interactive,
                        secure: !revealed,
                    },
                )
            } else {
                input.into()
            }
        } else {
            let is_placeholder = self.value.is_empty();
            let shown = if is_placeholder {
                self.placeholder.to_string()
            } else {
                // Disabled fields with a stale reveal flag must still mask their static display.
                "•".repeat(self.value.chars().count())
            };
            let disp_pinned = pinned.clone();
            iced::widget::text(shown)
                .size(text_size)
                .style(move |theme: &iced::Theme| {
                    let p = disp_pinned
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                    TextStyle {
                        color: Some(crate::color::disabled_text_ink(&p, p.surface_alt)),
                    }
                })
                .width(Length::Fill)
                .into()
        };
        let eye_name = if revealed {
            "status.hidden"
        } else {
            "status.visible"
        };
        let eye_label = if revealed {
            enzui_core::localization::text("enzui-secure-field-hide-password")
        } else {
            enzui_core::localization::text("enzui-secure-field-show-password")
        };
        let eye_pinned = pinned.clone();
        let eye_icon = crate::icon::enz_icon(eye_name)
            .size(eye_icon_size)
            .tint_resolved(move |theme: &iced::Theme| {
                let p = eye_pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                if enabled {
                    p.text_muted
                } else {
                    crate::color::disabled_text_ink(&p, p.surface_alt)
                }
            });
        let toggle_msg = if enabled { Some(self.on_toggle) } else { None };
        let eye_ring_pinned = pinned.clone();
        let mut eye_btn = focus_button_maybe_with(
            container(eye_icon)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            toggle_msg,
        )
        .padding(Padding::ZERO)
        .width(Length::Fixed(eye_side))
        .height(Length::Fixed(eye_side))
        .semantics(
            Semantics::new(Role::Button)
                .label(eye_label)
                .disabled(!enabled)
                .toggled(if revealed {
                    Toggle::True
                } else {
                    Toggle::False
                })
                .default_action(),
        )
        .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
            let p = eye_ring_pinned
                .clone()
                .unwrap_or_else(|| crate::theme_lookup::palette(theme));
            let bg = match status {
                FocusStatus::Hovered => p.accent_soft,
                FocusStatus::Pressed => p.surface_alt,
                _ => Color::TRANSPARENT,
            };
            let (border_c, border_w) = if matches!(status, FocusStatus::Focused) {
                (p.focus_ring, 2.0)
            } else {
                (Color::TRANSPARENT, 0.0)
            };
            ButtonStyle {
                background: Some(Background::Color(bg)),
                border: Border {
                    color: border_c,
                    width: border_w,
                    radius: size.resolve_radius(theme, radius_pin).into(),
                },
                ..ButtonStyle::default()
            }
        });
        if let Some(field_id) = user_id.as_ref() {
            eye_btn = eye_btn.id(reveal_toggle_id(field_id));
        }
        let inner = Row::new()
            .align_y(iced::alignment::Vertical::Center)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(middle)
            .push(container(eye_btn).center_y(Length::Fill));
        let chrome_pinned = pinned.clone();
        let field: Element<'a, Message> = FieldChrome::new(
            inner,
            field_height,
            Padding {
                top: 0.0,
                bottom: 0.0,
                left: padding_x,
                right: 4.0,
            },
            move |theme: &iced::Theme, status: ChromeStatus| {
                let p = chrome_pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                let (border_color, border_width) = if has_error {
                    (
                        p.error,
                        if matches!(status, ChromeStatus::Focused) {
                            2.0
                        } else {
                            1.0
                        },
                    )
                } else {
                    match status {
                        ChromeStatus::Focused => (p.focus_ring, 2.0),
                        ChromeStatus::Hovered => {
                            (crate::tokens::field_edge(&p, variant, true), 1.0)
                        }
                        ChromeStatus::Active | ChromeStatus::Inert => {
                            (crate::tokens::field_edge(&p, variant, false), 1.0)
                        }
                    }
                };
                ChromePaint {
                    background: dim_if(crate::tokens::field_fill_for(&p, variant), !enabled, theme),
                    border_color: dim_if(border_color, !enabled, theme),
                    border_width,
                    radius: size.resolve_radius(theme, radius_pin),
                }
            },
        )
        .interactive(enabled)
        .into();
        let label_el: Option<Element<'a, Message>> = if !self.label.is_empty() {
            let lbl_pinned = pinned.clone();
            Some(
                text(self.label)
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| TextStyle {
                        color: Some(dim_if(
                            lbl_pinned
                                .clone()
                                .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                                .text_muted,
                            !enabled,
                            theme,
                        )),
                    })
                    .font(enzui_theme::text_defaults::font(
                        enzui_theme::FontRole::Ui,
                        enzui_theme::WeightRung::Medium,
                    ))
                    .into(),
            )
        } else {
            None
        };
        let routed = Rc::clone(&on_input);
        let field_node: Element<'a, Message> =
            crate::text_field_semantics::TextFieldSemantics::new(
                field,
                probe_id.clone(),
                user_id.clone(),
                field_semantics,
            )
            .on_input(move |s| routed(s))
            .on_submit_with_maybe(self.on_submit_lazy.filter(|_| interactive))
            .into();
        let field_el: Element<'a, Message> = command_host::CommandHost::new(
            field_node,
            enabled.then(|| user_id.clone()).flatten(),
            probe_id,
        )
        .into();

        let caption_el: Option<Element<'a, Message>> = if let Some(err) = self.error_text {
            let err_pinned = pinned;
            Some(
                text(err)
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| TextStyle {
                        color: Some(dim_if(
                            err_pinned
                                .clone()
                                .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                                .error,
                            !enabled,
                            theme,
                        )),
                    })
                    .into(),
            )
        } else if let Some(desc) = self.description {
            let desc_pinned = pinned;
            Some(
                text(desc)
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| TextStyle {
                        color: Some(dim_if(
                            desc_pinned
                                .clone()
                                .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                                .text_muted,
                            !enabled,
                            theme,
                        )),
                    })
                    .into(),
            )
        } else {
            None
        };
        crate::field_column::FieldColumn::new(4.0)
            .width(Length::Fill)
            .slot("label", label_el)
            .field("field", field_el)
            .slot("caption", caption_el)
            .into_element()
    }
}

impl<'a, Message: Clone + 'a> EnzSecureField<'a, Message> {
    /// Build a context-menu host around the field using the supplied model unchanged.
    /// Use [`restricted_context_menu`] for Paste and Select All only. An explicit
    /// [`Self::id`] supplies the focus-restoration invoker; derived field identities
    /// are not forwarded. Disabled fields suppress invocation.
    #[must_use]
    pub fn into_context_host_with(
        self,
        model: &'a MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        self.into_context_menu_host(model, state, on_event)
            .into_element()
    }

    /// Configure a context-menu host before conversion to an element.
    /// The supplied model is unchanged; the host does not enforce the restricted row
    /// set. Only an explicit field id is forwarded as invoker. The menu name uses
    /// the visible label or placeholder, without the aria-label fallback.
    pub fn into_context_menu_host(
        self,
        model: &'a MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> crate::context_menu_host::EnzContextMenuHost<'a, Message> {
        let invoker = self.id.clone();
        let enabled = self.enabled;
        let label = if !self.label.is_empty() {
            Some(self.label.to_string())
        } else if !self.placeholder.is_empty() {
            Some(self.placeholder.to_string())
        } else {
            None
        };

        let mut host = crate::context_menu_host::enz_context_menu_host(
            self.edit_menu(false).into_element(),
            model,
            state,
            on_event,
        )
        .enabled(enabled);
        if let Some(invoker) = invoker {
            host = host.invoker(invoker);
        }
        if let Some(label) = label {
            host = host.label(label);
        }
        host
    }
}

impl<'a, Message: Clone + 'a> From<EnzSecureField<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzSecureField<'a, Message>) -> Self {
        w.into_element()
    }
}
fn dim_if(c: Color, disabled: bool, theme: &iced::Theme) -> Color {
    if disabled {
        crate::color::with_opacity(c, crate::theme_lookup::opacity(theme).disabled())
    } else {
        c
    }
}
fn input_style(p: &Palette, status: Status, disabled: bool, disabled_opacity: f32) -> InputStyle {
    let disabled = disabled || matches!(status, Status::Disabled);
    let dis_text = crate::color::disabled_text_ink(p, p.surface_alt);
    InputStyle {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: if disabled { dis_text } else { p.text_muted },
        placeholder: if disabled { dis_text } else { p.text_muted },
        value: if disabled {
            crate::color::with_opacity(p.text, disabled_opacity)
        } else {
            p.text
        },
        selection: p.accent_soft,
    }
}
