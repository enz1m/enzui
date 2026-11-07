//! Editable combo boxes with caller-filtered suggestions and typed payload commits.
//!
//! The caller owns the query and supplies already ranked options. Keys preserve
//! highlights across refiltering; labels may repeat. Removing the highlighted key
//! clears the highlight rather than selecting a nearby option.
//!
//! Toolkit focus stays in the input; the editable-combobox node identifies the
//! highlighted option through `active_descendant`. Suggestions appear while the
//! operable input is focused and has options, loading state, or an edited nonempty
//! query. Escape suppresses them until the value changes. A commit also absorbs
//! one subsequent value change so the caller's chosen-label rewrite stays closed.
//!
//! Narrow fields collapse to a named search button by default, using the shared
//! [`crate::search_field`] layout and hidden-input focus routing. Read-only fields
//! remain reachable but cannot edit or open suggestions; disabled fields are inert.
//!
//! [`Role::ComboBox`]: enzui_core::semantics::Role::ComboBox
//! [`Role::EditableComboBox`]: enzui_core::semantics::Role::EditableComboBox
//! [`MenuNavigation`]: enzui_core::context_menu::MenuNavigation

use enzui_core::context_menu::{MenuEntry, MenuMove, MenuNavigation};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::Id;
use iced::widget::{column, container, row, text};
use iced::{Background, Border, Color, Element, Length, Padding, Rectangle, Size};
use std::borrow::Cow;

use crate::text_field::TextFieldSize;
mod field_widget;

/// Size tier — the same one [`crate::text_field`] and [`crate::combo_box`] use,
/// so an autocomplete field lines up with the fields beside it in a form.
pub type AutocompleteSize = TextFieldSize;

/// The namespace ids are derived under when the caller supplies no
/// [`EnzAutocompleteField::id`].
pub const DEFAULT_ID_PREFIX: &str = "enz-autocomplete-field";

/// Native accessible shortcut carried by the expanded editable combobox and
/// compact search button unless the caller overrides it.
pub const DEFAULT_SHORTCUT: &str = crate::search_field::responsive::DEFAULT_SHORTCUT;

/// Minimum useful visible query span used by the shared responsive-search
/// threshold. Placeholder prose is deliberately not part of the measurement.
pub const MIN_VISIBLE_QUERY_CHARACTERS: usize =
    crate::search_field::responsive::MIN_VISIBLE_QUERY_CHARACTERS;

/// How many suggestion rows are visible before the list scrolls. Matches
/// [`crate::combo_box::VISIBLE_ROWS`] so the two popups have the same shape.
pub const VISIBLE_ROWS: usize = 6;

/// Gap between the field's bottom edge and the popup's top edge.
pub const POPUP_GAP: f32 = 4.0;

const POPUP_PADDING: f32 = 4.0;

const POPUP_RADIUS_FALLBACK: f32 = 10.0;

const ROW_RADIUS_FALLBACK: f32 = 6.0;

/// Maximum simple one-line popup height in logical pixels before scrolling.
/// Subtitle and rich-result options use a larger uniform height and corresponding six-row cap.
#[must_use]
pub fn popup_max_height(size: AutocompleteSize) -> f32 {
    row_height(size) * VISIBLE_ROWS as f32 + POPUP_PADDING * 2.0
}

/// Base height of a simple one-line suggestion in logical pixels for a size tier.
#[must_use]
pub fn row_height(size: AutocompleteSize) -> f32 {
    tokens_for(size).height
}

fn tokens_for(size: AutocompleteSize) -> crate::tokens::FieldSizeTokens {
    size.tokens()
}

/// One keyed suggestion whose display content is separate from its typed payload.
/// Use stable unique keys to preserve highlights across refiltering; labels may repeat.
/// Commit callbacks borrow the payload without requiring it to implement `Clone`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteOption<T> {
    key: String,
    content: crate::suggestion_list::OptionRowContent,
    payload: T,
}

impl<T> AutocompleteOption<T> {
    /// Build a one-line suggestion. Add [`Self::subtitle`] for a simple rich
    /// row or [`Self::rich_result`] for the three-line search-result shape.
    pub fn new(key: impl Into<String>, label: impl Into<String>, payload: T) -> Self {
        Self {
            key: key.into(),
            content: crate::suggestion_list::OptionRowContent::new(label),
            payload,
        }
    }

    /// Add a secondary line below the label.
    ///
    /// The subtitle remains part of this one listbox option: it does not add a
    /// nested control or focus stop. Its text is included in the option's
    /// accessible name as well as rendered on the second line.
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.content = self.content.subtitle(subtitle);
        self
    }

    /// Use the same rich search-result row as [`crate::suggestion_list`].
    ///
    /// The label is the leading title. `trailing` is protected metadata on its
    /// first line; `subject` and `preview` share one wrapping detail paragraph
    /// whose continuation returns to the line origin. This remains one option
    /// node and replaces any simple subtitle configured earlier.
    #[must_use]
    pub fn rich_result(
        mut self,
        trailing: impl Into<String>,
        subject: impl Into<String>,
        preview: impl Into<String>,
    ) -> Self {
        self.content = self.content.rich_result(trailing, subject, preview);
        self
    }

    /// The stable identity.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The primary display line.
    #[must_use]
    pub fn label(&self) -> &str {
        self.content.title()
    }

    /// The secondary display line, when non-empty.
    #[must_use]
    pub fn subtitle_text(&self) -> Option<&str> {
        self.content.subtitle_text()
    }

    /// The shared row content consumed by both option widgets.
    #[must_use]
    pub fn row_content(&self) -> &crate::suggestion_list::OptionRowContent {
        &self.content
    }

    fn accessible_name(&self) -> String {
        self.content.accessible_name()
    }

    /// The typed value a commit publishes.
    #[must_use]
    pub fn payload(&self) -> &T {
        &self.payload
    }
}

/// Builder for an editable combo box. See [`enz_autocomplete_field`].
#[must_use]
pub struct EnzAutocompleteField<'a, T, Message> {
    aria_label: Option<String>,
    trailing: Option<Element<'a, Message>>,
    on_dismiss: Option<Box<dyn Fn() -> Message + 'a>>,
    inline: bool,
    label: Cow<'a, str>,
    placeholder: String,
    value: &'a str,
    options: Vec<AutocompleteOption<T>>,
    on_input: Box<dyn Fn(String) -> Message + 'a>,
    on_select: Box<dyn Fn(&T) -> Message + 'a>,
    on_custom_value: Option<Box<dyn Fn(String) -> Message + 'a>>,
    size: AutocompleteSize,
    radius: Option<f32>,
    enabled: bool,
    read_only: bool,
    loading: bool,
    error_text: Option<Cow<'a, str>>,
    empty_text: String,
    loading_text: String,
    palette: Option<Palette>,
    width: Length,
    id: Option<Id>,
    id_prefix: String,
    direction: Option<enzui_core::placement::Direction>,
    variant: crate::tokens::FieldVariant,
    responsive: bool,
    shortcut: String,
}

/// Creates an editable combo box with caller-owned query text and prefiltered options.
/// Edits publish through `on_input`; commits borrow the selected payload.
/// Defaults to `Md`, fill width, responsive collapse, and no custom-value handler.
pub fn enz_autocomplete_field<'a, T, Message>(
    label: impl Into<Cow<'a, str>>,
    value: &'a str,
    options: Vec<AutocompleteOption<T>>,
    on_input: impl Fn(String) -> Message + 'a,
    on_select: impl Fn(&T) -> Message + 'a,
) -> EnzAutocompleteField<'a, T, Message> {
    EnzAutocompleteField {
        aria_label: None,
        trailing: None,
        on_dismiss: None,
        inline: false,
        label: label.into(),
        placeholder: String::new(),
        value,
        options,
        on_input: Box::new(on_input),
        on_select: Box::new(on_select),
        on_custom_value: None,
        size: AutocompleteSize::Md,
        radius: None,
        enabled: true,
        read_only: false,
        loading: false,
        error_text: None,
        empty_text: enzui_core::localization::text("enzui-autocomplete-field-no-matches"),
        loading_text: enzui_core::localization::text("enzui-autocomplete-field-searching"),
        palette: None,
        width: Length::Fill,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        direction: None,
        variant: crate::tokens::FieldVariant::default(),
        responsive: true,
        shortcut: DEFAULT_SHORTCUT.to_owned(),
    }
}

impl<'a, T, Message> EnzAutocompleteField<'a, T, Message> {
    /// Override the accessible name without adding a visible label.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Place an accessory inside the field's trailing chrome slot.
    pub fn trailing(mut self, trailing: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// Publish a message when Escape dismisses an open suggestion popup.
    pub fn on_dismiss_with(mut self, on_dismiss: impl Fn() -> Message + 'a) -> Self {
        self.on_dismiss = Some(Box::new(on_dismiss));
        self
    }

    /// Placeholder shown while the field is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Accepts nonempty typed text when Enter has no visible highlighted suggestion.
    /// Unset by default. Enter is still captured when no value is committed; validation is caller-owned.
    pub fn on_custom_value(mut self, on_custom_value: impl Fn(String) -> Message + 'a) -> Self {
        self.on_custom_value = Some(Box::new(on_custom_value));
        self
    }

    /// Fixed size tier (default `Md`). Unpinned field corners follow live
    /// `radius.sm` / `md` / `lg`.
    pub fn size(mut self, size: AutocompleteSize) -> Self {
        self.size = size;
        self
    }

    /// Pin expanded-field corners in logical pixels instead of following the theme.
    /// Negative or non-finite input pins square corners. Suggestion-popup and
    /// row corners retain their independent live theme radii.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }

    /// Fill treatment. Defaults to
    /// [`crate::tokens::FieldVariant::Filled`], giving the field a body;
    /// [`crate::tokens::FieldVariant::Outlined`] leaves it page-coloured for
    /// dense/inline placements.
    pub fn variant(mut self, variant: crate::tokens::FieldVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Enables interactions, defaulting to true; false removes the input focus target and popup.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Keeps the enabled field reachable while refusing edits and suppressing suggestions.
    /// Defaults to false.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Marks a lookup in flight; with no options, shows the loading notice instead of the empty notice.
    /// Existing options remain selectable while loading.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Attach an inline validation message; recolours the border and publishes
    /// `invalid` to AT.
    pub fn error(mut self, error: impl Into<Cow<'a, str>>) -> Self {
        let error = error.into();
        self.error_text = if error.is_empty() { None } else { Some(error) };
        self
    }

    /// Text for the "nothing matched" row (default `"No matches"`).
    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// Text for the in-flight row (default `"Searching…"`).
    pub fn loading_text(mut self, text: impl Into<String>) -> Self {
        self.loading_text = text.into();
        self
    }

    /// Pin a palette instead of resolving from the runtime theme.
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Enables compact presentation when allotted width falls below measured usability needs, defaulting to true.
    /// The probe uses an icon, six query characters, clear slot, and padding; placeholder length has no effect.
    pub fn responsive(mut self, responsive: bool) -> Self {
        self.responsive = responsive;
        self
    }

    /// Set the keyboard shortcut announced by both the expanded editable
    /// combobox and compact search button (default [`DEFAULT_SHORTCUT`]).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = shortcut.into();
        self
    }

    /// Stable id for the text input, so `focus(id)` reaches it and the AT node
    /// is routable.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespaces default input identities derived from label, falling back to placeholder.
    /// Use distinct prefixes for repeated labels. An explicit input id wins, but this prefix
    /// still determines the separate responsive compact-trigger identity.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Overrides ambient writing direction for popup leading-edge alignment.
    /// The popup normally matches the field width, so the alignment may have no visible offset.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Whether the field accepts input and can show suggestions.
    #[must_use]
    pub fn is_operable(&self) -> bool {
        self.enabled && !self.read_only
    }

    /// Whether the field is enabled and keyboard-reachable, including read-only fields.
    #[must_use]
    pub fn is_focusable(&self) -> bool {
        self.enabled
    }
}

#[derive(Debug, Clone, Default)]
struct AutocompleteState {
    active: Option<usize>,
    active_key: Option<String>,
    hovered_row: Option<usize>,
    pressed_row: Option<usize>,
    dismissed: bool,
    edited: bool,
    skip_next_reopen: bool,
    last_value: String,
    pending_value: Option<String>,
    window_key: u64,
    /// The suggestions fading and scaling in as they open.
    reveal: crate::anchored_overlay::PopupReveal,
}

/// A decoded autocomplete navigation, commit, or dismissal command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteKey {
    /// Moves the highlighted suggestion.
    Move(MenuMove),
    /// Commits the visible highlight or eligible custom text.
    Commit,
    /// Suppresses suggestions without changing the query.
    Dismiss,
}

/// Decodes autocomplete key presses, leaving text editing keys to the input.
/// Enter commits in either open state. While open, Up/Down and Home/End move and Escape dismisses.
/// Ctrl, Alt, or Logo leave non-Escape keys unclaimed; Escape is modifier-blind while open.
#[must_use]
pub fn key_for_autocomplete(open: bool, event: &iced::keyboard::Event) -> Option<AutocompleteKey> {
    use iced::keyboard::key::Named;
    let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    let iced::keyboard::Key::Named(named) = key else {
        return None;
    };

    if *named != Named::Escape && (modifiers.control() || modifiers.alt() || modifiers.logo()) {
        return None;
    }

    match named {
        Named::Enter => Some(AutocompleteKey::Commit),
        Named::Escape if open => Some(AutocompleteKey::Dismiss),
        Named::ArrowDown if open => Some(AutocompleteKey::Move(MenuMove::Next)),
        Named::ArrowUp if open => Some(AutocompleteKey::Move(MenuMove::Previous)),
        Named::Home if open => Some(AutocompleteKey::Move(MenuMove::First)),
        Named::End if open => Some(AutocompleteKey::Move(MenuMove::Last)),
        _ => None,
    }
}

fn resolve_node_id(id: Option<&Id>, id_prefix: &str, label: &str, placeholder: &str) -> Id {
    if let Some(id) = id {
        return id.clone();
    }
    let seed = if label.is_empty() { placeholder } else { label };
    enzui_core::identity::derive_id(id_prefix, seed)
}

fn option_ids_and_keys<T>(
    namespace: &Id,
    options: &[AutocompleteOption<T>],
) -> (Vec<Id>, Vec<String>) {
    let prefix = format!("{namespace:?}-sug");
    let mut keyed = enzui_core::identity::KeyedIds::new(prefix.clone());
    let mut ids = Vec::with_capacity(options.len());
    let mut keys = Vec::with_capacity(options.len());
    for option in options {
        // Both key_for and id_for advance occurrence counts; derive each id from the one assigned key.
        let key = keyed.key_for(&option.key);
        ids.push(enzui_core::identity::derive_id(&prefix, &key));
        keys.push(key);
    }
    (ids, keys)
}

struct AutocompleteWidget<'a, T, Message> {
    field: Element<'a, Message>,
    chrome_child: usize,
    options: Vec<AutocompleteOption<T>>,
    on_input: std::rc::Rc<dyn Fn(String) -> Message + 'a>,
    on_select: Box<dyn Fn(&T) -> Message + 'a>,
    on_dismiss: Option<Box<dyn Fn() -> Message + 'a>>,
    on_custom_value: Option<Box<dyn Fn(String) -> Message + 'a>>,
    value: String,
    size: AutocompleteSize,
    enabled: bool,
    read_only: bool,
    loading: bool,
    empty_text: String,
    loading_text: String,
    palette: Option<Palette>,
    semantics: Semantics,
    listbox_semantics: Semantics,
    node_id: Id,
    scroll_id: Id,
    row_ids: Vec<Id>,
    row_keys: Vec<String>,
    direction: enzui_core::placement::Direction,
}

impl<'a, T, Message> EnzAutocompleteField<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn accessible_label(&self) -> String {
        if let Some(label) = &self.aria_label {
            label.clone()
        } else if !self.label.is_empty() {
            self.label.to_string()
        } else if !self.placeholder.is_empty() {
            self.placeholder.clone()
        } else {
            enzui_core::localization::text("enzui-autocomplete-field-search")
        }
    }

    fn base_semantics(&self) -> Semantics {
        let mut s = Semantics::new(Role::EditableComboBox)
            .editable(self.is_operable())
            .read_only(self.read_only)
            .disabled(!self.enabled)
            .invalid(self.error_text.as_deref().is_some_and(|e| !e.is_empty()))
            .keyboard_shortcut(self.shortcut.clone())
            .label(self.accessible_label());
        if let Some(err) = self.error_text.as_deref().filter(|e| !e.is_empty()) {
            s = s.description(err.to_owned());
        }
        s
    }

    pub(crate) fn breadcrumb_editor(
        mut self,
        id: Id,
        trailing: Option<Element<'a, Message>>,
        on_dismiss: impl Fn() -> Message + 'a,
    ) -> Element<'a, Message> {
        self.label = Cow::Borrowed("");
        self.inline = true;
        self.responsive = false;
        self.size = crate::breadcrumb::EDIT_FIELD_SIZE;
        self.width = Length::Fill;
        self.id = Some(id);
        self.aria_label = Some(enzui_core::localization::text(
            "enzui-autocomplete-field-breadcrumb",
        ));
        "Ctrl+L".clone_into(&mut self.shortcut);
        self.on_dismiss = Some(Box::new(on_dismiss));
        if trailing.is_some() {
            self.trailing = trailing;
        }
        self.into_element()
    }

    /// Materialise the widget tree.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let responsive_width = self.width;
        let responsive_enabled = self.responsive;
        let responsive_interactive = self.enabled;
        let responsive_active = !self.value.is_empty();
        let responsive_value = self.value.to_owned();
        let responsive_label = self.accessible_label();
        let responsive_shortcut = self.shortcut.clone();
        let responsive_palette = self.palette.clone();
        let responsive_prefix = self.id_prefix.clone();
        let tokens = self.size.tokens();
        let size = self.size;
        let radius = self.radius;
        let icon_size = crate::search_field::responsive::icon_size_for(self.size);

        let widget = AutocompleteWidget::new(self);
        let field_id = widget.node_id.clone();
        let field: Element<'a, Message> = Element::new(widget);

        // ResponsiveSearch owns the compact control node; this icon is decorative.
        let compact_icon: Element<'a, Message> = crate::icon::enz_icon("action.search")
            .size(crate::icon::DEFAULT_ICON_SIZE)
            .tint_resolved({
                let palette = responsive_palette.clone();
                move |theme| {
                    let palette = palette
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                    let toolbar = crate::toolbar_icon::ToolbarIconTokens::from_palette(&palette);
                    if responsive_active {
                        toolbar.active_color
                    } else {
                        toolbar.icon_color
                    }
                }
            })
            .into();
        let trigger_semantics = Semantics::new(Role::Button)
            .label(responsive_label.clone())
            .keyboard_shortcut(responsive_shortcut)
            .selected(responsive_active)
            .default_action();
        let trigger_id = enzui_core::identity::derive_id(
            &format!("{responsive_prefix}-{responsive_label}"),
            "collapsed-trigger",
        );

        let probe = crate::search_field::responsive::usability_probe(
            icon_size,
            tokens.text_size,
            tokens.padding_x,
            tokens.height,
        );

        crate::search_field::responsive::ResponsiveSearch::new(
            field,
            compact_icon,
            probe,
            responsive_width,
            responsive_interactive,
            responsive_enabled,
            responsive_active,
            responsive_value,
            field_id,
            trigger_id,
            trigger_semantics,
            responsive_palette,
            size,
            radius,
            crate::search_field::SearchPresentation::Inline,
            crate::search_field::responsive::ResultSurface::none(),
        )
        .into()
    }
}

impl<'a, T, Message> From<EnzAutocompleteField<'a, T, Message>> for Element<'a, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn from(v: EnzAutocompleteField<'a, T, Message>) -> Self {
        v.into_element()
    }
}

impl<'a, T, Message> AutocompleteWidget<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn new(builder: EnzAutocompleteField<'a, T, Message>) -> Self {
        use crate::field_chrome::{ChromePaint, ChromeStatus, FieldChrome};

        let tokens = tokens_for(builder.size);
        let semantics = builder.base_semantics();
        let operable = builder.is_operable();
        let enabled = builder.enabled;
        let variant = builder.variant;
        let read_only = builder.read_only;
        let invalid = builder.error_text.as_deref().is_some_and(|e| !e.is_empty());
        let pinned = builder.palette.clone();
        let node_id = resolve_node_id(
            builder.id.as_ref(),
            &builder.id_prefix,
            &builder.label,
            &builder.placeholder,
        );
        let (row_ids, row_keys) = option_ids_and_keys(&node_id, &builder.options);

        let resolve = {
            let pinned = pinned.clone();
            move |theme: &iced::Theme| -> Palette {
                pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme))
            }
        };

        // Disabled fields omit the input because an input without on_input can still receive focus.
        let input_resolve = resolve.clone();
        let on_input: std::rc::Rc<dyn Fn(String) -> Message + 'a> =
            std::rc::Rc::from(builder.on_input);
        let mut input = iced::widget::text_input(&builder.placeholder, builder.value)
            .id(node_id.clone())
            .size(tokens.text_size)
            .padding(Padding::from([tokens.padding_y(), 0.0]))
            .width(Length::Fill)
            .style(
                move |theme: &iced::Theme, status: iced::widget::text_input::Status| {
                    let p = input_resolve(theme);
                    let dim = |c: Color, a: f32| Color { a: c.a * a, ..c };
                    let base = iced::widget::text_input::Style {
                        // FieldChrome owns the whole field surface and border.
                        background: Background::Color(Color::TRANSPARENT),
                        border: Border::default(),
                        icon: p.text_muted,
                        placeholder: p.text_muted,
                        value: p.text,
                        selection: dim(p.accent, 0.30),
                    };
                    match status {
                        iced::widget::text_input::Status::Disabled => {
                            iced::widget::text_input::Style {
                                value: dim(p.text, 0.5),
                                placeholder: dim(p.text_muted, 0.5),
                                ..base
                            }
                        }
                        _ => base,
                    }
                },
            );
        if operable {
            let typed = std::rc::Rc::clone(&on_input);
            input = input.on_input(move |s| typed(s));
        }

        let inner: Element<'a, Message> = if enabled {
            input.into()
        } else {
            let shown_is_placeholder = builder.value.is_empty();
            let shown = if shown_is_placeholder {
                builder.placeholder.clone()
            } else {
                builder.value.to_owned()
            };
            let disp_resolve = resolve.clone();
            text(shown)
                .size(tokens.text_size)
                .style(move |theme: &iced::Theme| {
                    let p = disp_resolve(theme);
                    let base = if shown_is_placeholder {
                        p.text_muted
                    } else {
                        p.text
                    };
                    iced::widget::text::Style {
                        color: Some(crate::color::with_opacity(
                            base,
                            crate::theme_lookup::opacity(theme).disabled(),
                        )),
                    }
                })
                .width(Length::Fill)
                .into()
        };

        let inner = if let Some(trailing) = builder.trailing {
            iced::widget::Row::new()
                .push(inner)
                .push(trailing)
                .spacing(crate::text_field::SLOT_SPACING)
                .align_y(iced::alignment::Vertical::Center)
                .width(Length::Fill)
                .into()
        } else {
            inner
        };

        let chrome_resolve = resolve.clone();
        let size = builder.size;
        let radius_pin = builder.radius;
        let chrome: Element<'a, Message> = FieldChrome::new(
            inner,
            tokens.height,
            Padding::from([0.0, tokens.padding_x]),
            move |theme: &iced::Theme, status: ChromeStatus| {
                let radius = size.resolve_radius(theme, radius_pin);
                let p = chrome_resolve(theme);
                let dim = |c: Color, a: f32| Color { a: c.a * a, ..c };
                let fill = crate::tokens::field_fill_for(&p, variant);
                let (rest_bg, rest_border) = if enabled {
                    (fill, p.field_border)
                } else {
                    {
                        let fade = crate::theme_lookup::opacity(theme).disabled();
                        (dim(fill, fade), dim(p.field_border, fade))
                    }
                };
                let border_color = if invalid { p.error } else { rest_border };
                match status {
                    ChromeStatus::Focused => ChromePaint {
                        background: rest_bg,
                        border_color: if invalid { p.error } else { p.focus_ring },
                        border_width: 2.0,
                        radius,
                    },
                    ChromeStatus::Hovered => ChromePaint {
                        background: rest_bg,
                        border_color: if invalid { p.error } else { p.text_muted },
                        border_width: 1.0,
                        radius,
                    },
                    ChromeStatus::Active | ChromeStatus::Inert => ChromePaint {
                        background: rest_bg,
                        border_color,
                        border_width: 1.0,
                        radius,
                    },
                }
            },
        )
        .interactive(operable)
        .into();

        // Stable field-column keys keep caption changes from transferring retained input state.
        let mut chrome_child = 0;
        let label_el: Option<Element<'a, Message>> = if !builder.label.is_empty() {
            let label_resolve = resolve.clone();
            chrome_child = 1;
            Some(
                text(builder.label.clone())
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(label_resolve(theme).text_muted),
                    })
                    .into(),
            )
        } else {
            None
        };
        let caption_el: Option<Element<'a, Message>> = builder
            .error_text
            .filter(|e| !builder.inline && !e.is_empty())
            .map(|err| {
                let err_resolve = resolve.clone();
                text(err.into_owned())
                    .size(enzui_theme::text_scale::ui_px(11.0))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(err_resolve(theme).error),
                    })
                    .into()
            });
        let col = crate::field_column::FieldColumn::new(4.0)
            .width(builder.width)
            .slot("label", label_el)
            .field("field", chrome)
            .slot("caption", caption_el);

        let listbox_semantics = crate::listbox_shape::listbox(if builder.label.is_empty() {
            enzui_core::localization::text("enzui-autocomplete-field-suggestions")
        } else {
            enzui_core::localized!("enzui-autocomplete-labelled-suggestions", "label" => builder.label)
        });

        Self {
            field: col.into_element(),
            chrome_child,
            options: builder.options,
            on_input,
            on_select: builder.on_select,
            on_dismiss: builder.on_dismiss,
            on_custom_value: builder.on_custom_value,
            value: builder.value.to_owned(),
            size: builder.size,
            enabled,
            read_only,
            loading: builder.loading,
            empty_text: builder.empty_text,
            loading_text: builder.loading_text,
            palette: pinned,
            semantics,
            listbox_semantics,
            scroll_id: enzui_core::identity::derive_id(
                &format!("{node_id:?}"),
                "suggestion-scroll",
            ),
            node_id,
            row_ids,
            row_keys,
            direction: enzui_core::placement::resolve_direction(builder.direction),
        }
    }
}

impl<'a, T, Message> AutocompleteWidget<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn is_operable(&self) -> bool {
        self.enabled && !self.read_only
    }

    fn has_content(&self, state: &AutocompleteState) -> bool {
        !self.options.is_empty() || self.loading || (state.edited && !self.value.is_empty())
    }

    fn is_open(&self, state: &AutocompleteState, focused: bool) -> bool {
        focused && self.is_operable() && !state.dismissed && self.has_content(state)
    }

    fn nav_target(&self, active: Option<usize>, movement: MenuMove) -> Option<usize> {
        let mut nav = MenuNavigation::new(self.options.iter().map(|_| MenuEntry::item()).collect());
        if let Some(index) = active {
            nav.set_active(index);
        }
        nav.apply(movement)
    }

    // A missing key clears the cursor; choosing a nearby survivor could commit the wrong payload.
    fn reconcile_active(&self, state: &mut AutocompleteState) {
        let Some(old_key) = state.active_key.clone() else {
            state.active = None;
            return;
        };
        match self.row_keys.iter().position(|k| *k == old_key) {
            Some(index) => state.active = Some(index),
            None => {
                state.active = None;
                state.active_key = None;
            }
        }
    }

    fn set_active(&self, state: &mut AutocompleteState, index: Option<usize>) {
        state.active = index;
        state.active_key = index.and_then(|i| self.row_keys.get(i).cloned());
    }

    fn chrome_bounds(
        &self,
        layout: iced::advanced::Layout,
        column: &[iced::advanced::widget::Tree],
    ) -> Rectangle {
        layout
            .children(column)
            .nth(self.chrome_child)
            .map_or_else(|| layout.bounds(), |l| l.bounds())
    }

    fn suggestion_row_height(&self) -> f32 {
        let tokens = tokens_for(self.size);
        crate::suggestion_list::option_row_height(
            tokens,
            self.options
                .iter()
                .any(|option| option.subtitle_text().is_some()),
            self.options
                .iter()
                .any(|option| option.content.is_rich_result()),
        )
    }

    fn suggestion_popup_max_height(&self) -> f32 {
        self.suggestion_row_height() * VISIBLE_ROWS as f32 + POPUP_PADDING * 2.0
    }
}

impl<'a, T, Message> AutocompleteWidget<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn popup(&self, state: &AutocompleteState, focused: bool) -> Option<Element<'a, Message>> {
        if !self.is_open(state, focused) {
            return None;
        }

        let tokens = tokens_for(self.size);
        let pinned = self.palette.clone();
        let resolve = move |theme: &iced::Theme| -> Palette {
            pinned
                .clone()
                .unwrap_or_else(|| crate::theme_lookup::palette(theme))
        };

        let total = self.options.len();
        let option_row_height = self.suggestion_row_height();
        let mut list = column![].width(Length::Fill);

        if total == 0 {
            let notice = if self.loading {
                self.loading_text.clone()
            } else {
                self.empty_text.clone()
            };
            let notice_resolve = resolve.clone();
            let cell = container(text(notice.clone()).size(tokens.text_size).style(
                move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(notice_resolve(theme).text_muted),
                },
            ))
            .width(Length::Fill)
            .height(tokens.height)
            .center_y(tokens.height)
            .padding(Padding::ZERO.left(tokens.padding_x).right(tokens.padding_x));
            list = list.push(crate::semantics_container::SemanticsContainer::leaf(
                cell,
                crate::listbox_shape::listbox_notice(notice),
            ));
        }

        for (index, option) in self.options.iter().enumerate() {
            let is_active = state.active == Some(index);
            let is_hovered = state.hovered_row == Some(index);
            let label = crate::suggestion_list::option_row_content(
                option.content.clone(),
                tokens,
                self.palette.clone(),
            );

            let fill_resolve = resolve.clone();
            let cell = container(
                row![label]
                    .align_y(iced::Alignment::Center)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(option_row_height)
            .center_y(option_row_height)
            .padding(Padding::ZERO.left(tokens.padding_x).right(tokens.padding_x))
            .clip(true)
            .style(move |theme: &iced::Theme| {
                let p = fill_resolve(theme);
                container::Style {
                    background: (is_active || is_hovered)
                        .then_some(Background::Color(p.surface_hover)),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: enzui_core::metrics::FiniteNonNeg::new_or(
                            crate::theme_lookup::shape_radius(theme, |s| s.row),
                            ROW_RADIUS_FALLBACK,
                        )
                        .get()
                        .into(),
                    },
                    ..container::Style::default()
                }
            });

            // The input reports the highlight through active_descendant; options do not take focus.
            let row_semantics = crate::listbox_shape::listbox_option(
                option.accessible_name(),
                is_active,
                index + 1,
                total,
            );
            let row_id = self.row_ids.get(index).cloned().unwrap_or_else(Id::unique);
            list = list.push(
                crate::semantics_container::SemanticsContainer::leaf(cell, row_semantics)
                    .id(row_id),
            );
        }

        let content = container(list).padding(POPUP_PADDING).width(Length::Fill);

        let scroller = crate::scroll_view::enz_scrollable(content)
            .height(Length::Shrink)
            .id(self.scroll_id.clone());
        let scroller =
            crate::scroll_view::contain_wheel(scroller, crate::scroll_view::Axis::Vertical);
        let capped =
            crate::max_height::MaxHeight::new(scroller, self.suggestion_popup_max_height());
        let fill_resolve = resolve.clone();
        // Wrap the viewport so rounded clipping does not scroll away with the content.
        let surface = crate::surface::enz_surface(capped)
            .width(Length::Fill)
            .border_width(1.0)
            .fill_resolved(move |theme| fill_resolve(theme).surface_elevated)
            .border_color_resolved(move |theme| resolve(theme).border)
            .radius_resolved(|theme| {
                enzui_core::metrics::FiniteNonNeg::new_or(
                    crate::theme_lookup::shape_radius(theme, |s| s.popup),
                    POPUP_RADIUS_FALLBACK,
                )
                .get()
                .into()
            });

        Some(
            crate::semantics_container::SemanticsContainer::new(
                surface,
                self.listbox_semantics.clone(),
            )
            .into(),
        )
    }
}

fn probe_focus<Message>(
    field: &mut Element<'_, Message>,
    tree: &mut iced::advanced::widget::Tree,
    layout: iced::advanced::Layout,
    renderer: &iced::Renderer,
    target: &Id,
) -> bool {
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::{Focusable, Outcome};

    struct Probe<'t> {
        target: &'t Id,
        focused: Option<bool>,
    }
    impl<X> Operation<X> for Probe<'_> {
        fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
            if self.focused.is_none() && id.is_some_and(|id| id == self.target) {
                self.focused = Some(state.is_focused());
            }
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<X>)) {
            if self.focused.is_none() {
                operate(self);
            }
        }
        fn finish(&self) -> Outcome<X> {
            Outcome::None
        }
    }
    let mut probe = Probe {
        target,
        focused: None,
    };
    field
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut probe);
    probe.focused.unwrap_or(false)
}

impl<'a, T, Message> AutocompleteWidget<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    #[allow(
        clippy::too_many_arguments,
        reason = "the widget-event arguments iced passes, plus the field's state"
    )]
    fn handle_key(
        &self,
        tree: &mut iced::advanced::widget::Tree,
        action: AutocompleteKey,
        open: bool,
        shell: &mut iced::advanced::Shell<'_, Message>,
        renderer: &iced::Renderer,
        layout: iced::advanced::Layout,
        viewport: Rectangle,
    ) {
        match action {
            AutocompleteKey::Move(movement) => {
                let active = tree.state.downcast_ref::<AutocompleteState>().active;
                if let Some(target) = self.nav_target(active, movement) {
                    self.set_active(tree.state.downcast_mut::<AutocompleteState>(), Some(target));
                    self.reveal_highlight(tree, renderer, layout, viewport, target);
                    shell.request_redraw();
                }
                shell.capture_event();
            }
            AutocompleteKey::Commit => {
                let active = tree
                    .state
                    .downcast_ref::<AutocompleteState>()
                    .active
                    .filter(|_| open);
                match active.and_then(|i| self.options.get(i)) {
                    Some(option) => {
                        shell.publish((self.on_select)(&option.payload));
                        self.close_after_commit(tree);
                        shell.capture_event();
                        shell.invalidate_layout();
                        shell.request_redraw();
                    }
                    None => {
                        if let Some(on_custom) = &self.on_custom_value
                            && !self.value.is_empty()
                        {
                            shell.publish(on_custom(self.value.clone()));
                            self.close_after_commit(tree);
                            shell.invalidate_layout();
                            shell.request_redraw();
                        }
                        shell.capture_event();
                    }
                }
            }
            AutocompleteKey::Dismiss => {
                if let Some(on_dismiss) = &self.on_dismiss {
                    shell.publish(on_dismiss());
                }
                let state = tree.state.downcast_mut::<AutocompleteState>();
                state.dismissed = true;
                state.hovered_row = None;
                state.pressed_row = None;
                self.set_active(state, None);
                shell.capture_event();
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
    }

    fn close_after_commit(&self, tree: &mut iced::advanced::widget::Tree) {
        let state = tree.state.downcast_mut::<AutocompleteState>();
        state.dismissed = true;
        // Absorb the caller's first post-commit value rewrite without reopening.
        state.skip_next_reopen = true;
        state.hovered_row = None;
        state.pressed_row = None;
        self.set_active(state, None);
    }
}

impl<'a, T, Message> AutocompleteWidget<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn reveal_highlight(
        &self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        layout: iced::advanced::Layout,
        viewport: Rectangle,
        index: usize,
    ) {
        use iced::advanced::widget::Operation;
        use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};

        let state = tree.state.downcast_ref::<AutocompleteState>().clone();
        let Some(mut popup) = self.popup(&state, true) else {
            return;
        };
        if tree.children.len() < 2 {
            return;
        }
        let anchor = self.chrome_bounds(layout, &tree.children[0].children);
        let popup_tree = &mut tree.children[1];
        popup_tree.diff(&mut popup);

        let node = popup.as_widget_mut().measure(
            popup_tree,
            renderer,
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, viewport.size())
                .width(anchor.width),
        );
        let popup_layout = iced::advanced::Layout::of(&node);

        let Some(row) = row_rects_from_layout(
            popup_layout,
            &popup_tree.children,
            self.options.len(),
            self.suggestion_row_height(),
        )
        .and_then(|rects| rects.get(index).copied()) else {
            return;
        };

        struct Probe {
            target: Id,
            found: Option<(Rectangle, iced::Vector)>,
        }
        impl<X> Operation<X> for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<X>)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                _content_bounds: Size,
                translation: iced::Vector,
                _state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    self.found = Some((bounds, translation));
                }
            }
        }
        let mut probe = Probe {
            target: self.scroll_id.clone(),
            found: None,
        };
        popup.as_widget_mut().operate(
            popup_tree,
            popup_layout,
            &popup_layout.bounds(),
            renderer,
            &mut probe as &mut dyn Operation,
        );
        let Some((view, current)) = probe.found else {
            return;
        };

        // Descendant bounds precede scroll translation; subtract the viewport origin for content coordinates.
        let new_y = enzui_core::focus::reveal_axis_offset(
            current.y,
            view.height,
            row.y - view.y,
            row.height,
        );
        if (new_y - current.y).abs() < 0.5 {
            return;
        }

        struct DoScroll {
            target: Id,
            offset: AbsoluteOffset<Option<f32>>,
        }
        impl<X> Operation<X> for DoScroll {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<X>)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                content_bounds: Size,
                _translation: iced::Vector,
                state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    state.scroll_to(
                        self.offset,
                        iced::advanced::widget::operation::Animation::Instant,
                        bounds,
                        content_bounds,
                    );
                }
            }
        }
        let mut op = DoScroll {
            target: self.scroll_id.clone(),
            offset: AbsoluteOffset {
                x: None,
                y: Some(new_y),
            },
        };
        popup.as_widget_mut().operate(
            popup_tree,
            popup_layout,
            &popup_layout.bounds(),
            renderer,
            &mut op as &mut dyn Operation,
        );
    }
}

fn row_rects_from_layout(
    layout: iced::advanced::Layout,
    children: &[iced::advanced::widget::Tree],
    want: usize,
    row_h: f32,
) -> Option<Vec<Rectangle>> {
    fn is_row(h: f32, row_h: f32) -> bool {
        (h - row_h).abs() < 0.5
    }
    fn walk(
        layout: iced::advanced::Layout,
        children: &[iced::advanced::widget::Tree],
        want: usize,
        row_h: f32,
        depth: usize,
    ) -> Option<Vec<Rectangle>> {
        if depth > 8 {
            return None;
        }
        let children: Vec<(iced::advanced::Layout, &iced::advanced::widget::Tree)> =
            layout.iter(children).collect();
        if children.len() >= want {
            let heights: Vec<f32> = children.iter().map(|(c, _)| c.bounds().height).collect();
            for start in 0..=heights.len() - want {
                if (start..start + want).all(|i| is_row(heights[i], row_h)) {
                    return Some(
                        children[start..start + want]
                            .iter()
                            .map(|(c, _)| c.bounds())
                            .collect(),
                    );
                }
            }
        }
        children
            .into_iter()
            .find_map(|(child, tree)| walk(child, &tree.children, want, row_h, depth + 1))
    }
    if want == 0 {
        return None;
    }
    walk(layout, children, want, row_h, 0)
}

fn row_at(
    layout: iced::advanced::Layout,
    children: &[iced::advanced::widget::Tree],
    want: usize,
    row_h: f32,
    point: iced::Point,
) -> Option<usize> {
    let rects = row_rects_from_layout(layout, children, want, row_h)?;
    rects.iter().position(|r| r.contains(point))
}

mod overlay;
use overlay::SuggestionOverlay;
