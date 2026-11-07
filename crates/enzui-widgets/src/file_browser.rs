//! Render a file listing from a host-owned model without reading the filesystem.
//!
//! [`enz_file_browser`] renders [`FileBrowserModel::visible`](enzui_core::file_browser::FileBrowserModel::visible)
//! entries in list or grid mode, using per-type icons. The host performs
//! IO, sorting and navigation. Every entry is built eagerly unless the host lends
//! a [`ScrollTranslation`](crate::scroll_view::ScrollTranslation) through [`EnzFileBrowser::scroll`]: then only the rows
//! (or grid rows of tiles) in and near the viewport are built, so a large folder
//! costs what the window shows. [`EnzFileBrowser::on_sort`] reports a requested
//! column; apply it to the model with `toggle_sort` and rebuild.
//!
//! Pointer clicks select by default; [`EnzFileBrowser::single_click_activates`]
//! switches them to activation. Enter requests activation and Space requests
//! selection. If only one of [`EnzFileBrowser::on_activate`] or
//! [`EnzFileBrowser::on_select`] is wired, it handles both kinds of request.
//! The host stores selected paths and passes them through [`EnzFileBrowser::selected`].
//!
//! List metadata columns follow authored order after the name lane and fold away
//! when width is scarce. [`EnzFileBrowser::view_mode`], row/tile sizes and grid
//! column count control presentation. Empty-state wording can distinguish an empty
//! folder from a search with no matches. Use a distinct [`EnzFileBrowser::id_prefix`]
//! for each browser. [`EnzFileBrowser::theme`] pins a palette; otherwise row and
//! header colors resolve from the runtime theme at draw time.
//! [`EnzFileBrowser::metadata_column`] inserts host-loaded cells in that order;
//! [`EnzFileBrowser::header_radius`] rounds only the header, preserving row geometry.
//!
//! [`EnzFileBrowser::on_context`] reports an entry path and window-space point;
//! the host supplies the menu model and opens it through [`crate::context_menu_host`].
//! For hierarchical navigation use [`crate::tree_view`]; for a desktop portal
//! chooser use [`crate::file_chooser`].

use std::path::PathBuf;

use enzui_core::file_browser::{EntryKind, FileBrowserModel, FileEntry, SortKey};
use enzui_core::local_time::{
    LocalCivilTime, LocalCivilTimeError, current_local_civil_time, local_civil_time_at,
    local_civil_time_at_with,
};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{EnzTheme, Palette};
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length, border::Radius};

use crate::enz_text::TextSize;
use crate::grid::enz_grid;
use crate::icon_label::{IconLabelA11y, IconLabelTone, enz_icon_label};
use crate::list_row_surface::{ListRowSurfaceTokens, enz_list_row_surface_lazy};
use crate::scroll_view::{ScrollTranslation, enz_scroll_view};
use crate::status_page::enz_status_page;
use crate::table_header_cell::{EnzTableHeaderCellAlign, enz_table_header_cell};
use crate::table_text_cell::{EnzTableTextAlign, enz_table_text_cell};
use enzui_core::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};

/// How the listing is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileBrowserView {
    /// One row per entry, with sortable metadata columns.
    #[default]
    List,
    /// A wrapping grid of icon-over-name tiles.
    Grid,
}

/// A metadata column in [`FileBrowserView::List`].
///
/// The Name column is not a member: it is always shown, always first, and
/// always the flexible one. Making it optional would allow a browser with no
/// names in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileColumn {
    /// Size in bytes, humanised. Blank for a directory.
    Size,
    /// Last-modified time.
    Modified,
    /// The human-readable type name, from the resolved MIME.
    Kind,
}

impl FileColumn {
    /// The header label, and the [`SortKey`] a click on it signals.
    pub fn sort_key(self) -> SortKey {
        match self {
            FileColumn::Size => SortKey::Size,
            FileColumn::Modified => SortKey::Modified,
            FileColumn::Kind => SortKey::Kind,
        }
    }

    /// The column header label.
    pub fn label(self) -> String {
        self.sort_key().label()
    }
}

/// The metadata columns requested by default, in priority order.
pub const DEFAULT_COLUMNS: &[FileColumn] =
    &[FileColumn::Size, FileColumn::Modified, FileColumn::Kind];

/// The default row height. Denser than the generic `list-row-height` (34)
/// because a file listing is a dense surface, and still clears the 24 px
/// minimum hit target.
pub const DEFAULT_ROW_HEIGHT: f32 = 30.0;

/// The default icon size in a list row.
pub const DEFAULT_ICON_SIZE: f32 = 18.0;

/// The default grid tile edge.
pub const DEFAULT_TILE_SIZE: f32 = 96.0;

/// English fallback suffix identifying a directory row.
pub const DIRECTORY_ROLE_SUFFIX: &str = " (folder)";

/// English fallback name for a row context-menu affordance.
pub const CONTEXT_MENU_LABEL: &str = "File actions";

type MetadataCell<'a, Message> = std::rc::Rc<dyn Fn(&'a FileEntry) -> Element<'a, Message> + 'a>;
type MetadataHeader<'a, Message> = std::rc::Rc<dyn Fn() -> Element<'a, Message> + 'a>;

/// A fixed-width list column whose cells use already-loaded host metadata.
///
/// Defaults to 32 logical pixels after the built-in columns. Header labels remain
/// accessible when custom header content is supplied. Factories run only for
/// admitted list columns; grid and empty views do not invoke them.
#[must_use]
pub struct FileMetadataColumn<'a, Message> {
    key: String,
    label: String,
    width: f32,
    before: Option<FileColumn>,
    cell: MetadataCell<'a, Message>,
    header: Option<MetadataHeader<'a, Message>>,
}

impl<'a, Message: 'a> FileMetadataColumn<'a, Message> {
    /// Creates a column with a stable host key and accessible header label.
    /// Cell factories receive the model entry, including its original native path.
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        cell: impl Fn(&'a FileEntry) -> Element<'a, Message> + 'a,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            width: 32.0,
            before: None,
            cell: std::rc::Rc::new(cell),
            header: None,
        }
    }

    /// Sets the fixed admission width in logical pixels.
    /// Non-positive or non-finite values use the 32 px default.
    pub fn width(mut self, width: f32) -> Self {
        self.width = enzui_core::metrics::FinitePositive::new_or(width, 32.0).get();
        self
    }

    /// Inserts before this built-in column's first occurrence.
    /// An absent anchor places the column after the requested built-ins.
    pub fn before(mut self, column: FileColumn) -> Self {
        self.before = Some(column);
        self
    }

    /// Supplies custom header paint, such as an icon with a tooltip.
    /// The constructor label remains the column header's accessible name.
    pub fn header(mut self, header: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.header = Some(std::rc::Rc::new(header));
        self
    }
}

#[derive(Clone, Copy)]
enum ListColumn {
    BuiltIn(FileColumn),
    Metadata(usize),
}

type RowAction<'a, Message> = std::rc::Rc<dyn Fn(PathBuf) -> Message + 'a>;
type SortAction<'a, Message> = std::rc::Rc<dyn Fn(SortKey) -> Message + 'a>;

/// A file listing over a host-owned model.
///
/// Construct with [`enz_file_browser`].
pub struct EnzFileBrowser<'a, Message> {
    model: &'a FileBrowserModel,
    view: FileBrowserView,
    columns: Vec<FileColumn>,
    metadata_columns: Vec<FileMetadataColumn<'a, Message>>,
    header_radius: Radius,
    selected: Vec<PathBuf>,
    theme: Option<&'a EnzTheme>,
    row_height: f32,
    icon_size: f32,
    tile_size: f32,
    grid_columns: usize,
    single_click_activates: bool,
    empty_title: String,
    empty_description: String,
    empty_icon: String,
    id_prefix: String,
    on_activate: Option<RowAction<'a, Message>>,
    on_select: Option<RowAction<'a, Message>>,
    on_sort: Option<SortAction<'a, Message>>,
    on_context: Option<std::rc::Rc<dyn Fn(PathBuf, iced::Point) -> Message + 'a>>,
    scroll: Option<ScrollTranslation>,
}

/// A file listing over `model`.
pub fn enz_file_browser<Message>(model: &FileBrowserModel) -> EnzFileBrowser<'_, Message> {
    EnzFileBrowser {
        model,
        view: FileBrowserView::List,
        columns: DEFAULT_COLUMNS.to_vec(),
        metadata_columns: Vec::new(),
        header_radius: Radius::default(),
        selected: Vec::new(),
        theme: None,
        row_height: DEFAULT_ROW_HEIGHT,
        icon_size: DEFAULT_ICON_SIZE,
        tile_size: DEFAULT_TILE_SIZE,
        grid_columns: 4,
        single_click_activates: false,
        empty_title: enzui_core::localization::text("enzui-file-browser-empty"),
        empty_description: String::new(),
        empty_icon: "folder-open".to_owned(),
        id_prefix: "enz-file-browser".to_owned(),
        on_activate: None,
        on_select: None,
        on_sort: None,
        on_context: None,
        scroll: None,
    }
}

impl<'a, Message: Clone + 'static> EnzFileBrowser<'a, Message> {
    /// List or grid.
    #[must_use]
    pub fn view_mode(mut self, view: FileBrowserView) -> Self {
        self.view = view;
        self
    }

    /// Request metadata columns in priority order; an empty slice gives a name-only list.
    ///
    /// Name is always first. The list shows the largest prefix that leaves
    /// a 160 px name lane, reserving more space for larger icons. At widths
    /// below that lane, metadata is hidden and Name uses the available width.
    #[must_use]
    pub fn columns(mut self, columns: impl Into<Vec<FileColumn>>) -> Self {
        self.columns = columns.into();
        self
    }

    /// Adds a host metadata column to the responsive list prefix.
    /// A repeated key replaces its existing definition in place. Built-in
    /// sorting and row selection/activation remain owned by the browser.
    #[must_use]
    pub fn metadata_column(mut self, column: FileMetadataColumn<'a, Message>) -> Self {
        if let Some(existing) = self
            .metadata_columns
            .iter_mut()
            .find(|c| c.key == column.key)
        {
            *existing = column;
        } else {
            self.metadata_columns.push(column);
        }
        self
    }

    /// Sets header corner radii in logical pixels; defaults to square corners.
    /// Negative or non-finite values become zero. The header's paint and pointer
    /// hits share its rounded clip; scrolling rows keep their existing geometry.
    #[must_use]
    pub fn header_radius(mut self, radius: impl Into<Radius>) -> Self {
        self.header_radius = crate::rounded_clip::sane_radius(radius.into());
        self
    }

    /// The selected paths, for the row highlight and `aria-selected`.
    #[must_use]
    pub fn selected(mut self, selected: impl Into<Vec<PathBuf>>) -> Self {
        self.selected = selected.into();
        self
    }

    /// Pin row and header colors to the supplied palette.
    ///
    /// When unset, colors resolve from the live runtime theme at draw time.
    #[must_use]
    pub fn theme(mut self, theme: &'a EnzTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Row height in the list view.
    ///
    /// Normalized: a NaN, infinite or negative height falls back to
    /// [`DEFAULT_ROW_HEIGHT`] rather than reaching layout. These four
    /// dimension setters are independent fields, so normalizing at the setter
    /// cannot make the builder order-dependent.
    #[must_use]
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height =
            enzui_core::metrics::FiniteNonNeg::new_or(height, DEFAULT_ROW_HEIGHT).get();
        self
    }

    /// Icon size in a list row.
    ///
    /// Normalized to a finite non-negative extent; see [`Self::row_height`].
    #[must_use]
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = enzui_core::metrics::FiniteNonNeg::new_or(size, DEFAULT_ICON_SIZE).get();
        self
    }

    /// Tile edge in the grid view.
    ///
    /// Normalised to a finite positive extent (a zero-edge tile
    /// is not a tile); see [`Self::row_height`].
    #[must_use]
    pub fn tile_size(mut self, size: f32) -> Self {
        self.tile_size = enzui_core::metrics::FinitePositive::new_or(size, DEFAULT_TILE_SIZE).get();
        self
    }

    /// Column count in the grid view.
    ///
    /// Clamped to at least 1 at the model boundary: a zero-column grid would
    /// divide by zero when placing tiles.
    #[must_use]
    pub fn grid_columns(mut self, columns: usize) -> Self {
        self.grid_columns = columns.max(1);
        self
    }

    /// Choose activation rather than selection for pointer clicks. Defaults to false.
    ///
    /// Enter requests activation and Space requests selection independently. An unwired
    /// action falls back to the other handler if available.
    #[must_use]
    pub fn single_click_activates(mut self, single: bool) -> Self {
        self.single_click_activates = single;
        self
    }

    /// The empty state's title.
    #[must_use]
    pub fn empty_title(mut self, title: impl Into<String>) -> Self {
        self.empty_title = title.into();
        self
    }

    /// The empty state's description.
    #[must_use]
    pub fn empty_description(mut self, description: impl Into<String>) -> Self {
        self.empty_description = description.into();
        self
    }

    /// The empty state's glyph.
    #[must_use]
    pub fn empty_icon(mut self, icon: impl Into<String>) -> Self {
        self.empty_icon = icon.into();
        self
    }

    /// Build only the rows near the viewport, keeping the scroll position in
    /// `scroll`. Keep the handle in application state so the position survives
    /// rebuilds; one handle per browser. Rows outside the realized window are not
    /// in the widget tree, so they cannot take focus until scrolled near.
    #[must_use]
    pub fn scroll(mut self, scroll: ScrollTranslation) -> Self {
        self.scroll = Some(scroll);
        self
    }

    /// The widget-id prefix, for tests and focus targeting.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Handle activation requests with the entry’s path.
    ///
    /// Enter and activation-mode pointer clicks use this callback. It also receives
    /// selection requests when [`Self::on_select`] is unset. The host performs navigation.
    #[must_use]
    pub fn on_activate(mut self, f: impl Fn(PathBuf) -> Message + 'a) -> Self {
        self.on_activate = Some(std::rc::Rc::new(f));
        self
    }

    /// Handle selection requests with the entry’s path.
    ///
    /// Space and default pointer clicks use this callback. It also receives activation
    /// requests when [`Self::on_activate`] is unset. Feed selected paths back into the widget.
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(PathBuf) -> Message + 'a) -> Self {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }

    /// A column header was clicked. The host applies it to the model.
    /// The factory runs on activation, never during view construction or layout.
    #[must_use]
    pub fn on_sort(mut self, f: impl Fn(SortKey) -> Message + 'a) -> Self {
        self.on_sort = Some(std::rc::Rc::new(f));
        self
    }

    /// Handle a row context-menu request with its path and window-space point.
    ///
    /// The host owns menu content and state. Pointer invocation uses the cursor position;
    /// routed accessibility invocation can use the row center when no cursor exists.
    /// Open the menu through [`crate::context_menu_host::enz_point_context_menu_host`].
    #[must_use]
    pub fn on_context(mut self, f: impl Fn(PathBuf, iced::Point) -> Message + 'a) -> Self {
        self.on_context = Some(std::rc::Rc::new(f));
        self
    }

    /// Build the element.
    pub fn into_element(self) -> Element<'a, Message> {
        let rows = self.model.visible();
        if rows.is_empty() {
            return self.empty_state();
        }
        match self.view {
            FileBrowserView::List => self.list_view(rows),
            FileBrowserView::Grid => self.grid_view(rows),
        }
    }

    fn palette(&self) -> Palette {
        self.theme
            .map_or_else(|| EnzTheme::dark().palette, |t| t.palette.clone())
    }

    fn has_pinned_theme(&self) -> bool {
        self.theme.is_some()
    }

    fn empty_state(self) -> Element<'a, Message> {
        let mut page = enz_status_page(self.empty_title.clone()).icon(self.empty_icon.clone());
        if !self.empty_description.is_empty() {
            page = page.description(self.empty_description);
        }
        container(page.into_element())
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn is_selected(&self, entry: &FileEntry) -> bool {
        self.selected.contains(&entry.path)
    }

    fn click_action(&self) -> Option<RowAction<'a, Message>> {
        if self.single_click_activates {
            self.activation_action()
        } else {
            self.selection_action()
        }
    }

    fn activation_action(&self) -> Option<RowAction<'a, Message>> {
        self.on_activate.clone().or_else(|| self.on_select.clone())
    }

    fn selection_action(&self) -> Option<RowAction<'a, Message>> {
        self.on_select.clone().or_else(|| self.on_activate.clone())
    }

    fn row_label(entry: &FileEntry) -> String {
        if matches!(entry.kind, EntryKind::Directory) {
            enzui_core::localized!("enzui-file-browser-directory-row", "name" => entry.name)
        } else {
            entry.name.clone()
        }
    }

    fn list_view(self, rows: Vec<&'a FileEntry>) -> Element<'a, Message> {
        let authored = self.authored_columns();
        let this = std::rc::Rc::new(self);
        let rows: std::rc::Rc<[&'a FileEntry]> = rows.into();
        crate::responsive::enz_responsive(move |size| -> Element<'a, Message> {
            let this = &this;
            let name_width = (this.icon_size + TextSize::Body.px() * 8.0 + 8.0).max(160.0);
            let row_gutter = this.metadata_row_gutter();
            let mut remaining = (size.width - name_width - 2.0 * row_gutter).max(0.0);
            let columns: Vec<_> = authored
                .iter()
                .copied()
                .take_while(|column| {
                    let width = this.column_width(*column);
                    if width <= remaining {
                        remaining -= width;
                        true
                    } else {
                        false
                    }
                })
                .collect();
            let palette = this.palette();
            let header = this.header(&palette, &columns);
            let scrolled: Element<'a, Message> = if let Some(scroll) = this.scroll.clone() {
                let browser = std::rc::Rc::clone(this);
                crate::virtual_list::enz_virtual_list(
                    std::rc::Rc::new(EntryRows(std::rc::Rc::clone(&rows))),
                    this.row_height,
                    move |_, entry: &&'a FileEntry| browser.list_row(entry, &palette, &columns),
                )
                .viewport_height((size.height - this.row_height).max(0.0))
                .self_scrolling(scroll)
                .width(Length::Fill)
                .height(Length::Fill)
                .into_element()
            } else {
                let mut body = column![].width(Length::Fill);
                for entry in rows.iter() {
                    body = body.push(this.list_row(entry, &palette, &columns));
                }
                enz_scroll_view(body)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            };
            column![header, scrolled]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        })
        .into()
    }

    fn authored_columns(&self) -> Vec<ListColumn> {
        let mut columns = Vec::new();
        let mut inserted = vec![false; self.metadata_columns.len()];
        for builtin in &self.columns {
            for (index, metadata) in self.metadata_columns.iter().enumerate() {
                if !inserted[index] && metadata.before == Some(*builtin) {
                    columns.push(ListColumn::Metadata(index));
                    inserted[index] = true;
                }
            }
            columns.push(ListColumn::BuiltIn(*builtin));
        }
        for (index, inserted) in inserted.into_iter().enumerate() {
            if !inserted {
                columns.push(ListColumn::Metadata(index));
            }
        }
        columns
    }

    fn column_width(&self, column: ListColumn) -> f32 {
        match column {
            ListColumn::BuiltIn(column) => metadata_column_width(column),
            ListColumn::Metadata(index) => self.metadata_columns[index].width,
        }
    }

    fn metadata_row_gutter(&self) -> f32 {
        if self.metadata_columns.is_empty() {
            0.0
        } else {
            ListRowSurfaceTokens::from_palette(&self.palette()).outer_gutter
        }
    }

    fn header(&self, palette: &Palette, columns: &[ListColumn]) -> Element<'a, Message> {
        let mut header_row = row![self.header_cell(SortKey::Name, Length::Fill, palette)]
            .align_y(Alignment::Center)
            .width(Length::Fill);
        for col in columns {
            header_row = header_row.push(match *col {
                ListColumn::BuiltIn(col) => self.header_cell(
                    col.sort_key(),
                    Length::Fixed(metadata_column_width(col)),
                    palette,
                ),
                ListColumn::Metadata(index) => self.metadata_header(index, palette),
            });
        }
        let pinned = self.has_pinned_theme().then_some(palette.surface);
        let radius = self.header_radius;
        let header = container(container(header_row).padding([0.0, self.metadata_row_gutter()]))
            .height(Length::Fixed(self.row_height))
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .style(move |t: &iced::Theme| container::Style {
                background: Some(
                    pinned
                        .unwrap_or_else(|| crate::theme_lookup::palette(t).surface)
                        .into(),
                ),
                ..Default::default()
            });
        if radius == Radius::default() {
            header.into()
        } else {
            crate::rounded_clip::rounded_clip(header, radius).into()
        }
    }

    fn metadata_header(&self, index: usize, palette: &Palette) -> Element<'a, Message> {
        use crate::semantics_container::SemanticsContainer;
        let metadata = &self.metadata_columns[index];
        let semantics = Semantics::new(Role::ColumnHeader).label(metadata.label.clone());
        let id = enzui_core::identity::OwnerId::of_kind("file-metadata", &self.id_prefix)
            .child(&metadata.key)
            .id("header");
        let content: Element<'a, Message> = if let Some(factory) = &metadata.header {
            SemanticsContainer::new(factory(), semantics).id(id).into()
        } else {
            let mut header =
                enz_table_header_cell(metadata.label.clone()).header_height(self.row_height);
            if self.has_pinned_theme() {
                header = header.text_color(palette.text_muted);
            }
            SemanticsContainer::leaf(header.into_element(), semantics)
                .id(id)
                .into()
        };
        crate::rounded_clip::rounded_clip(
            container(content)
                .width(metadata.width)
                .height(self.row_height)
                .align_y(Alignment::Center),
            0.0,
        )
        .into()
    }

    fn header_cell(&self, key: SortKey, width: Length, palette: &Palette) -> Element<'a, Message> {
        let active = self.model.sort_key() == key;
        let mut cell = enz_table_header_cell(key.label())
            .align(EnzTableHeaderCellAlign::Left)
            .sortable(self.on_sort.is_some())
            .sort_active(active)
            .sort_ascending(self.model.direction().is_ascending())
            .header_height(self.row_height)
            .id(enzui_core::identity::derive_id(
                &format!("{}-header", self.id_prefix),
                match key {
                    SortKey::Name => "Name",
                    SortKey::Size => "Size",
                    SortKey::Modified => "Modified",
                    SortKey::Kind => "Type",
                },
            ));
        // Only explicit host palettes may freeze draw-time ink.
        if self.has_pinned_theme() {
            cell = cell
                .text_color(if active {
                    palette.text
                } else {
                    palette.text_muted
                })
                .accent_color(palette.accent);
        }
        if let Some(f) = self.on_sort.clone() {
            cell = cell.on_sort_with(move || f(key));
        }
        container(cell.into_element())
            .width(width)
            .align_y(Alignment::Center)
            .into()
    }

    fn list_row(
        &self,
        entry: &'a FileEntry,
        palette: &Palette,
        columns: &[ListColumn],
    ) -> Element<'a, Message> {
        // The row owns the accessible name; its content label stays decorative.
        let mut label = enz_icon_label(entry.icon_name(), entry.name.clone())
            .size(TextSize::Body)
            .icon_size(self.icon_size)
            .slot_width(self.icon_size)
            .a11y(IconLabelA11y::Decorative)
            .fill(true);
        label = if matches!(entry.kind, EntryKind::Directory) {
            label.icon_tone(IconLabelTone::Accent)
        } else {
            label.icon_tone(IconLabelTone::Muted)
        };
        if self.has_pinned_theme() {
            label = label
                .icon_color(if matches!(entry.kind, EntryKind::Directory) {
                    palette.accent
                } else {
                    palette.text_muted
                })
                .text_color(palette.text);
        }
        let name_cell = container(label.into_element()).width(Length::Fill);

        let mut content = row![name_cell]
            .align_y(Alignment::Center)
            .width(Length::Fill);
        for column in columns {
            match *column {
                ListColumn::Metadata(index) => {
                    let metadata = &self.metadata_columns[index];
                    content = content.push(crate::rounded_clip::rounded_clip(
                        container((metadata.cell)(entry))
                            .width(metadata.width)
                            .height(self.row_height)
                            .align_y(Alignment::Center),
                        0.0,
                    ));
                }
                ListColumn::BuiltIn(column) => {
                    let mut cell = enz_table_text_cell(cell_text(entry, column))
                        .align(EnzTableTextAlign::Left)
                        .body_size(TextSize::Caption.px())
                        .cell_padding_x(0.0);
                    if self.has_pinned_theme() {
                        cell = cell.text_color(palette.text_muted);
                    }
                    content = content
                        .push(container(cell.into_element()).width(metadata_column_width(column)));
                }
            }
        }

        let row_el = container(content)
            .height(Length::Fixed(self.row_height))
            .width(Length::Fill)
            .align_y(Alignment::Center);

        self.wrap_row(row_el.into(), entry, palette, self.row_height)
    }

    fn wrap_row(
        &self,
        content: Element<'a, Message>,
        entry: &FileEntry,
        palette: &Palette,
        height: f32,
    ) -> Element<'a, Message> {
        let mut surface = enz_list_row_surface_lazy(content)
            .content_padding(0.0)
            .selectable(true)
            .selected(self.is_selected(entry))
            .label(Self::row_label(entry))
            .row_height(height)
            .id_prefix(format!("{}-row", self.id_prefix))
            .width(Length::Fill);
        // Unpinned hover and selection fills must follow runtime theme changes.
        surface = if self.has_pinned_theme() {
            surface.tokens(ListRowSurfaceTokens::from_palette(palette))
        } else {
            surface.tokens_resolved(|t| {
                crate::theme_lookup::resolve_live(t, ListRowSurfaceTokens::from_palette)
            })
        };
        if let Some(action) = self.click_action() {
            let path = entry.path.clone();
            surface = surface.on_activate_with(move || action(path.clone()));
        }
        if let Some(action) = self.activation_action() {
            let path = entry.path.clone();
            surface = surface.on_enter(move || action(path.clone()));
        }
        if let Some(action) = self.selection_action() {
            let path = entry.path.clone();
            surface = surface.on_space(move || action(path.clone()));
        }
        if let Some(f) = self.on_context.clone() {
            let path = entry.path.clone();
            surface = surface.on_context(move |at| f(path.clone(), at));
            surface = surface.context_label(enzui_core::localization::text("enzui-file-actions"));
        }
        surface.into_element()
    }

    fn grid_view(self, rows: Vec<&'a FileEntry>) -> Element<'a, Message> {
        if let Some(scroll) = self.scroll.clone() {
            return self.virtual_grid_view(rows, scroll);
        }
        let palette = self.palette();
        let tiles: Vec<Element<'a, Message>> =
            rows.iter().map(|e| self.tile(e, &palette)).collect();
        let grid = enz_grid(self.grid_columns, tiles)
            .gap(8.0)
            .width(Length::Fill)
            .into_element();
        enz_scroll_view(grid)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The grid as a virtual list of tile rows, each laid out by the same
    /// fixed-column grid as the eager view.
    fn virtual_grid_view(
        self,
        rows: Vec<&'a FileEntry>,
        scroll: ScrollTranslation,
    ) -> Element<'a, Message> {
        const GAP: f32 = 8.0;
        let this = std::rc::Rc::new(self);
        let tile_rows = std::rc::Rc::new(TileRows {
            len: rows.len(),
            columns: this.grid_columns,
        });
        let entries: std::rc::Rc<[&'a FileEntry]> = rows.into();
        crate::responsive::enz_responsive(move |size| -> Element<'a, Message> {
            let browser = std::rc::Rc::clone(&this);
            let entries = std::rc::Rc::clone(&entries);
            let palette = this.palette();
            crate::virtual_list::enz_virtual_list(
                std::rc::Rc::clone(&tile_rows),
                this.tile_size + GAP,
                move |_, row: &std::ops::Range<usize>| {
                    let tiles = entries[row.clone()]
                        .iter()
                        .map(|e| browser.tile(e, &palette))
                        .collect();
                    container(
                        enz_grid(browser.grid_columns, tiles)
                            .gap(GAP)
                            .width(Length::Fill)
                            .into_element(),
                    )
                    .padding(iced::Padding::ZERO.bottom(GAP))
                    .into()
                },
            )
            .viewport_height(size.height)
            .self_scrolling(scroll.clone())
            .width(Length::Fill)
            .height(Length::Fill)
            .into_element()
        })
        .into()
    }

    fn tile(&self, entry: &FileEntry, palette: &Palette) -> Element<'a, Message> {
        let mut g = enz_icon_label(entry.icon_name(), entry.name.clone())
            .icon_size(self.tile_size * 0.5)
            .a11y(IconLabelA11y::Decorative)
            .display(crate::icon_label::IconLabelDisplay::IconOnly);
        g = if matches!(entry.kind, EntryKind::Directory) {
            g.icon_tone(IconLabelTone::Accent)
        } else {
            g.icon_tone(IconLabelTone::Muted)
        };
        if self.has_pinned_theme() {
            g = g.icon_color(if matches!(entry.kind, EntryKind::Directory) {
                palette.accent
            } else {
                palette.text_muted
            });
        }
        let glyph: Element<'a, Message> = g.into_element();
        let mut cap = enz_table_text_cell(entry.name.clone())
            .align(EnzTableTextAlign::Center)
            .body_size(TextSize::Caption.px())
            .cell_padding_x(0.0);
        if self.has_pinned_theme() {
            cap = cap.text_color(palette.text);
        }
        let caption: Element<'a, Message> = cap.into_element();

        let body = container(
            column![glyph, caption]
                .align_x(Alignment::Center)
                .width(Length::Fixed(self.tile_size)),
        )
        .width(Length::Fixed(self.tile_size))
        .height(Length::Fixed(self.tile_size))
        .center_x(Length::Fixed(self.tile_size))
        .center_y(Length::Fixed(self.tile_size));

        self.wrap_row(body.into(), entry, palette, self.tile_size)
    }
}

impl<'a, Message: Clone + 'static> From<EnzFileBrowser<'a, Message>> for Element<'a, Message> {
    fn from(browser: EnzFileBrowser<'a, Message>) -> Self {
        browser.into_element()
    }
}

/// The visible entries as a keyed row source for the virtual list.
struct EntryRows<'a>(std::rc::Rc<[&'a FileEntry]>);

impl<'a> ItemProvider<&'a FileEntry> for EntryRows<'a> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn item(&self, index: u64) -> Option<&'a FileEntry> {
        self.0.get(usize::try_from(index).ok()?).copied()
    }
}

impl<'a> KeyedProvider<&'a FileEntry> for EntryRows<'a> {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.item(index)
            .map(|entry| SequenceKey::new(entry.path.to_string_lossy()))
    }
}

/// The visible entries cut into grid rows of `columns` tiles; each item is the
/// row's range of entry indices.
struct TileRows {
    len: usize,
    columns: usize,
}

impl ItemProvider<std::ops::Range<usize>> for TileRows {
    fn len(&self) -> u64 {
        self.len.div_ceil(self.columns) as u64
    }

    fn item(&self, index: u64) -> Option<std::ops::Range<usize>> {
        let start = usize::try_from(index).ok()?.checked_mul(self.columns)?;
        (start < self.len).then(|| start..start.saturating_add(self.columns).min(self.len))
    }
}

impl KeyedProvider<std::ops::Range<usize>> for TileRows {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.item(index)
            .map(|row| SequenceKey::new(format!("tiles-{}", row.start)))
    }
}

/// Return the list’s fixed logical-pixel width for a metadata column.
pub fn metadata_column_width(column: FileColumn) -> f32 {
    match column {
        FileColumn::Size => 88.0,
        FileColumn::Modified => 132.0,
        FileColumn::Kind => 120.0,
    }
}

/// The text a metadata cell shows for an entry.
///
/// Public so a host can render the identical string elsewhere (a properties
/// pane, a status line) without re-deriving it and drifting.
pub fn cell_text(entry: &FileEntry, column: FileColumn) -> String {
    match column {
        FileColumn::Size => format_size(entry),
        FileColumn::Modified => format_modified(entry),
        FileColumn::Kind => kind_label(entry),
    }
}

/// A directory has no meaningful size, so its Size cell is blank rather than
/// `0 B` — which would read as an empty file.
pub fn format_size(entry: &FileEntry) -> String {
    if matches!(entry.kind, EntryKind::Directory) {
        return String::new();
    }
    format_bytes(entry.size)
}

/// Format file sizes using decimal, base-1000 units from bytes through petabytes.
///
/// Values below 1000 use integer bytes. Scaled values use two decimals below ten,
/// one below one hundred, and integer rounding otherwise.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = [
        "enzui-file-size-bytes",
        "enzui-file-size-kilobytes",
        "enzui-file-size-megabytes",
        "enzui-file-size-gigabytes",
        "enzui-file-size-terabytes",
        "enzui-file-size-petabytes",
    ];
    let localizer = enzui_core::localization::current();
    if bytes < 1000 {
        let value = localizer
            .locale_for(enzui_core::localization::LocaleCategory::Numeric)
            .format_decimal(&bytes.into())
            .unwrap_or_else(|_| bytes.to_string());
        return enzui_core::localized!(UNITS[0], "value" => value);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let digits = if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    let localized = digits
        .parse()
        .ok()
        .and_then(|number| {
            localizer
                .locale_for(enzui_core::localization::LocaleCategory::Numeric)
                .format_decimal(&number)
                .ok()
        })
        .unwrap_or(digits);
    enzui_core::localized!(UNITS[unit], "value" => localized)
}

/// A short modification string, or blank when the host read no time.
pub fn format_modified(entry: &FileEntry) -> String {
    match entry.modified {
        Some(t) => format_time(t),
        None => String::new(),
    }
}

/// The visible marker used when local civil-time conversion is unavailable.
pub const LOCAL_TIME_UNAVAILABLE: &str = "—";

/// Formats the local month and day with a clock within the current year, or with the year otherwise.
///
/// The current localizer (the scope, else the environment's locale) supplies
/// the CLDR date and time conventions.
/// Linux timestamps use the system timezone through
/// [`enzui_core::local_time`]. Unsupported targets and conversion failures
/// render [`LOCAL_TIME_UNAVAILABLE`], never a UTC value presented as local.
pub fn format_time(t: std::time::SystemTime) -> String {
    try_format_time(t).unwrap_or_else(|_| LOCAL_TIME_UNAVAILABLE.to_owned())
}

/// Formats a timestamp in the system timezone without hiding conversion errors.
///
/// # Errors
///
/// Returns [`LocalCivilTimeError`] when local civil time is unsupported or the
/// timestamp, timezone offset, conversion, or civil fields are invalid.
pub fn try_format_time(t: std::time::SystemTime) -> Result<String, LocalCivilTimeError> {
    let local = local_civil_time_at(t)?;
    let local_now = current_local_civil_time()?;
    Ok(format_local_time(local, local_now))
}

/// Formats with an injected UTC-offset provider.
///
/// `utc_offset` maps a signed epoch second to the UTC offset in seconds at
/// that instant (positive = east). Both the file timestamp and the "now"
/// reference are converted through it before formatting. Invalid inputs render
/// [`LOCAL_TIME_UNAVAILABLE`].
pub fn format_time_with(t: std::time::SystemTime, utc_offset: fn(i64) -> i32) -> String {
    format_time_with_now(t, std::time::SystemTime::now(), utc_offset)
}

/// Formats with an injected UTC-offset provider and an explicit "now".
///
/// The `now` parameter controls the year-vs-clock format decision; both `t`
/// and `now` use local years after checked offset conversion. Invalid inputs
/// render [`LOCAL_TIME_UNAVAILABLE`].
pub fn format_time_with_now(
    t: std::time::SystemTime,
    now: std::time::SystemTime,
    utc_offset: fn(i64) -> i32,
) -> String {
    try_format_time_with_now(t, now, |seconds| Ok(i64::from(utc_offset(seconds))))
        .unwrap_or_else(|_| LOCAL_TIME_UNAVAILABLE.to_owned())
}

/// Formats with an injected, fallible UTC-offset provider and explicit "now".
///
/// The provider is called once for each instant and returns seconds east of
/// UTC. The output shape matches [`format_time`].
///
/// # Errors
///
/// Propagates resolver failures and returns [`LocalCivilTimeError`] for an
/// unrepresentable instant, offset, or local civil value.
pub fn try_format_time_with_now(
    t: std::time::SystemTime,
    now: std::time::SystemTime,
    mut utc_offset: impl FnMut(i64) -> Result<i64, LocalCivilTimeError>,
) -> Result<String, LocalCivilTimeError> {
    let local = local_civil_time_at_with(t, &mut utc_offset)?;
    let local_now = local_civil_time_at_with(now, utc_offset)?;
    Ok(format_local_time(local, local_now))
}

fn format_local_time(time: LocalCivilTime, now: LocalCivilTime) -> String {
    let localizer = enzui_core::localization::current();
    {
        let locale = localizer.locale_for(enzui_core::localization::LocaleCategory::Time);
        let formatted = if time.date.year() == now.date.year() {
            let clock = enzui_core::time_of_day::TimeOfDay::hm(
                u32::from(time.minute / 60),
                u32::from(time.minute % 60),
            );
            clock.and_then(|clock| {
                let date = locale.format_month_day(time.date).ok()?;
                let time = locale.format_time(clock, false).ok()?;
                Some(localizer.text_with("enzui-file-modified-date-time", &{
                    let mut arguments = enzui_core::localization::Arguments::new();
                    arguments.set("date", date);
                    arguments.set("time", time);
                    arguments
                }))
            })
        } else {
            locale
                .format_date(time.date, enzui_core::localization::DateStyle::Medium)
                .ok()
        };
        formatted.unwrap_or_else(|| LOCAL_TIME_UNAVAILABLE.to_owned())
    }
}

/// A human-readable type name for the Kind column.
///
/// The MIME string itself (`application/vnd.oasis.opendocument.text`) is not a
/// label a user recognises, so common types get a name and everything else
/// falls back to a title-cased subtype.
pub fn kind_label(entry: &FileEntry) -> String {
    match entry.kind {
        EntryKind::Directory => return enzui_core::localization::text("enzui-file-kind-folder"),
        EntryKind::Symlink => return enzui_core::localization::text("enzui-file-kind-link"),
        EntryKind::File => {}
    }
    let Some(mime) = entry.mime.as_deref() else {
        return enzui_core::localization::text("enzui-file-kind-file");
    };
    label_for_mime(mime)
}

/// The human-readable type name for a MIME string.
///
/// Split out from [`kind_label`] so an app carrying its own
/// entry type — as every hand-rolled browser in this tree does
/// — can reach the same table without first converting to a
/// [`FileEntry`]. That conversion is exactly the friction that
/// leaves an app hand-writing a second table, so the seam is
/// deliberate.
///
/// ```
/// use enzui_widgets::file_browser::label_for_mime;
/// assert_eq!(label_for_mime("image/png"), "PNG Image");
/// assert_eq!(label_for_mime("text/plain; charset=utf-8"), "Plain Text");
/// // Unknown types get a readable generated name, never the raw MIME string.
/// // The `x-` registration-tree prefix is dropped — it is IANA bookkeeping,
/// // not something to show a user as the word "X".
/// assert_eq!(label_for_mime("application/x-madeup"), "Madeup");
/// assert_eq!(label_for_mime("text/x-log"), "LOG");
/// ```
pub fn label_for_mime(mime: &str) -> String {
    let base = mime.split(';').next().unwrap_or(mime).trim();
    if let Some(name) = MIME_MESSAGES.iter().find(|(m, _)| *m == base) {
        return enzui_core::localization::text(name.1);
    }
    fallback_mime_label(base)
}

fn fallback_mime_label(mime: &str) -> String {
    let sub = mime.split('/').next_back().unwrap_or(mime);
    // Strip the registration-tree prefix, but never the whole
    // subtype: a type that is only a prefix (`text/x-`) keeps
    // what it has rather than becoming an empty label.
    let sub = sub
        .strip_prefix("x-")
        .or_else(|| sub.strip_prefix("vnd."))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(sub);
    let mut parts: Vec<String> = Vec::new();
    for run in sub.split(['-', '+', '.']) {
        if run.is_empty() {
            continue;
        }
        if run.len() <= 4
            && run
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            parts.push(run.to_ascii_uppercase());
        } else {
            let mut s = String::with_capacity(run.len());
            let mut first = true;
            for ch in run.chars() {
                if first {
                    s.extend(ch.to_uppercase());
                    first = false;
                } else {
                    s.push(ch);
                }
            }
            parts.push(s);
        }
    }
    if parts.is_empty() {
        return enzui_core::localization::text("enzui-file-kind-file");
    }
    parts.join(" ")
}

const MIME_MESSAGES: &[(&str, &str)] = &[
    ("application/gzip", "enzui-file-kind-gzip-archive"),
    ("application/json", "enzui-file-kind-json-document"),
    ("application/msword", "enzui-file-kind-word-document"),
    ("application/octet-stream", "enzui-file-kind-binary-file"),
    ("application/pdf", "enzui-file-kind-pdf-document"),
    ("application/rtf", "enzui-file-kind-rtf-document"),
    ("application/sql", "enzui-file-kind-sql-script"),
    ("application/toml", "enzui-file-kind-toml-configuration"),
    (
        "application/vnd.debian.binary-package",
        "enzui-file-kind-debian-package",
    ),
    (
        "application/vnd.ms-excel",
        "enzui-file-kind-excel-spreadsheet",
    ),
    (
        "application/vnd.ms-powerpoint",
        "enzui-file-kind-powerpoint-presentation",
    ),
    (
        "application/vnd.oasis.opendocument.spreadsheet",
        "enzui-file-kind-odf-spreadsheet",
    ),
    (
        "application/vnd.oasis.opendocument.text",
        "enzui-file-kind-odf-document",
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "enzui-file-kind-powerpoint-presentation",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "enzui-file-kind-excel-spreadsheet",
    ),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "enzui-file-kind-word-document",
    ),
    ("application/vnd.rar", "enzui-file-kind-rar-archive"),
    ("application/x-7z-compressed", "enzui-file-kind-7z-archive"),
    ("application/x-bzip2", "enzui-file-kind-bzip2-archive"),
    (
        "application/x-compressed-tar",
        "enzui-file-kind-compressed-tar-archive",
    ),
    ("application/x-executable", "enzui-file-kind-program"),
    ("application/x-sharedlib", "enzui-file-kind-shared-library"),
    ("application/x-tar", "enzui-file-kind-tar-archive"),
    ("application/x-xz", "enzui-file-kind-xz-archive"),
    ("application/xml", "enzui-file-kind-xml-document"),
    ("application/yaml", "enzui-file-kind-yaml-configuration"),
    ("application/zip", "enzui-file-kind-zip-archive"),
    ("audio/flac", "enzui-file-kind-flac-audio"),
    ("audio/mpeg", "enzui-file-kind-mp3-audio"),
    ("audio/ogg", "enzui-file-kind-ogg-audio"),
    ("audio/vnd.wave", "enzui-file-kind-wav-audio"),
    ("audio/wav", "enzui-file-kind-wav-audio"),
    ("font/ttf", "enzui-file-kind-truetype-font"),
    ("font/woff2", "enzui-file-kind-web-font"),
    ("image/gif", "enzui-file-kind-gif-image"),
    ("image/jpeg", "enzui-file-kind-jpeg-image"),
    ("image/png", "enzui-file-kind-png-image"),
    ("image/svg+xml", "enzui-file-kind-svg-image"),
    ("image/tiff", "enzui-file-kind-tiff-image"),
    ("image/webp", "enzui-file-kind-webp-image"),
    ("inode/directory", "enzui-file-kind-folder"),
    ("inode/symlink", "enzui-file-kind-link"),
    ("text/css", "enzui-file-kind-stylesheet"),
    ("text/csv", "enzui-file-kind-csv-spreadsheet"),
    ("text/html", "enzui-file-kind-html-document"),
    ("text/javascript", "enzui-file-kind-javascript-source"),
    ("text/markdown", "enzui-file-kind-markdown-document"),
    ("text/plain", "enzui-file-kind-plain-text"),
    ("text/rust", "enzui-file-kind-rust-source"),
    ("text/x-csrc", "enzui-file-kind-c-source"),
    ("text/x-go", "enzui-file-kind-go-source"),
    ("text/x-java", "enzui-file-kind-java-source"),
    ("text/x-makefile", "enzui-file-kind-makefile"),
    ("text/x-python", "enzui-file-kind-python-source"),
    ("text/x-rust", "enzui-file-kind-rust-source"),
    ("text/x-shellscript", "enzui-file-kind-shell-script"),
    ("video/matroska", "enzui-file-kind-matroska-video"),
    ("video/mp4", "enzui-file-kind-mp4-video"),
    ("video/webm", "enzui-file-kind-webm-video"),
    ("video/x-matroska", "enzui-file-kind-matroska-video"),
];
