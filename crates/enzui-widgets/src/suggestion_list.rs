//! Standalone suggestions with focusable, named listbox options.
//!
//! The caller owns filtering, ranking, placement, arrow-key routing and
//! highlight state. Rows emit selection messages on pointer or keyboard
//! activation; highlighting alone does not move focus or scroll into view.
//! Use distinct list prefixes and unique option keys to avoid identity collisions.
//! Conditional siblings of a focused input need [`crate::keyed_column`] to
//! preserve its retained state. Overlay placement is caller-owned.
//!
//! [`crate::autocomplete_field`] owns a separate single-line input and overlay;
//! both widgets share option presentation. [`crate::command_palette`] owns its
//! query input and results.

use enzui_core::context_menu::{MenuEntry, MenuMove, MenuNavigation};
use enzui_theme::Palette;
use iced::advanced::widget::Id;
use iced::widget::{column, container, rich_text, row, span, text};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::enz_text::TextTone;
use crate::text_field::TextFieldSize;
use crate::text_pair::{TextPairTokens, enz_text_pair};

/// Row size tiers shared with text fields.
pub type SuggestionSize = TextFieldSize;

/// Number of common-height rows admitted by the popup height cap.
pub const VISIBLE_ROWS: usize = 6;

const POPUP_PADDING: f32 = 4.0;

/// Legacy subject-width share retained for source compatibility.
///
/// Rich results shape subject and preview as one wrapping paragraph, so this
/// value is not consulted. Callers should migrate to
/// [`SuggestionOption::rich_result`] or
/// [`crate::autocomplete_field::AutocompleteOption::rich_result`], which own
/// the wrapping layout instead of applying a fixed subject-width share.
#[deprecated(
    since = "0.2.0",
    note = "use `SuggestionOption::rich_result` or \
            `autocomplete_field::AutocompleteOption::rich_result`; both own rich-result wrapping"
)]
pub const RICH_SUBJECT_MAX_SHARE: f32 = 0.5;

// Padding reduces shaped width; viewport clipping alone does not constrain text glyphs.
const GLYPH_SAFETY_INSET: f32 = 8.0;

/// The default namespace prefix for suggestion-list row ids.
pub const DEFAULT_ID_PREFIX: &str = "enz-suggestion-list";

/// Nominal standalone height cap in pixels for title-only rows.
///
/// Subtitle and rich-result lists use a larger common row height; embedded lists
/// omit outer padding. This helper does not compute those caps.
#[must_use]
pub fn popup_max_height(size: SuggestionSize) -> f32 {
    row_height(size) * VISIBLE_ROWS as f32 + POPUP_PADDING * 2.0
}

/// Base title-only row height in logical pixels.
#[must_use]
pub fn row_height(size: SuggestionSize) -> f32 {
    tokens_for(size).height
}

fn tokens_for(size: SuggestionSize) -> crate::tokens::FieldSizeTokens {
    size.tokens()
}

/// Shared title, subtitle and rich-result presentation for suggestion and autocomplete options.
///
/// Rich results use a title/metadata line and a subject/preview paragraph bounded
/// to two detail lines. The enclosing option owns activation and accessibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionRowContent {
    title: String,
    subtitle: String,
    trailing: String,
    subject: String,
    preview: String,
}

impl OptionRowContent {
    pub(crate) fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: String::new(),
            trailing: String::new(),
            subject: String::new(),
            preview: String::new(),
        }
    }

    pub(crate) fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    pub(crate) fn rich_result(
        mut self,
        trailing: impl Into<String>,
        subject: impl Into<String>,
        preview: impl Into<String>,
    ) -> Self {
        self.subtitle.clear();
        self.trailing = trailing.into();
        self.subject = subject.into();
        self.preview = preview.into();
        self
    }

    /// The leading title (for mail results, the sender).
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Stored subtitle when it contains non-whitespace text; surrounding whitespace is preserved.
    #[must_use]
    pub fn subtitle_text(&self) -> Option<&str> {
        (!self.subtitle.trim().is_empty()).then_some(self.subtitle.as_str())
    }

    /// Stored trailing metadata when it contains non-whitespace text.
    #[must_use]
    pub fn trailing_metadata_text(&self) -> Option<&str> {
        (!self.trailing.trim().is_empty()).then_some(self.trailing.as_str())
    }

    /// Stored subject when it contains non-whitespace text.
    #[must_use]
    pub fn subject_text(&self) -> Option<&str> {
        (!self.subject.trim().is_empty()).then_some(self.subject.as_str())
    }

    /// Stored preview when it contains non-whitespace text; visual rendering is bounded.
    #[must_use]
    pub fn preview_text(&self) -> Option<&str> {
        (!self.preview.trim().is_empty()).then_some(self.preview.as_str())
    }

    pub(crate) fn is_rich_result(&self) -> bool {
        self.trailing_metadata_text().is_some()
            || self.subject_text().is_some()
            || self.preview_text().is_some()
    }

    pub(crate) fn accessible_name(&self) -> String {
        if self.is_rich_result() {
            [
                Some(self.title.as_str()),
                self.trailing_metadata_text(),
                self.subject_text(),
                self.preview_text(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
        } else {
            self.subtitle_text().map_or_else(
                || self.title.clone(),
                |subtitle| format!("{}, {subtitle}", self.title),
            )
        }
    }
}

pub(crate) fn option_row_content<'a, Message: 'a>(
    content: OptionRowContent,
    tokens: crate::tokens::FieldSizeTokens,
    palette: Option<Palette>,
) -> Element<'a, Message> {
    if content.is_rich_result() {
        let title = content.title.clone();
        let trailing = content.trailing.clone();
        let subject = content.subject.clone();
        let preview = content.preview;
        let pair_tokens = palette.as_ref().map(TextPairTokens::from_palette);
        let title_line: Element<'a, Message> = {
            let mut pair = enz_text_pair(title, trailing)
                .leading_size(tokens.text_size)
                .trailing_size((tokens.text_size - 2.0).max(10.0))
                .announce(false);
            if let Some(pair_tokens) = pair_tokens {
                pair = pair.tokens(pair_tokens);
            }
            pair.into()
        };
        let text_size = (tokens.text_size - 2.0).max(10.0);
        // One paragraph lets wrapped previews return to the detail-line origin.
        let mut subject_span = span(format!("{subject} — "));
        let mut preview_span = span(preview);
        if let Some(p) = palette {
            subject_span = subject_span.color(p.text);
            preview_span = preview_span.color(p.text_muted);
        } else {
            preview_span = preview_span.color_key(crate::rich_text::tone_key(TextTone::Muted));
        }
        let detail_line: Element<'a, Message> = container(
            rich_text::<(), Message, iced::Theme>([subject_span, preview_span])
                .size(text_size)
                .line_height(iced::widget::text::LineHeight::Relative(1.2))
                .width(Length::Fill)
                .height(Length::Fixed(text_size * 2.4))
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                .ellipsis(iced::widget::text::Ellipsis::End)
                .resolve_color(crate::rich_text::resolve_tone_key),
        )
        .padding(Padding::ZERO.right(GLYPH_SAFETY_INSET))
        .clip(true)
        .width(Length::Fill)
        .into();
        column![title_line, detail_line]
            .spacing(2.0)
            .width(Length::Fill)
            .into()
    } else if let Some(subtitle) = content.subtitle_text().map(str::to_owned) {
        let title_palette = palette.clone();
        let subtitle_palette = palette;
        column![
            text(content.title)
                .size(tokens.text_size)
                .style(move |theme: &iced::Theme| {
                    iced::widget::text::Style {
                        color: Some(
                            title_palette
                                .clone()
                                .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                                .text,
                        ),
                    }
                }),
            text(subtitle)
                .size((tokens.text_size - 2.0).max(10.0))
                .style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(
                        subtitle_palette
                            .clone()
                            .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                            .text_muted,
                    ),
                }),
        ]
        .spacing(2.0)
        .into()
    } else {
        let title_palette = palette;
        text(content.title)
            .size(tokens.text_size)
            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(
                    title_palette
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                        .text,
                ),
            })
            .into()
    }
}

pub(crate) fn option_row_height(
    tokens: crate::tokens::FieldSizeTokens,
    has_subtitle: bool,
    has_rich_result: bool,
) -> f32 {
    if has_rich_result {
        tokens.height + tokens.text_size * 2.0
    } else if has_subtitle {
        tokens.height + tokens.text_size
    } else {
        tokens.height
    }
}

/// Suggestion identity, presentation and typed selection payload.
///
/// Keys must be unique within the list and stable across reorderings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionOption<T> {
    key: String,
    content: OptionRowContent,
    payload: T,
}

impl<T> SuggestionOption<T> {
    /// Build a one-line suggestion. Add [`Self::subtitle`] for a simple rich
    /// row or [`Self::rich_result`] for the three-line search-result shape.
    pub fn new(key: impl Into<String>, label: impl Into<String>, payload: T) -> Self {
        Self {
            key: key.into(),
            content: OptionRowContent::new(label),
            payload,
        }
    }

    /// Set a secondary line included in the option name.
    ///
    /// Nonblank rich-result fields take precedence over this subtitle.
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.content = self.content.subtitle(subtitle);
        self
    }

    /// Use the rich search-result hierarchy.
    ///
    /// The existing label is the leading title (for example, sender).
    /// `trailing` is protected, right-aligned metadata on that title line.
    /// `subject` starts the next line and `preview` follows it in the same
    /// wrapping paragraph. If the preview wraps, its continuation returns to
    /// the beginning of the line instead of staying indented after the subject;
    /// the detail paragraph is clipped to two visual lines, giving the option
    /// at most three lines overall.
    /// Calling this clears a previously configured subtitle. If all three
    /// fields are blank, the option renders as a title-only row.
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

    /// Stored subtitle when it contains non-whitespace text, even if rich fields hide it.
    #[must_use]
    pub fn subtitle_text(&self) -> Option<&str> {
        self.content.subtitle_text()
    }

    /// The shared row content consumed by both option widgets.
    #[must_use]
    pub fn row_content(&self) -> &OptionRowContent {
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

/// Builder for a standalone suggestion list. See [`enz_suggestion_list`].
pub struct EnzSuggestionList<'a, T, Message> {
    options: Vec<SuggestionOption<T>>,
    on_select: Box<dyn Fn(&T) -> Message + 'a>,
    highlighted: Option<usize>,
    size: SuggestionSize,
    palette: Option<Palette>,
    width: Length,
    id_prefix: String,
    label: String,
    empty_text: String,
    loading_text: String,
    loading: bool,
    show_empty: bool,
    embedded: bool,
}

/// Construct a list from already filtered and ranked options.
///
/// `on_select` runs for every option while constructing the element to prepare its
/// message; keep it free of selection side effects. Rows publish the prepared
/// message on pointer, keyboard or accessibility activation.
///
/// Defaults are medium rows, fill width and no highlight. The list owns no overlay.
pub fn enz_suggestion_list<'a, T, Message>(
    options: Vec<SuggestionOption<T>>,
    on_select: impl Fn(&T) -> Message + 'a,
) -> EnzSuggestionList<'a, T, Message> {
    EnzSuggestionList {
        options,
        on_select: Box::new(on_select),
        highlighted: None,
        size: SuggestionSize::Md,
        palette: None,
        width: Length::Fill,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        label: enzui_core::localization::text("enzui-suggestion-list-suggestions"),
        empty_text: enzui_core::localization::text("enzui-suggestion-list-no-matches"),
        loading_text: enzui_core::localization::text("enzui-suggestion-list-searching"),
        loading: false,
        show_empty: false,
        embedded: false,
    }
}

impl<'a, T, Message> EnzSuggestionList<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    /// Set the caller-owned highlight index without moving focus or scrolling.
    ///
    /// An out-of-range index highlights no row.
    #[must_use]
    pub fn highlighted(mut self, index: Option<usize>) -> Self {
        self.highlighted = index;
        self
    }

    /// Size tier (default `Md`).
    #[must_use]
    pub fn size(mut self, size: SuggestionSize) -> Self {
        self.size = size;
        self
    }

    /// Pin a palette instead of resolving from the runtime theme.
    #[must_use]
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Outer width (default `Length::Fill`).
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Namespace for row identities; defaults to [`DEFAULT_ID_PREFIX`].
    ///
    /// Use distinct prefixes for lists that share a widget tree.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Accessible label for the listbox node (default `"Suggestions"`).
    #[must_use]
    pub fn list_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Text for the "nothing matched" notice (default `"No matches"`).
    #[must_use]
    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// Text for the in-flight notice (default `"Searching…"`).
    #[must_use]
    pub fn loading_text(mut self, text: impl Into<String>) -> Self {
        self.loading_text = text.into();
        self
    }

    /// Show the loading notice when there are no options, even if `show_empty` is false.
    ///
    /// Existing options remain visible while loading.
    #[must_use]
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Show the empty notice when there are no options and loading is false.
    ///
    /// Defaults to false. Loading independently keeps an empty list visible.
    #[must_use]
    pub fn show_empty(mut self, show: bool) -> Self {
        self.show_empty = show;
        self
    }

    /// Remove the outer popup fill, border and padding; defaults to false.
    ///
    /// Rows remain borderless at rest in either mode. The cap still admits six
    /// common-height rows, without outer padding in embedded mode.
    #[must_use]
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Compute a highlight index without changing this list.
    ///
    /// Next and previous wrap; an invalid current index is treated as no highlight.
    /// Returns `None` for an empty list. The caller applies the result through
    /// [`Self::highlighted`] or [`Self::set_highlight`]. Each call builds and scans
    /// a navigation model proportional to the option count.
    #[must_use]
    pub fn nav_target(&self, current: Option<usize>, movement: MenuMove) -> Option<usize> {
        let mut nav = MenuNavigation::new(self.options.iter().map(|_| MenuEntry::item()).collect());
        if let Some(index) = current {
            nav.set_active(index);
        }
        nav.apply(movement)
    }

    /// Set or clear the caller-owned highlight without moving focus or scrolling.
    ///
    /// An out-of-range index highlights no row.
    pub fn set_highlight(&mut self, index: Option<usize>) {
        self.highlighted = index;
    }

    /// The [`Id`] of the currently highlighted row, for
    /// `aria-activedescendant`. Returns `None` when nothing is highlighted or
    /// the index is out of bounds.
    ///
    /// The caller's focused input should publish this via
    /// [`Semantics::active_descendant`](enzui_core::semantics::Semantics::active_descendant).
    #[must_use]
    pub fn highlighted_row_id(&self) -> Option<Id> {
        let index = self.highlighted?;
        let option = self.options.get(index)?;
        Some(enzui_core::identity::derive_id(
            &self.id_prefix,
            &option.key,
        ))
    }

    /// Highlighted payload, or `None` for an absent or out-of-range highlight.
    #[must_use]
    pub fn highlighted_payload(&self) -> Option<&T> {
        self.highlighted
            .and_then(|i| self.options.get(i))
            .map(|o| &o.payload)
    }

    /// How many options are in the list.
    #[must_use]
    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    /// Construct the list, or an empty `Space` when there are no options,
    /// loading is false and [`Self::show_empty`] is false.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.build_element()
            .unwrap_or_else(|| iced::widget::Space::new().into())
    }

    fn build_element(self) -> Option<Element<'a, Message>> {
        let has_content = !self.options.is_empty() || self.loading || self.show_empty;
        if !has_content {
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
        let highlighted = self.highlighted;
        // Mixed content shares a row height so targets and the six-row cap agree.
        let option_row_height = option_row_height(
            tokens,
            self.options
                .iter()
                .any(|option| option.subtitle_text().is_some()),
            self.options
                .iter()
                .any(|option| option.content.is_rich_result()),
        );

        let row_ids: Vec<Id> = self
            .options
            .iter()
            .map(|opt| enzui_core::identity::derive_id(&self.id_prefix, &opt.key))
            .collect();

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
            let is_active = highlighted == Some(index);
            let label = option_row_content(option.content.clone(), tokens, self.palette.clone());

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
                    background: is_active.then_some(Background::Color(p.surface_hover)),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: crate::theme_lookup::shape_radius(theme, |s| s.row).into(),
                    },
                    ..container::Style::default()
                }
            });

            // The focusable surface owns the option name, avoiding a separate unnamed button.
            let row_semantics = crate::listbox_shape::listbox_option(
                option.accessible_name(),
                is_active,
                index + 1,
                total,
            );

            let row_id = row_ids.get(index).cloned().unwrap_or_else(Id::unique);

            let msg = (self.on_select)(&option.payload);
            let mut pressable = crate::pressable::enz_pressable(cell.into(), msg)
                .style(crate::pressable::PressableStyle::Row)
                .radius(enzui_theme::text_defaults::shape_radius(|s| s.row))
                .semantics(row_semantics)
                .id(row_id);
            if let Some(palette) = self.palette.as_ref() {
                pressable =
                    pressable.tokens(crate::pressable::PressableTokens::from_palette(palette));
            }
            let pressable_row = pressable.into_element();

            list = list.push(pressable_row);
        }

        let embedded = self.embedded;
        let surface_resolve = resolve.clone();
        let surface = container(list)
            .padding(if embedded { 0.0 } else { POPUP_PADDING })
            .width(self.width)
            .style(move |theme: &iced::Theme| {
                let p = surface_resolve(theme);
                container::Style {
                    background: (!embedded).then_some(Background::Color(p.surface_elevated)),
                    border: if embedded {
                        Border::default()
                    } else {
                        Border {
                            color: crate::surface_edge::edge_color(
                                p.surface_elevated,
                                crate::surface_edge::DEFAULT_EDGE_DEPTH,
                            ),
                            width: 1.0,
                            radius: crate::theme_lookup::shape_radius(theme, |s| s.popup).into(),
                        }
                    },
                    ..container::Style::default()
                }
            });

        let listbox_semantics = crate::listbox_shape::listbox(self.label);

        let scroller = crate::scroll_view::contain_wheel(
            crate::scroll_view::enz_scrollable(surface).height(Length::Shrink),
            crate::scroll_view::Axis::Vertical,
        );
        let popup_padding = if embedded { 0.0 } else { POPUP_PADDING * 2.0 };
        let popup_max_height = option_row_height * VISIBLE_ROWS as f32 + popup_padding;
        let capped = crate::max_height::MaxHeight::new(scroller, popup_max_height);

        Some(crate::semantics_container::SemanticsContainer::new(capped, listbox_semantics).into())
    }
}

impl<'a, T, Message> From<EnzSuggestionList<'a, T, Message>> for Element<'a, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn from(list: EnzSuggestionList<'a, T, Message>) -> Self {
        list.into_element()
    }
}
