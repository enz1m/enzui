//! An emoji chooser: search, categories and recently used.
//!
//! [`enz_emoji_picker`] shows a search field, a row of category buttons and
//! a grid of emoji. The application keeps three things: the query, the
//! category, and an [`EmojiRecents`] list; every interaction comes back as
//! one [`EmojiPickerEvent`]. A typed query searches every category by name,
//! shortcode (`:tada:`) and keyword (`happy`, `party`); Enter picks the
//! first match. Each emoji is a
//! button named for screen readers by its Unicode name.
//!
//! The search field, the category buttons and the emoji have stable ids
//! under the picker's [`id_prefix`](EnzEmojiPicker::id_prefix) ([`search_id`],
//! [`category_id`], [`emoji_id`]), so assistive technology can press them and
//! the application can focus the search. Two pickers on screen at once take
//! different prefixes.
//!
//! ```no_run
//! use enzui_widgets::emoji_picker::{
//!     EmojiCategory, EmojiPickerEvent, EmojiRecents, enz_emoji_picker,
//! };
//! # #[derive(Clone)] enum Message { Emoji(EmojiPickerEvent) }
//! # struct App { query: String, category: EmojiCategory, recents: EmojiRecents }
//! # impl App {
//! fn view(&self) -> iced::Element<'_, Message> {
//!     enz_emoji_picker(&self.query, self.category, &self.recents, Message::Emoji).into()
//! }
//! fn update(&mut self, event: EmojiPickerEvent) {
//!     match event {
//!         EmojiPickerEvent::Query(query) => self.query = query,
//!         EmojiPickerEvent::Category(category) => self.category = category,
//!         EmojiPickerEvent::Picked(emoji) => {
//!             self.recents.record(&emoji);
//!             // insert `emoji` where the user was typing
//!         }
//!     }
//! }
//! # }
//! ```
//!
//! Emoji are drawn with the system's colour emoji font (Noto Color Emoji on
//! most distributions); without one they fall back to monochrome glyphs.
//! Emoji newer than the fonts are missing glyphs, so the picker offers those
//! up to Unicode [`NEWEST_UNICODE`]; [`EnzEmojiPicker::newest_unicode`]
//! raises it where the fonts are newer.

use crate::button::{ButtonKind, enz_button};
use crate::scroll_view::enz_scrollable;
use crate::text_field::enz_text_field;
use crate::tokens::ButtonSizeTokens;
use iced::widget::{Column, Row, column};
use iced::{Element, Length};

/// How many recently used emoji are kept.
pub const MAX_RECENTS: usize = 24;
/// How many matches a search shows.
pub const MAX_RESULTS: usize = 160;
/// The newest Unicode version whose emoji the picker offers by default:
/// what the colour emoji fonts of current distributions draw.
pub const NEWEST_UNICODE: (u32, u32) = (15, 0);
/// Emoji per grid row.
const COLUMNS: usize = 8;
/// The side of one emoji cell, in logical pixels.
const CELL: f32 = 38.0;

/// A tab of the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EmojiCategory {
    /// The emoji the user picked lately.
    Recent,
    /// Faces and emotions.
    #[default]
    Smileys,
    /// People, gestures and body parts.
    People,
    /// Animals and plants.
    Nature,
    /// Food and drink.
    Food,
    /// Travel and places.
    Travel,
    /// Sports, games and events.
    Activities,
    /// Objects.
    Objects,
    /// Symbols.
    Symbols,
    /// Flags.
    Flags,
}

impl EmojiCategory {
    /// Every category, in tab order.
    pub const ALL: [Self; 10] = [
        Self::Recent,
        Self::Smileys,
        Self::People,
        Self::Nature,
        Self::Food,
        Self::Travel,
        Self::Activities,
        Self::Objects,
        Self::Symbols,
        Self::Flags,
    ];

    fn icon(self) -> &'static str {
        match self {
            Self::Recent => "clock",
            Self::Smileys => "smile",
            Self::People => "hand",
            Self::Nature => "leaf",
            Self::Food => "apple",
            Self::Travel => "plane",
            Self::Activities => "trophy",
            Self::Objects => "lightbulb",
            Self::Symbols => "heart",
            Self::Flags => "flag",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::Smileys => "smileys",
            Self::People => "people",
            Self::Nature => "nature",
            Self::Food => "food",
            Self::Travel => "travel",
            Self::Activities => "activities",
            Self::Objects => "objects",
            Self::Symbols => "symbols",
            Self::Flags => "flags",
        }
    }

    fn group(self) -> Option<emojis::Group> {
        Some(match self {
            Self::Recent => return None,
            Self::Smileys => emojis::Group::SmileysAndEmotion,
            Self::People => emojis::Group::PeopleAndBody,
            Self::Nature => emojis::Group::AnimalsAndNature,
            Self::Food => emojis::Group::FoodAndDrink,
            Self::Travel => emojis::Group::TravelAndPlaces,
            Self::Activities => emojis::Group::Activities,
            Self::Objects => emojis::Group::Objects,
            Self::Symbols => emojis::Group::Symbols,
            Self::Flags => emojis::Group::Flags,
        })
    }
}

/// What the user did in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmojiPickerEvent {
    /// The search text changed.
    Query(String),
    /// A category tab was chosen.
    Category(EmojiCategory),
    /// An emoji was chosen.
    Picked(String),
}

/// The recently used emoji, newest first, at most [`MAX_RECENTS`]. Store
/// [`EmojiRecents::list`] with the app's settings and restore it with
/// [`EmojiRecents::from_list`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmojiRecents {
    list: Vec<String>,
}

impl EmojiRecents {
    /// No recent emoji.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The recents saved earlier, newest first; unknown entries are dropped.
    pub fn from_list(list: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut recents = Self::new();
        let list: Vec<String> = list.into_iter().map(Into::into).collect();
        for emoji in list.iter().rev() {
            recents.record(emoji);
        }
        recents
    }

    /// Puts `emoji` first.
    pub fn record(&mut self, emoji: &str) {
        if emojis::get(emoji).is_none() {
            return;
        }
        self.list.retain(|kept| kept != emoji);
        self.list.insert(0, emoji.to_owned());
        self.list.truncate(MAX_RECENTS);
    }

    /// The recent emoji, newest first.
    #[must_use]
    pub fn list(&self) -> &[String] {
        &self.list
    }
}

/// The emoji up to Unicode [`NEWEST_UNICODE`] matching `query` by every
/// word of it: in the name, a shortcode (`tada` finds 🎉), or the start of
/// one of its keywords (`happy` finds 😀 and 🥳). Best matches first: a
/// name or shortcode that starts with the query, then emoji whose every
/// query word is a whole word of the name or a keyword, then the rest.
#[must_use]
pub fn search(query: &str) -> Vec<&'static emojis::Emoji> {
    search_up_to(query, NEWEST_UNICODE)
}

/// [`search`] among the emoji up to Unicode `newest` (`(major, minor)`).
#[must_use]
pub fn search_up_to(query: &str, newest: (u32, u32)) -> Vec<&'static emojis::Emoji> {
    let newest = emojis::UnicodeVersion::new(newest.0, newest.1);
    let query = query.trim().trim_matches(':').to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    // Three bounded buckets, each in Unicode order, best first.
    let mut tiers: [Vec<&'static emojis::Emoji>; 3] = Default::default();
    for emoji in emojis::iter().filter(|emoji| emoji.unicode_version() <= newest) {
        let Some(tier) = match_tier(emoji, &query, &words) else {
            continue;
        };
        if tiers[tier].len() < MAX_RESULTS {
            tiers[tier].push(emoji);
        }
        if tier == 0 && tiers[0].len() == MAX_RESULTS {
            break;
        }
    }
    tiers.into_iter().flatten().take(MAX_RESULTS).collect()
}

/// How well `emoji` matches `query` (split into `words`): `0` for a name or
/// shortcode starting with the query, `1` when every word is a whole word
/// of the name or a keyword, `2` for any other match, `None` for none.
fn match_tier(emoji: &emojis::Emoji, query: &str, words: &[&str]) -> Option<usize> {
    let name = emoji.name().to_lowercase();
    let keywords = keywords_of(emoji);
    let found = words.iter().all(|word| {
        name.contains(word)
            || emoji.shortcodes().any(|code| code.contains(word))
            || keywords
                .clone()
                .any(|keyword| keyword.split(' ').any(|part| part.starts_with(word)))
    });
    if !found {
        return None;
    }
    if name.starts_with(query) || emoji.shortcodes().any(|code| code.starts_with(query)) {
        return Some(0);
    }
    let whole = words.iter().all(|word| {
        name.split([' ', '-', ':']).any(|part| part == *word)
            || keywords.clone().any(|keyword| keyword == *word)
    });
    Some(if whole { 1 } else { 2 })
}

/// The search keywords of `emoji` beyond its name, from Unicode CLDR (see
/// `emoji_picker/README.md`).
fn keywords_of(emoji: &emojis::Emoji) -> impl Iterator<Item = &'static str> + Clone {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static KEYWORDS: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let table = KEYWORDS.get_or_init(|| {
        include_str!("emoji_picker/keywords-en.txt")
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect()
    });
    let key: String = emoji
        .as_str()
        .chars()
        .filter(|c| *c != '\u{fe0f}')
        .collect();
    table
        .get(key.as_str())
        .copied()
        .unwrap_or_default()
        .split('|')
        .filter(|keyword| !keyword.is_empty())
}

/// The id kind every picker's owner namespace takes.
const ID_KIND: &str = "emoji-picker";

fn owner(prefix: &str) -> enzui_core::identity::OwnerId {
    enzui_core::identity::OwnerId::of_kind(ID_KIND, prefix)
}

/// The id of the search field of the picker with `prefix` (see
/// [`EnzEmojiPicker::id_prefix`]); focus it to type a query.
pub fn search_id(prefix: &str) -> iced::advanced::widget::Id {
    owner(prefix).id("search")
}

/// The id of `category`'s button in the picker with `prefix`.
pub fn category_id(prefix: &str, category: EmojiCategory) -> iced::advanced::widget::Id {
    owner(prefix).child("category").id(category.key())
}

/// The id of `emoji`'s button in the picker with `prefix`.
pub fn emoji_id(prefix: &str, emoji: &str) -> iced::advanced::widget::Id {
    owner(prefix).child("emoji").id(emoji)
}

/// An emoji picker; see the module docs.
pub struct EnzEmojiPicker<'a, Message> {
    id_prefix: String,
    query: &'a str,
    category: EmojiCategory,
    recents: &'a EmojiRecents,
    on_event: std::rc::Rc<dyn Fn(EmojiPickerEvent) -> Message + 'a>,
    height: f32,
    newest: (u32, u32),
}

/// An emoji picker showing `category` (or the matches for a non-empty
/// `query`), with `recents` under [`EmojiCategory::Recent`].
pub fn enz_emoji_picker<'a, Message>(
    query: &'a str,
    category: EmojiCategory,
    recents: &'a EmojiRecents,
    on_event: impl Fn(EmojiPickerEvent) -> Message + 'a,
) -> EnzEmojiPicker<'a, Message> {
    EnzEmojiPicker {
        id_prefix: String::new(),
        query,
        category,
        recents,
        on_event: std::rc::Rc::new(on_event),
        height: 6.0 * CELL,
        newest: NEWEST_UNICODE,
    }
}

impl<'a, Message: Clone + 'a> EnzEmojiPicker<'a, Message> {
    /// Names this picker's ids (empty by default); see [`search_id`],
    /// [`category_id`] and [`emoji_id`]. Pickers shown at the same time need
    /// different prefixes.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// The grid's height in logical pixels (six rows by default).
    #[must_use]
    pub fn grid_height(mut self, height: f32) -> Self {
        self.height = height.max(CELL);
        self
    }

    /// Offers emoji up to Unicode `major.minor`, for fonts newer than
    /// [`NEWEST_UNICODE`].
    #[must_use]
    pub fn newest_unicode(mut self, major: u32, minor: u32) -> Self {
        self.newest = (major, minor);
        self
    }

    fn cell(&self, glyph: &str, name: &str) -> Element<'a, Message> {
        let tokens = ButtonSizeTokens {
            height: CELL,
            padding_x: 0.0,
            text_size: 22.0,
            icon_size: 22.0,
            radius: 8.0,
            gap: 0.0,
        };
        enz_button(
            glyph.to_owned(),
            (self.on_event)(EmojiPickerEvent::Picked(glyph.to_owned())),
        )
        .kind(ButtonKind::Quiet)
        .size_tokens(tokens)
        .width(Length::Fixed(CELL))
        .aria_label(name.to_owned())
        .id(emoji_id(&self.id_prefix, glyph))
        .into_element()
    }

    fn grid(&self, emoji: Vec<(String, String)>) -> Element<'a, Message> {
        if emoji.is_empty() {
            let empty = if self.query.trim().is_empty() {
                "enzui-emoji-no-recents"
            } else {
                "enzui-emoji-no-results"
            };
            return crate::text::enz_text(enzui_core::localization::text(empty))
                .size(crate::text::TextSize::Caption)
                .into_element();
        }
        let mut rows = Column::new().spacing(2.0);
        for chunk in emoji.chunks(COLUMNS) {
            let mut line = Row::new().spacing(2.0);
            for (glyph, name) in chunk {
                line = line.push(self.cell(glyph, name));
            }
            rows = rows.push(line);
        }
        rows.into()
    }

    /// The picker as an element.
    pub fn into_element(self) -> Element<'a, Message> {
        let searching = !self.query.trim().is_empty();
        let shown: Vec<(String, String)> = if searching {
            search_up_to(self.query, self.newest)
                .into_iter()
                .map(|emoji| (emoji.as_str().to_owned(), emoji.name().to_owned()))
                .collect()
        } else if let Some(group) = self.category.group() {
            let newest = emojis::UnicodeVersion::new(self.newest.0, self.newest.1);
            group
                .emojis()
                .filter(|emoji| emoji.unicode_version() <= newest)
                .map(|emoji| (emoji.as_str().to_owned(), emoji.name().to_owned()))
                .collect()
        } else {
            self.recents
                .list()
                .iter()
                .filter_map(|glyph| emojis::get(glyph))
                .map(|emoji| (emoji.as_str().to_owned(), emoji.name().to_owned()))
                .collect()
        };
        let first = shown.first().map(|(glyph, _)| glyph.clone());

        let on_query = {
            let on_event = std::rc::Rc::clone(&self.on_event);
            move |query: String| on_event(EmojiPickerEvent::Query(query))
        };
        let mut field = enz_text_field(
            "",
            enzui_core::localization::text("enzui-emoji-search"),
            self.query,
            on_query,
        )
        .id(search_id(&self.id_prefix))
        .leading(
            crate::icon::enz_icon("search")
                .size(16.0)
                .tint_theme_tone(crate::icon::IconTone::Muted)
                .into_element(),
        );
        if let Some(first) = first {
            field = field.on_submit((self.on_event)(EmojiPickerEvent::Picked(first)));
        }

        let mut tabs = Row::new().spacing(2.0).width(Length::Fill);
        for category in EmojiCategory::ALL {
            let label = enzui_core::localization::text(&format!("enzui-emoji-{}", category.key()));
            tabs = tabs.push(
                enz_button("", (self.on_event)(EmojiPickerEvent::Category(category)))
                    .icon(category.icon())
                    .icon_position(crate::button::ButtonIconPosition::Only)
                    .size_tokens(ButtonSizeTokens {
                        height: 30.0,
                        padding_x: 0.0,
                        text_size: 13.0,
                        icon_size: 17.0,
                        radius: 6.0,
                        gap: 0.0,
                    })
                    .kind(ButtonKind::Quiet)
                    .selected(!searching && category == self.category)
                    .aria_label(label)
                    .id(category_id(&self.id_prefix, category))
                    // Ten tabs share the picker's width.
                    .width(Length::Fill)
                    .into_element(),
            );
        }

        let grid = crate::scroll_view::contain_wheel(
            enz_scrollable(self.grid(shown))
                .height(Length::Fixed(self.height))
                .width(Length::Fill),
            crate::scroll_view::Axis::Vertical,
        );
        column![field, tabs, grid]
            .spacing(8.0)
            .width(Length::Fixed(COLUMNS as f32 * (CELL + 2.0) + 16.0))
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzEmojiPicker<'a, Message>> for Element<'a, Message> {
    fn from(picker: EnzEmojiPicker<'a, Message>) -> Self {
        picker.into_element()
    }
}
