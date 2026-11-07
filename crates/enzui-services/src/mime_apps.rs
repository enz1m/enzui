//! The desktop's default applications for MIME types, read and written under an
//! explicit user and session context.
//!
//! [`MimeApps`] implements the freedesktop *Association between MIME types and
//! applications* and *Desktop Entry* specifications in process. It resolves
//! the default application for a type, lists candidate applications with
//! their metadata, and atomically sets a user default, always against a
//! [`MimeContext`] the caller captured (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, their
//! search lists and `XDG_CURRENT_DESKTOP`).
//!
//! Every query reads the files again, so a default the user changed while the
//! app is open is seen at once. Nothing parses `Exec=`, so entries with quoted
//! or spaced executable paths are read like any other. Files above
//! [`MAX_FILE_BYTES`] and scans above [`MAX_DESKTOP_FILES`] are refused rather
//! than read partially, and scans check a [`CancelToken`] per entry.
//!
//! [`MimeApps::set_default`] writes only the user's own `mimeapps.list` and
//! establishes its association as well as its `[Default Applications]` entry.
//! A declared handler can be used as a fallback when no explicit default exists.

use enzui_core::cancellation::CancelToken;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The largest `mimeapps.list`, desktop entry or MIME database file read.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The most desktop entries one scan examines.
pub const MAX_DESKTOP_FILES: usize = 20_000;

/// The directories and desktops a lookup runs against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeContext {
    /// `XDG_CONFIG_HOME`, where the user's own `mimeapps.list` lives.
    pub config_home: PathBuf,
    /// `XDG_CONFIG_DIRS`, most important first.
    pub config_dirs: Vec<PathBuf>,
    /// `XDG_DATA_HOME`.
    pub data_home: PathBuf,
    /// `XDG_DATA_DIRS`, most important first.
    pub data_dirs: Vec<PathBuf>,
    /// `XDG_CURRENT_DESKTOP` names, most important first.
    pub desktops: Vec<String>,
}

impl MimeContext {
    /// The context of this process's session, with the specification's
    /// defaults for unset variables. `None` without `HOME`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let home = PathBuf::from(var("HOME")?);
        let list = |name: &str, default: &str| -> Vec<PathBuf> {
            std::env::split_paths(&var(name).unwrap_or_else(|| default.into()))
                .filter(|path| path.is_absolute())
                .collect()
        };
        Some(Self {
            config_home: var("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from),
            config_dirs: list("XDG_CONFIG_DIRS", "/etc/xdg"),
            data_home: var("XDG_DATA_HOME")
                .map_or_else(|| home.join(".local/share"), PathBuf::from),
            data_dirs: list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            desktops: var("XDG_CURRENT_DESKTOP")
                .and_then(|value| value.into_string().ok())
                .map(|value| {
                    value
                        .split(':')
                        .filter(|name| !name.is_empty())
                        .map(str::to_ascii_lowercase)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn association_files(&self) -> Result<Vec<AssociationFile>, MimeError> {
        let mut files = Vec::new();
        let mut directory = |dir: &Path, data_root| -> Result<(), MimeError> {
            for desktop in &self.desktops {
                files.push(AssociationFile {
                    groups: read_keyfile(&dir.join(format!("{desktop}-mimeapps.list")))?
                        .unwrap_or_default(),
                    data_root,
                    defaults_only: true,
                });
            }
            files.push(AssociationFile {
                groups: read_keyfile(&dir.join("mimeapps.list"))?.unwrap_or_default(),
                data_root,
                defaults_only: false,
            });
            Ok(())
        };
        directory(&self.config_home, None)?;
        for dir in &self.config_dirs {
            directory(dir, None)?;
        }
        for (rank, root) in self.data_roots().enumerate() {
            directory(&root.join("applications"), Some(rank))?;
        }
        Ok(files)
    }

    fn data_roots(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.data_home.as_path()).chain(self.data_dirs.iter().map(PathBuf::as_path))
    }
}

/// One installed application, from its desktop entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    /// The desktop-file ID, e.g. `org.gnome.TextEditor.desktop`.
    pub id: String,
    /// The untranslated `Name=`.
    pub name: String,
    /// `Icon=`, a theme name or an absolute path.
    pub icon: Option<String>,
    /// The entry the ID resolved to.
    pub path: PathBuf,
    /// `MimeType=`.
    pub mime_types: Vec<String>,
    /// `NoDisplay=true`: launchable, but not listed in menus.
    pub no_display: bool,
}

/// The default application for a type and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultApp {
    /// The application.
    pub app: DesktopApp,
    /// Whether a `[Default Applications]` entry chose it, rather than the
    /// first candidate standing in for a missing default.
    pub explicit: bool,
}

/// Why a MIME lookup or write failed.
#[derive(Debug)]
pub enum MimeError {
    /// The scan was cancelled.
    Cancelled,
    /// A MIME type that is not `type/subtype`.
    InvalidMimeType,
    /// A desktop ID that is not an installed, non-hidden `*.desktop` entry.
    UnknownApplication(String),
    /// A file above [`MAX_FILE_BYTES`], or a scan above [`MAX_DESKTOP_FILES`].
    TooLarge(PathBuf),
    /// Reading or writing failed.
    Io(PathBuf, std::io::Error),
}

impl std::fmt::Display for MimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the application lookup was cancelled"),
            Self::InvalidMimeType => f.write_str("not a MIME type"),
            Self::UnknownApplication(id) => write!(f, "{id} is not an installed application"),
            Self::TooLarge(path) => write!(f, "{} is too large to read", path.display()),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for MimeError {}

/// Default applications and candidates under one [`MimeContext`].
#[derive(Debug, Clone)]
pub struct MimeApps {
    context: MimeContext,
}

impl MimeApps {
    /// Lookups against `context`.
    #[must_use]
    pub fn new(context: MimeContext) -> Self {
        Self { context }
    }

    /// The context lookups run against.
    #[must_use]
    pub fn context(&self) -> &MimeContext {
        &self.context
    }

    /// The first associated default for `mime`, or its preferred handler.
    ///
    /// All files are checked before falling back to a handler for the same
    /// type. More-specific handlers take precedence over parent defaults;
    /// aliases share their canonical type's precedence.
    ///
    /// # Errors
    ///
    /// An invalid type, cancellation, or an unreadable or oversized file.
    pub fn default_for(
        &self,
        mime: &str,
        cancel: &CancelToken,
    ) -> Result<Option<DefaultApp>, MimeError> {
        let installed = self.installed(cancel)?;
        let types = self.lineage(mime)?;
        let files = self.context.association_files()?;
        let candidates = candidate_levels(&installed, &types, &files);
        for (level, equivalents) in types.iter().enumerate() {
            for file in &files {
                for mime in equivalents {
                    for id in file.ids("Default Applications", mime) {
                        if let Some(app) = candidates[level..]
                            .iter()
                            .flatten()
                            .find(|app| app.id == id)
                        {
                            return Ok(Some(DefaultApp {
                                app: app.clone(),
                                explicit: true,
                            }));
                        }
                    }
                }
            }
            if let Some(app) = candidates[level].first() {
                return Ok(Some(DefaultApp {
                    app: app.clone(),
                    explicit: false,
                }));
            }
        }
        Ok(None)
    }

    /// The applications that can open `mime`, most preferred first: added
    /// associations and declared handlers in directory precedence, without
    /// removed associations or hidden entries. More-specific types precede
    /// their parents; aliases are equivalent. Desktop-specific files supply
    /// defaults only, and lower directories cannot alter a user override.
    ///
    /// # Errors
    ///
    /// As [`Self::default_for`].
    pub fn candidates(
        &self,
        mime: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<DesktopApp>, MimeError> {
        let installed = self.installed(cancel)?;
        let types = self.lineage(mime)?;
        let files = self.context.association_files()?;
        let mut seen = HashSet::new();
        Ok(candidate_levels(&installed, &types, &files)
            .into_iter()
            .flatten()
            .filter(|app| seen.insert(app.id.clone()))
            .collect())
    }

    /// The installed application with desktop ID `id`.
    ///
    /// # Errors
    ///
    /// As [`Self::default_for`].
    pub fn application(
        &self,
        id: &str,
        cancel: &CancelToken,
    ) -> Result<Option<DesktopApp>, MimeError> {
        Ok(self
            .installed(cancel)?
            .remove(id)
            .map(|located| located.app))
    }

    /// Makes `desktop_id` the user's default for `mime`, in
    /// `$XDG_CONFIG_HOME/mimeapps.list`, replacing the file atomically and
    /// establishing an added association and reversing any user removal for
    /// the same type. Unrelated entries and comments are preserved.
    ///
    /// # Errors
    ///
    /// An invalid type, an application that is not installed, cancellation,
    /// or a read or write failure.
    pub fn set_default(
        &self,
        mime: &str,
        desktop_id: &str,
        cancel: &CancelToken,
    ) -> Result<(), MimeError> {
        let types = self.lineage(mime)?;
        let equivalents = &types[0];
        let mime = &equivalents[0];
        if !self.installed(cancel)?.contains_key(desktop_id) {
            return Err(MimeError::UnknownApplication(desktop_id.to_owned()));
        }
        let path = self.context.config_home.join("mimeapps.list");
        std::fs::create_dir_all(&self.context.config_home)
            .map_err(|error| MimeError::Io(self.context.config_home.clone(), error))?;
        // Writers in this process take turns; another process's write between
        // this read and the publication is noticed and merged into again.
        static WRITERS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _turn = WRITERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for _ in 0..MERGE_ATTEMPTS {
            let current = read_bounded(&path)?.unwrap_or_default();
            let updated = default_entries(&current, mime, desktop_id, equivalents);
            let mut stage = enzui_security::tempstage::TempStage::create_document(&path)
                .map_err(|error| MimeError::Io(path.clone(), error))?;
            stage
                .write_all(updated.as_bytes())
                .map_err(|error| MimeError::Io(path.clone(), error))?;
            if read_bounded(&path)?.unwrap_or_default() != current {
                // Dropping the stage removes it; merge into the newer file.
                continue;
            }
            return stage
                .commit()
                .map_err(|error| MimeError::Io(path.clone(), error));
        }
        Err(MimeError::Io(
            path,
            std::io::Error::other("mimeapps.list kept changing while it was updated"),
        ))
    }

    fn lineage(&self, mime: &str) -> Result<Vec<Vec<String>>, MimeError> {
        if !valid_mime(mime) {
            return Err(MimeError::InvalidMimeType);
        }
        let mut aliases = HashMap::new();
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        for root in self.context.data_roots() {
            if let Some(text) = read_bounded(&root.join("mime/aliases"))? {
                for (alias, canonical) in pair_lines(&text) {
                    if valid_mime(&alias) && valid_mime(&canonical) {
                        let _ = aliases.entry(alias).or_insert(canonical);
                    }
                }
            }
            if let Some(text) = read_bounded(&root.join("mime/subclasses"))? {
                for (child, parent) in pair_lines(&text) {
                    if valid_mime(&child) && valid_mime(&parent) {
                        parents.entry(child).or_default().push(parent);
                    }
                }
            }
        }
        let mut lineage = Vec::new();
        let mut pending = vec![canonical_mime(mime, &aliases)];
        let mut index = 0;
        while index < pending.len() {
            let canonical = &pending[index];
            let mut equivalents: Vec<_> = aliases
                .keys()
                .filter(|alias| {
                    *alias != canonical && canonical_mime(alias, &aliases) == *canonical
                })
                .cloned()
                .collect();
            equivalents.sort();
            equivalents.insert(0, canonical.clone());
            for equivalent in &equivalents {
                for parent in parents.get(equivalent).into_iter().flatten() {
                    let parent = canonical_mime(parent, &aliases);
                    if pending.len() < 32 && !pending.contains(&parent) {
                        pending.push(parent);
                    }
                }
            }
            lineage.push(equivalents);
            index += 1;
        }
        Ok(lineage)
    }

    fn installed(&self, cancel: &CancelToken) -> Result<HashMap<String, LocatedApp>, MimeError> {
        let mut apps = HashMap::new();
        let mut hidden = HashSet::new();
        let mut examined = 0;
        for (root_rank, root) in self.context.data_roots().enumerate() {
            let applications = root.join("applications");
            let mut pending = vec![applications.clone()];
            while let Some(dir) = pending.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
                entries.sort_by_key(std::fs::DirEntry::file_name);
                for entry in entries {
                    if cancel.is_terminal() {
                        return Err(MimeError::Cancelled);
                    }
                    examined += 1;
                    if examined > MAX_DESKTOP_FILES {
                        return Err(MimeError::TooLarge(applications));
                    }
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_dir() {
                        pending.push(path);
                        continue;
                    }
                    if path.extension().is_none_or(|ext| ext != "desktop") {
                        continue;
                    }
                    let Ok(relative) = path.strip_prefix(&applications) else {
                        continue;
                    };
                    let id = relative.to_string_lossy().replace('/', "-");
                    if apps.contains_key(&id) || hidden.contains(&id) {
                        continue;
                    }
                    match desktop_app(&path, &id)? {
                        Some(app) => {
                            let _ = apps.insert(
                                id,
                                LocatedApp {
                                    app,
                                    root: root_rank,
                                },
                            );
                        }
                        None => {
                            // Hidden and invalid entries mask the same ID in lower roots.
                            let _ = hidden.insert(id);
                        }
                    }
                }
            }
        }
        Ok(apps)
    }
}

struct LocatedApp {
    app: DesktopApp,
    root: usize,
}

struct AssociationFile {
    groups: Groups,
    data_root: Option<usize>,
    defaults_only: bool,
}

impl AssociationFile {
    fn ids(&self, group: &str, mime: &str) -> Vec<String> {
        self.groups
            .get(group)
            .and_then(|group| group.get(mime))
            .map(|value| split_list(value))
            .unwrap_or_default()
    }

    fn applies_to(&self, app: &LocatedApp) -> bool {
        self.data_root.is_none_or(|root| app.root >= root)
    }
}

/// How many times [`MimeApps::set_default`] merges into a `mimeapps.list`
/// that another writer keeps changing before it gives up.
const MERGE_ATTEMPTS: usize = 4;

/// `current` with `desktop_id` as the default for `mime`, added as an
/// association, and no longer removed for any of `equivalents`.
fn default_entries(current: &str, mime: &str, desktop_id: &str, equivalents: &[String]) -> String {
    let groups = parse_keyfile(current);
    let mut added = groups
        .get("Added Associations")
        .and_then(|group| group.get(mime))
        .map(|value| split_list(value))
        .unwrap_or_default();
    if !added.iter().any(|id| id == desktop_id) {
        added.push(desktop_id.to_owned());
    }
    let mut updated = set_list_entry(
        current,
        "Default Applications",
        mime,
        &[desktop_id.to_owned()],
    );
    updated = set_list_entry(&updated, "Added Associations", mime, &added);
    for equivalent in equivalents {
        if let Some(value) = groups
            .get("Removed Associations")
            .and_then(|group| group.get(equivalent))
        {
            let kept: Vec<_> = split_list(value)
                .into_iter()
                .filter(|id| id != desktop_id)
                .collect();
            updated = set_list_entry(&updated, "Removed Associations", equivalent, &kept);
        }
    }
    updated
}

fn candidate_levels(
    installed: &HashMap<String, LocatedApp>,
    types: &[Vec<String>],
    files: &[AssociationFile],
) -> Vec<Vec<DesktopApp>> {
    types
        .iter()
        .map(|equivalents| {
            let mut result = Vec::new();
            let mut seen = HashSet::new();
            let mut removed = HashSet::new();
            for file in files.iter().filter(|file| !file.defaults_only) {
                for mime in equivalents {
                    for id in file.ids("Added Associations", mime) {
                        if let Some(app) = installed.get(&id).filter(|app| file.applies_to(app))
                            && !removed.contains(&id)
                            && seen.insert(id)
                        {
                            result.push(app.app.clone());
                        }
                    }
                }
                for mime in equivalents {
                    for id in file.ids("Removed Associations", mime) {
                        if installed.get(&id).is_some_and(|app| file.applies_to(app)) {
                            let _ = removed.insert(id);
                        }
                    }
                }
                if let Some(root) = file.data_root {
                    let mut supporting: Vec<_> = installed
                        .values()
                        .filter(|app| app.root == root)
                        .filter(|app| !removed.contains(&app.app.id) && !seen.contains(&app.app.id))
                        .filter(|app| {
                            app.app
                                .mime_types
                                .iter()
                                .any(|mime| equivalents.contains(mime))
                        })
                        .collect();
                    supporting.sort_by(|a, b| a.app.id.cmp(&b.app.id));
                    for app in supporting {
                        let _ = seen.insert(app.app.id.clone());
                        result.push(app.app.clone());
                    }
                }
            }
            result
        })
        .collect()
}

fn canonical_mime(mime: &str, aliases: &HashMap<String, String>) -> String {
    let mut visited = Vec::new();
    let mut current = mime;
    for _ in 0..32 {
        if let Some(cycle) = visited.iter().position(|name| *name == current) {
            return visited[cycle..]
                .iter()
                .min()
                .unwrap_or(&current)
                .to_string();
        }
        visited.push(current);
        let Some(next) = aliases.get(current) else {
            break;
        };
        current = next;
    }
    current.to_owned()
}

fn desktop_app(path: &Path, id: &str) -> Result<Option<DesktopApp>, MimeError> {
    let Some(groups) = read_keyfile(path)? else {
        return Ok(None);
    };
    let Some(entry) = groups.get("Desktop Entry") else {
        return Ok(None);
    };
    let flag = |key: &str| entry.get(key).is_some_and(|value| value == "true");
    if flag("Hidden")
        || entry.get("Type").map(String::as_str) != Some("Application")
        || (!flag("DBusActivatable")
            && entry
                .get("Exec")
                .is_none_or(|command| command.trim().is_empty()))
    {
        return Ok(None);
    }
    let Some(name) = entry.get("Name").filter(|name| !name.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(DesktopApp {
        id: id.to_owned(),
        name: name.clone(),
        icon: entry.get("Icon").filter(|icon| !icon.is_empty()).cloned(),
        path: path.to_owned(),
        mime_types: entry
            .get("MimeType")
            .map(|value| split_list(value))
            .unwrap_or_default(),
        no_display: flag("NoDisplay"),
    }))
}

fn valid_mime(mime: &str) -> bool {
    let mut parts = mime.split('/');
    let token = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
        })
    };
    token(parts.next()) && token(parts.next()) && parts.next().is_none() && mime.len() <= 255
}

fn read_bounded(path: &Path) -> Result<Option<String>, MimeError> {
    use std::io::Read as _;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MimeError::Io(path.to_owned(), error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| MimeError::Io(path.to_owned(), error))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(MimeError::TooLarge(path.to_owned()));
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

type Groups = BTreeMap<String, BTreeMap<String, String>>;

fn read_keyfile(path: &Path) -> Result<Option<Groups>, MimeError> {
    let Some(text) = read_bounded(path)? else {
        return Ok(None);
    };
    Ok(Some(parse_keyfile(&text)))
}

fn parse_keyfile(text: &str) -> Groups {
    let mut groups = Groups::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            current = Some(name.to_owned());
            let _ = groups.entry(name.to_owned()).or_default();
            continue;
        }
        let (Some(group), Some((key, value))) = (current.as_ref(), line.split_once('=')) else {
            continue;
        };
        let _ = groups
            .entry(group.clone())
            .or_default()
            .entry(key.trim().to_owned())
            .or_insert_with(|| unescape(value.trim()));
    }
    groups
}

// Preserve escaped semicolons until list splitting.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push_str("\\;"),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn split_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut item = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&';') {
            item.push(';');
            let _ = chars.next();
        } else if c == ';' {
            items.push(std::mem::take(&mut item));
        } else {
            item.push(c);
        }
    }
    items.push(item);
    items
        .into_iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

fn pair_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
        })
        .collect()
}

fn set_list_entry(text: &str, group: &str, mime: &str, ids: &[String]) -> String {
    let value: String = ids
        .iter()
        .map(|id| {
            let escaped = id
                .replace('\\', "\\\\")
                .replace(';', "\\;")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            format!("{escaped};")
        })
        .collect();
    let entry = format!("{mime}={value}");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let header = lines
        .iter()
        .position(|line| line.trim() == format!("[{group}]"));
    match header {
        Some(header) => {
            let end = lines[header + 1..]
                .iter()
                .position(|line| line.trim_start().starts_with('['))
                .map_or(lines.len(), |offset| header + 1 + offset);
            let existing = lines[header + 1..end].iter().position(|line| {
                line.split_once('=')
                    .is_some_and(|(key, _)| key.trim() == mime)
            });
            match existing {
                Some(offset) => lines[header + 1 + offset] = entry,
                None => lines.insert(end, entry),
            }
        }
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{group}]"));
            lines.push(entry);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
