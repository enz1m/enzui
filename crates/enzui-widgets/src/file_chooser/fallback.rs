//! An in-app file chooser for hosts without a file-chooser portal.
//!
//! On a desktop with no xdg-desktop-portal, or with a backend that does not
//! implement FileChooser (xdg-desktop-portal-wlr), every portal request
//! reports [`PortalOutcome::Unavailable`] or [`PortalOutcome::Unsupported`].
//! [`FallbackChooser`] answers the same request inside the application: it
//! lists folders on a worker thread, applies the request's filters, and ends
//! with the same [`FileResult`] a portal would return. Most apps reach it
//! through [`ChooserSession::with_fallback`](super::ChooserSession::with_fallback)
//! rather than directly.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use enzui_core::file_browser::{FileBrowserModel, FileEntry, SortKey};
use enzui_core::localization::text;
use enzui_core::portal::open_uri::OpenUri;
use enzui_icons::mime::GlobTable;
use iced::widget::{Space, row};
use iced::{Alignment, Element, Length};

use super::{
    ChooserOptions, CompletionSignal, FileFilter, FileResult, FileUri, FilterRule, PortalOperation,
    PortalOutcome,
};
use crate::button::{ButtonIconPosition, ButtonKind, ButtonSize, enz_button, enz_filter_chip};
use crate::file_browser::enz_file_browser;
use crate::modal_host::{ModalHostEvent, enz_modal_host};

/// Namespace of the chooser's own routable control ids.
const CHOOSER_ID_PREFIX: &str = "enzui-fallback-chooser";

/// The most entries one folder listing keeps; the rest are dropped so a huge
/// directory cannot stall the dialog.
pub const MAX_LISTED_ENTRIES: usize = 20_000;

/// What the dialog asks the host to do with one input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackEvent {
    /// A row was selected.
    Select(PathBuf),
    /// A row was activated (double click or Enter).
    Activate(PathBuf),
    /// Go to the parent folder.
    Up,
    /// The save name changed.
    NameChanged(String),
    /// Another filter was chosen.
    FilterChanged(usize),
    /// Show or hide dot files.
    ToggleHidden,
    /// Sort by another column.
    Sort(SortKey),
    /// The accept button (Open, Save, Select).
    Accept,
    /// Cancel, Escape or a click outside the dialog.
    Cancel,
    /// Tab inside the dialog; run [`focus_task`] for it.
    FocusNext,
    /// Shift+Tab inside the dialog; run [`focus_task`] for it.
    FocusPrevious,
    /// Ctrl+L or a click on the location: edit it as text; run
    /// [`focus_task`] for it.
    EditLocation,
    /// The location being typed changed.
    LocationChanged(String),
    /// Enter in the location: open that folder, or the folder of that file
    /// with the file selected.
    GoToLocation,
}

/// The focus move a [`FallbackEvent::FocusNext`] or
/// [`FallbackEvent::FocusPrevious`] asks for, wrapping inside the dialog.
/// [`FallbackEvent::EditLocation`] focuses and selects the location, including
/// when its editor already has focus.
pub fn focus_task<T: Send + 'static>(event: &FallbackEvent) -> iced::Task<T> {
    let host_event = match event {
        FallbackEvent::FocusNext => ModalHostEvent::FocusNext,
        FallbackEvent::FocusPrevious => ModalHostEvent::FocusPrevious,
        FallbackEvent::EditLocation => {
            return iced::Task::batch([
                iced::widget::operation::focus(location_id()),
                iced::advanced::widget::operate(crate::text_field::select_all(location_id())),
            ]);
        }
        _ => return iced::Task::none(),
    };
    match crate::modal_host::focus_wrap_operation::<T>(host_event) {
        Some(operation) => iced::advanced::widget::operate(operation),
        None => iced::Task::none(),
    }
}

/// The location field's id.
fn location_id() -> iced::widget::Id {
    enzui_core::identity::derive_id(CHOOSER_ID_PREFIX, "location")
}

/// One folder listing produced on the worker thread.
type Listing = (u64, PathBuf, Result<Vec<FileEntry>, String>);

/// An in-app answer to one file-chooser request.
pub struct FallbackChooser {
    operation: PortalOperation,
    title: String,
    filters: Vec<FileFilter>,
    filter_index: usize,
    directory: PathBuf,
    listed: Vec<FileEntry>,
    model: FileBrowserModel,
    selected: Vec<PathBuf>,
    name: String,
    /// The location as typed, while it is being edited.
    location: Option<String>,
    error: Option<String>,
    confirm_replace: bool,
    loading: bool,
    generation: u64,
    inbox: Arc<Mutex<Option<Listing>>>,
    signal: CompletionSignal,
    globs: Option<Arc<GlobTable>>,
}

impl std::fmt::Debug for FallbackChooser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackChooser")
            .field("operation", &self.operation)
            .field("directory", &self.directory)
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

impl FallbackChooser {
    /// Opens the dialog for `operation` with the request's options, starting
    /// in the requested folder, else the home folder.
    #[must_use]
    pub fn new(operation: PortalOperation, options: &ChooserOptions) -> Self {
        Self::with_signal(operation, options, CompletionSignal::new())
    }

    /// [`Self::new`], raising `signal` when a folder listing finishes: the
    /// signal a host already wakes on, so the dialog needs no wake source of
    /// its own.
    #[must_use]
    pub fn with_signal(
        operation: PortalOperation,
        options: &ChooserOptions,
        signal: CompletionSignal,
    ) -> Self {
        let directory = options
            .folder()
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let filters = options.filters().to_vec();
        let filter_index = options
            .selected_filter()
            .and_then(|selected| filters.iter().position(|filter| filter == selected))
            .unwrap_or(0);
        let globs = filters
            .iter()
            .flat_map(FileFilter::patterns)
            .any(|(rule, _)| *rule == FilterRule::MimeType)
            .then(|| Arc::new(system_globs()));
        let title = options
            .title_str()
            .map_or_else(|| default_title(operation), str::to_owned);
        let mut chooser = Self {
            operation,
            title,
            filters,
            filter_index,
            directory: directory.clone(),
            listed: Vec::new(),
            model: FileBrowserModel::new(),
            selected: Vec::new(),
            name: options.name().unwrap_or_default().to_owned(),
            location: None,
            error: None,
            confirm_replace: false,
            loading: false,
            generation: 0,
            inbox: Arc::default(),
            signal,
            globs,
        };
        chooser.navigate(directory);
        chooser
    }

    /// The signal raised when a folder listing finishes; wake on it and call
    /// [`Self::absorb`].
    #[must_use]
    pub fn signal(&self) -> &CompletionSignal {
        &self.signal
    }

    /// The folder being shown.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether a listing is still running.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The model the listing is shown from.
    #[must_use]
    pub fn model(&self) -> &FileBrowserModel {
        &self.model
    }

    /// Takes a finished folder listing, if one is waiting. Returns whether
    /// anything changed.
    pub fn absorb(&mut self) -> bool {
        let finished = self
            .inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some((generation, _, result)) = finished else {
            return false;
        };
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(entries) => {
                self.listed = entries;
                self.error = None;
            }
            Err(reason) => {
                self.listed.clear();
                self.error = Some(enzui_core::localized!(
                    "enzui-file-chooser-unreadable",
                    "folder" => self.directory.display().to_string(),
                    "reason" => reason
                ));
            }
        }
        self.refilter();
        true
    }

    fn navigate(&mut self, directory: PathBuf) {
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        // The target becomes current at once, so Up and the location line
        // follow the folder being opened rather than the one still listed.
        self.directory.clone_from(&directory);
        self.selected.clear();
        self.confirm_replace = false;
        let generation = self.generation;
        let inbox = Arc::clone(&self.inbox);
        let signal = self.signal.clone();
        let spawned = std::thread::Builder::new()
            .name("enzui-file-chooser-list".into())
            .spawn(move || {
                let result = list_directory(&directory);
                let mut slot = inbox.lock().unwrap_or_else(PoisonError::into_inner);
                // A slower listing for a folder the user already left must not
                // replace the newer one waiting in the slot.
                if slot
                    .as_ref()
                    .is_none_or(|(waiting, _, _)| *waiting <= generation)
                {
                    *slot = Some((generation, directory, result));
                }
                drop(slot);
                signal.notify();
            });
        if spawned.is_err() {
            self.loading = false;
            self.error = Some(text("enzui-file-chooser-busy"));
        }
    }

    /// Opens what the location names: a folder, or the folder of an existing
    /// file with that file selected (named, when saving). `~` is the home
    /// folder; a relative path is read from the current folder. A location
    /// that names nothing stays in the field with the error under it.
    fn go_to_location(&mut self) {
        let Some(typed) = self.location.as_deref().map(str::trim) else {
            return;
        };
        let expanded = match typed.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(rest.trim_start_matches('/'))),
            _ => Some(PathBuf::from(typed)),
        };
        let Some(path) = expanded.map(|path| {
            if path.is_absolute() {
                path
            } else {
                self.directory.join(path)
            }
        }) else {
            self.error = Some(text("enzui-file-chooser-no-such-location"));
            return;
        };
        if path.is_dir() {
            self.location = None;
            self.error = None;
            self.navigate(path);
        } else if path.is_file()
            && self.operation != PortalOperation::OpenDirectory
            && let Some(parent) = path.parent()
        {
            self.location = None;
            self.error = None;
            self.navigate(parent.to_path_buf());
            if self.operation == PortalOperation::SaveFile
                && let Some(name) = path.file_name()
            {
                self.name = name.to_string_lossy().into_owned();
            }
            self.selected = vec![path];
        } else {
            self.error = Some(text("enzui-file-chooser-no-such-location"));
        }
    }

    fn refilter(&mut self) {
        let operation = self.operation;
        let filter = self.filters.get(self.filter_index);
        let globs = self.globs.as_deref();
        let show_hidden = self.model.show_hidden();
        let entries: Vec<FileEntry> = self
            .listed
            .iter()
            .filter(|entry| entry.is_navigable() || operation != PortalOperation::OpenDirectory)
            .filter(|entry| {
                entry.is_navigable() || filter.is_none_or(|f| filter_accepts(f, &entry.name, globs))
            })
            .cloned()
            .collect();
        self.model.set_entries(entries);
        self.model.set_show_hidden(show_hidden);
    }

    /// Applies one input. Returns the request's outcome once the user
    /// accepts or cancels.
    pub fn update(&mut self, event: FallbackEvent) -> Option<PortalOutcome<FileResult>> {
        match event {
            FallbackEvent::Select(path) => {
                self.confirm_replace = false;
                if self.operation == PortalOperation::OpenMultiple {
                    if let Some(index) = self.selected.iter().position(|p| *p == path) {
                        self.selected.remove(index);
                    } else {
                        self.selected.push(path);
                    }
                } else {
                    if self.operation == PortalOperation::SaveFile
                        && let Some(name) = path.file_name()
                        && !path.is_dir()
                    {
                        self.name = name.to_string_lossy().into_owned();
                    }
                    self.selected = vec![path];
                }
                None
            }
            FallbackEvent::Activate(path) => {
                if path.is_dir() {
                    self.navigate(path);
                    return None;
                }
                self.selected = vec![path.clone()];
                match self.operation {
                    PortalOperation::SaveFile => {
                        if let Some(name) = path.file_name() {
                            self.name = name.to_string_lossy().into_owned();
                        }
                        self.accept()
                    }
                    PortalOperation::OpenDirectory => None,
                    PortalOperation::OpenFile | PortalOperation::OpenMultiple => self.accept(),
                }
            }
            FallbackEvent::Up => {
                if let Some(parent) = self.directory.parent() {
                    self.navigate(parent.to_path_buf());
                }
                None
            }
            FallbackEvent::NameChanged(name) => {
                self.name = name;
                self.confirm_replace = false;
                None
            }
            FallbackEvent::FilterChanged(index) => {
                if index < self.filters.len() {
                    self.filter_index = index;
                    self.refilter();
                }
                None
            }
            FallbackEvent::ToggleHidden => {
                let show = !self.model.show_hidden();
                self.model.set_show_hidden(show);
                None
            }
            FallbackEvent::Sort(key) => {
                self.model.toggle_sort(key);
                None
            }
            FallbackEvent::Accept => self.accept(),
            FallbackEvent::Cancel => Some(PortalOutcome::Cancelled(super::CancelReason::User)),
            FallbackEvent::FocusNext | FallbackEvent::FocusPrevious => None,
            FallbackEvent::EditLocation => {
                if self.location.is_none() {
                    self.location = Some(self.directory.display().to_string());
                }
                None
            }
            FallbackEvent::LocationChanged(text) => {
                self.location = Some(text);
                self.error = None;
                None
            }
            FallbackEvent::GoToLocation => {
                self.go_to_location();
                None
            }
        }
    }

    fn accept(&mut self) -> Option<PortalOutcome<FileResult>> {
        let chosen: Vec<PathBuf> = match self.operation {
            PortalOperation::OpenFile | PortalOperation::OpenMultiple => {
                if let [only] = self.selected.as_slice()
                    && only.is_dir()
                {
                    let only = only.clone();
                    self.navigate(only);
                    return None;
                }
                self.selected
                    .iter()
                    .filter(|path| !path.is_dir())
                    .cloned()
                    .collect()
            }
            PortalOperation::OpenDirectory => match self.selected.as_slice() {
                [only] if only.is_dir() => vec![only.clone()],
                _ => vec![self.directory.clone()],
            },
            PortalOperation::SaveFile => {
                let name = self.name.trim();
                if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                    self.error = Some(text("enzui-file-chooser-invalid-name"));
                    return None;
                }
                let target = self.directory.join(name);
                if target.is_dir() {
                    self.navigate(target);
                    return None;
                }
                if target.exists() && !self.confirm_replace {
                    self.confirm_replace = true;
                    self.error = Some(enzui_core::localized!(
                        "enzui-file-chooser-replace-question",
                        "name" => name.to_owned()
                    ));
                    return None;
                }
                vec![target]
            }
        };
        if chosen.is_empty() {
            return None;
        }
        let uris = chosen
            .iter()
            .filter_map(|path| OpenUri::from_file_path(path).ok())
            .map(|uri| FileUri::new(uri.as_str()))
            .collect();
        Some(PortalOutcome::Completed(FileResult { uris }))
    }

    fn accept_label(&self) -> String {
        if self.confirm_replace {
            return text("enzui-file-chooser-replace");
        }
        text(match self.operation {
            PortalOperation::OpenFile | PortalOperation::OpenMultiple => "enzui-file-chooser-open",
            PortalOperation::OpenDirectory => "enzui-file-chooser-select",
            PortalOperation::SaveFile => "enzui-file-chooser-save",
        })
    }

    fn can_accept(&self) -> bool {
        match self.operation {
            PortalOperation::OpenFile | PortalOperation::OpenMultiple => !self.selected.is_empty(),
            PortalOperation::OpenDirectory => !self.loading,
            PortalOperation::SaveFile => !self.name.trim().is_empty(),
        }
    }

    /// The dialog's content, before hosting.
    #[must_use]
    pub fn view(&self) -> Element<'_, FallbackEvent> {
        let title =
            iced::widget::text(self.title.clone()).size(enzui_theme::text_scale::ui_px(18.0));
        // Stable ids route each control's invoke action to assistive
        // technology, under the prefix the file browser below uses.
        let id = |suffix: &str| enzui_core::identity::derive_id(CHOOSER_ID_PREFIX, suffix);
        let up = enz_button(text("enzui-file-chooser-parent"), FallbackEvent::Up)
            .id(id("up"))
            .icon("arrow-up")
            .icon_position(ButtonIconPosition::Only)
            .aria_label(text("enzui-file-chooser-parent"))
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Sm)
            .enabled(self.directory.parent().is_some());
        // The location is a button until it is edited as text (Ctrl+L or a
        // click), then a field: Enter opens what it names.
        let location: Element<'_, FallbackEvent> = match &self.location {
            Some(typed) => crate::text_field::enz_text_field_owned(
                "",
                "",
                typed.clone(),
                FallbackEvent::LocationChanged,
            )
            .aria_label(text("enzui-file-chooser-location"))
            .id(location_id())
            .on_submit(FallbackEvent::GoToLocation)
            .into_element(),
            None => enz_button(
                self.directory.display().to_string(),
                FallbackEvent::EditLocation,
            )
            .id(id("location-path"))
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Sm)
            .aria_label(text("enzui-file-chooser-location"))
            .into_element(),
        };
        let location = iced::widget::container(location).width(Length::Fill);
        let hidden = enz_filter_chip(
            text("enzui-file-chooser-show-hidden"),
            self.model.show_hidden(),
            FallbackEvent::ToggleHidden,
        )
        .id(id("hidden"));
        let toolbar = row![up, location, hidden]
            .spacing(8)
            .align_y(Alignment::Center);

        let browser = enz_file_browser(&self.model)
            .selected(self.selected.clone())
            .on_select(FallbackEvent::Select)
            .on_activate(FallbackEvent::Activate)
            .on_sort(FallbackEvent::Sort)
            .id_prefix(CHOOSER_ID_PREFIX)
            .empty_title(if self.loading {
                text("enzui-file-chooser-loading")
            } else {
                text("enzui-file-chooser-empty")
            })
            .into_element();
        // The listing is 320 px, or what a short window leaves it once the
        // name field and the actions below are laid out (#3674).
        let browser = iced::widget::container(browser).height(Length::Fixed(320.0));

        let mut below: Vec<Element<'_, FallbackEvent>> = Vec::new();
        if self.operation == PortalOperation::SaveFile {
            below.push(
                crate::text_field::enz_text_field_owned(
                    text("enzui-file-chooser-name"),
                    "",
                    self.name.clone(),
                    FallbackEvent::NameChanged,
                )
                .on_submit(FallbackEvent::Accept)
                .into_element(),
            );
        }
        if self.filters.len() > 1 {
            let mut chips = crate::flow::enz_flow();
            for (index, filter) in self.filters.iter().enumerate() {
                chips = chips.push(
                    enz_filter_chip(
                        filter.name().to_owned(),
                        index == self.filter_index,
                        FallbackEvent::FilterChanged(index),
                    )
                    .id(id(&format!("filter-{index}"))),
                );
            }
            below.push(chips.into_element());
        }
        if let Some(error) = &self.error {
            below.push(
                iced::widget::text(error.clone())
                    .size(enzui_theme::text_scale::ui_px(13.0))
                    .into(),
            );
        }
        let cancel = enz_button(text("enzui-file-chooser-cancel"), FallbackEvent::Cancel)
            .id(id("cancel"))
            .kind(ButtonKind::Secondary);
        let accept = enz_button(self.accept_label(), FallbackEvent::Accept)
            .id(id("accept"))
            .kind(if self.confirm_replace {
                ButtonKind::Destructive
            } else {
                ButtonKind::Accent
            })
            .enabled(self.can_accept());
        // The action row runs from the start edge: Open trails at the left
        // in right-to-left text.
        below.push(
            row![Space::new().width(Length::Fill), cancel, accept]
                .spacing(10)
                .mirrored(
                    enzui_core::placement::ambient_direction()
                        == enzui_core::placement::Direction::RightToLeft,
                )
                .into(),
        );
        let body = crate::capped_dialog_body::enz_capped_dialog_body(
            vec![title.into(), toolbar.into()],
            browser,
            below,
            12.0,
            f32::INFINITY,
        );

        let dialog = crate::modal_surface::enz_modal_surface(body.into())
            .width(Length::Fill.max(640.0))
            .into_element();
        Element::new(LocationKey { content: dialog })
    }

    /// The dialog hosted modally over `background`, reporting through
    /// `on_event`. Escape and a click outside the dialog cancel.
    pub fn host<'a, Message: Clone + 'a>(
        &'a self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(FallbackEvent) -> Message + Clone + 'a,
    ) -> Element<'a, Message> {
        host(Some(self), background, on_event)
    }
}

/// `background` in a modal host that shows `chooser` while there is one.
///
/// The host is the same whether a chooser is open or not, so opening and
/// closing one leaves the background where it is in the widget tree and keeps
/// its scroll offsets, carets and editor state.
pub(crate) fn host<'a, Message: Clone + 'a>(
    chooser: Option<&'a FallbackChooser>,
    background: impl Into<Element<'a, Message>>,
    on_event: impl Fn(FallbackEvent) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    let on_dismiss = on_event.clone();
    let host = enz_modal_host(
        background,
        chooser.map(|chooser| chooser.view().map(on_event)),
        move |event| {
            on_dismiss(match event {
                ModalHostEvent::Dismissed(_) => FallbackEvent::Cancel,
                ModalHostEvent::FocusNext => FallbackEvent::FocusNext,
                ModalHostEvent::FocusPrevious => FallbackEvent::FocusPrevious,
            })
        },
    );
    match chooser {
        Some(chooser) => host.label(chooser.title.clone()).into_element(),
        None => host.into_element(),
    }
}

fn default_title(operation: PortalOperation) -> String {
    text(match operation {
        PortalOperation::OpenFile => "enzui-file-chooser-title-open",
        PortalOperation::OpenMultiple => "enzui-file-chooser-title-open-multiple",
        PortalOperation::OpenDirectory => "enzui-file-chooser-title-folder",
        PortalOperation::SaveFile => "enzui-file-chooser-title-save",
    })
}

fn list_directory(directory: &Path) -> Result<Vec<FileEntry>, String> {
    let read = std::fs::read_dir(directory).map_err(|error| error.kind().to_string())?;
    let mut entries = Vec::new();
    for entry in read.flatten().take(MAX_LISTED_ENTRIES) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // Follow symlinks so a link to a folder can be entered.
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let mut item = if metadata.is_dir() {
            FileEntry::directory(path, name)
        } else {
            FileEntry::file(path, name, metadata.len())
        };
        if let Ok(modified) = metadata.modified() {
            item = item.with_modified(modified);
        }
        entries.push(item);
    }
    Ok(entries)
}

/// Whether a file name passes a portal filter: glob patterns match the name
/// case-insensitively, MIME patterns match the type its name implies
/// (`image/*` matches every image type).
#[must_use]
pub fn filter_accepts(filter: &FileFilter, name: &str, globs: Option<&GlobTable>) -> bool {
    if filter.is_empty() {
        return true;
    }
    filter.patterns().iter().any(|(rule, pattern)| match rule {
        FilterRule::Glob => glob_matches(&pattern.to_lowercase(), &name.to_lowercase()),
        FilterRule::MimeType => globs
            .and_then(|table| table.resolve(name))
            .is_some_and(|found| mime_matches(pattern, &found.mime)),
    })
}

fn mime_matches(pattern: &str, mime: &str) -> bool {
    match pattern.strip_suffix("/*") {
        Some(major) => mime.split('/').next() == Some(major),
        None => pattern.eq_ignore_ascii_case(mime),
    }
}

/// `*` and `?` glob matching, as portal filters use it.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn system_globs() -> GlobTable {
    let env = enzui_icons::mime::xdg::Globs2Env::from_env();
    let (parsed, _) = enzui_icons::mime::xdg::load_globs2_from(&env.candidates());
    GlobTable::new(parsed.rules)
}

/// The chooser's own key: Ctrl+L, which nothing inside handled, edits the
/// location (#3673).
struct LocationKey<'a> {
    content: Element<'a, FallbackEvent>,
}

impl iced::advanced::Widget<FallbackEvent, iced::Theme, iced::Renderer> for LocationKey<'_> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.content.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.content.as_widget_mut().diff(tree);
    }

    fn size(&self) -> iced::Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.content.as_widget_mut().layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, FallbackEvent>,
        viewport: &iced::Rectangle,
    ) {
        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        if shell.is_event_captured() {
            return;
        }
        if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Character(c),
            modifiers,
            ..
        }) = event
            && c.eq_ignore_ascii_case("l")
            && modifiers.command()
            && !modifiers.shift()
            && !modifiers.alt()
        {
            shell.publish(FallbackEvent::EditLocation);
            shell.capture_event();
        }
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        self.content.as_widget().frame_interest(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, FallbackEvent>,
        viewport: &iced::Rectangle,
    ) {
        self.content
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, FallbackEvent, iced::Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

#[cfg(test)]
mod tests;
