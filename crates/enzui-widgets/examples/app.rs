//! A complete, session-only notes app using the standard enzUI application shell.
//!
//! Run with `cargo run --locked --example app`. The shell owns the titlebar,
//! window controls and responsive sidebar; the app owns its notes and messages.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "a runnable application example, including its packaging command"
)]

use enzui_core::packaging::AppPackage;
use enzui_core::theme_file::integration::{ThemeEvent, ThemeIntegration};
use enzui_theme::EnzThemeMode;
use enzui_widgets::action_bar::ActionItem;
use enzui_widgets::app_shell::{
    DesktopApplicationIdentity, ShellLayout, enz_app_shell, enz_window_settings,
};
use enzui_widgets::button::enz_button;
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::radio_group::{Orientation, enz_radio_group};
use enzui_widgets::sidebar_content::enz_sidebar_content;
use enzui_widgets::sidebar_row::enz_sidebar_row;
use enzui_widgets::text::{TextSize, TextTone, enz_text};
use enzui_widgets::text_area::enz_text_area;
use enzui_widgets::text_field::enz_text_field;
use enzui_widgets::title_bar::enz_title_bar;
use enzui_widgets::title_bar_gesture::WindowCommand;
use iced::widget::text_editor::{Action, Content};
use iced::widget::{column, container};
use iced::{Element, Length, Subscription, Task};

const LAYOUT: ShellLayout = ShellLayout::TitlebarSidebarContent;
const APPEARANCES: [&str; 3] = ["System", "Light", "Dark"];

struct Note {
    title: String,
    body: Content,
}

impl Note {
    fn new(title: &str, body: &str) -> Self {
        Self {
            title: title.to_owned(),
            body: Content::with_text(body),
        }
    }

    fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            "Untitled"
        } else {
            &self.title
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    SelectNote(usize),
    NewNote,
    ActionsMenu,
    ActionsClosed,
    DeleteNote,
    Rename(String),
    Edit(Action),
    Appearance(usize),
    Sidebar(NavSplitState),
    Window(WindowCommand),
    Theme(Box<ThemeEvent>),
}

struct App {
    themes: ThemeIntegration,
    sidebar: NavSplitState,
    notes: Vec<Note>,
    selected: usize,
    appearance: usize,
    actions_open: bool,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let (themes, boot) = ThemeIntegration::boot();
        let app = Self {
            themes,
            sidebar: NavSplitState::new(),
            notes: vec![
                Note::new(
                    "Welcome",
                    "Write a note, add another, and switch between them in the sidebar.\n\nThese notes stay in memory for this session. Try the light and dark appearance controls, then choose System to resume the shared preferences.",
                ),
                Note::new(
                    "A small plan",
                    "Make something useful.\nKeep the interface clear.\nUse the framework's shared controls.",
                ),
            ],
            selected: 0,
            appearance: 0,
            actions_open: false,
        };
        (app, boot.map(|event| Message::Theme(Box::new(event))))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectNote(index) => {
                if index < self.notes.len() {
                    self.selected = index;
                    self.sidebar.dismiss_overlay();
                }
            }
            Message::ActionsMenu => self.actions_open = !self.actions_open,
            Message::ActionsClosed => self.actions_open = false,
            Message::NewNote => {
                self.actions_open = false;
                self.notes.push(Note::new("Untitled", ""));
                self.selected = self.notes.len() - 1;
            }
            Message::DeleteNote => {
                if self.notes.len() > 1 {
                    self.notes.remove(self.selected);
                    self.selected = self.selected.min(self.notes.len() - 1);
                }
            }
            Message::Rename(title) => self.notes[self.selected].title = title,
            Message::Edit(action) => {
                let _ = self.notes[self.selected].body.perform(action);
            }
            Message::Appearance(index) => {
                if index < APPEARANCES.len() {
                    self.appearance = index;
                    let mode = match index {
                        1 => Some(EnzThemeMode::Light),
                        2 => Some(EnzThemeMode::Dark),
                        _ => None,
                    };
                    let _ = self.themes.set_mode_override(mode);
                }
            }
            Message::Sidebar(state) => self.sidebar = state,
            Message::Theme(event) => {
                return self
                    .themes
                    .update(*event)
                    .map(|event| Message::Theme(Box::new(event)));
            }
            Message::Window(command) => {
                return iced::window::latest().then(move |window| match window {
                    Some(id) => command.into_task(id),
                    None => Task::none(),
                });
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        enz_app_shell(|| self.editor())
            .layout(LAYOUT)
            .breakpoints(self.themes.theme().breakpoints)
            .sidebar_titlebar(|| enz_title_bar().title("Notes").into_element())
            .sidebar(|| {
                let mut sidebar = enz_sidebar_content();
                for (index, note) in self.notes.iter().enumerate() {
                    sidebar = sidebar.push(
                        enz_sidebar_row(note.label(), Message::SelectNote(index))
                            .icon("file-text")
                            .selected(index == self.selected),
                    );
                }
                sidebar.into()
            })
            .titlebar(|| {
                enz_title_bar()
                    .title(self.notes[self.selected].label())
                    .actions(
                        vec![ActionItem::icon(
                            "note.new",
                            "square-pen",
                            "New note",
                            Message::NewNote,
                        )],
                        Message::ActionsMenu,
                    )
                    .overflow_open(self.actions_open)
                    .action_dismiss(Message::ActionsClosed)
                    .action_menu_id_prefix("note-actions")
                    .into_element()
            })
            .state(self.sidebar)
            .sidebar_label("Notes")
            .on_sidebar_state(Message::Sidebar)
            .on_window_gesture(Message::Window)
            .into()
    }

    fn editor(&self) -> Element<'_, Message> {
        let note = &self.notes[self.selected];
        container(
            column![
                enz_text_field("Title", "Untitled", &note.title, Message::Rename)
                    .id_prefix("note-title")
                    .into_element(),
                enz_text_area(&note.body, Message::Edit)
                    .aria_label("Note")
                    .placeholder("Start writing…")
                    .fill_height()
                    .into_element(),
                enz_button("Delete note", Message::DeleteNote)
                    .enabled(self.notes.len() > 1)
                    .into_element(),
                enz_radio_group(
                    "Appearance",
                    &APPEARANCES,
                    Some(self.appearance),
                    Message::Appearance
                )
                .orientation(Orientation::Horizontal)
                .into_element(),
                enz_text("Notes are kept for this session only.")
                    .size(TextSize::Footnote)
                    .tone(TextTone::Muted)
                    .into_element(),
            ]
            .spacing(16)
            .height(Length::Fill),
        )
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn theme(&self) -> iced::Theme {
        self.themes.iced_theme()
    }

    fn subscription(&self) -> Subscription<Message> {
        self.themes
            .subscription()
            .map(|event| Message::Theme(Box::new(event)))
    }
}

fn package() -> AppPackage {
    AppPackage::new(
        "org.enzui.AppExample",
        "Notes example",
        "A complete enzUI application",
        "app",
    )
    .expect("valid application identity")
    .description(["A session-only notes app demonstrating the enzUI application shell."])
    .developer("org.enzui", "The enzUI contributors")
    .categories(["Development"])
    .project_license("MIT")
    .icon_svg(include_str!("app.svg"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, root, prefix] = args.as_slice()
        && flag == "--write-package-files"
    {
        for path in package().write(root.as_ref(), prefix.as_ref())? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    let identity = DesktopApplicationIdentity::for_package(&package());
    enzui_widgets::app_setup::run(
        iced::application(App::new, App::update, App::view)
            .title("Notes — enzUI app example")
            .window(identity.apply_to_window_settings(iced::window::Settings {
                size: iced::Size::new(860.0, 600.0),
                ..enz_window_settings(LAYOUT)
            }))
            .theme(App::theme)
            .subscription(App::subscription),
    )?;
    Ok(())
}
