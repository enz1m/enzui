//! The edit menu every editable text widget opens on a right click.
//!
//! [`edit_menu`] wraps a text field or text area. A right click, the Menu key
//! or Shift+F10 opens Undo, Redo, Cut, Copy, Paste, Delete and Select All at
//! the pointer (or under the field), with the localized labels and shortcut
//! hints of the framework catalog. A read-only field offers Copy and Select
//! All; a secure field never offers Cut or Copy.
//!
//! The menu holds its own open state, so the application wires nothing. A
//! chosen entry is carried out by giving the wrapped widget the entry's own
//! keyboard shortcut (Ctrl+X, Ctrl+V and so on): the widget's existing
//! clipboard, selection and undo handling does the work, and edits reach the
//! application through the widget's ordinary callbacks.
//!
//! A right click on an unfocused field first focuses it with a left click at
//! the same point, as other toolkits do; a right click on a focused field
//! keeps its selection.
//!
//! A text area with spell checking adds, on a misspelled word, its
//! suggestions above the edit commands (choosing one replaces the word),
//! "Add to Dictionary" and "Ignore".

use enzui_core::localization::text;
use enzui_core::shortcut::{Modifier, Shortcut};
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard::{self, Key, Modifiers, key::Code, key::Named, key::Physical};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

use crate::menu::{EnzMenuItem, enz_menu};

/// One edit command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    /// Replace the misspelled word with this suggestion.
    Suggestion(usize),
    AddToDictionary,
    Ignore,
}

/// The spell check behind a text area's menu.
pub(crate) struct SpellMenu<'a> {
    pub content: &'a iced::widget::text_editor::Content,
    pub check: enzui_core::spelling::SpellCheck,
}

/// The misspelled word the open menu is about.
#[derive(Debug, Clone)]
struct SpellTarget {
    word: crate::spell_underline::Misspelling,
    suggestions: Vec<String>,
}

/// Which commands a field offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditMenuOptions {
    /// The value can be changed (not read-only).
    pub editable: bool,
    /// The value is masked: never copied out.
    pub secure: bool,
}

impl EditMenuOptions {
    fn actions(self) -> Vec<Option<EditAction>> {
        use EditAction::{Copy, Cut, Delete, Paste, Redo, SelectAll, Undo};
        match (self.editable, self.secure) {
            (true, false) => vec![
                Some(Undo),
                Some(Redo),
                None,
                Some(Cut),
                Some(Copy),
                Some(Paste),
                Some(Delete),
                None,
                Some(SelectAll),
            ],
            (false, false) => vec![Some(Copy), None, Some(SelectAll)],
            (true, true) => vec![Some(Paste), None, Some(SelectAll)],
            (false, true) => vec![Some(SelectAll)],
        }
    }
}

impl EditAction {
    fn key(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Cut => "cut",
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::Delete => "delete",
            Self::SelectAll => "select-all",
            Self::Suggestion(_) | Self::AddToDictionary | Self::Ignore => "spelling",
        }
    }

    /// The shortcut the wrapped widget already answers for this command.
    fn shortcut(self) -> (Modifiers, Key, Physical) {
        let ctrl = |c: char, code: Code, shift: bool| {
            let modifiers = if shift {
                Modifiers::COMMAND | Modifiers::SHIFT
            } else {
                Modifiers::COMMAND
            };
            (
                modifiers,
                Key::Character(c.to_string().into()),
                Physical::Code(code),
            )
        };
        match self {
            Self::Undo => ctrl('z', Code::KeyZ, false),
            Self::Redo => ctrl('z', Code::KeyZ, true),
            Self::Cut => ctrl('x', Code::KeyX, false),
            Self::Copy => ctrl('c', Code::KeyC, false),
            Self::Paste => ctrl('v', Code::KeyV, false),
            Self::SelectAll => ctrl('a', Code::KeyA, false),
            Self::Delete => (
                Modifiers::empty(),
                Key::Named(Named::Delete),
                Physical::Code(Code::Delete),
            ),
            // Spelling entries are carried out directly, never as keys.
            Self::Suggestion(_) | Self::AddToDictionary | Self::Ignore => (
                Modifiers::empty(),
                Key::Unidentified,
                Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
            ),
        }
    }

    /// The command's symbol, as other menus show theirs (#3672).
    fn icon(self) -> Option<&'static str> {
        Some(match self {
            Self::Undo => "undo-2",
            Self::Redo => "redo-2",
            Self::Cut => "scissors",
            Self::Copy => "copy",
            Self::Paste => "clipboard-paste",
            Self::Delete => "delete",
            Self::SelectAll => "scan-text",
            Self::Suggestion(_) | Self::AddToDictionary | Self::Ignore => return None,
        })
    }

    fn shortcut_label(self) -> Option<String> {
        let (modifiers, key, _) = self.shortcut();
        let Key::Character(c) = key else {
            return None;
        };
        let mut held = Modifier::CTRL;
        if modifiers.shift() {
            held |= Modifier::SHIFT;
        }
        Some(Shortcut::new(held, c.chars().next().unwrap_or('?')).localized_label())
    }

    /// The events a keyboard produces for this command's shortcut.
    fn events(self) -> Vec<Event> {
        let (modifiers, key, physical_key) = self.shortcut();
        vec![
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key.clone(),
                physical_key,
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            }),
            Event::Keyboard(keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers,
            }),
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::empty())),
        ]
    }
}

fn menu<'a>(options: EditMenuOptions, spell: Option<&SpellTarget>) -> Element<'a, EditAction> {
    let mut menu = enz_menu()
        .hug_content(true)
        // 240, the menu default: at 180 the longest row, "Redo  Ctrl+Shift+Z",
        // elided to "Re…".
        .hug_min_width(240.0)
        .label(text("enzui-edit-menu"))
        .id_prefix("enzui-edit-menu");
    if let Some(spell) = spell {
        if spell.suggestions.is_empty() {
            menu = menu.push_item(
                EnzMenuItem::command(
                    "spell-none",
                    text("enzui-spell-no-suggestions"),
                    EditAction::Ignore,
                )
                .enabled(false),
            );
        }
        for (index, suggestion) in spell.suggestions.iter().enumerate() {
            menu = menu.push_item(EnzMenuItem::command(
                format!("spell-suggestion-{index}"),
                suggestion.clone(),
                EditAction::Suggestion(index),
            ));
        }
        menu = menu
            .push_item(EnzMenuItem::separator("spell-separator"))
            .push_item(EnzMenuItem::command(
                "spell-add",
                text("enzui-spell-add-to-dictionary"),
                EditAction::AddToDictionary,
            ))
            .push_item(EnzMenuItem::command(
                "spell-ignore",
                text("enzui-spell-ignore"),
                EditAction::Ignore,
            ))
            .push_item(EnzMenuItem::separator("spell-end"));
    }
    for (index, action) in options.actions().into_iter().enumerate() {
        menu = match action {
            Some(action) => {
                let mut item = EnzMenuItem::command(
                    action.key(),
                    text(&format!("enzui-edit-menu-{}", action.key())),
                    action,
                );
                if let Some(label) = action.shortcut_label() {
                    item = item.shortcut(label);
                }
                if let Some(icon) = action.icon() {
                    item = item.icon(icon);
                }
                menu.push_item(item)
            }
            None => menu.push_item(EnzMenuItem::separator(format!("separator-{index}"))),
        };
    }
    menu.into_element()
}

/// Wraps an editable text widget with the built-in edit menu.
pub(crate) fn edit_menu<'a, Message: 'a>(
    content: Element<'a, Message>,
    options: EditMenuOptions,
) -> Element<'a, Message> {
    edit_menu_with_spelling(content, options, None)
}

/// [`edit_menu`] with spelling entries on misspelled words.
pub(crate) fn edit_menu_with_spelling<'a, Message: 'a>(
    content: Element<'a, Message>,
    options: EditMenuOptions,
    spelling: Option<SpellMenu<'a>>,
) -> Element<'a, Message> {
    Element::new(EditMenu {
        content,
        menu: None,
        options,
        spelling,
    })
}

struct EditMenu<'a, Message> {
    content: Element<'a, Message>,
    /// The menu, built only while it is open: every editable field carries
    /// one, and building a dozen rows per field on every view made a page of
    /// fields slow to rebuild.
    menu: Option<Element<'a, EditAction>>,
    options: EditMenuOptions,
    spelling: Option<SpellMenu<'a>>,
}

impl<'a, Message> EditMenu<'a, Message> {
    /// Rebuilds the menu for the word the state is about, if any.
    fn rebuild_menu(&mut self, menu_tree: &mut Tree, spell: Option<&SpellTarget>) {
        let menu = self.menu.insert(menu(self.options, spell));
        *menu_tree = Tree::new(menu.as_widget());
        // A new tree has no children until it is diffed.
        menu_tree.diff(menu.as_widget_mut());
    }

    /// The open menu, built on first use in this view.
    fn open_menu(&mut self, menu_tree: &mut Tree, spell: Option<&SpellTarget>) {
        if self.menu.is_none() {
            self.rebuild_menu(menu_tree, spell);
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// Where the open menu is anchored, in layout coordinates.
    open_at: Option<Point>,
    /// A chosen command, carried out on the wrapper's next update.
    pending: Option<EditAction>,
    /// Opened from the keyboard: move focus into the menu.
    focus_first: bool,
    /// The misspelled word the open menu is about.
    spell: Option<SpellTarget>,
}

impl tree::Interactive for State {
    fn cancel_interaction(&mut self) {
        self.open_at = None;
        self.pending = None;
        self.focus_first = false;
        self.spell = None;
    }
}

/// Selects `start..end` in the text widget an operation reaches.
struct SelectRange {
    start: iced::advanced::text::editor::Position,
    end: iced::advanced::text::editor::Position,
}

impl Operation for SelectRange {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text_input(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::TextInput,
    ) {
        state.select_range(self.start, self.end);
    }
}

/// Whether a text input or editor inside `tree` holds focus.
fn content_focused(
    content: &mut Element<'_, impl Sized>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &iced::Renderer,
) -> bool {
    focused_in(content, tree, layout, renderer).is_some()
}

/// The focus owner inside `tree`, if any, with its id when it has one.
fn focused_in(
    content: &mut Element<'_, impl Sized>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &iced::Renderer,
) -> Option<Option<iced::advanced::widget::Id>> {
    #[derive(Default)]
    struct Probe {
        focused: Option<Option<iced::advanced::widget::Id>>,
    }
    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn focusable(
            &mut self,
            id: Option<&iced::advanced::widget::Id>,
            _bounds: Rectangle,
            state: &mut dyn iced::advanced::widget::operation::Focusable,
        ) {
            if state.is_focused() && self.focused.is_none() {
                self.focused = Some(id.cloned());
            }
        }
    }
    let mut probe = Probe::default();
    content
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut probe);
    probe.focused
}

impl<Message> EditMenu<'_, Message> {
    /// Carries out a spelling entry for `spell`'s word. A suggestion selects
    /// the word and commits the suggestion as typed text, so the field's own
    /// edit callback replaces it and its undo history records it.
    #[allow(
        clippy::too_many_arguments,
        reason = "the widget update arguments, passed through to the field"
    )]
    fn spelling_action(
        &mut self,
        action: EditAction,
        spell: SpellTarget,
        tree: &mut Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let Some(spelling) = &self.spelling else {
            return;
        };
        match action {
            EditAction::Suggestion(index) => {
                let Some(replacement) = spell.suggestions.get(index) else {
                    return;
                };
                let mut select = SelectRange {
                    start: spell.word.start,
                    end: spell.word.end,
                };
                self.content.as_widget_mut().operate(
                    &mut tree.children[0],
                    layout,
                    viewport,
                    renderer,
                    &mut select,
                );
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    &Event::InputMethod(iced::advanced::input_method::Event::Commit(
                        replacement.clone(),
                    )),
                    layout,
                    cursor,
                    renderer,
                    shell,
                    viewport,
                );
            }
            EditAction::AddToDictionary => spelling.check.add_to_dictionary(&spell.word.word),
            EditAction::Ignore => spelling.check.ignore(&spell.word.word),
            _ => {}
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EditMenu<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        if tree.children.len() != 2 {
            tree.children = vec![Tree::new(self.content.as_widget()), Tree::empty()];
        }
        tree.children[0].diff(self.content.as_widget_mut());
        let state = tree.state.downcast_ref::<State>();
        if state.open_at.is_some() {
            // The open menu keeps its tree; it is about the word, if any.
            let spell = state.spell.clone();
            let menu = self.menu.insert(menu(self.options, spell.as_ref()));
            tree.children[1].diff(menu.as_widget_mut());
        } else {
            tree.children[1] = Tree::empty();
        }
        tree.refresh_frame_children();
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
        if let Some(action) = tree.state.downcast_mut::<State>().pending.take() {
            if let Some(spell) = tree.state.downcast_mut::<State>().spell.take() {
                self.spelling_action(
                    action, spell, tree, layout, cursor, renderer, shell, viewport,
                );
                let menu_tree = &mut tree.children[1];
                self.rebuild_menu(menu_tree, None);
                shell.capture_event();
                shell.request_redraw();
                return;
            }
            for replayed in action.events() {
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    &replayed,
                    layout,
                    cursor,
                    renderer,
                    shell,
                    viewport,
                );
            }
            // The event that chose the command belongs to the menu, not the
            // field: a click outside the field would unfocus it.
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        let bounds = layout.bounds();
        let open_at = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => cursor
                .position_over(bounds)
                .map(|position| (position, false)),
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if matches!(key, Key::Named(Named::ContextMenu))
                    || (matches!(key, Key::Named(Named::F10)) && modifiers.shift()) =>
            {
                content_focused(&mut self.content, &mut tree.children[0], layout, renderer)
                    .then(|| (Point::new(bounds.x + 8.0, bounds.y + bounds.height), true))
            }
            _ => None,
        };
        if let Some((position, from_keyboard)) = open_at {
            if !from_keyboard
                && !content_focused(&mut self.content, &mut tree.children[0], layout, renderer)
            {
                // Focus the field and place the caret where the user pointed.
                let pointer = mouse::Cursor::Available(position);
                for click in [
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                ] {
                    self.content.as_widget_mut().update(
                        &mut tree.children[0],
                        &click,
                        layout,
                        pointer,
                        renderer,
                        shell,
                        viewport,
                    );
                }
                // Only the field saw that click: claim focus across the window,
                // or the previous owner and the field both claim it later.
                if let Some(Some(id)) =
                    focused_in(&mut self.content, &mut tree.children[0], layout, renderer)
                {
                    crate::focus_claim::claim(id);
                }
            }
            let spell = self.spelling.as_ref().and_then(|spelling| {
                let point = (!from_keyboard).then(|| position - Vector::new(bounds.x, bounds.y));
                let word = crate::spell_underline::misspelling_at(
                    spelling.content,
                    &spelling.check,
                    point,
                )?;
                let suggestions = spelling.check.suggestions(&word.word);
                Some(SpellTarget { word, suggestions })
            });
            self.rebuild_menu(&mut tree.children[1], spell.as_ref());
            let state = tree.state.downcast_mut::<State>();
            state.open_at = Some(position);
            state.focus_first = from_keyboard;
            state.spell = spell;
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        // A press on this field's own open menu reaches the field below it
        // with the pointer levitating; the field keeps its focus and caret
        // for the command the menu is choosing.
        if tree.state.downcast_ref::<State>().open_at.is_some()
            && cursor.is_levitating()
            && matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)))
        {
            return;
        }

        self.content.as_widget_mut().update(
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
        self.content.as_widget().draw(
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // While its menu is open the field keeps its keyboard input (the
        // menu's commands are carried out through it) but reports no focus,
        // so the field and a menu row never both claim it.
        if tree.state.downcast_ref::<State>().open_at.is_some() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                &mut YieldFocus { inner: operation },
            );
            return;
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        // A fresh tree asks before its children exist.
        tree.children
            .first()
            .is_none_or(|content| self.content.as_widget().frame_interest(content))
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
        let Tree {
            state, children, ..
        } = tree;
        let (content_tree, menu_tree) = children.split_at_mut(1);
        let state = state.downcast_mut::<State>();
        if state.open_at.is_some() {
            self.open_menu(&mut menu_tree[0], state.spell.as_ref());
        }
        let (Some(anchor), Some(menu)) = (state.open_at, self.menu.as_mut()) else {
            return self.content.as_widget_mut().overlay(
                &mut content_tree[0],
                layout,
                renderer,
                viewport,
                translation,
                window,
            );
        };
        vec![crate::overlay_adapter::element(
            MenuOverlay {
                menu,
                tree: &mut menu_tree[0],
                state,
                anchor: anchor + translation,
                _message: std::marker::PhantomData,
            },
            renderer,
            window,
        )]
    }
}

struct MenuOverlay<'a, 'b, Message> {
    menu: &'b mut Element<'a, EditAction>,
    tree: &'b mut Tree,
    _message: std::marker::PhantomData<fn() -> Message>,
    state: &'b mut State,
    anchor: Point,
}

impl<Message> MenuOverlay<'_, '_, Message> {
    fn close(&mut self, shell_redraw: &mut dyn FnMut()) {
        self.state.open_at = None;
        self.state.focus_first = false;
        if self.state.pending.is_none() {
            self.state.spell = None;
        }
        shell_redraw();
    }
}

impl<Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for MenuOverlay<'_, '_, Message>
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let margin = crate::positioned_overlay::EDGE_MARGIN;
        let placed = crate::anchored_overlay::place_in_window_first(bounds, |room, viewport| {
            let cap = Size::new(
                (room.width - 2.0 * margin).max(0.0),
                (room.height - 2.0 * margin).max(0.0),
            );
            let node = self.menu.as_widget_mut().measure(
                self.tree,
                renderer,
                &layout::Limits::new(Size::ZERO, cap),
            );
            let resolved =
                enzui_core::placement::place_at_point(self.anchor, node.size(), viewport, margin);
            let fits = crate::anchored_overlay::fits_within(viewport, resolved.bounds);
            (
                node.move_to(Point::new(resolved.bounds.x, resolved.bounds.y)),
                fits,
            )
        });
        layout::Node::with_children(bounds, vec![placed])
    }

    fn area(&self, node: &layout::Node) -> Option<Rectangle> {
        crate::overlay_adapter::children_area(node)
    }

    fn anchor(&self) -> Option<Rectangle> {
        Some(Rectangle::new(self.anchor, Size::new(1.0, 1.0)))
    }

    fn grabs(&self) -> bool {
        true
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: layout::Placed<'_>,
        cursor: mouse::Cursor,
    ) {
        let Some(menu_layout) = layout.children().next() else {
            return;
        };
        self.menu.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            menu_layout.to_layout(),
            cursor,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let Some(menu_layout) = layout.children().next() else {
            return;
        };
        let menu_bounds = menu_layout.bounds();
        let infinite = crate::overlay_adapter::UNBOUNDED_VIEWPORT;

        if std::mem::take(&mut self.state.focus_first) {
            let mut first: Box<dyn Operation> =
                enzui_core::focus::traversal_operation(enzui_core::focus::Traversal::Next);
            loop {
                self.menu.as_widget_mut().operate(
                    self.tree,
                    menu_layout.to_layout(),
                    &infinite,
                    renderer,
                    &mut first,
                );
                match first.finish() {
                    iced::advanced::widget::operation::Outcome::Chain(next) => first = next,
                    _ => break,
                }
            }
        }

        // The menu publishes edit commands, not application messages: run it
        // on a local bus and keep what it chose.
        let mut bus = iced::advanced::shell::Bus::new();
        let (captured, redraw) = {
            let mut local = shell.local(&mut bus);
            self.menu.as_widget_mut().update(
                self.tree,
                event,
                menu_layout.to_layout(),
                cursor,
                renderer,
                &mut local,
                &infinite,
            );
            (local.is_event_captured(), local.redraw_request())
        };
        if let Some((action, _)) = bus.drain().next() {
            // The activating event is left uncaptured so it reaches the
            // wrapper in the base pass, which carries the command out there:
            // the field's new value and selection are then painted like any
            // edit, which a change made in this overlay pass would not be.
            self.state.pending = Some(action);
            self.close(&mut || shell.request_redraw());
            return;
        }
        if !matches!(redraw, iced::window::RedrawRequest::Wait) {
            shell.request_redraw();
        }
        if captured {
            shell.capture_event();
            return;
        }

        let traversal = match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(Named::ArrowDown),
                ..
            }) => Some(enzui_core::focus::Traversal::Next),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(Named::ArrowUp),
                ..
            }) => Some(enzui_core::focus::Traversal::Previous),
            _ => None,
        };
        if let Some(direction) = traversal {
            let mut operation: Box<dyn Operation> =
                enzui_core::focus::traversal_operation(direction);
            loop {
                self.menu.as_widget_mut().operate(
                    self.tree,
                    menu_layout.to_layout(),
                    &infinite,
                    renderer,
                    &mut operation,
                );
                match operation.finish() {
                    iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
            shell.request_redraw();
            shell.capture_event();
            return;
        }

        let dismiss = match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            }) => true,
            Event::Mouse(mouse::Event::ButtonPressed(_)) => cursor
                .position()
                .is_some_and(|position| !menu_bounds.contains(position)),
            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => {
                !menu_bounds.contains(*position)
            }
            _ => false,
        };
        if dismiss {
            self.close(&mut || shell.request_redraw());
            shell.capture_event();
        }
    }

    fn operate(
        &mut self,
        layout: layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let Some(menu_layout) = layout.children().next() else {
            return;
        };
        self.menu.as_widget_mut().operate(
            self.tree,
            menu_layout.to_layout(),
            &menu_layout.bounds(),
            renderer,
            operation,
        );
    }

    fn mouse_interaction(
        &self,
        layout: layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(menu_layout) = layout.children().next() else {
            return mouse::Interaction::default();
        };
        if !cursor.is_over(menu_layout.bounds()) {
            return mouse::Interaction::default();
        }
        match self.menu.as_widget().mouse_interaction(
            self.tree,
            menu_layout.to_layout(),
            cursor,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            renderer,
        ) {
            mouse::Interaction::None => mouse::Interaction::Idle,
            inner => inner,
        }
    }

    fn index(&self) -> f32 {
        enzui_core::overlay_layer::OverlayLayer::Menu.z_index()
    }
}

/// The field under an open edit menu, as operations see it: not focused,
/// while it keeps the keyboard input the menu's commands go through.
struct Yielded<'a>(&'a mut dyn iced::advanced::widget::operation::Focusable);

impl iced::advanced::widget::operation::Focusable for Yielded<'_> {
    fn is_focused(&self) -> bool {
        false
    }

    fn focus(&mut self) {
        self.0.focus();
    }

    fn unfocus(&mut self) {
        self.0.unfocus();
    }
}

/// Forwards an operation, except that the field it reaches reports no focus:
/// the field under an open edit menu. Focus, for assistive technology, is in
/// the menu then.
struct YieldFocus<'a> {
    inner: &'a mut dyn Operation,
}

impl Operation for YieldFocus<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.inner
            .traverse(&mut |inner| operate(&mut YieldFocus { inner }));
    }

    fn is_native_reconciliation(&self) -> bool {
        self.inner.is_native_reconciliation()
    }

    fn is_docked_clock_rebase(&self) -> bool {
        self.inner.is_docked_clock_rebase()
    }

    fn docked_clock(&mut self, clock: &mut dyn iced::advanced::widget::operation::DockedClock) {
        self.inner.docked_clock(clock);
    }

    fn projection_applied(&mut self, root: iced::advanced::renderer::NodeId) {
        self.inner.projection_applied(root);
    }

    fn needs_editor_reconciliation(&self, node: iced::advanced::renderer::NodeId) -> bool {
        self.inner.needs_editor_reconciliation(node)
    }

    fn editor_reconciled(&mut self, node: iced::advanced::renderer::NodeId, success: bool) {
        self.inner.editor_reconciled(node, success);
    }

    fn needs_scroll_reconciliation(&self, node: iced::advanced::renderer::NodeId) -> bool {
        self.inner.needs_scroll_reconciliation(node)
    }

    fn scroll_reconciled(&mut self, node: iced::advanced::renderer::NodeId, success: bool) {
        self.inner.scroll_reconciled(node, success);
    }

    fn container(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        viewport: &Rectangle,
    ) {
        self.inner.container(id, bounds, viewport);
    }

    fn focusable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        self.inner.focusable(id, bounds, &mut Yielded(state));
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        content: Size,
        translation: Vector,
        state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.inner
            .scrollable(id, bounds, content, translation, state);
    }

    fn text_input(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::TextInput,
    ) {
        self.inner.text_input(id, bounds, state);
    }

    fn text(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: Rectangle, text: &str) {
        self.inner.text(id, bounds, text);
    }

    fn selectable_text(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        text: &str,
        layout: &dyn iced::advanced::widget::operation::SelectableTextLayout,
    ) {
        self.inner.selectable_text(id, bounds, text, layout);
    }

    fn custom(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        self.inner.custom(id, bounds, state);
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<()> {
        self.inner.finish()
    }
}
