//! A font chooser: the installed families, their styles, a size and a
//! preview.
//!
//! The application keeps a [`FontChooserState`] (the installed families,
//! the search text and the list's scroll position) and the [`FontChoice`]
//! being made; [`enz_font_chooser`] shows them and reports each change as a
//! [`FontChooserEvent`], which [`FontChooserState::apply`] folds into both.
//! Each family name is drawn in its own font; only the rows in view are
//! built, so a system with hundreds of families stays quick. The preview
//! shows a sample text in the chosen family, style and size, and
//! [`FontChoice::font`] is the `iced::Font` to use.
//!
//! ```no_run
//! use enzui_widgets::font_chooser::{
//!     FontChoice, FontChooserEvent, FontChooserState, enz_font_chooser,
//! };
//! # #[derive(Clone)] enum Message { Font(FontChooserEvent) }
//! # struct App { fonts: FontChooserState, choice: FontChoice }
//! # impl App {
//! fn view(&self) -> iced::Element<'_, Message> {
//!     enz_font_chooser(&self.fonts, &self.choice, Message::Font).into()
//! }
//! fn update(&mut self, event: FontChooserEvent) {
//!     self.fonts.apply(&event, &mut self.choice);
//! }
//! # }
//! ```

use crate::list_row_surface::enz_list_row_surface;
use crate::scroll_view::enz_scrollable;
use crate::text::{TextSize, enz_text};
use crate::text_field::enz_text_field;
use enzui_core::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};
use iced::widget::{Column, column, container, row, text};
use iced::{Element, Font, Length, font};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

/// The height of a family row, in logical pixels.
const ROW: f32 = 30.0;
/// The height of the family list.
const LIST_HEIGHT: f32 = 270.0;

/// A family name with a `'static` lifetime, as `iced::Font` needs; each
/// name is stored once for the life of the process.
fn interned(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(name) = names.get(name) {
        return name;
    }
    let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(name);
    name
}

/// One style of a family: its weight (100 to 900) and slant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontStyle {
    /// Italic or oblique.
    pub italic: bool,
    /// The CSS weight: 400 regular, 700 bold.
    pub weight: u16,
}

impl FontStyle {
    /// The upright regular style.
    pub const REGULAR: Self = Self {
        italic: false,
        weight: 400,
    };

    /// The style's usual name: `Regular`, `Bold`, `Light Italic`; a weight
    /// between the standard ones is named by its number (`Weight 450`).
    #[must_use]
    pub fn name(self) -> String {
        let weight = match self.weight {
            100 => "Thin".to_owned(),
            200 => "Extra Light".to_owned(),
            300 => "Light".to_owned(),
            400 => "Regular".to_owned(),
            500 => "Medium".to_owned(),
            600 => "Semi Bold".to_owned(),
            700 => "Bold".to_owned(),
            800 => "Extra Bold".to_owned(),
            900 => "Black".to_owned(),
            other => format!("Weight {other}"),
        };
        match (weight.as_str(), self.italic) {
            ("Regular", true) => "Italic".to_owned(),
            (_, true) => format!("{weight} Italic"),
            (_, false) => weight,
        }
    }

    fn iced_weight(self) -> font::Weight {
        match self.weight {
            0..=149 => font::Weight::Thin,
            150..=249 => font::Weight::ExtraLight,
            250..=349 => font::Weight::Light,
            350..=449 => font::Weight::Normal,
            450..=549 => font::Weight::Medium,
            550..=649 => font::Weight::Semibold,
            650..=749 => font::Weight::Bold,
            750..=849 => font::Weight::ExtraBold,
            _ => font::Weight::Black,
        }
    }
}

/// An installed family and its styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFamilyInfo {
    /// The family name.
    pub name: &'static str,
    /// Whether every face is fixed-width.
    pub monospace: bool,
    /// The styles, upright before italic, light before bold.
    pub styles: Vec<FontStyle>,
}

/// The families of the process's font system (the system's fonts and any
/// the application loaded), sorted by name without case. Hidden families
/// (names starting with a dot) are left out.
#[must_use]
pub fn system_font_families() -> Vec<FontFamilyInfo> {
    let Ok(mut system) = iced::advanced::graphics::text::font_system().write() else {
        return Vec::new();
    };
    let mut families: std::collections::BTreeMap<String, FontFamilyInfo> =
        std::collections::BTreeMap::new();
    for face in system.raw().db().faces() {
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let entry = families
            .entry(name.to_lowercase())
            .or_insert_with(|| FontFamilyInfo {
                name: interned(name),
                monospace: true,
                styles: Vec::new(),
            });
        entry.monospace &= face.monospaced;
        let style = FontStyle {
            italic: face.style
                != iced::advanced::graphics::text::cosmic_text::fontdb::Style::Normal,
            weight: face.weight.0,
        };
        if !entry.styles.contains(&style) {
            entry.styles.push(style);
        }
    }
    families
        .into_values()
        .map(|mut family| {
            family.styles.sort_unstable();
            family
        })
        .collect()
}

/// The font being chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct FontChoice {
    /// The family name.
    pub family: String,
    /// The style.
    pub style: FontStyle,
    /// The size in points (logical pixels).
    pub size: f32,
}

impl FontChoice {
    /// `family` in its regular style at `size`.
    pub fn new(family: impl Into<String>, size: f32) -> Self {
        Self {
            family: family.into(),
            style: FontStyle::REGULAR,
            size,
        }
    }

    /// The choice as an `iced::Font` (family, weight and slant; use
    /// [`Self::size`] for the size).
    #[must_use]
    pub fn font(&self) -> Font {
        Font {
            family: font::Family::Name(interned(&self.family)),
            weight: self.style.iced_weight(),
            style: if self.style.italic {
                font::Style::Italic
            } else {
                font::Style::Normal
            },
            ..Font::DEFAULT
        }
    }
}

/// A change made in the chooser.
#[derive(Debug, Clone, PartialEq)]
pub enum FontChooserEvent {
    /// The search text changed.
    Query(String),
    /// A family was chosen.
    Family(String),
    /// A style was chosen.
    Style(FontStyle),
    /// The size changed.
    Size(f32),
}

/// The chooser's own state: the families, the search text and the list's
/// scroll position.
#[derive(Debug, Clone)]
pub struct FontChooserState {
    families: Rc<Vec<FontFamilyInfo>>,
    /// Each family's name folded for search, in `families` order.
    folded: Rc<Vec<String>>,
    query: String,
    monospace_only: bool,
    scroll: crate::scroll_view::ScrollTranslation,
}

impl Default for FontChooserState {
    fn default() -> Self {
        Self::new()
    }
}

impl FontChooserState {
    /// The installed families, from [`system_font_families`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_families(system_font_families())
    }

    /// A chooser over `families`.
    #[must_use]
    pub fn with_families(families: Vec<FontFamilyInfo>) -> Self {
        let folded = families
            .iter()
            .map(|family| enzui_core::text_fold::fold(family.name))
            .collect();
        Self {
            families: Rc::new(families),
            folded: Rc::new(folded),
            query: String::new(),
            monospace_only: false,
            scroll: crate::scroll_view::ScrollTranslation::new(),
        }
    }

    /// Lists fixed-width families only, for a code or terminal font.
    #[must_use]
    pub fn monospace_only(mut self, only: bool) -> Self {
        self.monospace_only = only;
        self
    }

    /// The search text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The families shown: those matching the search text (and fixed-width
    /// ones only, when asked).
    #[must_use]
    pub fn shown(&self) -> Vec<&FontFamilyInfo> {
        self.matching().collect()
    }

    fn matching(&self) -> impl Iterator<Item = &FontFamilyInfo> {
        let query = enzui_core::text_fold::fold(self.query.trim());
        self.families
            .iter()
            .zip(self.folded.iter())
            .filter(|(family, _)| !self.monospace_only || family.monospace)
            .filter(move |(_, folded)| query.is_empty() || folded.contains(&query))
            .map(|(family, _)| family)
    }

    /// The family named `name`.
    #[must_use]
    pub fn family(&self, name: &str) -> Option<&FontFamilyInfo> {
        self.families.iter().find(|family| family.name == name)
    }

    /// Folds `event` into the state and `choice`. A new family keeps the
    /// chosen style when it has it, and otherwise takes its regular style
    /// (or its first).
    pub fn apply(&mut self, event: &FontChooserEvent, choice: &mut FontChoice) {
        match event {
            FontChooserEvent::Query(query) => {
                self.query.clone_from(query);
                self.scroll.set(iced::Vector::ZERO);
            }
            FontChooserEvent::Family(family) => {
                choice.family.clone_from(family);
                if let Some(info) = self.family(family)
                    && !info.styles.contains(&choice.style)
                {
                    choice.style = if info.styles.contains(&FontStyle::REGULAR) {
                        FontStyle::REGULAR
                    } else {
                        info.styles.first().copied().unwrap_or(FontStyle::REGULAR)
                    };
                }
            }
            FontChooserEvent::Style(style) => choice.style = *style,
            FontChooserEvent::Size(size) => choice.size = size.clamp(4.0, 400.0),
        }
    }
}

/// The shown families, for the virtual list.
struct Shown(Vec<&'static str>);

impl ItemProvider<&'static str> for Shown {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn item(&self, index: u64) -> Option<&'static str> {
        self.0.get(usize::try_from(index).ok()?).copied()
    }
}

impl KeyedProvider<&'static str> for Shown {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.item(index).map(SequenceKey::new)
    }
}

/// A font chooser; see the module docs.
pub struct EnzFontChooser<'a, Message> {
    state: &'a FontChooserState,
    choice: &'a FontChoice,
    on_event: Rc<dyn Fn(FontChooserEvent) -> Message + 'a>,
    preview: String,
}

/// A font chooser over `state` showing `choice`.
pub fn enz_font_chooser<'a, Message>(
    state: &'a FontChooserState,
    choice: &'a FontChoice,
    on_event: impl Fn(FontChooserEvent) -> Message + 'a,
) -> EnzFontChooser<'a, Message> {
    EnzFontChooser {
        state,
        choice,
        on_event: Rc::new(on_event),
        preview: enzui_core::localization::text("enzui-font-preview"),
    }
}

impl<'a, Message: Clone + 'a> EnzFontChooser<'a, Message> {
    /// The sample text of the preview (a pangram by default).
    #[must_use]
    pub fn preview_text(mut self, text: impl Into<String>) -> Self {
        self.preview = text.into();
        self
    }

    /// The chooser as an element.
    pub fn into_element(self) -> Element<'a, Message> {
        let on_event = Rc::clone(&self.on_event);
        let search = enz_text_field(
            "",
            enzui_core::localization::text("enzui-font-search"),
            self.state.query(),
            move |query| on_event(FontChooserEvent::Query(query)),
        )
        .leading(
            crate::icon::enz_icon("search")
                .size(16.0)
                .tint_theme_tone(crate::icon::IconTone::Muted)
                .into_element(),
        );

        let shown = Rc::new(Shown(self.state.matching().map(|f| f.name).collect()));
        let selected = self.choice.family.clone();
        let rows_event = Rc::clone(&self.on_event);
        let families =
            crate::virtual_list::enz_virtual_list(shown, ROW, move |_, name: &&'static str| {
                let name: &'static str = name;
                enz_list_row_surface(
                    text(name)
                        .font(Font::new(name))
                        .size(15.0)
                        .wrapping(text::Wrapping::None),
                    rows_event(FontChooserEvent::Family(name.to_owned())),
                )
                .selected(name == selected)
                .label(name)
                .row_height(ROW)
                .into()
            })
            .viewport_height(LIST_HEIGHT)
            .height(Length::Fixed(LIST_HEIGHT))
            .self_scrolling(self.state.scroll.clone())
            .into_element();

        let mut styles = Column::new().spacing(2.0);
        if let Some(family) = self.state.family(&self.choice.family) {
            for style in &family.styles {
                let font = FontChoice {
                    family: self.choice.family.clone(),
                    style: *style,
                    size: 14.0,
                }
                .font();
                styles = styles.push(
                    enz_list_row_surface(
                        text(style.name()).font(font).size(14.0),
                        (self.on_event)(FontChooserEvent::Style(*style)),
                    )
                    .selected(*style == self.choice.style)
                    .label(style.name())
                    .row_height(ROW),
                );
            }
        }
        let size_event = Rc::clone(&self.on_event);
        #[allow(
            clippy::cast_possible_truncation,
            reason = "sizes are small whole points"
        )]
        let size = crate::stepper::enz_stepper(self.choice.size.round() as i32, move |size| {
            #[allow(clippy::cast_precision_loss, reason = "sizes are small whole points")]
            size_event(FontChooserEvent::Size(size as f32))
        })
        .min(4)
        .max(400)
        .aria_label(enzui_core::localization::text("enzui-font-size"));

        let preview = container(
            text(self.preview.clone())
                .font(self.choice.font())
                .size(self.choice.size)
                .width(Length::Fill),
        )
        .padding(12)
        .height(Length::Fixed(110.0))
        .width(Length::Fill)
        .clip(true)
        .style(|theme: &iced::Theme| {
            let palette = crate::theme_lookup::palette(theme);
            container::Style {
                background: Some(palette.surface_alt.into()),
                border: iced::Border::default().rounded(8.0),
                ..container::Style::default()
            }
        });

        let label = |key: &str| -> Element<'a, Message> {
            enz_text(enzui_core::localization::text(key))
                .size(TextSize::Caption)
                .into_element()
        };
        column![
            row![
                column![search, families]
                    .spacing(8)
                    .width(Length::FillPortion(3)),
                column![
                    label("enzui-font-style"),
                    crate::scroll_view::contain_wheel(
                        enz_scrollable(styles).height(Length::Fixed(LIST_HEIGHT - 70.0)),
                        crate::scroll_view::Axis::Vertical,
                    ),
                    label("enzui-font-size"),
                    size,
                ]
                .spacing(8)
                .width(Length::FillPortion(2)),
            ]
            .spacing(16),
            preview,
        ]
        .spacing(12)
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzFontChooser<'a, Message>> for Element<'a, Message> {
    fn from(chooser: EnzFontChooser<'a, Message>) -> Self {
        chooser.into_element()
    }
}
