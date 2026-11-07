//! A tour of the widget library, one page per catalog group, in light and
//! dark.
//!
//! ```sh
//! cargo run -p enzui-widgets --example gallery --features emoji-picker
//! ```
//!
//! `ENZUI_GALLERY_PAGE=Input` opens a page, `ENZUI_GALLERY_LIGHT=1` the light
//! theme, and `ENZUI_GALLERY_ONLY=enz_slider,enz_toggle` shows only those
//! widgets.
//!
//! Every widget is shown by name with a live preview, controls for some of
//! its attributes, and the code that builds the preview as those controls
//! set it (see `specimen.rs`). The pages are in `pages/`, one module each.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "an example reports by panicking and printing, and none of it is public API"
)]

mod pages;
mod specimen;

use enzui_core::chart::{Series, SeriesId};
use enzui_core::confirm::ConfirmState;
use enzui_core::theme_file::integration::{ThemeEvent, ThemeIntegration};
use enzui_theme::EnzThemeMode;
use enzui_theme::{ExpressiveMotion, Spring};
use enzui_widgets::animated_svg::{AnimatedSvg, enz_animated_svg};
use enzui_widgets::app_shell::{ShellLayout, enz_app_shell, enz_window_settings};
use enzui_widgets::banner::BannerTone;
use enzui_widgets::bar_chart::BarSelection;
use enzui_widgets::button::{ButtonKind, ButtonSize};
use enzui_widgets::confirm_dialog::{ConfirmDialogEvent, enz_confirm_dialog};
use enzui_widgets::motion::{MotionEffect, SlideDirection, SpringAnimation, motion};
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::scroll_view::enz_scroll_view;
use enzui_widgets::skeleton::SkeletonKind;
use enzui_widgets::status_dot::StatusKind;
use enzui_widgets::text::{TextSize, enz_text};
use enzui_widgets::title_bar::enz_title_bar;
use enzui_widgets::title_bar_gesture::WindowCommand;
use enzui_widgets::view_switcher::SwitcherPage;
use enzui_widgets::{
    enz_avatar, enz_badge, enz_banner, enz_bar_chart, enz_button, enz_card, enz_checkbox,
    enz_combo_box, enz_gauge, enz_line_chart, enz_preferences_group, enz_preferences_row,
    enz_progress_bar, enz_radio_group, enz_search_field, enz_secure_field, enz_sidebar_content,
    enz_sidebar_row, enz_skeleton, enz_slider, enz_spinner, enz_status_dot, enz_stepper,
    enz_text_field, enz_toggle, enz_view_switcher,
};
use iced::time::Instant;
use iced::widget::{Space, container};
use iced::{Element, Length, Task};
use specimen::{Knob, Knobs, code, number, quoted, specimen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Buttons,
    Input,
    Selection,
    Navigation,
    Feedback,
    Pickers,
    Charts,
    Data,
    Content,
    Containers,
    Layouts,
    Materials,
    Motion,
    Images,
    Dialogs,
}

impl Page {
    const ALL: [Page; 15] = [
        Page::Buttons,
        Page::Input,
        Page::Selection,
        Page::Pickers,
        Page::Navigation,
        Page::Feedback,
        Page::Charts,
        Page::Data,
        Page::Content,
        Page::Containers,
        Page::Layouts,
        Page::Materials,
        Page::Motion,
        Page::Images,
        Page::Dialogs,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::Buttons => "Buttons and actions",
            Page::Input => "Text input and forms",
            Page::Selection => "Selection and pickers",
            Page::Navigation => "Navigation",
            Page::Feedback => "Feedback and progress",
            Page::Pickers => "Date, colour and emoji",
            Page::Charts => "Charts",
            Page::Data => "Tables and lists",
            Page::Content => "Text and content",
            Page::Containers => "Containers and surfaces",
            Page::Layouts => "Layouts",
            Page::Materials => "Materials and depth",
            Page::Motion => "Animation and motion",
            Page::Images => "Images and SVG",
            Page::Dialogs => "Overlays and dialogs",
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    Show(Page),
    Dark(bool),
    /// Put the shown page's controls and demo state back as they opened.
    Reset,
    /// The titlebar's overflow menu opened or closed.
    Actions(bool),
    Theme(Box<ThemeEvent>),
    Bar(BarSelection),
    Clicked(&'static str),
    Name(String),
    Password(String),
    Reveal,
    Search(String),
    Notify(bool),
    Agree(bool),
    Extra(usize, bool),
    AllExtras(bool),
    Volume(f32),
    Copies(i32),
    Size(usize),
    Fruit(String),
    FruitQuery(String),
    Price((f32, f32)),
    LineHeight(f64),
    Tags(enzui_widgets::chip_input::ChipInputEvent),
    Time(enzui_widgets::time_picker::TimePickerEvent),
    Notes(enzui_widgets::rich_edit::RichEditAction),
    Tab(usize),
    Progress(f32),
    OpenDialog,
    Dialog(ConfirmDialogEvent),
    Sidebar(NavSplitState),
    Window(WindowCommand),
    Align(usize),
    Chip(usize),
    Code(String),
    Stars(u8),
    Plan(u8),
    Crumb(usize),
    PageNo(i32),
    Section(usize),
    Details(bool),
    Advanced(bool),
    Faq(Option<usize>),
    Date(enzui_widgets::date_picker::DatePickerEvent),
    Color(enzui_widgets::color_picker::Hsva),
    Emoji(enzui_widgets::emoji_picker::EmojiPickerEvent),
    TableScroll(f32),
    TableSort(enzui_widgets::table::SortCommand),
    TableSelect(u64),
    TableResize(enzui_widgets::table::ColumnResize),
    Inbox(enzui_widgets::notification_inbox::InboxEvent),
    Wizard(enzui_widgets::wizard::WizardEvent),
    Menu(enzui_widgets::menu_button::MenuButtonEvent),
    Share,
    ShareClosed,
    Media(enzui_widgets::media_controls::MediaControlsMessage),
    TreeToggle(usize),
    TreeSelect(usize),
    Slide(enzui_core::paging::PageId),
    OpenAbout,
    About(enzui_widgets::about_dialog::AboutDialogEvent),
    Frame(Instant),
    SpringsGo,
    FlingPressed(iced::Point),
    FlingDragged(iced::Point),
    FlingReleased(iced::Point),
    RevealPanels,
    SvgPlaying(bool),
    ReduceMotion(bool),
    NextValue,
    /// A specimen control changed an attribute.
    Knob(&'static str, Knob),
    /// A specimen's code was copied.
    CopyCode(&'static str, String),
}

struct Gallery {
    page: Page,
    sidebar_state: NavSplitState,
    dark: bool,
    actions_open: bool,
    // The theme follows the desktop (accent, contrast, text size); the toggle
    // picks the mode.
    themes: ThemeIntegration,
    last_action: String,
    name: String,
    password: String,
    revealed: bool,
    search: String,
    notify: bool,
    agree: bool,
    extras: [bool; 2],
    volume: f32,
    copies: i32,
    size: Option<usize>,
    fruit: Option<String>,
    fruit_query: String,
    price: (f32, f32),
    line_height: f64,
    tags: Vec<String>,
    tag_draft: String,
    time: enzui_core::time_of_day::TimeOfDay,
    time_draft: String,
    time_open: bool,
    notes: enzui_widgets::rich_edit::RichEditor,
    spinner: enzui_widgets::animated_image::AnimatedFrames,
    tab: usize,
    progress: f32,
    dialog: Option<ConfirmState>,
    bar: Option<BarSelection>,
    align: usize,
    chips: [bool; 3],
    code: String,
    stars: u8,
    plan: Option<u8>,
    crumb: usize,
    page_no: i32,
    section: usize,
    details: bool,
    advanced: bool,
    faq: Option<usize>,
    date: Option<enzui_core::civil_date::Date>,
    date_state: enzui_widgets::date_picker::DatePickerState,
    color: enzui_widgets::color_picker::Hsva,
    emoji_query: String,
    emoji_category: enzui_widgets::emoji_picker::EmojiCategory,
    emoji_recents: enzui_widgets::emoji_picker::EmojiRecents,
    files: Files,
    table_viewport: enzui_widgets::table::TableViewportState,
    table_offset: f32,
    table_sort: Option<(
        enzui_core::table::ColumnKey,
        enzui_core::table::SortDirection,
    )>,
    table_selected: Option<u64>,
    table_widths: [f32; FILE_COLUMNS.len()],
    inbox: enzui_core::notification_inbox::Inbox,
    wizard: enzui_core::wizard::Wizard,
    menu: enzui_core::menu_model::MenuModel,
    menu_state: enzui_core::menu_model::MenuState,
    share_open: bool,
    playing: bool,
    position_s: f32,
    media_volume: f64,
    muted: bool,
    shuffle: bool,
    repeat: enzui_core::media::RepeatMode,
    tree_open: [bool; 2],
    tree_selected: Option<usize>,
    slide: enzui_core::paging::PageId,
    about: bool,
    /// The frame instant animations are drawn at.
    now: Instant,
    springs: Vec<(&'static str, SpringAnimation<f32>)>,
    springs_right: bool,
    fling: SpringAnimation<iced::Point>,
    fling_grab: Option<iced::Vector>,
    fling_tracker: enzui_core::velocity::VelocityTracker,
    panels_shown: bool,
    svgs: Vec<(&'static str, AnimatedSvg)>,
    svg_playing: bool,
    reduce_motion: bool,
    photo: iced::advanced::image::Handle,
    /// The specimen controls' values.
    knobs: Knobs,
    /// The page each control was last changed on, for resetting one page.
    knob_pages: std::collections::HashMap<&'static str, Page>,
    /// The specimen whose code was copied last.
    copied: Option<&'static str>,
}

/// The rows the gallery's tree shows: (label, depth, branch index when a
/// branch), given which branches are open.
fn tree_rows(open: [bool; 2]) -> Vec<(&'static str, u8, Option<usize>)> {
    let mut rows = vec![("Projects", 0, Some(0))];
    if open[0] {
        rows.extend([("enzUI", 1, None), ("Notes", 1, None), ("Mail", 1, None)]);
    }
    rows.push(("Pictures", 0, Some(1)));
    if open[1] {
        rows.extend([("Holidays", 1, None), ("Screenshots", 1, None)]);
    }
    rows
}

const FILE_ROWS: u64 = 500;
const FILE_ROW_HEIGHT: f32 = 32.0;
const SIZE_COLUMN: usize = 1;
const FILE_COLUMNS: [(&str, &str, f32); 3] = [
    ("name", "Name", 240.0),
    ("size", "Size", 110.0),
    ("owner", "Owner", 140.0),
];

/// The rows the table shows, in display order.
struct Files {
    rows: Vec<(String, u64, &'static str)>,
    order: Vec<usize>,
}

impl Files {
    fn generate() -> Self {
        const OWNERS: [&str; 4] = ["ada", "grace", "linus", "margaret"];
        let rows: Vec<_> = (0..FILE_ROWS)
            .map(|id| {
                (
                    format!("report-{:03}.odt", (id * 37) % FILE_ROWS),
                    (id * 104_729) % 9_000_000,
                    OWNERS[(id % 4) as usize],
                )
            })
            .collect();
        let order = (0..rows.len()).collect();
        Self { rows, order }
    }

    fn sort(&mut self, column: &str, direction: enzui_core::table::SortDirection) {
        let rows = &self.rows;
        match column {
            "size" => self.order.sort_by_key(|i| rows[*i].1),
            "owner" => self.order.sort_by(|a, b| rows[*a].2.cmp(rows[*b].2)),
            _ => self.order.sort_by(|a, b| rows[*a].0.cmp(&rows[*b].0)),
        }
        if direction == enzui_core::table::SortDirection::Descending {
            self.order.reverse();
        }
    }

    fn row(&self, index: u64) -> Option<&(String, u64, &'static str)> {
        self.order
            .get(usize::try_from(index).ok()?)
            .map(|i| &self.rows[*i])
    }
}

impl enzui_core::virtual_sequence::ItemProvider<enzui_core::virtual_sequence::SequenceKey>
    for Files
{
    fn len(&self) -> u64 {
        self.order.len() as u64
    }
    fn item(&self, index: u64) -> Option<enzui_core::virtual_sequence::SequenceKey> {
        enzui_core::virtual_sequence::KeyedProvider::key(self, index)
    }
}

impl enzui_core::virtual_sequence::KeyedProvider<enzui_core::virtual_sequence::SequenceKey>
    for Files
{
    fn key(&self, index: u64) -> Option<enzui_core::virtual_sequence::SequenceKey> {
        let position = *self.order.get(usize::try_from(index).ok()?)?;
        Some(enzui_core::virtual_sequence::SequenceKey::new(format!(
            "file-{position}"
        )))
    }
}

impl enzui_core::table::TableProvider<String> for Files {
    fn column_count(&self) -> usize {
        FILE_COLUMNS.len()
    }
    fn column_key(&self, column: usize) -> Option<enzui_core::table::ColumnKey> {
        FILE_COLUMNS
            .get(column)
            .map(|(key, _, _)| enzui_core::table::ColumnKey::new(*key))
    }
    fn cell(&self, index: u64, column: usize) -> Option<String> {
        let (name, size, owner) = self.row(index)?;
        Some(match column {
            0 => name.clone(),
            1 => enzui_widgets::file_browser::format_bytes(*size),
            2 => (*owner).to_owned(),
            _ => return None,
        })
    }
}

fn file_menu() -> enzui_core::menu_model::MenuModel {
    use enzui_core::menu_model::{MenuEntryModel, MenuModel};
    MenuModel::new(vec![
        MenuEntryModel::command("new", "New").shortcut("Ctrl+N"),
        MenuEntryModel::command("open", "Open…").shortcut("Ctrl+O"),
        MenuEntryModel::submenu("recent", "Open Recent"),
        MenuEntryModel::separator("rule"),
        MenuEntryModel::command("quit", "Quit").shortcut("Ctrl+Q"),
    ])
    .with_submenu(
        "recent",
        MenuModel::new(vec![
            MenuEntryModel::command("notes", "notes.txt"),
            MenuEntryModel::command("todo", "todo.md"),
        ]),
    )
}

fn inbox() -> enzui_core::notification_inbox::Inbox {
    use std::time::{Duration, SystemTime};
    let mut inbox = enzui_core::notification_inbox::Inbox::default();
    let now = SystemTime::now();
    inbox.push(
        "Backup finished",
        Some("1,204 files".into()),
        now - Duration::from_secs(3600),
    );
    inbox.push(
        "Update available",
        Some("Version 2.1 is ready".into()),
        now - Duration::from_secs(600),
    );
    let read = inbox.push("Saved “Report.odt”", None, now - Duration::from_secs(120));
    inbox.mark_read(read);
    inbox
}

const SIZES: [&str; 3] = ["Small", "Medium", "Large"];

/// Eight frames of a dot going round, encoded as a GIF the way a file
/// on disk would arrive.
fn spinner_gif() -> enzui_widgets::animated_image::AnimatedFrames {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .expect("repeat");
        for step in 0..8 {
            let angle = std::f32::consts::TAU * step as f32 / 8.0;
            let (cx, cy) = (24.0 + 14.0 * angle.cos(), 24.0 + 14.0 * angle.sin());
            let frame = image::RgbaImage::from_fn(48, 48, |x, y| {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                if dx * dx + dy * dy <= 36.0 {
                    image::Rgba([53, 132, 228, 255])
                } else {
                    image::Rgba([0, 0, 0, 0])
                }
            });
            encoder
                .encode_frame(image::Frame::from_parts(
                    frame,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(90, 1),
                ))
                .expect("frame");
        }
    }
    enzui_widgets::animated_image::AnimatedFrames::decode(&bytes).expect("spinner")
}

fn time_format() -> enzui_core::time_of_day::TimeFormat {
    enzui_core::time_of_day::TimeFormat::new(enzui_core::time_of_day::HourCycle::H23)
}

impl Gallery {
    fn new() -> (Self, Task<Message>) {
        let page = std::env::var("ENZUI_GALLERY_PAGE")
            .ok()
            .and_then(|name| {
                Page::ALL
                    .into_iter()
                    .find(|page| format!("{page:?}").eq_ignore_ascii_case(&name))
            })
            .unwrap_or(Page::Buttons);
        let dark = std::env::var_os("ENZUI_GALLERY_LIGHT").is_none();
        let (mut themes, boot) = ThemeIntegration::boot();
        let _ = themes.set_mode_override(Some(mode_of(dark)));
        (
            Self::fresh(page, dark, themes),
            boot.map(|event| Message::Theme(Box::new(event))),
        )
    }

    /// Every demo as it first opens, on `page` in `themes`.
    fn fresh(page: Page, dark: bool, themes: ThemeIntegration) -> Self {
        Self {
            page,
            sidebar_state: NavSplitState::new(),
            dark,
            actions_open: false,
            themes,
            last_action: String::from("Nothing clicked yet"),
            name: String::new(),
            password: String::from("hunter2"),
            revealed: false,
            search: String::new(),
            notify: true,
            agree: false,
            extras: [true, false],
            volume: 0.6,
            copies: 1,
            size: Some(1),
            fruit: None,
            fruit_query: String::new(),
            price: (20.0, 60.0),
            line_height: 1.5,
            tags: vec!["rust".to_owned(), "linux".to_owned()],
            tag_draft: String::new(),
            time: enzui_core::time_of_day::TimeOfDay::hm(9, 30).expect("time"),
            time_draft: String::from("09:30"),
            time_open: false,
            notes: enzui_widgets::rich_edit::RichEditor::from_html(
                "<strong>Agenda</strong> for <em>Friday</em>",
            )
            .expect("notes"),
            spinner: spinner_gif(),
            tab: 0,
            progress: 0.42,
            dialog: None,
            bar: None,
            align: 0,
            chips: [true, false, false],
            code: String::from("42"),
            stars: 4,
            plan: Some(1),
            crumb: 2,
            page_no: 3,
            section: 0,
            details: true,
            advanced: false,
            faq: Some(0),
            date: enzui_core::civil_date::Date::new(2026, 10, 23),
            date_state: enzui_widgets::date_picker::DatePickerState::new(
                enzui_core::civil_date::Date::new(2026, 10, 23).expect("date"),
                enzui_core::civil_date::Weekday::Monday,
                enzui_core::civil_date::DateBounds::new(None, None).expect("bounds"),
            ),
            color: enzui_widgets::color_picker::Hsva::new(212.0, 0.77, 0.89),
            emoji_query: String::new(),
            emoji_category: enzui_widgets::emoji_picker::EmojiCategory::default(),
            emoji_recents: enzui_widgets::emoji_picker::EmojiRecents::new(),
            files: Files::generate(),
            table_viewport: enzui_widgets::table::TableViewportState::default(),
            table_offset: 0.0,
            table_sort: None,
            table_selected: None,
            table_widths: FILE_COLUMNS.map(|(_, _, width)| width),
            inbox: inbox(),
            wizard: enzui_core::wizard::Wizard::new(3),
            menu: file_menu(),
            menu_state: enzui_core::menu_model::MenuState::closed(),
            share_open: false,
            playing: true,
            position_s: 74.0,
            media_volume: 0.6,
            muted: false,
            shuffle: false,
            repeat: enzui_core::media::RepeatMode::Off,
            tree_open: [true, false],
            tree_selected: Some(1),
            slide: enzui_core::paging::PageId::from("dawn"),
            about: false,
            now: Instant::now(),
            springs: SPRING_PRESETS
                .iter()
                .map(|(name, spring)| (*name, SpringAnimation::new(0.0, *spring).precision(1e-4)))
                .collect(),
            springs_right: false,
            fling: SpringAnimation::new(
                fling_anchor(false),
                ExpressiveMotion::DefaultSpatial.spring(),
            )
            .precision(0.05),
            fling_grab: None,
            fling_tracker: enzui_core::velocity::VelocityTracker::new(),
            panels_shown: true,
            svgs: ANIMATED_SVGS
                .iter()
                .map(|(name, text)| (*name, AnimatedSvg::from_text(text).expect("built-in SVG")))
                .collect(),
            svg_playing: true,
            reduce_motion: false,
            photo: landscape(),
            knobs: Knobs::default(),
            knob_pages: std::collections::HashMap::new(),
            copied: None,
        }
    }

    /// Put the shown page back as it opened: its controls go to their
    /// defaults and the demo state goes back to its first value. The theme,
    /// the sidebar and the other pages' controls stay as they are.
    fn reset_page(&mut self) {
        let placeholder =
            ThemeIntegration::unmanaged_compiled_theme(enzui_theme::EnzTheme::default()).0;
        let themes = std::mem::replace(&mut self.themes, placeholder);
        let mut fresh = Self::fresh(self.page, self.dark, themes);
        fresh.sidebar_state = self.sidebar_state;
        let page = self.page;
        self.knob_pages.retain(|key, on| {
            let keep = *on != page;
            if !keep {
                self.knobs.forget(key);
            }
            keep
        });
        fresh.knobs = std::mem::take(&mut self.knobs);
        fresh.knob_pages = std::mem::take(&mut self.knob_pages);
        *self = fresh;
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Show(page) => {
                self.page = page;
                self.copied = None;
            }
            Message::Knob(key, value) => {
                self.knobs.set(key, value);
                let _ = self.knob_pages.insert(key, self.page);
                self.copied = None;
            }
            Message::CopyCode(name, code) => {
                self.copied = Some(name);
                return iced::clipboard::write(code).discard();
            }
            Message::Sidebar(state) => self.sidebar_state = state,
            Message::Window(command) => {
                return iced::window::latest().then(move |id| match id {
                    Some(id) => command.into_task(id),
                    None => Task::none(),
                });
            }
            Message::Bar(selection) => self.bar = Some(selection),
            Message::Align(index) => self.align = index,
            Message::Chip(index) => self.chips[index] = !self.chips[index],
            Message::Code(code) => self.code = code,
            Message::Stars(stars) => self.stars = stars,
            Message::Plan(plan) => self.plan = Some(plan),
            Message::Crumb(index) => self.crumb = index,
            Message::PageNo(page) => self.page_no = page,
            Message::Section(index) => self.section = index,
            Message::Details(open) => self.details = open,
            Message::Advanced(open) => self.advanced = open,
            Message::Faq(open) => self.faq = open,
            Message::Date(event) => {
                use enzui_widgets::date_picker::DatePickerEvent;
                match event {
                    DatePickerEvent::TriggerPressed => {
                        let _ = self.date_state.toggle(self.date, None);
                    }
                    DatePickerEvent::Calendar(event) => {
                        let (picked, _) = self.date_state.apply_calendar_event_and_close(event);
                        if picked.is_some() {
                            self.date = picked;
                        }
                    }
                    DatePickerEvent::Dismissed => {
                        let _ = self.date_state.close();
                    }
                }
            }
            Message::Color(color) => self.color = color,
            Message::Emoji(event) => {
                use enzui_widgets::emoji_picker::EmojiPickerEvent;
                match event {
                    EmojiPickerEvent::Query(query) => self.emoji_query = query,
                    EmojiPickerEvent::Category(category) => self.emoji_category = category,
                    EmojiPickerEvent::Picked(emoji) => {
                        self.emoji_recents.record(&emoji);
                        self.last_action = format!("Picked {emoji}");
                    }
                }
            }
            Message::TableScroll(dy) => {
                let body = self
                    .table_viewport
                    .resolved()
                    .map_or(0.0, |viewport| viewport.body_height);
                let max = (FILE_ROWS as f32 * FILE_ROW_HEIGHT - body).max(0.0);
                self.table_offset = (self.table_offset - dy).clamp(0.0, max);
            }
            Message::TableSort(command) => {
                self.files.sort(command.column.as_str(), command.direction);
                self.table_sort = Some((command.column, command.direction));
                self.table_selected = None;
            }
            Message::TableSelect(row) => self.table_selected = Some(row),
            Message::TableResize(resize) => self.table_widths[resize.column] = resize.width,
            Message::Inbox(event) => {
                enzui_widgets::notification_inbox::apply(&mut self.inbox, event);
            }
            Message::Wizard(event) => {
                if !enzui_widgets::wizard::apply(&mut self.wizard, event)
                    && event == enzui_widgets::wizard::WizardEvent::Finish
                {
                    self.wizard = enzui_core::wizard::Wizard::new(3);
                }
            }
            Message::Menu(event) => {
                if let Some(chosen) = enzui_widgets::menu_button::apply_event(
                    &mut self.menu,
                    &mut self.menu_state,
                    &event,
                ) {
                    self.last_action = format!("Menu: {}", chosen.as_str());
                }
            }
            Message::Share => self.share_open = !self.share_open,
            Message::ShareClosed => self.share_open = false,
            Message::Media(event) => {
                use enzui_widgets::media_controls::MediaControlsMessage as M;
                match event {
                    M::PlayPause => self.playing = !self.playing,
                    M::Seek(at) => self.position_s = at.as_secs_f32(),
                    M::SetVolume(volume) => {
                        self.media_volume = volume;
                        self.muted = false;
                    }
                    M::ToggleMute => self.muted = !self.muted,
                    M::ToggleShuffle => self.shuffle = !self.shuffle,
                    M::CycleRepeat => self.repeat = self.repeat.cycle(),
                    M::PreviousTrack | M::NextTrack => self.position_s = 0.0,
                }
            }
            Message::TreeToggle(row) => {
                if let Some((_, _, Some(branch))) = tree_rows(self.tree_open).get(row) {
                    self.tree_open[*branch] = !self.tree_open[*branch];
                }
            }
            Message::TreeSelect(row) => self.tree_selected = Some(row),
            Message::Slide(page) => self.slide = page,
            Message::OpenAbout => self.about = true,
            Message::Frame(at) => self.now = at,
            Message::SpringsGo => {
                let now = Instant::now();
                self.now = now;
                self.springs_right = !self.springs_right;
                let target = if self.springs_right { 1.0 } else { 0.0 };
                for (_, spring) in &mut self.springs {
                    spring.set_target(target, now);
                }
            }
            Message::FlingPressed(at) => {
                let now = Instant::now();
                self.now = now;
                let card = self.fling.value(now);
                let hit = iced::Rectangle::new(card, iced::Size::new(FLING_CARD, FLING_CARD));
                if hit.contains(at) {
                    self.fling_grab = Some(card - at);
                    self.fling.snap(card);
                    self.fling_tracker.reset();
                    self.fling_tracker.add(now, [at.x, at.y]);
                }
            }
            Message::FlingDragged(at) => {
                if let Some(grab) = self.fling_grab {
                    let now = Instant::now();
                    self.now = now;
                    self.fling.snap(at + grab);
                    self.fling_tracker.add(now, [at.x, at.y]);
                }
            }
            Message::FlingReleased(at) => {
                if let Some(grab) = self.fling_grab.take() {
                    let now = Instant::now();
                    self.now = now;
                    self.fling_tracker.add(now, [at.x, at.y]);
                    let [vx, vy] = self.fling_tracker.velocity(now);
                    let from = at + grab;
                    // Where the throw would carry it picks the side.
                    let right = from.x + FLING_CARD / 2.0 + vx * 0.2 > FLING_WIDTH / 2.0;
                    self.fling
                        .release(from, iced::Point::new(vx, vy), fling_anchor(right), now);
                }
            }
            Message::RevealPanels => self.panels_shown = !self.panels_shown,
            Message::ReduceMotion(reduce) => {
                self.reduce_motion = reduce;
                let _ = self.themes.set_reduced_motion_override(Some(reduce));
            }
            Message::NextValue => {
                self.progress = match self.progress {
                    p if p < 0.3 => 0.85,
                    p if p < 0.7 => 0.2,
                    _ => 0.5,
                };
            }
            Message::SvgPlaying(playing) => self.svg_playing = playing,
            Message::About(event) => {
                if let enzui_widgets::about_dialog::AboutDialogEvent::Dismissed(_) = event {
                    self.about = false;
                }
            }
            Message::Reset => self.reset_page(),
            Message::Actions(open) => self.actions_open = open,
            Message::Dark(dark) => {
                self.dark = dark;
                let _ = self.themes.set_mode_override(Some(mode_of(dark)));
            }
            Message::Theme(event) => {
                return self
                    .themes
                    .update(*event)
                    .map(|event| Message::Theme(Box::new(event)));
            }
            Message::Clicked(what) => self.last_action = format!("Clicked: {what}"),
            Message::Name(name) => self.name = name,
            Message::Password(password) => self.password = password,
            Message::Reveal => self.revealed = !self.revealed,
            Message::Search(search) => self.search = search,
            Message::Notify(on) => self.notify = on,
            Message::Agree(on) => self.agree = on,
            Message::Extra(index, on) => self.extras[index] = on,
            Message::AllExtras(on) => self.extras = [on; 2],
            Message::Volume(volume) => self.volume = volume,
            Message::Copies(copies) => self.copies = copies.clamp(1, 99),
            Message::Size(size) => self.size = Some(size),
            Message::Fruit(fruit) => self.fruit = Some(fruit),
            Message::FruitQuery(query) => self.fruit_query = query,
            Message::Price(price) => self.price = price,
            Message::LineHeight(height) => self.line_height = height,
            Message::Tags(event) => enzui_widgets::chip_input::apply_chip_event(
                &mut self.tags,
                &mut self.tag_draft,
                &event,
                enzui_widgets::chip_input::DEFAULT_SEPARATORS,
            ),
            Message::Notes(action) => {
                let _ = self.notes.update(action);
            }
            Message::Time(event) => {
                use enzui_widgets::time_field::TimeFieldEvent;
                use enzui_widgets::time_picker::TimePickerEvent;
                match event {
                    TimePickerEvent::Toggle => self.time_open = !self.time_open,
                    TimePickerEvent::Dismiss => self.time_open = false,
                    TimePickerEvent::Chosen(time)
                    | TimePickerEvent::Field(TimeFieldEvent::Committed(time)) => {
                        self.time = time;
                        self.time_draft = time_format().format(time);
                        self.time_open = false;
                    }
                    TimePickerEvent::Field(TimeFieldEvent::Drafted(text)) => self.time_draft = text,
                    TimePickerEvent::Field(TimeFieldEvent::Refused(_)) => {}
                }
            }
            Message::Tab(tab) => self.tab = tab,
            Message::Progress(progress) => self.progress = progress,
            Message::OpenDialog => self.dialog = Some(ConfirmState::new(None)),
            Message::Dialog(event) => {
                self.last_action = format!("Dialog: {event:?}");
                self.dialog = None;
            }
        }
        Task::none()
    }

    fn media_snapshot(&self) -> enzui_core::media::Snapshot {
        enzui_core::media::Snapshot {
            generation: enzui_core::media::Generation::FIRST,
            source: None,
            capabilities: enzui_core::media::Capabilities {
                seekable: true,
                has_audio: true,
                has_video: false,
            },
            playback: if self.playing {
                enzui_core::media::PlaybackState::Playing
            } else {
                enzui_core::media::PlaybackState::Paused
            },
            position: std::time::Duration::from_secs_f32(self.position_s),
            duration: Some(std::time::Duration::from_secs(215)),
            volume: self.media_volume,
            muted: self.muted,
            rate: 1.0,
            error: None,
        }
    }

    fn sidebar(&self) -> Element<'_, Message> {
        Page::ALL
            .into_iter()
            .fold(enz_sidebar_content(), |content, page| {
                content.push(
                    enz_sidebar_row(page.title(), Message::Show(page)).selected(page == self.page),
                )
            })
            .height(Length::Fill)
            .into()
    }

    fn page(&self) -> Element<'_, Message> {
        match self.page {
            Page::Buttons => self.buttons_page(),
            Page::Input => self.input_page(),
            Page::Selection => self.selection_page(),
            Page::Navigation => self.navigation_page(),
            Page::Feedback => self.feedback_page(),
            Page::Pickers => self.pickers_page(),
            Page::Data => self.data_page(),
            Page::Charts => self.charts_page(),
            Page::Content => self.content_page(),
            Page::Containers => self.containers_page(),
            Page::Layouts => self.layouts_page(),
            Page::Materials => self.materials_page(),
            Page::Motion => self.motion_page(),
            Page::Images => self.images_page(),
            Page::Dialogs => self.dialogs_page(),
        }
    }

    fn content(&self) -> Element<'_, Message> {
        // The scroll view spans the pane and insets the page inside it, so
        // its overlay bar sits at the window edge; the page reads in one
        // column of a comfortable width.
        enz_scroll_view(container(self.page()).width(Length::Fill.max(760.0)))
            .insets(24.0, 24.0, 24.0, 24.0)
            .into()
    }

    fn view(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = enz_app_shell(|| self.content())
            .layout(LAYOUT)
            .titlebar(|| self.titlebar())
            .sidebar(|| self.sidebar())
            .state(self.sidebar_state)
            .sidebar_label("Pages")
            .on_sidebar_state(Message::Sidebar)
            .on_window_gesture(Message::Window)
            .into();
        let body = self.about_host(body);
        // Hosted on every frame, open or not, so the page keeps its place
        // in the tree (and its scroll offset) as the dialog comes and goes.
        let dialog = self.dialog.as_ref().map(|state| {
            enz_confirm_dialog(
                "Delete “report.pdf”?",
                "The file is moved to the trash.",
                state,
                Message::Dialog,
            )
            .confirm_label(self.knobs.text("confirm.label", "Delete"))
            .destructive(self.knobs.flag("confirm.destructive", true))
        });
        enzui_widgets::confirm_dialog_host(body, dialog, Message::Dialog)
    }

    /// The page's name, then the theme switch and the page reset beside the
    /// window controls.
    fn titlebar(&self) -> Element<'_, Message> {
        use enzui_widgets::action_bar::ActionItem;
        let theme = if self.dark {
            ActionItem::icon("gallery.theme", "sun", "Light theme", Message::Dark(false))
        } else {
            ActionItem::icon("gallery.theme", "moon", "Dark theme", Message::Dark(true))
        };
        enz_title_bar()
            .title(self.page.title())
            .actions(
                vec![
                    theme,
                    ActionItem::icon("gallery.reset", "rotate-ccw", "Reset page", Message::Reset),
                ],
                Message::Actions(!self.actions_open),
            )
            .overflow_open(self.actions_open)
            .action_dismiss(Message::Actions(false))
            .action_menu_id_prefix("gallery-actions")
            .into_element()
    }

    fn about_host<'a>(&'a self, body: Element<'a, Message>) -> Element<'a, Message> {
        enzui_widgets::about_dialog::enz_about_dialog(self.about, "enzUI gallery")
            .icon("layout-grid")
            .version("0.1.0")
            .developer("The enzUI developers")
            .website("https://example.org/enzui")
            .release_notes("Pill buttons, a [shape] theme section, livelier controls.")
            .developers(["Ada Lovelace", "Alan Turing"])
            .designers(["Susan Kare"])
            .copyright("© 2026 The enzUI developers")
            .license("MIT OR Apache-2.0")
            .into_host_with(body, Message::About)
    }

    fn theme(&self) -> iced::Theme {
        self.themes.theme().iced_theme()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        let themes = self
            .themes
            .subscription()
            .map(|event| Message::Theme(Box::new(event)));
        // Frames only while the flung card is moving (the spring dots animate
        // themselves), judged at the last frame drawn so the landing is drawn.
        let now = self.now;
        let moving = self.page == Page::Motion && self.fling.is_animating(now);
        if moving {
            iced::Subscription::batch([themes, iced::window::frames().map(Message::Frame)])
        } else {
            themes
        }
    }
}

const LAYOUT: ShellLayout = ShellLayout::TitlebarSidebarContent;

fn mode_of(dark: bool) -> EnzThemeMode {
    if dark {
        EnzThemeMode::Dark
    } else {
        EnzThemeMode::Light
    }
}

fn main() -> iced::Result {
    enzui_widgets::app_setup::run(
        iced::application(Gallery::new, Gallery::update, Gallery::view)
            .title("enzUI gallery")
            .window(iced::window::Settings {
                size: iced::Size::new(1100.0, 760.0),
                ..enz_window_settings(LAYOUT)
            })
            .theme(Gallery::theme)
            .subscription(Gallery::subscription),
    )
}

const SPRING_PRESETS: [(&str, Spring); 6] = [
    ("Expressive fast", ExpressiveMotion::FastSpatial.spring()),
    ("Expressive", ExpressiveMotion::DefaultSpatial.spring()),
    ("Expressive slow", ExpressiveMotion::SlowSpatial.spring()),
    (
        "Standard",
        ExpressiveMotion::DefaultSpatial.standard_spring(),
    ),
    ("Smooth", Spring::SMOOTH),
    ("Bouncy", Spring::BOUNCY),
];

const FLING_CARD: f32 = 72.0;
const FLING_WIDTH: f32 = 640.0;
const FLING_HEIGHT: f32 = 180.0;

/// The fling card's resting place on the left or right of its stage.
fn fling_anchor(right: bool) -> iced::Point {
    let x = if right {
        FLING_WIDTH - FLING_CARD - 20.0
    } else {
        20.0
    };
    iced::Point::new(x, (FLING_HEIGHT - FLING_CARD) / 2.0)
}

struct FlingStage {
    card: iced::Point,
}

impl iced::widget::canvas::Program<Message> for FlingStage {
    type State = bool;

    fn update(
        &self,
        dragging: &mut bool,
        event: &iced::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<iced::widget::canvas::Action<Message>> {
        use iced::mouse;
        let at = cursor.position_in(bounds).or_else(|| {
            dragging
                .then(|| cursor.position())
                .flatten()
                .map(|p| iced::Point::new(p.x - bounds.x, p.y - bounds.y))
        })?;
        let message = match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                *dragging = true;
                Message::FlingPressed(at)
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) if *dragging => {
                Message::FlingDragged(at)
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if *dragging => {
                *dragging = false;
                Message::FlingReleased(at)
            }
            _ => return None,
        };
        Some(iced::widget::canvas::Action::publish(message).and_capture())
    }

    fn draw(
        &self,
        _dragging: &bool,
        renderer: &iced::Renderer,
        theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path, Stroke};
        let palette = enzui_widgets::theme_lookup::palette(theme);
        let mut frame = Frame::new(renderer, bounds.size());
        let size = iced::Size::new(FLING_CARD, FLING_CARD);
        for right in [false, true] {
            frame.stroke(
                &Path::rounded_rectangle(fling_anchor(right), size, 18.0.into()),
                Stroke::default().with_color(palette.border).with_width(2.0),
            );
        }
        frame.fill(
            &Path::rounded_rectangle(self.card, size, 18.0.into()),
            palette.accent,
        );
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        dragging: &bool,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        let over = cursor.position_in(bounds).is_some_and(|p| {
            iced::Rectangle::new(self.card, iced::Size::new(FLING_CARD, FLING_CARD)).contains(p)
        });
        if *dragging {
            iced::mouse::Interaction::Grabbing
        } else if over {
            iced::mouse::Interaction::Grab
        } else {
            iced::mouse::Interaction::default()
        }
    }
}

/// A generated landscape photo: sky, sun and two hills, 480 × 300.
fn landscape() -> iced::advanced::image::Handle {
    let (width, height) = (480_u32, 300_u32);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
            let sky = [0.98 - 0.45 * fy, 0.62 + 0.1 * fy, 0.45 + 0.45 * (1.0 - fy)];
            let sun = ((fx - 0.7).powi(2) + (fy - 0.35).powi(2)).sqrt();
            let mut colour = if sun < 0.09 {
                [1.0, 0.92, 0.6]
            } else {
                let glow = (0.25 - sun).max(0.0) * 1.6;
                [sky[0] + glow, sky[1] + glow * 0.8, sky[2] + glow * 0.3]
            };
            let far = 0.62 + 0.08 * (fx * 9.0).sin() + 0.04 * (fx * 23.0).cos();
            let near = 0.78 + 0.06 * (fx * 5.0 + 1.0).sin();
            if fy > near {
                colour = [0.16, 0.42 - 0.1 * fy, 0.25];
            } else if fy > far {
                colour = [0.3, 0.38, 0.55];
            }
            pixels.extend(colour.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8));
            pixels.push(255);
        }
    }
    iced::advanced::image::Handle::from_rgba(width, height, pixels)
}

const SYMBOLIC_SVG: &str = r#"<svg width="16" height="16" viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M8 1.5l1.9 4 4.4.5-3.3 3 .9 4.3L8 11.1l-3.9 2.2.9-4.3-3.3-3 4.4-.5z"/></svg>"#;

const TRANSLUCENT_SVG: &str = r##"<svg width="96" height="96" viewBox="0 0 96 96" xmlns="http://www.w3.org/2000/svg"><circle cx="36" cy="38" r="26" fill="#ff3b30" fill-opacity="0.6"/><circle cx="60" cy="38" r="26" fill="#34c759" fill-opacity="0.6"/><circle cx="48" cy="60" r="26" fill="#0a84ff" fill-opacity="0.6"/></svg>"##;

const ANIMATED_SVGS: [(&str, &str); 5] = [
    (
        "SMIL ring",
        r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M12,1A11,11,0,1,0,23,12,11,11,0,0,0,12,1Zm0,19a8,8,0,1,1,8-8A8,8,0,0,1,12,20Z" opacity=".25" fill="#3d6ef5"/><path fill="#3d6ef5" d="M10.14,1.16a11,11,0,0,0-9,8.92A1.59,1.59,0,0,0,2.46,12,1.52,1.52,0,0,0,4.11,10.7a8,8,0,0,1,6.66-6.61A1.42,1.42,0,0,0,12,2.69h0A1.57,1.57,0,0,0,10.14,1.16Z"><animateTransform attributeName="transform" type="rotate" dur="0.75s" values="0 12 12;360 12 12" repeatCount="indefinite"/></path></svg>"##,
    ),
    (
        "CSS keyframes",
        r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><style>circle{fill:#e8547a;transform-box:fill-box;transform-origin:center;animation:pulse 1.2s cubic-bezier(.52,.6,.25,.99) infinite}.b{animation-delay:.2s}.c{animation-delay:.4s}@keyframes pulse{0%,100%{transform:scale(.3);opacity:.4}40%{transform:scale(1);opacity:1}}</style><circle cx="4" cy="12" r="3"/><circle class="b" cx="12" cy="12" r="3"/><circle class="c" cx="20" cy="12" r="3"/></svg>"##,
    ),
    (
        "Syncbase chain",
        r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" fill="#2fa86b"><rect x="1" y="1" width="6" height="22"><animate id="a" begin="0;c.end-0.1s" attributeName="y" calcMode="spline" dur="0.6s" values="1;5;1" keySplines=".36,.61,.3,.98;.36,.61,.3,.98"/><animate begin="0;c.end-0.1s" attributeName="height" calcMode="spline" dur="0.6s" values="22;14;22" keySplines=".36,.61,.3,.98;.36,.61,.3,.98"/></rect><rect x="9" y="1" width="6" height="22"><animate begin="a.begin+0.1s" attributeName="y" calcMode="spline" dur="0.6s" values="1;5;1" keySplines=".36,.61,.3,.98;.36,.61,.3,.98"/><animate begin="a.begin+0.1s" attributeName="height" calcMode="spline" dur="0.6s" values="22;14;22" keySplines=".36,.61,.3,.98;.36,.61,.3,.98"/></rect><rect x="17" y="1" width="6" height="22"><animate id="c" begin="a.begin+0.2s" attributeName="y" calcMode="spline" dur="0.6s" values="1;5;1" keySplines=".36,.61,.3,.98;.36,.61,.3,.98"/><animate begin="a.begin+0.2s" attributeName="height" calcMode="spline" dur="0.6s" values="22;14;22" keySplines=".36,.61,.3,.98;.36,.61,.3,.98"/></rect></svg>"##,
    ),
    (
        "Motion path",
        r##"<svg width="48" height="48" viewBox="0 0 48 48" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><path id="loop" d="M24 6 C38 6 42 20 42 24 C42 38 28 42 24 42 C10 42 6 28 6 24 C6 10 20 6 24 6 Z" fill="none" stroke="#8888" stroke-width="1.5"/><path d="M-4 -3 L5 0 L-4 3 Z" fill="#f2a33a"><animateMotion dur="3s" repeatCount="indefinite" rotate="auto"><mpath xlink:href="#loop"/></animateMotion></path></svg>"##,
    ),
    (
        "Click me",
        r##"<svg width="48" height="48" viewBox="0 0 48 48" xmlns="http://www.w3.org/2000/svg"><rect x="8" y="8" width="32" height="32" rx="6" fill="#7a5af8"><animateTransform attributeName="transform" type="rotate" from="0 24 24" to="90 24 24" dur="0.4s" begin="click" calcMode="spline" keyTimes="0;1" keySplines=".3 1.4 .5 1" fill="freeze"/><animate attributeName="fill" values="#7a5af8;#f25f5c;#7a5af8" dur="0.4s" begin="click"/></rect></svg>"##,
    ),
];
