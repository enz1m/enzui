//! Selection-field builders and caller-owned selection inputs.

use super::*;
use std::borrow::Cow;

/// Builder for an enzUI-styled combo box.
#[must_use]
pub struct EnzComboBox<'a, Message> {
    pub(crate) label: Cow<'a, str>,
    pub(crate) aria_label: Option<String>,
    pub(crate) placeholder: std::borrow::Cow<'a, str>,
    pub(crate) identity_placeholder: std::borrow::Cow<'a, str>,
    pub(crate) options: Vec<String>,
    pub(crate) selected: Option<String>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) on_select: Box<dyn Fn(String) -> Message + 'a>,
    pub(crate) on_select_indexed: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    pub(crate) size: TextFieldSize,
    pub(crate) radius: Option<f32>,
    pub(crate) error_text: Option<Cow<'a, str>>,
    pub(crate) enabled: bool,
    pub(crate) read_only: bool,
    pub(crate) palette: Option<Palette>,
    pub(crate) width: Length,
    pub(crate) id: Option<iced::advanced::widget::Id>,
    pub(crate) id_prefix: String,
    pub(crate) direction: Option<enzui_core::placement::Direction>,
    pub(crate) scroll_cell: Option<crate::scroll_view::ScrollTranslation>,
    pub(crate) variant: crate::tokens::FieldVariant,
    pub(crate) option_icon: Option<Box<dyn Fn(usize) -> Option<crate::icon::IconSource> + 'a>>,
    pub(crate) separators: Vec<usize>,
    pub(crate) search: Option<ComboSearch<'a, Message>>,
}

/// The query of a searchable combo box and where its edits go.
pub(crate) struct ComboSearch<'a, Message> {
    pub(crate) query: String,
    pub(crate) on_query: std::rc::Rc<dyn Fn(String) -> Message + 'a>,
    /// Whether the unfiltered list had options: an empty filter result
    /// keeps the field operable.
    pub(crate) any_options: bool,
}

/// Build a selection field with caller-owned selected text.
///
/// An empty label hides the caption. The label callback receives the chosen
/// option's text; [`EnzComboBox::on_select_index`] takes precedence when installed.
/// Defaults to enabled, medium size and [`Length::Fill`] width.
pub fn enz_combo_box<'a, Message: Clone + 'a>(
    label: impl Into<Cow<'a, str>>,
    options: Vec<String>,
    selected: Option<String>,
    on_select: impl Fn(String) -> Message + 'a,
) -> EnzComboBox<'a, Message> {
    EnzComboBox {
        label: label.into(),
        aria_label: None,
        placeholder: enzui_core::localization::text("enzui-combo-box-select").into(),
        identity_placeholder: std::borrow::Cow::Borrowed("Select..."),
        options,
        selected,
        selected_index: None,
        on_select: Box::new(on_select),
        on_select_indexed: None,
        size: TextFieldSize::Md,
        radius: None,
        error_text: None,
        enabled: true,
        read_only: false,
        palette: None,
        width: Length::Fill,
        id: None,
        id_prefix: super::DEFAULT_ID_PREFIX.to_owned(),
        direction: None,
        scroll_cell: None,
        variant: crate::tokens::FieldVariant::default(),
        option_icon: None,
        separators: Vec::new(),
        search: None,
    }
}

impl<'a, Message: Clone + 'a> EnzComboBox<'a, Message> {
    /// An accessible name published without painting a label, for a combo
    /// box labelled by its surroundings, such as an
    /// [`EnzPreferencesRow`](crate::preferences_row::EnzPreferencesRow)'s
    /// title. A nonblank name wins over the visible label and the
    /// placeholder; geometry, value, state and identity are unchanged.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Placeholder shown when nothing is selected.
    pub fn placeholder(mut self, placeholder: impl Into<std::borrow::Cow<'a, str>>) -> Self {
        self.placeholder = placeholder.into();
        self.identity_placeholder.clone_from(&self.placeholder);
        self
    }

    /// Set the fixed size tier. Unpinned trigger corners follow live
    /// `radius.sm` / `md` / `lg`, matching [`crate::text_field::EnzTextField`].
    pub fn size(mut self, size: TextFieldSize) -> Self {
        self.size = size;
        self
    }

    /// Pin the trigger and its focus-ring corners in logical pixels.
    /// Negative or non-finite input pins square corners. Popup and option-row
    /// corners retain their independent live theme radii.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }

    /// Fill treatment. Defaults to
    /// [`crate::tokens::FieldVariant::Filled`], giving the trigger a body;
    /// [`crate::tokens::FieldVariant::Outlined`] leaves it page-coloured for
    /// dense/inline placements.
    pub fn variant(mut self, variant: crate::tokens::FieldVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Attach an inline error message. A non-empty error recolours the
    /// border and renders a message line under the field.
    pub fn error(mut self, error: impl Into<Cow<'a, str>>) -> Self {
        self.error_text = Some(error.into());
        self
    }

    /// Enable / disable the field.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Keep the field focusable while preventing trigger and keyboard selection.
    ///
    /// A lock replaces the dropdown arrow. Read-only fields remain focusable even
    /// when disabled or empty. Switching an already-open popup to read-only does
    /// not dismiss it or prevent pointer selection in that retained popup.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Receive the chosen option's current index instead of its label.
    ///
    /// This callback takes precedence over the constructor's label callback and
    /// distinguishes duplicate labels. The index refers to the current option list.
    pub fn on_select_index(mut self, on_select_index: impl Fn(usize) -> Message + 'a) -> Self {
        self.on_select_indexed = Some(Box::new(on_select_index));
        self
    }

    /// Name the selected option by its position, the incoming half of
    /// [`Self::on_select_index`].
    ///
    /// The `selected` label passed to [`enz_combo_box`] marks the first option
    /// that displays it, so of two options sharing a label the second can
    /// never stay selected. Resolve a stored key to a position each frame and
    /// pass it here instead, and the selection survives a reorder or a
    /// relabel.
    ///
    /// `Some` wins over the label for the row mark, the trigger value, the
    /// value announced to assistive technology and the row an opening list
    /// highlights; an index past the last option selects nothing and does not
    /// fall back to the label. `None` is the default and restores the
    /// label-only behaviour.
    ///
    /// ```
    /// use enzui_widgets::combo_box::enz_combo_box;
    ///
    /// #[derive(Clone)]
    /// enum Message {
    ///     CityPicked(usize),
    /// }
    ///
    /// let keys = ["paris-fr", "berlin-de", "paris-us"];
    /// let labels = vec!["Paris".to_owned(), "Berlin".to_owned(), "Paris".to_owned()];
    /// let stored = Some("paris-us");
    ///
    /// let combo = enz_combo_box("City", labels, None, |_: String| Message::CityPicked(0))
    ///     .selected_index(stored.and_then(|key| keys.iter().position(|k| *k == key)))
    ///     .on_select_index(Message::CityPicked);
    ///
    /// assert!(combo.is_available());
    /// ```
    pub fn selected_index(mut self, index: Option<usize>) -> Self {
        self.selected_index = index;
        self
    }

    /// Pin a specific palette (otherwise resolved from the runtime theme).
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Set the outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the stable trigger identity for routed focus and accessibility actions.
    ///
    /// When unset, identity is derived from the visible label or placeholder,
    /// using [`Self::id_prefix`]. An accessible-name override does not change it.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the label/placeholder-derived default id.
    ///
    /// Two combo boxes sharing a placeholder ("Select…") on one screen
    /// derive the same id under the default prefix; a distinct prefix per
    /// instance keeps both individually routable. Ignored when [`Self::id`]
    /// is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the ambient writing direction used by popup placement.
    ///
    /// The popup matches the trigger width, so its leading-edge alignment normally
    /// has the same position in either direction.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Build a bounded popup window from a retained scroll-translation cell.
    ///
    /// The window covers [`VISIBLE_ROWS`] plus two overscan rows on each side,
    /// clamped to the option count and shifted to include the active option. Spacer
    /// rows preserve full content height. Keep the same cell across frames; a fresh
    /// cell resets the window's observed scroll position to the top. Without this
    /// setting every option row is built eagerly.
    pub fn virtualized(mut self, cell: crate::scroll_view::ScrollTranslation) -> Self {
        self.scroll_cell = Some(cell);
        self
    }

    /// A leading icon for each option, by index, shown in its popup row and,
    /// for the selected option, in the field. Indices identify options, so
    /// duplicate labels can carry different icons. A bundled name or a
    /// [`SymbolicSvg`](crate::icon::IconSource::SymbolicSvg) follows the text
    /// tone; any other app-owned SVG or raster keeps its own colours.
    /// Rows without an icon keep the slot, so labels stay aligned; when no
    /// option has one the layout is the same as without a resolver.
    pub fn option_icon(
        mut self,
        icon: impl Fn(usize) -> Option<crate::icon::IconSource> + 'a,
    ) -> Self {
        self.option_icon = Some(Box::new(icon));
        self
    }

    /// Paint a divider before this option without adding a keyboard selection stop.
    /// Indices refer to the original options, including when the list is filtered.
    pub fn separator_before(mut self, index: usize) -> Self {
        if !self.separators.contains(&index) {
            self.separators.push(index);
        }
        self
    }

    /// Filter the options as the user types: with the field focused, typed
    /// text, Backspace and Escape edit `query` through `on_query`, the list
    /// shows only options containing the query (ignoring case) with the first
    /// one highlighted, and the field shows the query instead of the
    /// selection. The application owns the query; the field clears it with
    /// `on_query("")` after a selection. Indices passed to
    /// [`Self::on_select_index`], [`Self::selected_index`] and
    /// [`Self::option_icon`] stay those of the full list.
    pub fn searchable(
        mut self,
        query: impl Into<String>,
        on_query: impl Fn(String) -> Message + 'a,
    ) -> Self {
        self.search = Some(ComboSearch {
            query: query.into(),
            on_query: std::rc::Rc::new(on_query),
            any_options: false,
        });
        self
    }

    /// Whether the field is enabled and has at least one option.
    ///
    /// Read-only status is checked separately when opening and committing by keyboard.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.enabled
            && (!self.options.is_empty()
                || self
                    .search
                    .as_ref()
                    .is_some_and(|search| search.any_options))
    }

    pub(crate) fn semantics(&self, open: bool) -> Semantics {
        let mut s = Semantics::new(Role::ComboBox)
            .expanded(open)
            .disabled(!self.is_available() && !self.read_only)
            .read_only(self.read_only)
            .invalid(self.error_text.as_deref().is_some_and(|e| !e.is_empty()));
        if let Some(name) = self.aria_label.as_deref().filter(|n| !n.trim().is_empty()) {
            s = s.label(name.to_owned());
        } else if !self.label.is_empty() {
            s = s.label(self.label.to_string());
        } else if !self.placeholder.is_empty() {
            s = s.label(self.placeholder.to_string());
        }
        // Placeholder text names an empty field but must not become its announced value.
        if let Some(value) = self.resolved_value() {
            s = s.value(value);
        }
        if let Some(err) = self.error_text.as_deref().filter(|e| !e.is_empty()) {
            s = s.description(err.to_owned());
        }
        if self.is_available() {
            s = s.default_action();
        }
        s
    }

    pub(crate) fn listbox_semantics(&self) -> Semantics {
        let name = if self.label.is_empty() {
            enzui_core::localization::text("enzui-combo-box-options")
        } else {
            enzui_core::localized!("enzui-combo-box-labelled-options", "label" => self.label)
        };
        Semantics::new(Role::ListBox).label(name)
    }

    pub(crate) fn resolved_selected_index(&self) -> Option<usize> {
        // An explicit invalid index selects nothing instead of falling back to the label.
        if let Some(index) = self.selected_index {
            return (index < self.options.len()).then_some(index);
        }
        let selected = self.selected.as_deref()?;
        self.options.iter().position(|o| o == selected)
    }

    pub(crate) fn resolved_value(&self) -> Option<String> {
        if self.selected_index.is_some() {
            return self
                .resolved_selected_index()
                .and_then(|index| self.options.get(index).cloned());
        }
        self.selected.clone()
    }

    /// Materialise the widget tree.
    pub fn into_element(mut self) -> Element<'a, Message> {
        self.apply_search();
        Element::new(ComboBoxWidget::new(self))
    }

    /// Narrow the options to those matching the query, keeping the
    /// caller's indices those of the full list.
    fn apply_search(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.any_options = !self.options.is_empty();
        let needle = enzui_core::text_fold::fold(search.query.trim());
        if needle.is_empty() {
            return;
        }
        let keep: std::rc::Rc<Vec<usize>> = std::rc::Rc::new(
            self.options
                .iter()
                .enumerate()
                .filter(|(_, option)| enzui_core::text_fold::fold(option).contains(&needle))
                .map(|(index, _)| index)
                .collect(),
        );
        if self.selected_index.is_some() {
            let selected = self.resolved_selected_index();
            self.selected_index = Some(
                selected
                    .and_then(|index| keep.iter().position(|kept| *kept == index))
                    .unwrap_or(usize::MAX),
            );
        }
        self.separators = keep
            .iter()
            .enumerate()
            .filter_map(|(index, original)| self.separators.contains(original).then_some(index))
            .collect();
        // Moved, not copied: the builder owns its options.
        let mut all = std::mem::take(&mut self.options);
        self.options = keep
            .iter()
            .map(|&index| std::mem::take(&mut all[index]))
            .collect();
        if let Some(on_index) = self.on_select_indexed.take() {
            let keep = std::rc::Rc::clone(&keep);
            self.on_select_indexed = Some(Box::new(move |index| on_index(keep[index])));
        }
        if let Some(icon) = self.option_icon.take() {
            let keep = std::rc::Rc::clone(&keep);
            self.option_icon = Some(Box::new(move |index| icon(keep[index])));
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzComboBox<'a, Message>> for Element<'a, Message> {
    fn from(v: EnzComboBox<'a, Message>) -> Self {
        v.into_element()
    }
}
