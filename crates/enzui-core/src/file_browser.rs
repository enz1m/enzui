//! Headless file-browser listing, classification, filtering, and sorting.
//!
//! The model performs no I/O: callers supply entries and enforce filesystem policy. Defaults are folders first, ascending name order in the `LC_COLLATE` language with digit runs by value, and hidden files excluded. Name queries match without case or accents ([`crate::text_fold`]), and [`FileBrowserModel::visible`] filters and sorts the full listing on each call.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::drag::ItemId;
use crate::list_selection::ClickModel;

use enzui_icons::file_type::{FileTypeTable, icon_for_mime};
use enzui_icons::mime::Certainty;

/// Filesystem kind supplied by the listing host.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EntryKind {
    /// A directory.
    Directory,
    /// A regular file.
    File,
    /// A symbolic link whose target is not a directory, or is broken.
    ///
    /// Hosts report links to directories as [`EntryKind::Directory`] so navigation and folder grouping follow target behavior.
    Symlink,
}

/// A host-supplied entry in a file-browser listing.
///
/// MIME type and glyph are derived by [`FileEntry::classify`], while [`FileEntry::icon_name`] also supports unclassified entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// The full path. Used as the entry's stable identity.
    pub path: PathBuf,
    /// The display name (the base name).
    pub name: String,
    /// What the entry is.
    pub kind: EntryKind,
    /// Size in bytes. Meaningless for a directory; hosts conventionally pass 0.
    pub size: u64,
    /// Last-modified time, when the host could read one.
    pub modified: Option<SystemTime>,
    /// Resolved MIME type, or `None` until classification; [`FileEntry::icon_name`] can classify lazily for display.
    pub mime: Option<String>,
    /// How the MIME type was determined. `None` while `mime` is `None`.
    pub certainty: Option<Certainty>,
}

impl FileEntry {
    /// A regular file entry.
    pub fn file(path: impl Into<PathBuf>, name: impl Into<String>, size: u64) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            kind: EntryKind::File,
            size,
            modified: None,
            mime: None,
            certainty: None,
        }
    }

    /// A directory entry.
    pub fn directory(path: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            kind: EntryKind::Directory,
            size: 0,
            modified: None,
            mime: None,
            certainty: None,
        }
    }

    /// A symbolic link whose target is not a directory.
    pub fn symlink(path: impl Into<PathBuf>, name: impl Into<String>, size: u64) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            kind: EntryKind::Symlink,
            size,
            modified: None,
            mime: None,
            certainty: None,
        }
    }

    /// Builder: attach a modification time.
    #[must_use]
    pub fn with_modified(mut self, modified: SystemTime) -> Self {
        self.modified = Some(modified);
        self
    }

    /// Returns whether the display name follows the Unix dotfile convention.
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// Returns whether activation should navigate into this entry.
    ///
    /// Only [`EntryKind::Directory`] is navigable; hosts classify directory symlinks as directories.
    pub fn is_navigable(&self) -> bool {
        matches!(self.kind, EntryKind::Directory)
    }

    /// Populates MIME type and certainty from `table`.
    ///
    /// Directories receive `inode/directory` instead of filename-based classification.
    pub fn classify(&mut self, table: &FileTypeTable) {
        match self.kind {
            EntryKind::Directory => {
                self.mime = Some("inode/directory".to_owned());
                self.certainty = Some(Certainty::FilenameLiteral);
            }
            EntryKind::Symlink => {
                self.mime = Some("inode/symlink".to_owned());
                self.certainty = Some(Certainty::FilenameLiteral);
            }
            EntryKind::File => {
                let c = table.classify(&self.name);
                self.mime = Some(c.mime);
                self.certainty = Some(c.certainty);
            }
        }
    }

    /// Returns the bundled Lucide glyph for this entry.
    ///
    /// Directory and symlink kind takes precedence over MIME; unclassified regular files are classified against the host table on demand.
    pub fn icon_name(&self) -> &'static str {
        match self.kind {
            EntryKind::Directory => FileTypeTable::DIRECTORY_ICON,
            EntryKind::Symlink => FileTypeTable::SYMLINK_ICON,
            EntryKind::File => match &self.mime {
                Some(mime) => icon_for_mime(mime),
                None => FileTypeTable::host().icon_name(&self.name),
            },
        }
    }

    /// Returns the stable selection and drag id derived losslessly from the native path.
    pub fn id(&self) -> ItemId {
        ItemId::from_path(&self.path)
    }

    /// Returns the ASCII-lowercased extension without its dot, or `None` for names without one and dotfiles with no second dot.
    pub fn extension(&self) -> Option<String> {
        let (stem, extension) = self.name.rsplit_once('.')?;
        if stem.is_empty() || extension.is_empty() {
            return None;
        }
        Some(extension.to_ascii_lowercase())
    }
}

/// Purpose-level filtering applied independently of hidden-file and query filters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EntryFilter {
    /// Everything the host listed.
    #[default]
    All,
    /// Directories only — the folder-picker case.
    DirectoriesOnly,
    /// All non-directory entries, including file symlinks.
    FilesOnly,
    /// Directories plus non-directory entries whose ASCII-lowercased extension is allowed; directories remain visible for navigation.
    Extensions(Vec<String>),
}

impl EntryFilter {
    /// Builds an extension allowlist, ASCII-lowercasing entries and stripping an optional leading dot.
    pub fn extensions<I, S>(exts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        EntryFilter::Extensions(
            exts.into_iter()
                .map(|e| e.as_ref().trim_start_matches('.').to_ascii_lowercase())
                .collect(),
        )
    }

    /// Whether `entry` passes this filter.
    pub fn accepts(&self, entry: &FileEntry) -> bool {
        let is_dir = matches!(entry.kind, EntryKind::Directory);
        match self {
            EntryFilter::All => true,
            EntryFilter::DirectoriesOnly => is_dir,
            EntryFilter::FilesOnly => !is_dir,
            EntryFilter::Extensions(allowed) => {
                is_dir || entry.extension().is_some_and(|ext| allowed.contains(&ext))
            }
        }
    }
}

/// What a listing is ordered by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortKey {
    /// The display name, compared naturally (see [`natural_cmp`]).
    #[default]
    Name,
    /// Size in bytes.
    Size,
    /// Last-modified time; in ascending order, entries with no timestamp sort first.
    Modified,
    /// The resolved MIME type, compared naturally. Groups files of a kind.
    Kind,
}

impl SortKey {
    /// A short column label.
    pub fn label(self) -> String {
        match self {
            SortKey::Name => crate::localization::text("enzui-file-browser-name"),
            SortKey::Size => crate::localization::text("enzui-file-browser-size"),
            SortKey::Modified => crate::localization::text("enzui-file-browser-modified"),
            SortKey::Kind => crate::localization::text("enzui-file-browser-type"),
        }
    }
}

/// Ascending or descending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortDirection {
    /// A→Z, smallest first, oldest first.
    #[default]
    Ascending,
    /// The reverse.
    Descending,
}

impl SortDirection {
    /// The other direction.
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }

    /// Returns whether the direction is ascending.
    pub fn is_ascending(self) -> bool {
        matches!(self, SortDirection::Ascending)
    }
}

/// A headless model that derives filtered and sorted rows from a host-supplied listing without performing I/O.
#[derive(Debug, Clone)]
pub struct FileBrowserModel {
    entries: Vec<FileEntry>,
    show_hidden: bool,
    filter: EntryFilter,
    query: String,
    sort_key: SortKey,
    direction: SortDirection,
    folders_first: bool,
    /// Sorts names in the user's language; `None` falls back to [`natural_cmp`].
    collator: Option<std::sync::Arc<crate::localization::Collator>>,
    /// The visible rows' indices in display order, made on first use after
    /// any change to what decides them.
    visible: std::sync::OnceLock<Vec<usize>>,
}

impl Default for FileBrowserModel {
    fn default() -> Self {
        Self::new()
    }
}

impl FileBrowserModel {
    /// Creates an empty model with folders first, ascending name order in the
    /// `LC_COLLATE` language (digit runs by value), hidden files off, and no
    /// purpose or query filter.
    pub fn new() -> Self {
        let collation = crate::localization::category_locale_from_env(
            crate::localization::LocaleCategory::Collate,
            |key| std::env::var(key).ok(),
        )
        .unwrap_or_default();
        let mut model = Self {
            entries: Vec::new(),
            show_hidden: false,
            filter: EntryFilter::All,
            query: String::new(),
            sort_key: SortKey::Name,
            direction: SortDirection::Ascending,
            folders_first: true,
            collator: None,
            visible: std::sync::OnceLock::new(),
        };
        model.set_collation_locale(&collation);
        model
    }

    /// Sorts names in `locale`'s order, digit runs by value.
    pub fn set_collation_locale(&mut self, locale: &crate::localization::Locale) {
        self.visible = std::sync::OnceLock::new();
        self.collator = locale
            .collator(crate::localization::Collation::NATURAL)
            .ok()
            .map(std::sync::Arc::new);
    }

    /// Compares two names: the locale's order, then [`natural_cmp`] so
    /// distinct names never compare equal.
    fn name_cmp(&self, a: &str, b: &str) -> Ordering {
        match &self.collator {
            Some(collator) => collator.compare(a, b).then_with(|| natural_cmp(a, b)),
            None => natural_cmp(a, b),
        }
    }

    /// Replaces the listing and classifies every entry with the host MIME table.
    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.set_entries_with(entries, FileTypeTable::host());
    }

    /// Replaces the listing and classifies entries with an explicit MIME table.
    pub fn set_entries_with(&mut self, mut entries: Vec<FileEntry>, table: &FileTypeTable) {
        self.visible = std::sync::OnceLock::new();
        for e in &mut entries {
            e.classify(table);
        }
        self.entries = entries;
    }

    /// Returns the full classified listing without visibility filtering or sorting.
    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    /// Whether dotfiles are shown.
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Show or hide dotfiles.
    pub fn set_show_hidden(&mut self, show: bool) {
        self.visible = std::sync::OnceLock::new();
        self.show_hidden = show;
    }

    /// The active purpose filter.
    pub fn filter(&self) -> &EntryFilter {
        &self.filter
    }

    /// Set the purpose filter.
    pub fn set_filter(&mut self, filter: EntryFilter) {
        self.visible = std::sync::OnceLock::new();
        self.filter = filter;
    }

    /// The active name query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Sets a case-insensitive substring query; an empty query disables name filtering.
    ///
    /// The query is stored verbatim for redisplay and normalized only during [`Self::visible`].
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.visible = std::sync::OnceLock::new();
        self.query = query.into();
    }

    /// The active sort key.
    pub fn sort_key(&self) -> SortKey {
        self.sort_key
    }

    /// The active sort direction.
    pub fn direction(&self) -> SortDirection {
        self.direction
    }

    /// Whether directories are grouped ahead of files.
    pub fn folders_first(&self) -> bool {
        self.folders_first
    }

    /// Group directories ahead of files, or do not.
    pub fn set_folders_first(&mut self, folders_first: bool) {
        self.visible = std::sync::OnceLock::new();
        self.folders_first = folders_first;
    }

    /// Sets the sort key and direction without toggling when the key is already active.
    pub fn set_sort(&mut self, key: SortKey, direction: SortDirection) {
        self.visible = std::sync::OnceLock::new();
        self.sort_key = key;
        self.direction = direction;
    }

    /// Toggles direction for the active key, or selects a different key in ascending order.
    pub fn toggle_sort(&mut self, key: SortKey) {
        self.visible = std::sync::OnceLock::new();
        if self.sort_key == key {
            self.direction = self.direction.flipped();
        } else {
            self.sort_key = key;
            self.direction = SortDirection::Ascending;
        }
    }

    /// Returns rows after hidden, purpose, and normalized-query filtering, sorted in display order.
    pub fn visible(&self) -> Vec<&FileEntry> {
        self.visible_order()
            .iter()
            .map(|&index| &self.entries[index])
            .collect()
    }

    /// The visible rows' indices in display order, kept until a setter
    /// changes what decides them, so a view rebuilt every frame neither
    /// refolds names nor sorts again.
    fn visible_order(&self) -> &[usize] {
        self.visible.get_or_init(|| self.sorted_order())
    }

    fn sorted_order(&self) -> Vec<usize> {
        let mut rows: Vec<(usize, &FileEntry)> = self.passing().collect();

        let folders_first = self.folders_first;
        let key = self.sort_key;
        let ascending = self.direction.is_ascending();
        rows.sort_by(|(_, a), (_, b)| {
            if folders_first {
                let group = dir_rank(a).cmp(&dir_rank(b));
                if group != Ordering::Equal {
                    // Descending order reverses rows within groups but never reverses the folders-first grouping.
                    return group;
                }
            }
            let cmp = match key {
                SortKey::Name => self.name_cmp(&a.name, &b.name),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Modified => a.modified.cmp(&b.modified),
                SortKey::Kind => natural_cmp(mime_of(a), mime_of(b)),
            };
            // Break non-name-key ties by name so input order cannot reshuffle equal rows.
            let cmp = cmp.then_with(|| self.name_cmp(&a.name, &b.name));
            if ascending { cmp } else { cmp.reverse() }
        });
        rows.into_iter().map(|(index, _)| index).collect()
    }

    /// The number of visible rows.
    pub fn visible_len(&self) -> usize {
        self.visible_order().len()
    }

    /// The entries the hidden, type and query filters let through, with their
    /// indices, in listing order.
    fn passing(&self) -> impl Iterator<Item = (usize, &FileEntry)> {
        // Normalize only the name comparison; preserve original name and path bytes as filesystem identity.
        let query = crate::palette::normalize(&self.query);
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.show_hidden || !e.is_hidden())
            .filter(|(_, e)| self.filter.accepts(e))
            .filter(move |(_, e)| {
                query.is_empty() || crate::palette::normalize(&e.name).contains(&query)
            })
    }

    /// Builds a [`ClickModel`] whose ids follow the current visible display order.
    pub fn click_model(&self) -> ClickModel {
        ClickModel::new(self.visible().into_iter().map(FileEntry::id))
    }

    /// Finds an entry in the full listing by path, regardless of active filters.
    pub fn entry_at(&self, path: &Path) -> Option<&FileEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

fn mime_of(e: &FileEntry) -> &str {
    e.mime.as_deref().unwrap_or("application/octet-stream")
}

fn dir_rank(e: &FileEntry) -> u8 {
    match e.kind {
        EntryKind::Directory => 0,
        _ => 1,
    }
}

/// Compares names naturally: digit runs by saturating numeric value and other text case-insensitively.
///
/// Leading-zero run length and then raw bytes break ties, so distinct names never compare equal.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    // Retain the first equal-value digit-run length difference as a leading-zero tiebreak.
    let mut zero_tiebreak = Ordering::Equal;

    loop {
        match (ai.peek(), bi.peek()) {
            (None, None) => break,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(&ac), Some(&bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let (an, alen) = take_number(&mut ai);
                    let (bn, blen) = take_number(&mut bi);
                    match an.cmp(&bn) {
                        Ordering::Equal => {
                            if zero_tiebreak == Ordering::Equal {
                                zero_tiebreak = alen.cmp(&blen);
                            }
                        }
                        other => return other,
                    }
                } else {
                    let al = ac.to_lowercase().next().unwrap_or(ac);
                    let bl = bc.to_lowercase().next().unwrap_or(bc);
                    match al.cmp(&bl) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }

    // Use the leading-zero tiebreak, then raw bytes, to keep distinct names totally ordered.
    zero_tiebreak.then_with(|| a.cmp(b))
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> (u128, usize) {
    let mut value: u128 = 0;
    let mut len = 0usize;
    while let Some(&c) = it.peek() {
        let Some(d) = c.to_digit(10) else { break };
        value = value.saturating_mul(10).saturating_add(u128::from(d));
        len += 1;
        it.next();
    }
    (value, len)
}
