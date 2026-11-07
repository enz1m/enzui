//! Elevated, scrollable menu surfaces with keyed rows and optional roving focus.
//!
//! [`EnzMenuItem`] supplies stable keys, row metadata and activation messages.
//! Opaque [`EnzMenu::push`] children retain insertion order but do not contribute
//! to keyed navigation, positions or content-width measurement.
//! Use unique keys and separate id prefixes for menus that share item keys.
//!
//! Fixed width defaults to 240 px; hug mode defaults to a 240 px floor and
//! 420 px ceiling, subject to parent limits. Rows scroll within clipped chrome.
//! This surface does not position, open or dismiss itself; popup hosts include
//! [`crate::menu_button`] and [`crate::context_menu_host`].

use enzui_core::context_menu::MenuMove;
use enzui_core::identity::KeyedIds;
use enzui_core::menu_model::{CheckState, EntryId, EntryKind, MenuEntryModel, MenuModel};
use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
use enzui_core::semantics::{Role, Semantics, SetPosition};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::widget::{column, container};
use iced::{Background, Border, Color, Element, Length, Padding};
mod widget_impls;

/// Default id-namespace prefix for keyed menu items.
///
/// Two menus on one screen sharing a key would otherwise derive the same
/// widget id; override with [`EnzMenu::id_prefix`].
pub const ID_PREFIX: &str = "enz-menu";

/// Fallback accessible surface name; [`EnzMenu::label`] overrides it.
pub const DEFAULT_MENU_LABEL: &str = "Menu";

/// Theme elevation tier for menu surfaces: 3, between page cards and modal surfaces.
pub const MENU_ELEVATION: u8 = 3;

/// Colour tokens for the menu surface.
#[derive(Debug, Clone, Copy)]
pub struct MenuTokens {
    /// Menu surface fill (default `surface_elevated`).
    pub background: Color,
    /// Outline ink; defaults to the shared edge shade of the elevated surface.
    pub border: Color,
    /// Menu border width in px (default 1).
    pub border_width: f32,
}

impl MenuTokens {
    /// Derive tokens from a runtime palette (the normal path).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_elevated,
            border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            border_width: 1.0,
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
}

/// A keyed model entry paired with an eager activation message or lazy factory.
#[derive(Clone)]
pub struct EnzMenuItem<'a, Message> {
    entry: MenuEntryModel,
    on_activate: ItemActivation<'a, Message>,
    icon_source: Option<crate::icon::IconSource>,
}

enum ItemActivation<'a, Message> {
    None,
    Eager(Message),
    Lazy(std::rc::Rc<dyn Fn() -> Message + 'a>),
}

impl<Message: Clone> Clone for ItemActivation<'_, Message> {
    fn clone(&self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Eager(m) => Self::Eager(m.clone()),
            Self::Lazy(f) => Self::Lazy(std::rc::Rc::clone(f)),
        }
    }
}

impl<Message> ItemActivation<'_, Message> {
    fn is_some(&self) -> bool {
        !matches!(self, Self::None)
    }
}

impl<Message> std::fmt::Debug for EnzMenuItem<'_, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let activation = match &self.on_activate {
            ItemActivation::None => "None",
            ItemActivation::Eager(_) => "Eager",
            ItemActivation::Lazy(_) => "Lazy",
        };
        f.debug_struct("EnzMenuItem")
            .field("entry", &self.entry)
            .field("on_activate", &activation)
            .finish()
    }
}

impl<'a, Message> EnzMenuItem<'a, Message> {
    /// A command row: a stable key, a label, and the message it dispatches.
    pub fn command(
        key: impl Into<EntryId>,
        label: impl Into<String>,
        on_activate: Message,
    ) -> Self {
        Self {
            entry: MenuEntryModel::command(key, label),
            on_activate: ItemActivation::Eager(on_activate),
            icon_source: None,
        }
    }

    /// Build a non-focusable separator carrying a stable key.
    pub fn separator(key: impl Into<EntryId>) -> Self {
        Self {
            entry: MenuEntryModel::separator(key),
            on_activate: ItemActivation::None,
            icon_source: None,
        }
    }

    /// A row that opens a submenu. Its message is published on activation the
    /// same way a command's is; the *host* decides what opening means.
    pub fn submenu(
        key: impl Into<EntryId>,
        label: impl Into<String>,
        on_activate: Message,
    ) -> Self {
        Self {
            entry: MenuEntryModel::submenu(key, label),
            on_activate: ItemActivation::Eager(on_activate),
            icon_source: None,
        }
    }

    /// Pair an existing model entry with an optional activation message.
    /// Without a message the rendered row is disabled, while navigation eligibility
    /// in the model still follows the entry's enabled flag.
    pub fn from_entry(entry: MenuEntryModel, on_activate: Option<Message>) -> Self {
        Self {
            entry,
            on_activate: match on_activate {
                Some(m) => ItemActivation::Eager(m),
                None => ItemActivation::None,
            },
            icon_source: None,
        }
    }

    /// Build an item from a model entry with a message factory run only on
    /// activation (pointer release, focused Enter or Space, a routed `Click`).
    /// The lazy counterpart of [`from_entry`](Self::from_entry), for hosts that
    /// render an open menu every frame.
    pub fn from_entry_lazy(
        entry: MenuEntryModel,
        on_activate: Option<std::rc::Rc<dyn Fn() -> Message + 'a>>,
    ) -> Self {
        Self {
            entry,
            on_activate: match on_activate {
                Some(f) => ItemActivation::Lazy(f),
                None => ItemActivation::None,
            },
            icon_source: None,
        }
    }

    /// Enable or disable the row. A disabled row is skipped by navigation and
    /// dispatches nothing.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.entry = self.entry.enabled(enabled);
        self
    }

    /// Give the row a check affordance.
    #[must_use]
    pub fn check(mut self, check: CheckState) -> Self {
        self.entry = self.entry.check(check);
        self
    }

    /// Make a checkable item one choice of several (a radio item).
    #[must_use]
    pub fn radio(mut self, radio: bool) -> Self {
        self.entry = self.entry.radio(radio);
        self
    }

    /// Leading icon name (Lucide canonical or a semantic alias).
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.entry = self.entry.icon(icon);
        self
    }

    /// Paint the icon in `color` (a container's colour) instead of the
    /// row's ink; the label keeps its ink, a disabled row its disabled ink.
    #[must_use]
    pub fn icon_tint(mut self, color: iced::Color) -> Self {
        self.entry = self.entry.icon_tint(color);
        self
    }

    /// Override the model icon with an app-owned SVG, raster handle or bundled icon.
    /// Bundled icons follow row tint; app-owned sources retain their colours.
    /// The row label names the control and the leading icon remains decorative.
    #[must_use]
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.icon_source = Some(source.into());
        self
    }

    /// Set the trailing shortcut hint shown in the row and exposed through
    /// the native accessibility shortcut property. Dispatch belongs to the app.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.entry = self.entry.shortcut(shortcut);
        self
    }

    /// Second-line description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.entry = self.entry.description(description);
        self
    }

    /// Render the row in the destructive tone and publish its destructive
    /// action disposition to assistive technology.
    #[must_use]
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.entry = self.entry.destructive(destructive);
        self
    }

    /// This item's stable key.
    #[must_use]
    pub fn key(&self) -> &EntryId {
        self.entry.id()
    }

    /// The underlying model entry.
    #[must_use]
    pub fn entry(&self) -> &MenuEntryModel {
        &self.entry
    }
}

enum MenuChild<'a, Message> {
    Item(Box<EnzMenuItem<'a, Message>>),
    Opaque(Element<'a, Message>),
}

/// Builder for a popup menu surface. See [`enz_menu`].
#[must_use]
pub struct EnzMenu<'a, Message> {
    children: Vec<MenuChild<'a, Message>>,
    width: f32,
    hug_content: bool,
    hug_min_width: f32,
    hug_max_width: f32,
    padding: f32,
    radius: f32,
    material: Option<enzui_theme::MaterialStyle>,
    tokens: Option<MenuTokens>,
    row_tokens: Option<crate::menu_row::MenuRowTokens>,
    label: Option<String>,
    id: Option<iced::advanced::widget::Id>,
    active_descendant: Option<iced::advanced::widget::Id>,
    active: Option<EntryId>,
    id_prefix: String,
    on_navigate: Option<std::rc::Rc<dyn Fn(EntryId) -> Message + 'a>>,
    self_roving: Option<bool>,
    focus_return_to: Option<iced::advanced::widget::Id>,
    on_hover: Option<std::rc::Rc<dyn Fn(EntryId) -> Message + 'a>>,
}

/// Construct a menu surface.
pub fn enz_menu<'a, Message: 'a>() -> EnzMenu<'a, Message> {
    EnzMenu {
        children: Vec::new(),
        width: 240.0,
        hug_content: false,
        hug_min_width: 240.0,
        hug_max_width: 420.0,
        padding: 4.0,
        radius: enzui_theme::text_defaults::shape_radius(|s| s.popup),
        material: None,
        tokens: None,
        row_tokens: None,
        label: None,
        id: None,
        active_descendant: None,
        active: None,
        id_prefix: ID_PREFIX.to_owned(),
        on_navigate: None,
        self_roving: None,
        focus_return_to: None,
        on_hover: None,
    }
}

impl<'a, Message: 'a> EnzMenu<'a, Message> {
    /// Fixed menu width in px. Non-finite or non-positive input falls back to
    /// 240. Ignored when [`Self::hug_content`] is `true`.
    pub fn width(mut self, w: f32) -> Self {
        self.width = FinitePositive::new_or(w, 240.0).get();
        self
    }
    /// Use content width instead of fixed width; disabled by default.
    /// Only keyed items supply intrinsic measurement rows. Opaque-only menus use
    /// the floor. Floor and ceiling default to 240 and 420 px; parent limits can
    /// reduce the resulting width.
    pub fn hug_content(mut self, h: bool) -> Self {
        self.hug_content = h;
        self
    }
    /// Set the hug floor in logical pixels; defaults to 240.
    /// Zero is literal. Negative or non-finite values fall back to 240. Parent
    /// limits can make the actual menu narrower.
    pub fn hug_min_width(mut self, w: f32) -> Self {
        self.hug_min_width = FiniteNonNeg::new_or(w, 240.0).get();
        self
    }
    /// Set the hug ceiling in logical pixels; defaults to 420.
    /// Zero removes the ceiling. Negative or non-finite values fall back to 420.
    pub fn hug_max_width(mut self, w: f32) -> Self {
        self.hug_max_width = FiniteNonNeg::new_or(w, 420.0).get();
        self
    }
    /// Inner padding around the items in px. Non-finite or negative input falls
    /// back to 4.
    pub fn padding(mut self, p: f32) -> Self {
        self.padding = FiniteNonNeg::new_or(p, 4.0).get();
        self
    }
    /// Corner radius in px, instead of the theme's `popup` radius (10 in the
    /// built-in theme). Non-finite or negative input keeps the theme's.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = FiniteNonNeg::new_or(r, self.radius).get();
        self
    }
    /// Put the menu on liquid glass of `style` (see [`crate::glass`]). Its
    /// rows and controls default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }
    /// Pin opaque surface colours and border width instead of using the live palette.
    /// A material surface supplies its own chrome and bypasses these tokens.
    pub fn tokens(mut self, t: MenuTokens) -> Self {
        self.tokens = Some(t);
        self
    }
    /// Override geometry and colours for every keyed row and separator.
    /// Useful for dense command sets that must fit a compact viewport while
    /// retaining every routable item.
    pub fn row_tokens(mut self, t: crate::menu_row::MenuRowTokens) -> Self {
        self.row_tokens = Some(t.sanitized());
        self
    }
    /// Append an opaque child in insertion order.
    /// It does not contribute to keyed navigation, positions, derived ids or hug
    /// measurement. The child may publish its own identity and semantics.
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(MenuChild::Opaque(child.into()));
        self
    }

    /// Append one keyed item (accumulates). This is the path that gets
    /// navigation, stable identity and item semantics.
    pub fn push_item(mut self, item: EnzMenuItem<'a, Message>) -> Self {
        self.children.push(MenuChild::Item(Box::new(item)));
        self
    }

    /// Append many keyed items at once.
    pub fn extend_items(
        mut self,
        items: impl IntoIterator<Item = EnzMenuItem<'a, Message>>,
    ) -> Self {
        for item in items {
            self = self.push_item(item);
        }
        self
    }

    /// Namespace for derived item ids. Defaults to [`ID_PREFIX`].
    ///
    /// Two menus on one screen that share an item key would otherwise derive
    /// the same widget id, and `NodeIdMap` resolves a duplicate to the first
    /// match — so the second menu's row becomes unroutable.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Highlight a focusable keyed item; unknown, separator or disabled keys select nothing.
    /// Outside self-roving mode, the highlighted row also supplies the surface
    /// active-descendant relation unless explicitly overridden. Self-roving uses
    /// this value as a fallback seed for its private cursor.
    pub fn active(mut self, key: impl Into<EntryId>) -> Self {
        self.active = Some(key.into());
        self
    }

    /// Highlight an item, or nothing. The `Option` form of [`Self::active`].
    pub fn active_maybe(mut self, key: Option<EntryId>) -> Self {
        self.active = key;
        self
    }

    /// Embed keyed rows under one focus owner and publish cursor changes for [`Self::active`].
    /// Arrow/Home/End movement wraps and skips disabled entries and separators.
    /// Enter activates on press; Space binds activation to the press-time row and
    /// completes on release. Shift is accepted; Ctrl/Alt/Logo chords bubble.
    /// Dismissal and submenu keys remain the host's responsibility.
    /// [`Self::roving_self`] takes precedence whenever configured.
    pub fn roving(mut self, on_navigate: impl Fn(EntryId) -> Message + 'a) -> Self {
        self.on_navigate = Some(std::rc::Rc::new(on_navigate));
        self
    }

    /// Embed keyed rows under one focus owner with a private cursor and painted highlight.
    /// The current [`Self::active`] supplies a fallback until the private cursor
    /// moves. Enter activates on press and Space completes its armed row on release.
    /// When `focus_on_mount` is true, new widget state starts focused; the host must
    /// relinquish trigger focus separately. Retained state does not refocus on every
    /// rebuild. This mode takes precedence over [`Self::roving`] in either setter order.
    /// Opaque controls keep their own focus behavior.
    pub fn roving_self(mut self, focus_on_mount: bool) -> Self {
        self.self_roving = Some(focus_on_mount);
        self
    }

    /// Self-roving only: return keyboard focus to this trigger id when the
    /// popup closes (Escape / a committed choice), via the [`crate::focus_return`]
    /// seam. Pair the id with the trigger's
    /// [`crate::focus_button::FocusButton::reclaim_focus_on_return`], set only
    /// while the popup is CLOSED so an armed record cannot yank focus off the
    /// still-open popup. No-op unless [`roving_self`](Self::roving_self) is on.
    pub fn focus_return_to(mut self, id: iced::advanced::widget::Id) -> Self {
        self.focus_return_to = Some(id);
        self
    }

    /// Publish a model-focusable keyed row's key when the pointer enters it.
    /// Repeated inside events and pointer leave publish nothing. Unavailable cursor
    /// passes preserve the latch. The application stores the key in [`Self::active`];
    /// this callback does not update the self-roving cursor. Eligibility follows
    /// the model entry, including entries without an activation message.
    pub fn on_hover(mut self, on_hover: impl Fn(EntryId) -> Message + 'a) -> Self {
        self.on_hover = Some(std::rc::Rc::new(on_hover));
        self
    }

    /// The keyed items, in document order. Opaque `push`ed children are not
    /// items and do not appear.
    pub fn items(&self) -> impl Iterator<Item = &EnzMenuItem<'a, Message>> {
        self.children.iter().filter_map(|child| match child {
            MenuChild::Item(item) => Some(item.as_ref()),
            MenuChild::Opaque(_) => None,
        })
    }

    /// Build a model from keyed entries with the requested highlight applied.
    /// Opaque children and activation-message availability are excluded.
    #[must_use]
    pub fn model(&self) -> MenuModel {
        let mut model = MenuModel::new(self.items().map(|item| item.entry.clone()).collect());
        if let Some(active) = &self.active {
            model.set_active(active);
        }
        model
    }

    /// Return a changed active key through the shared model navigation without mutating this builder.
    /// Movement wraps and skips disabled entries and separators. No change returns
    /// `None`, including a single-row wrap or a model without focusable entries.
    #[must_use]
    pub fn navigate(&self, movement: MenuMove) -> Option<EntryId> {
        let mut model = self.model();
        model.navigate(movement)
    }

    /// Derive the first-occurrence widget id for a key under this menu's prefix.
    /// Repeated keys receive disambiguated occurrence ids when rendered; use unique
    /// keys because navigation and activation still address the shared key.
    #[must_use]
    pub fn item_id(&self, key: &EntryId) -> iced::advanced::widget::Id {
        enzui_core::identity::derive_id(&self.id_prefix, key.as_str())
    }

    /// Accessible name for the menu surface itself ("File", "Commands").
    ///
    /// Announced when AT enters the menu, so the user hears which menu they are
    /// in rather than only the item under the cursor.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Name the menu surface for accessibility relations.
    /// This does not name the private roving focus owner or add an activation handler.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// The row the keyboard is navigating while focus stays elsewhere, such as
    /// a palette's query field (the `aria-activedescendant` pattern). A row not
    /// in this frame's tree is dropped rather than published as a dangling
    /// reference.
    pub fn active_descendant(mut self, row: iced::advanced::widget::Id) -> Self {
        self.active_descendant = Some(row);
        self
    }

    fn row_for(
        item: EnzMenuItem<'a, Message>,
        id: iced::advanced::widget::Id,
        position: Option<SetPosition>,
        highlighted: bool,
        reserve_leading: bool,
        embedded: bool,
        row_tokens: Option<crate::menu_row::MenuRowTokens>,
    ) -> Element<'a, Message>
    where
        Message: Clone,
    {
        let EnzMenuItem {
            entry,
            on_activate,
            icon_source,
        } = item;

        if entry.kind() == EntryKind::Separator {
            let separator = crate::menu_row::enz_menu_separator_builder::<Message>();
            return if let Some(tokens) = row_tokens {
                separator.tokens(tokens).into_element()
            } else {
                separator.into_element()
            };
        }
        let enabled = entry.is_enabled() && on_activate.is_some();
        let mut row = match on_activate {
            ItemActivation::Lazy(factory) => crate::menu_row::enz_menu_row_maybe_with(
                entry.label().to_owned(),
                Some(Box::new(move || (factory)()) as Box<dyn Fn() -> Message + 'a>),
            ),
            ItemActivation::Eager(message) => {
                crate::menu_row::enz_menu_row_maybe(entry.label().to_owned(), Some(message))
            }
            ItemActivation::None => {
                crate::menu_row::enz_menu_row_maybe(entry.label().to_owned(), None)
            }
        }
        .id(id)
        .enabled(enabled)
        .highlighted(highlighted)
        .reserve_leading(reserve_leading)
        .embedded(embedded);

        if let Some(tokens) = row_tokens {
            row = row.tokens(tokens);
        }
        if let Some(position) = position {
            row = row.set_position(position);
        }
        if entry.is_destructive() {
            row = row.tone(crate::menu_row::MenuRowTone::Destructive);
        }
        if entry.kind() == EntryKind::Submenu {
            row = row.submenu_indicator(true);
        }
        if let Some(source) = icon_source {
            row = row.icon_source(source);
        } else if let Some(icon) = entry.icon_name() {
            row = row.icon(icon.to_owned());
        }
        if let Some(color) = entry.icon_tint_color() {
            row = row.icon_tint(color);
        }
        if let Some(shortcut) = entry.shortcut_hint() {
            row = row.trailing(shortcut.to_owned());
        }
        if let Some(description) = entry.description_text() {
            row = row.description(description.to_owned());
        }
        if entry.check_state().is_checkable() {
            row = row
                .checkable(true)
                .radio(entry.is_radio())
                .selected(entry.check_state().is_checked());
        }
        row.into()
    }

    fn measure_row_for(
        item: &EnzMenuItem<'a, Message>,
        reserve_leading: bool,
        row_tokens: Option<crate::menu_row::MenuRowTokens>,
    ) -> Element<'a, Message>
    where
        Message: Clone,
    {
        let entry = &item.entry;
        let mut row =
            crate::menu_row::enz_menu_row_maybe::<Message>(entry.label().to_owned(), None)
                .measure_shrink(true)
                .reserve_leading(reserve_leading);
        if let Some(tokens) = row_tokens {
            row = row.tokens(tokens);
        }
        if let Some(source) = item.icon_source.clone() {
            row = row.icon_source(source);
        } else if let Some(icon) = entry.icon_name() {
            row = row.icon(icon.to_owned());
        }
        if let Some(shortcut) = entry.shortcut_hint() {
            row = row.trailing(shortcut.to_owned());
        }
        if let Some(description) = entry.description_text() {
            row = row.description(description.to_owned());
        }
        if entry.check_state().is_checkable() {
            row = row
                .checkable(true)
                .radio(entry.is_radio())
                .selected(entry.check_state().is_checked());
        }
        row.into()
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: Clone,
    {
        let radius = self.radius;
        let material = self.material;
        let pinned = self.tokens;
        let reserve_leading = self.items().any(|item| {
            item.entry.icon_name().is_some()
                || item.icon_source.is_some()
                || item.entry.check_state().is_checkable()
        });
        // Fill rows need shrink measurement twins; parent limits alone do not reveal intrinsic width.
        let measure_rows: Vec<Element<'a, Message>> = if self.hug_content {
            self.items()
                .map(|item| Self::measure_row_for(item, reserve_leading, self.row_tokens))
                .collect()
        } else {
            Vec::new()
        };
        let focusable_total = self
            .items()
            .filter(|item| item.entry.is_focusable())
            .count();
        let active = self.active.as_ref().and_then(|key| {
            let mut probe = MenuModel::new(self.items().map(|i| i.entry.clone()).collect());
            probe.set_active(key).then(|| key.clone())
        });
        let self_roving = self.self_roving;
        let roving = self.on_navigate.is_some() || self_roving.is_some();
        let on_hover = self.on_hover.clone();
        let nav_model = MenuModel::new(self.items().map(|i| i.entry.clone()).collect());
        let mut activations: std::collections::HashMap<EntryId, ItemActivation<'a, Message>> =
            std::collections::HashMap::new();
        // Occurrence ids disambiguate duplicate keys, but model navigation still uses the key alone.
        let mut ids = KeyedIds::new(self.id_prefix.clone());
        let mut focusable_seen = 0usize;
        let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(self.children.len());
        let mut active_row_id: Option<iced::advanced::widget::Id> = None;
        let mut item_row_positions: Vec<(EntryId, usize)> = Vec::new();

        for child in self.children {
            match child {
                MenuChild::Opaque(element) => rows.push(element),
                MenuChild::Item(item) => {
                    let item = *item;
                    let id = ids.id_for(item.entry.id().as_str());
                    if roving {
                        activations.insert(item.entry.id().clone(), item.on_activate.clone());
                    }
                    let position = if item.entry.is_focusable() {
                        focusable_seen += 1;
                        Some(SetPosition {
                            index: focusable_seen,
                            count: focusable_total,
                        })
                    } else {
                        None
                    };
                    let highlighted =
                        self_roving.is_none() && active.as_ref() == Some(item.entry.id());
                    if highlighted {
                        active_row_id = Some(id.clone());
                    }
                    item_row_positions.push((item.entry.id().clone(), rows.len()));
                    let hover_key = on_hover
                        .as_ref()
                        .filter(|_| item.entry.is_focusable())
                        .map(|callback| (item.entry.id().clone(), std::rc::Rc::clone(callback)));
                    let row = Self::row_for(
                        item,
                        id,
                        position,
                        highlighted,
                        reserve_leading,
                        roving,
                        self.row_tokens,
                    );
                    rows.push(match hover_key {
                        Some((key, callback)) => HoverReporter {
                            child: row,
                            key,
                            on_hover: callback,
                        }
                        .into(),
                        None => row,
                    });
                }
            }
        }
        let highlight = RowHighlightCell::default();
        // Retained scroll offsets require a stable id across view rebuilds.
        let scroll_id = enzui_core::identity::derive_id(&self.id_prefix, "menu-scroll");
        let rows_column: Element<'a, Message> = column(rows).spacing(0).into();
        let rows_width = rows_column.as_widget().size().width;
        let inner = crate::scroll_view::enz_scrollable(RowHighlight {
            child: rows_column,
            row: std::rc::Rc::clone(&highlight),
            tokens: self.row_tokens,
        })
        .id(scroll_id.clone())
        .width(rows_width)
        .height(Length::Shrink);
        let active_row = active.as_ref().and_then(|key| {
            item_row_positions
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, row)| *row)
        });
        let inner = ActiveReveal {
            child: crate::scroll_view::contain_wheel(inner, crate::scroll_view::Axis::Vertical),
            active_row,
            scroll_id: scroll_id.clone(),
        };
        let c = container(inner)
            .padding(Padding::from(self.padding))
            .clip(true)
            .style(move |theme: &iced::Theme| {
                if material.is_some() {
                    return container::Style::default();
                }
                let t = pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, MenuTokens::from_palette)
                });
                container::Style {
                    background: Some(Background::Color(t.background)),
                    border: Border {
                        color: t.border,
                        width: t.border_width,
                        radius: radius.into(),
                    },
                    shadow: crate::elevation::iced_shadow_for_theme(theme, MENU_ELEVATION),
                    ..Default::default()
                }
            });
        let mut element: Element<'a, Message> = if self.hug_content {
            let ceiling = if self.hug_max_width == 0.0 {
                f32::INFINITY
            } else {
                self.hug_max_width
            };
            let measure = container(column(measure_rows).spacing(0))
                .padding(Padding::from(self.padding))
                .clip(true);
            crate::hug_width::hug_to_preferred(c, measure, self.hug_min_width, ceiling).into()
        } else {
            c.width(Length::Fixed(self.width)).into()
        };
        if let Some(style) = material {
            element = crate::glass::enz_glass(element)
                .style(style)
                .radius(radius)
                .elevation(MENU_ELEVATION)
                .into();
        }
        if let Some(focus_on_mount) = self_roving {
            element = MenuRovingWidget {
                child: element,
                model: nav_model,
                active: self.active.clone(),
                activations,
                rows: RowScroll {
                    positions: item_row_positions,
                    highlight,
                    scroll_id,
                },
                mode: RovingMode::SelfCursor(Box::new(SelfCursorMode {
                    focus_on_mount,
                    focus_return_to: self.focus_return_to.clone(),
                })),
            }
            .into();
        } else if let Some(on_navigate) = self.on_navigate {
            element = MenuRovingWidget {
                child: element,
                model: nav_model,
                active: self.active.clone(),
                activations,
                rows: RowScroll {
                    positions: item_row_positions,
                    highlight,
                    scroll_id,
                },
                mode: RovingMode::Publish(on_navigate),
            }
            .into();
        }
        let mut semantics = Semantics::new(Role::Menu);
        semantics = semantics.label(
            self.label
                .unwrap_or_else(|| enzui_core::localization::text("enzui-menu-label")),
        );
        if let Some(row) = self.active_descendant.or(active_row_id) {
            semantics = semantics.active_descendant(row);
        }
        let mut wrapper = crate::semantics_container::SemanticsContainer::new(element, semantics);
        if let Some(id) = self.id {
            wrapper = wrapper.id(id);
        }
        wrapper.into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzMenu<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzMenu<'a, Message>) -> Self {
        w.into_element()
    }
}

#[derive(Debug, Clone, Default)]
struct MenuRovingState {
    focus: enzui_core::focus::FocusState,
    cursor: Option<EntryId>,
    space_armed: Option<EntryId>,
}

impl iced::advanced::widget::operation::Focusable for MenuRovingState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
        self.space_armed = None;
    }
}

struct MenuRovingWidget<'a, Message> {
    child: Element<'a, Message>,
    model: MenuModel,
    active: Option<EntryId>,
    activations: std::collections::HashMap<EntryId, ItemActivation<'a, Message>>,
    rows: RowScroll,
    mode: RovingMode<'a, Message>,
}

struct RowScroll {
    positions: Vec<(EntryId, usize)>,
    highlight: RowHighlightCell,
    scroll_id: iced::advanced::widget::Id,
}

impl RowScroll {
    fn row_of(&self, key: &EntryId) -> Option<usize> {
        self.positions
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, row)| *row)
    }
}

enum RovingMode<'a, Message> {
    Publish(std::rc::Rc<dyn Fn(EntryId) -> Message + 'a>),
    SelfCursor(Box<SelfCursorMode>),
}

struct SelfCursorMode {
    focus_on_mount: bool,
    focus_return_to: Option<iced::advanced::widget::Id>,
}

impl<Message: Clone> MenuRovingWidget<'_, Message> {
    fn reveal_row(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        key: &EntryId,
    ) {
        let Some(row_index) = self.rows.row_of(key) else {
            return;
        };
        // Below transparent wrappers: container -> scrollable -> highlight painter -> column -> row.
        let (container, container_tree) =
            through_pass_throughs(layout.child(&tree.children, 0), &tree.children[0]);
        let Some(mut reveal) =
            container
                .iter(&container_tree.children)
                .next()
                .and_then(|(scroll, scroll_tree)| {
                    RevealRow::for_row(&self.rows.scroll_id, scroll, scroll_tree, row_index)
                })
        else {
            return;
        };
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut reveal,
        );
    }
    fn activate_key(&self, key: Option<&EntryId>, shell: &mut iced::advanced::Shell<'_, Message>) {
        let Some(key) = key else {
            return;
        };
        if !self
            .model
            .entry(key)
            .is_some_and(MenuEntryModel::is_focusable)
        {
            return;
        }
        match self.activations.get(key) {
            Some(ItemActivation::Eager(m)) => shell.publish(m.clone()),
            Some(ItemActivation::Lazy(f)) => shell.publish((f)()),
            Some(ItemActivation::None) | None => {}
        }
    }
}

impl<'a, Message: 'a + Clone> From<MenuRovingWidget<'a, Message>> for Element<'a, Message> {
    fn from(w: MenuRovingWidget<'a, Message>) -> Self {
        Element::new(w)
    }
}

#[derive(Debug, Default)]
struct HoverReporterState {
    inside: bool,
}

/// Publishes `key` when the pointer enters `child`.
pub(crate) fn report_hover<'a, Message: 'a>(
    child: Element<'a, Message>,
    key: EntryId,
    on_hover: std::rc::Rc<dyn Fn(EntryId) -> Message + 'a>,
) -> Element<'a, Message> {
    HoverReporter {
        child,
        key,
        on_hover,
    }
    .into()
}

struct HoverReporter<'a, Message> {
    child: Element<'a, Message>,
    key: EntryId,
    on_hover: std::rc::Rc<dyn Fn(EntryId) -> Message + 'a>,
}

impl<'a, Message: 'a> From<HoverReporter<'a, Message>> for Element<'a, Message> {
    fn from(w: HoverReporter<'a, Message>) -> Self {
        Element::new(w)
    }
}

type RowHighlightCell = std::rc::Rc<std::cell::Cell<Option<usize>>>;

struct RowHighlight<'a, Message> {
    child: Element<'a, Message>,
    row: RowHighlightCell,
    tokens: Option<crate::menu_row::MenuRowTokens>,
}

impl<'a, Message: 'a> From<RowHighlight<'a, Message>> for Element<'a, Message> {
    fn from(widget: RowHighlight<'a, Message>) -> Self {
        Element::new(widget)
    }
}

struct RevealRow {
    id: iced::advanced::widget::Id,
    top: f32,
    bottom: f32,
}

impl RevealRow {
    fn for_row(
        id: &iced::advanced::widget::Id,
        scroll: iced::advanced::Layout,
        scroll_tree: &iced::advanced::widget::Tree,
        row: usize,
    ) -> Option<Self> {
        let (scroll, scroll_tree) = through_pass_throughs(scroll, scroll_tree);
        let (painter, painter_tree) = scroll.iter(&scroll_tree.children).next()?;
        let (column, column_tree) = through_pass_throughs(painter, painter_tree);
        let row = column.children(&column_tree.children).nth(row)?;
        let top = row.bounds().y - column.bounds().y;
        Some(Self {
            id: id.clone(),
            top,
            bottom: top + row.bounds().height,
        })
    }
}

struct ActiveReveal<'a, Message> {
    child: Element<'a, Message>,
    active_row: Option<usize>,
    scroll_id: iced::advanced::widget::Id,
}

#[derive(Default)]
struct ActiveRevealState {
    revealed: Option<usize>,
}

impl<'a, Message: 'a> From<ActiveReveal<'a, Message>> for Element<'a, Message> {
    fn from(widget: ActiveReveal<'a, Message>) -> Self {
        Element::new(widget)
    }
}

impl iced::advanced::widget::Operation for RevealRow {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Size,
        translation: iced::Vector,
        state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id != Some(&self.id) {
            return;
        }
        let offset = translation.y;
        let target = if self.top < offset {
            Some(self.top)
        } else if self.bottom > offset + bounds.height {
            Some(self.bottom - bounds.height)
        } else {
            None
        };
        if let Some(y) = target {
            state.scroll_to(
                iced::widget::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(y.max(0.0)),
                },
                iced::advanced::widget::operation::Animation::Instant,
                bounds,
                content_bounds,
            );
        }
    }
}

fn through_pass_throughs(
    mut layout: iced::advanced::Layout,
    mut tree: &iced::advanced::widget::Tree,
) -> (iced::advanced::Layout, &iced::advanced::widget::Tree) {
    while let Some(index) = tree.pass_through_child() {
        layout = layout.child(&tree.children, index);
        tree = &tree.children[index];
    }
    (layout, tree)
}
