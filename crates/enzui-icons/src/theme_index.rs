//! Finding a named icon's file in an installed freedesktop icon theme.
//!
//! The disk-touching layer over [`crate::icon_theme`]: [`ThemeIndex::load`]
//! reads the selected theme's `index.theme` and those of every theme it
//! inherits, and [`ThemeIndex::find`] answers which file draws the first of
//! several candidate names at a size. Each theme's directories are listed once,
//! the first time a lookup reaches that theme, so a lookup after that is a
//! handful of hash probes and no `stat` calls. Build one index per process (or
//! per theme change) and keep it.
//!
//! # Name order
//!
//! [`ThemeIndex::find`] takes the candidates in preference order (`image-png`,
//! `image-x-generic`, `application-x-generic`) and walks the themes outermost,
//! as GTK does: the selected theme's generic icon beats an inherited theme's
//! exact one, so a file list never mixes two themes' artwork just because the
//! selected theme drew a family icon instead of a per-format one.
//!
//! # Example
//! ```no_run
//! use enzui_icons::theme_index::{ThemeIndex, icon_theme_roots};
//!
//! let index = ThemeIndex::load("enzim", &icon_theme_roots());
//! let png = index.find(&["image-png", "image-x-generic"], 24, 1);
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use crate::icon_theme::{DirEntry, Theme, ThemeSet};
use crate::mime::xdg::{Globs2Env, globs2_candidates_os};

/// Longest `index.theme` read; real ones are a few kilobytes.
const INDEX_BYTES: u64 = 256 * 1024;
/// Most themes followed through `Inherits`.
const MAX_THEMES: usize = 32;
/// Most directories one theme may declare.
const MAX_DIRECTORIES: usize = 1024;
/// Most files remembered from one theme directory in one root.
const MAX_FILES_PER_DIR: usize = 16_384;

/// The directories icon themes are installed under, in precedence order:
/// `$HOME/.icons`, then `icons` under `$XDG_DATA_HOME` and each of
/// `$XDG_DATA_DIRS`.
#[must_use]
pub fn icon_theme_roots() -> Vec<PathBuf> {
    icon_theme_roots_from(&Globs2Env::from_env())
}

/// [`icon_theme_roots`] over captured environment values.
#[must_use]
pub fn icon_theme_roots_from(env: &Globs2Env) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = env
        .home
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_absolute())
    {
        roots.push(home.join(".icons"));
    }
    for globs in globs2_candidates_os(
        env.data_home.as_deref(),
        env.data_dirs.as_deref(),
        env.home.as_deref(),
    ) {
        // Each candidate is `<root>/mime/globs2`.
        if let Some(root) = globs.parent().and_then(Path::parent) {
            let icons = root.join("icons");
            if !roots.contains(&icons) {
                roots.push(icons);
            }
        }
    }
    roots
}

/// Whether `name` is usable as a theme or icon name: no path separators,
/// controls, spaces or commas, and not `.` or `..`.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.is_ascii()
        && !name
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ' || b == b',' || b == b'/' || b == b'\\')
        && name != "."
        && name != ".."
}

/// Parse an `index.theme` document into a [`Theme`] named `name`.
///
/// Reads `Inherits`, `Directories` and `ScaledDirectories` from the
/// `[Icon Theme]` group and `Size`, `Scale`, `Type`, `MinSize`, `MaxSize` and
/// `Threshold` from each directory's group. A directory with a path that
/// leaves the theme, no group or a malformed size is skipped; a document
/// without an `[Icon Theme]` group or `Directories` key is `None`.
#[must_use]
pub fn parse_index_theme(name: &str, text: &str) -> Option<Theme> {
    let mut groups: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    let mut current = "";
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = group;
        } else if let Some((key, value)) = line.split_once('=') {
            groups
                .entry(current)
                .or_default()
                .insert(key.trim(), value.trim());
        }
    }
    let main = groups.get("Icon Theme")?;
    let directories = main.get("Directories")?;
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    for dir in directories.split(',').chain(
        main.get("ScaledDirectories")
            .into_iter()
            .flat_map(|s| s.split(',')),
    ) {
        let dir = dir.trim();
        if dir.is_empty()
            || !Path::new(dir)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
            || !seen.insert(dir)
        {
            continue;
        }
        if dirs.len() >= MAX_DIRECTORIES {
            break;
        }
        let Some(section) = groups.get(dir) else {
            continue;
        };
        let number = |key, fallback| {
            section
                .get(key)
                .map_or(Some(fallback), |s| s.parse::<u16>().ok())
        };
        let Some(size) = number("Size", 0).filter(|s| *s > 0) else {
            continue;
        };
        let Some(scale) = number("Scale", 1).filter(|s| *s > 0) else {
            continue;
        };
        let entry = match section.get("Type").copied().unwrap_or("Threshold") {
            "Fixed" => DirEntry::fixed(dir, size),
            "Scalable" => {
                let (Some(min), Some(max)) = (number("MinSize", size), number("MaxSize", size))
                else {
                    continue;
                };
                if min == 0 || min > max {
                    continue;
                }
                DirEntry::scalable(dir, size, min, max)
            }
            "Threshold" => {
                let Some(threshold) = number("Threshold", 2) else {
                    continue;
                };
                DirEntry::threshold(dir, size, threshold)
            }
            _ => continue,
        }
        .with_scale(scale);
        dirs.push(entry);
    }
    let parents = main
        .get("Inherits")
        .into_iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| is_valid_name(s))
        .map(str::to_owned);
    Some(Theme::new(name).inherits(parents).with_dirs(dirs))
}

/// One theme of the chain, with its directories listed on first use.
struct IndexedTheme {
    theme: Theme,
    /// `<root>/<theme>` for every root that holds this theme, in precedence
    /// order: a theme may be spread across several roots.
    bases: Vec<PathBuf>,
    listing: OnceLock<Vec<DirFiles>>,
}

/// The icon files of one theme directory, merged across roots.
struct DirFiles {
    entry: DirEntry,
    /// Icon name to file; an earlier root wins, and PNG beats SVG in one root
    /// as the specification orders extensions.
    files: HashMap<String, PathBuf>,
}

impl IndexedTheme {
    fn listing(&self) -> &[DirFiles] {
        self.listing.get_or_init(|| {
            self.theme
                .directories
                .iter()
                .map(|entry| {
                    let mut files = HashMap::new();
                    for base in &self.bases {
                        list_dir(&base.join(&entry.dir), &mut files);
                    }
                    DirFiles {
                        entry: entry.clone(),
                        files,
                    }
                })
                .filter(|dir| !dir.files.is_empty())
                .collect()
        })
    }

    fn find(&self, name: &str, size: u16, scale: u16) -> Option<&Path> {
        let listing = self.listing();
        if let Some(path) = listing
            .iter()
            .filter(|dir| dir.entry.matches_size(size, scale))
            .find_map(|dir| dir.files.get(name))
        {
            return Some(path);
        }
        listing
            .iter()
            .filter_map(|dir| {
                dir.files
                    .get(name)
                    .map(|path| (dir.entry.size_distance(size, scale), path))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, path)| path.as_path())
    }
}

/// Add the PNG and SVG files of `dir` to `files`, keeping names already there.
fn list_dir(dir: &Path, files: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut svgs = Vec::new();
    let mut added = 0;
    for entry in entries.flatten() {
        if added >= MAX_FILES_PER_DIR {
            break;
        }
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if let Some(stem) = file_name.strip_suffix(".png") {
            if !files.contains_key(stem) {
                files.insert(stem.to_owned(), entry.path());
                added += 1;
            }
        } else if let Some(stem) = file_name.strip_suffix(".svg") {
            svgs.push((stem.to_owned(), entry.path()));
        }
    }
    for (stem, path) in svgs {
        if added >= MAX_FILES_PER_DIR {
            break;
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = files.entry(stem) {
            slot.insert(path);
            added += 1;
        }
    }
}

fn read_index(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(INDEX_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// An installed icon theme and the themes it inherits, ready for lookups.
///
/// Loading reads only the `index.theme` files; each theme's directories are
/// listed the first time a lookup reaches it. Artwork installed or removed
/// after that is not seen: load a new index to pick it up.
pub struct ThemeIndex {
    selected: String,
    /// The chain in search order, `hicolor` last.
    themes: Vec<IndexedTheme>,
}

impl std::fmt::Debug for ThemeIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeIndex")
            .field("selected", &self.selected)
            .field(
                "themes",
                &self
                    .themes
                    .iter()
                    .map(|t| t.theme.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ThemeIndex {
    /// Load `theme` and its `Inherits` chain from `roots` (see
    /// [`icon_theme_roots`]). A theme missing from every root is skipped, so
    /// an unknown name still searches `hicolor`.
    #[must_use]
    pub fn load(theme: &str, roots: &[PathBuf]) -> Self {
        let mut loaded: Vec<(Theme, Vec<PathBuf>)> = Vec::new();
        let mut pending = vec!["hicolor".to_owned(), theme.to_owned()];
        let mut seen = HashSet::new();
        while let Some(name) = pending.pop() {
            if !is_valid_name(&name) || !seen.insert(name.clone()) || seen.len() > MAX_THEMES {
                continue;
            }
            let bases: Vec<PathBuf> = roots
                .iter()
                .map(|root| root.join(&name))
                .filter(|base| base.is_dir())
                .collect();
            let Some(parsed) = bases
                .iter()
                .find_map(|base| read_index(&base.join("index.theme")))
                .and_then(|text| parse_index_theme(&name, &text))
            else {
                continue;
            };
            pending.extend(parsed.inherits.iter().rev().cloned());
            loaded.push((parsed, bases));
        }
        let order =
            ThemeSet::new(loaded.iter().map(|(t, _)| t.clone()).collect()).search_order(theme);
        let mut by_name: HashMap<String, (Theme, Vec<PathBuf>)> = loaded
            .into_iter()
            .map(|(t, bases)| (t.name.clone(), (t, bases)))
            .collect();
        let themes = order
            .iter()
            .filter_map(|name| by_name.remove(name))
            .map(|(theme, bases)| IndexedTheme {
                theme,
                bases,
                listing: OnceLock::new(),
            })
            .collect();
        Self {
            selected: theme.to_owned(),
            themes,
        }
    }

    /// The theme this index was loaded for.
    #[must_use]
    pub fn selected(&self) -> &str {
        &self.selected
    }

    /// Whether the selected theme itself is installed (not just `hicolor`).
    #[must_use]
    pub fn has_selected(&self) -> bool {
        self.themes
            .first()
            .is_some_and(|t| t.theme.name == self.selected)
    }

    /// The themes searched, in order.
    #[must_use]
    pub fn search_order(&self) -> Vec<&str> {
        self.themes.iter().map(|t| t.theme.name.as_str()).collect()
    }

    /// The file drawing the first of `names` found, at `size` logical pixels
    /// and integer `scale`.
    ///
    /// Themes are the outer loop and names the inner one (see the module
    /// docs); within a theme an exact size match beats the closest size. A
    /// name that is not a valid icon name is skipped.
    #[must_use]
    pub fn find<S: AsRef<str>>(&self, names: &[S], size: u16, scale: u16) -> Option<PathBuf> {
        let size = size.max(1);
        let scale = scale.max(1);
        self.themes.iter().find_map(|theme| {
            names
                .iter()
                .map(AsRef::as_ref)
                .filter(|name| is_valid_name(name))
                .find_map(|name| theme.find(name, size, scale))
                .map(Path::to_path_buf)
        })
    }
}

#[cfg(test)]
mod tests;
