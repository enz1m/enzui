//! File-chooser filters, starting locations, and suggested names.
//!
//! Folder paths are encoded for the portal as NUL-terminated byte arrays; this
//! module only constructs option values and performs no D-Bus I/O.
//!
//! Every builder is bounded. A value over its limit is dropped rather than
//! stored, and the first bound exceeded is recorded as a [`ChooserLimit`] that
//! [`ChooserOptions::refusal`] reports; starting a request with such options
//! fails before the portal is contacted. Limits are byte lengths, so ordinary
//! Unicode titles, names, and patterns are unaffected.

use std::path::{Path, PathBuf};

/// Maximum byte length of a chooser dialog title.
pub const MAX_TITLE_BYTES: usize = 512;

/// Maximum byte length of a suggested `SaveFile` name.
///
/// Matches the usual filesystem `NAME_MAX`, so a remote-supplied filename can
/// never be longer than a name the user could keep.
pub const MAX_CURRENT_NAME_BYTES: usize = 255;

/// Maximum byte length of the starting folder, excluding its encoding NUL.
pub const MAX_CURRENT_FOLDER_BYTES: usize = 4096;

/// Maximum number of filters one request may offer.
pub const MAX_FILTERS: usize = 64;

/// Maximum byte length of one filter's display name.
pub const MAX_FILTER_LABEL_BYTES: usize = 128;

/// Maximum number of patterns one filter may hold.
pub const MAX_FILTER_PATTERNS: usize = 64;

/// Maximum byte length of one glob or MIME pattern.
pub const MAX_FILTER_PATTERN_BYTES: usize = 256;

/// Maximum total byte length of every filter label and pattern in one request.
pub const MAX_FILTER_TOTAL_BYTES: usize = 64 * 1024;

/// The option bound a request exceeded.
///
/// A variant names the field and its limit only; the refused value is never
/// carried, so a diagnostic built from this cannot echo user or remote content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChooserLimit {
    /// The dialog title exceeded [`MAX_TITLE_BYTES`].
    Title,
    /// The suggested name exceeded [`MAX_CURRENT_NAME_BYTES`].
    CurrentName,
    /// The starting folder exceeded [`MAX_CURRENT_FOLDER_BYTES`].
    CurrentFolder,
    /// The request offered more than [`MAX_FILTERS`] filters.
    FilterCount,
    /// A filter label exceeded [`MAX_FILTER_LABEL_BYTES`].
    FilterLabel,
    /// One filter held more than [`MAX_FILTER_PATTERNS`] patterns.
    PatternCount,
    /// A pattern exceeded [`MAX_FILTER_PATTERN_BYTES`].
    PatternBytes,
    /// The labels and patterns together exceeded [`MAX_FILTER_TOTAL_BYTES`].
    FilterTotalBytes,
}

impl ChooserLimit {
    /// The byte or item limit this bound enforces.
    #[must_use]
    pub fn limit(self) -> usize {
        match self {
            Self::Title => MAX_TITLE_BYTES,
            Self::CurrentName => MAX_CURRENT_NAME_BYTES,
            Self::CurrentFolder => MAX_CURRENT_FOLDER_BYTES,
            Self::FilterCount => MAX_FILTERS,
            Self::FilterLabel => MAX_FILTER_LABEL_BYTES,
            Self::PatternCount => MAX_FILTER_PATTERNS,
            Self::PatternBytes => MAX_FILTER_PATTERN_BYTES,
            Self::FilterTotalBytes => MAX_FILTER_TOTAL_BYTES,
        }
    }

    /// A fixed description of the bound, containing no refused value.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Title => "chooser title over its byte limit",
            Self::CurrentName => "suggested name over its byte limit",
            Self::CurrentFolder => "starting folder over its byte limit",
            Self::FilterCount => "too many filters",
            Self::FilterLabel => "filter label over its byte limit",
            Self::PatternCount => "too many patterns in one filter",
            Self::PatternBytes => "filter pattern over its byte limit",
            Self::FilterTotalBytes => "filters over their total byte limit",
        }
    }
}

impl std::fmt::Display for ChooserLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.describe(), self.limit())
    }
}

#[cfg(unix)]
fn folder_byte_len(dir: &Path) -> usize {
    use std::os::unix::ffi::OsStrExt;
    dir.as_os_str().as_bytes().len()
}

#[cfg(not(unix))]
fn folder_byte_len(dir: &Path) -> usize {
    dir.to_string_lossy().len()
}

/// Match rule kind for a file filter; glob and MIME rules use distinct portal wire codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterRule {
    /// A shell glob against the filename, e.g. `*.txt`. Portal kind `0`.
    Glob,
    /// A MIME type, e.g. `text/plain`. Portal kind `1`.
    MimeType,
}

impl FilterRule {
    /// The `u` the portal expects for this rule kind.
    #[must_use]
    pub fn code(self) -> u32 {
        match self {
            Self::Glob => 0,
            Self::MimeType => 1,
        }
    }
}

/// A named chooser filter containing one or more glob or MIME patterns.
///
/// A label or pattern over its limit is dropped rather than stored, and the
/// filter then reports a [`refusal`](Self::refusal) that propagates into the
/// [`ChooserOptions`] it is added to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    name: String,
    patterns: Vec<(FilterRule, String)>,
    // Label plus pattern bytes, for the request-wide filter budget.
    bytes: usize,
    refused: Option<ChooserLimit>,
}

impl FileFilter {
    /// A filter named `name` with no patterns yet.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        if name.len() > MAX_FILTER_LABEL_BYTES {
            return Self {
                name: String::new(),
                patterns: Vec::new(),
                bytes: 0,
                refused: Some(ChooserLimit::FilterLabel),
            };
        }
        let bytes = name.len();
        Self {
            name,
            patterns: Vec::new(),
            bytes,
            refused: None,
        }
    }

    // Record the first bound exceeded and drop the offending pattern.
    fn push(mut self, rule: FilterRule, pattern: String) -> Self {
        if self.refused.is_some() {
            return self;
        }
        if pattern.len() > MAX_FILTER_PATTERN_BYTES {
            self.refused = Some(ChooserLimit::PatternBytes);
            return self;
        }
        if self.patterns.len() >= MAX_FILTER_PATTERNS {
            self.refused = Some(ChooserLimit::PatternCount);
            return self;
        }
        self.bytes = self.bytes.saturating_add(pattern.len());
        self.patterns.push((rule, pattern));
        self
    }

    /// Adds a shell-glob pattern, e.g. `*.txt`.
    #[must_use]
    pub fn glob(self, pattern: impl Into<String>) -> Self {
        self.push(FilterRule::Glob, pattern.into())
    }

    /// Adds a MIME-type pattern, e.g. `text/plain`.
    #[must_use]
    pub fn mime(self, mime: impl Into<String>) -> Self {
        self.push(FilterRule::MimeType, mime.into())
    }

    /// Builds one filter from extensions, accepting forms such as `txt`, `.txt`,
    /// and `*.txt`.
    #[must_use]
    pub fn extensions<I, S>(name: impl Into<String>, exts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut f = Self::new(name);
        for e in exts {
            let e = e.as_ref();
            let bare = e.trim_start_matches('*').trim_start_matches('.');
            f = f.push(FilterRule::Glob, format!("*.{bare}"));
        }
        f
    }

    /// The bound this filter exceeded, or `None` when it is within every limit.
    #[must_use]
    pub fn refusal(&self) -> Option<ChooserLimit> {
        self.refused
    }

    /// An "all files" filter, `*`.
    #[must_use]
    pub fn all_files(name: impl Into<String>) -> Self {
        Self::new(name).glob("*")
    }

    /// The filter's display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `(kind, pattern)` pairs, in order, ready for the `a(us)` field.
    #[must_use]
    pub fn patterns(&self) -> &[(FilterRule, String)] {
        &self.patterns
    }

    /// The wire form of the patterns: `(u, s)` pairs with the rule codes
    /// resolved.
    #[must_use]
    pub fn wire_patterns(&self) -> Vec<(u32, String)> {
        self.patterns
            .iter()
            .map(|(r, p)| (r.code(), p.clone()))
            .collect()
    }

    /// Returns whether the filter has no patterns and therefore matches no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Optional title, filters, starting folder, and suggested name for a file chooser.
///
/// Each builder enforces its own bound: an oversize value is dropped instead of
/// stored, and [`refusal`](Self::refusal) then reports the first bound exceeded
/// so the request can be refused before the portal is contacted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChooserOptions {
    title: Option<String>,
    filters: Vec<FileFilter>,
    current_filter: Option<FileFilter>,
    current_folder: Option<PathBuf>,
    current_name: Option<String>,
    // Running label-plus-pattern total across `filters`.
    filter_bytes: usize,
    refused: Option<ChooserLimit>,
}

impl ChooserOptions {
    /// Options with nothing set — the portal's own defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // First bound exceeded wins, so a later violation cannot rename the refusal.
    fn refuse(&mut self, limit: ChooserLimit) {
        if self.refused.is_none() {
            self.refused = Some(limit);
        }
    }

    /// Sets the dialog title, or records [`ChooserLimit::Title`] and keeps none
    /// when it is longer than [`MAX_TITLE_BYTES`].
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        let title = title.into();
        if title.len() > MAX_TITLE_BYTES {
            self.refuse(ChooserLimit::Title);
            return self;
        }
        self.title = Some(title);
        self
    }

    /// Adds a non-empty filter to the file-type dropdown.
    ///
    /// Filters with no patterns are ignored. A filter carrying its own refusal,
    /// one past [`MAX_FILTERS`], or one that would push the labels and patterns
    /// past [`MAX_FILTER_TOTAL_BYTES`] is recorded as a refusal and not added.
    #[must_use]
    pub fn filter(mut self, filter: FileFilter) -> Self {
        if let Some(limit) = filter.refused {
            self.refuse(limit);
            return self;
        }
        if filter.is_empty() {
            return self;
        }
        if self.filters.len() >= MAX_FILTERS {
            self.refuse(ChooserLimit::FilterCount);
            return self;
        }
        let total = self.filter_bytes.saturating_add(filter.bytes);
        if total > MAX_FILTER_TOTAL_BYTES {
            self.refuse(ChooserLimit::FilterTotalBytes);
            return self;
        }
        self.filter_bytes = total;
        self.filters.push(filter);
        self
    }

    /// Pre-selects `filter` in the dropdown, adding it to the list if absent.
    ///
    /// A filter the list refuses is not pre-selected either.
    #[must_use]
    pub fn current_filter(mut self, filter: FileFilter) -> Self {
        if let Some(limit) = filter.refused {
            self.refuse(limit);
            return self;
        }
        if filter.is_empty() {
            return self;
        }
        if !self.filters.contains(&filter) {
            let listed = self.filters.len();
            self = self.filter(filter.clone());
            if self.filters.len() == listed {
                return self;
            }
        }
        self.current_filter = Some(filter);
        self
    }

    /// Starts the chooser in `dir`, or records [`ChooserLimit::CurrentFolder`]
    /// and keeps none when its encoding exceeds [`MAX_CURRENT_FOLDER_BYTES`].
    #[must_use]
    pub fn current_folder(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        if folder_byte_len(&dir) > MAX_CURRENT_FOLDER_BYTES {
            self.refuse(ChooserLimit::CurrentFolder);
            return self;
        }
        self.current_folder = Some(dir);
        self
    }

    /// Suggests a filename. `SaveFile` only; the portal ignores it for
    /// `OpenFile`.
    ///
    /// A name longer than [`MAX_CURRENT_NAME_BYTES`] records
    /// [`ChooserLimit::CurrentName`] and is not retained, so a remote-supplied
    /// filename is neither stored nor sent.
    #[must_use]
    pub fn current_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if name.len() > MAX_CURRENT_NAME_BYTES {
            self.refuse(ChooserLimit::CurrentName);
            return self;
        }
        self.current_name = Some(name);
        self
    }

    /// The first bound these options exceeded, or `None` when every value fits.
    ///
    /// Starting a request with a refusal fails before the portal is contacted.
    #[must_use]
    pub fn refusal(&self) -> Option<ChooserLimit> {
        self.refused
    }

    /// The dialog title, if set.
    #[must_use]
    pub fn title_str(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The filters, in the order they will appear.
    #[must_use]
    pub fn filters(&self) -> &[FileFilter] {
        &self.filters
    }

    /// The pre-selected filter, if any.
    #[must_use]
    pub fn selected_filter(&self) -> Option<&FileFilter> {
        self.current_filter.as_ref()
    }

    /// The starting directory, if set.
    #[must_use]
    pub fn folder(&self) -> Option<&Path> {
        self.current_folder.as_deref()
    }

    /// The suggested filename, if set.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.current_name.as_deref()
    }

    /// Encodes the starting folder as the portal `ay` value with a trailing NUL.
    ///
    /// Returns `None` when no folder is configured.
    #[must_use]
    pub fn current_folder_bytes(&self) -> Option<Vec<u8>> {
        let dir = self.current_folder.as_ref()?;
        #[cfg(unix)]
        let mut bytes = {
            use std::os::unix::ffi::OsStrExt;
            dir.as_os_str().as_bytes().to_vec()
        };
        #[cfg(not(unix))]
        let mut bytes = dir.to_string_lossy().into_owned().into_bytes();
        bytes.push(0);
        Some(bytes)
    }

    /// The filters in wire form: `(name, [(kind, pattern)])` per filter.
    #[must_use]
    pub fn wire_filters(&self) -> Vec<(String, Vec<(u32, String)>)> {
        self.filters
            .iter()
            .map(|f| (f.name.clone(), f.wire_patterns()))
            .collect()
    }

    /// The pre-selected filter in wire form, if any.
    #[must_use]
    pub fn wire_current_filter(&self) -> Option<(String, Vec<(u32, String)>)> {
        self.current_filter
            .as_ref()
            .map(|f| (f.name.clone(), f.wire_patterns()))
    }

    /// Whether these options would add nothing to the request.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.filters.is_empty()
            && self.current_filter.is_none()
            && self.current_folder.is_none()
            && self.current_name.is_none()
    }
}

/// What to tell the user when a file chooser did not run, translated
/// through the current localization scope; `None` when it completed or the
/// user cancelled it, which need no message.
#[must_use]
pub fn failure_text(
    outcome: &crate::portal::PortalOutcome<crate::portal::op::FileResult>,
) -> Option<String> {
    use crate::portal::PortalOutcome;
    let key = match outcome {
        PortalOutcome::Completed(_) | PortalOutcome::Cancelled(_) => return None,
        PortalOutcome::Unavailable => "enzui-file-chooser-unavailable",
        PortalOutcome::Unsupported => "enzui-file-chooser-unsupported",
        PortalOutcome::NotDeclared => "enzui-file-chooser-not-declared",
        PortalOutcome::Denied => "enzui-file-chooser-denied",
        PortalOutcome::ParentUnavailable => "enzui-file-chooser-parent-unavailable",
        PortalOutcome::Failed => "enzui-file-chooser-failed",
    };
    Some(crate::localization::text(key))
}
