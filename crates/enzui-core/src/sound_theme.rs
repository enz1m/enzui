//! Find a desktop event sound by name: the freedesktop sound theme and
//! sound naming specs.
//!
//! Event sounds have standard names (`message-new-instant`, `complete`,
//! `dialog-warning`, `bell`, `camera-shutter`, `trash-empty`...) and each sound
//! theme supplies its own recording. [`event_sound_path`] looks a name up in
//! the theme, then the themes it inherits, then `freedesktop`; a name that
//! is not found is tried shorter (`message-new-instant`, then
//! `message-new`, then `message`). A theme can switch a sound off with a
//! `<name>.disabled` file. `enzui_services::event_sound` plays what this
//! finds.

use std::path::{Path, PathBuf};

/// The theme every desktop has, and the last one asked.
pub const FALLBACK_THEME: &str = "freedesktop";

/// File types, in the spec's order of preference.
const EXTENSIONS: [&str; 3] = ["oga", "ogg", "wav"];

/// The `sounds` directories to search: `$XDG_DATA_HOME/sounds`, then each of
/// `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`).
#[must_use]
pub fn sound_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .map(|dir| dir.join("sounds"))
        .collect()
}

/// The file for event sound `name` in `theme` under the [`sound_dirs`], or
/// `None` when no theme has it or the theme disabled it.
#[must_use]
pub fn event_sound_path(name: &str, theme: &str) -> Option<PathBuf> {
    find_event_sound(name, theme, &sound_dirs())
}

/// [`event_sound_path`] under `dirs` (each a `sounds` directory).
#[must_use]
pub fn find_event_sound(name: &str, theme: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let themes = theme_chain(theme, dirs);
    for candidate in shorter_names(name) {
        for theme in &themes {
            match in_theme(&candidate, theme, dirs) {
                Found::File(path) => return Some(path),
                Found::Disabled => return None,
                Found::Nothing => {}
            }
        }
        // Unthemed sounds sit directly in a sounds directory.
        for dir in dirs {
            if let Some(path) = with_extension(dir, &candidate) {
                return Some(path);
            }
        }
    }
    None
}

enum Found {
    File(PathBuf),
    Disabled,
    Nothing,
}

/// `name`, then `name` shortened by one `-` component at a time.
fn shorter_names(name: &str) -> Vec<String> {
    let mut names = vec![name.to_owned()];
    let mut rest = name;
    while let Some((shorter, _)) = rest.rsplit_once('-') {
        names.push(shorter.to_owned());
        rest = shorter;
    }
    names
}

/// `theme`, the themes it inherits (depth first), and `freedesktop` last.
fn theme_chain(theme: &str, dirs: &[PathBuf]) -> Vec<String> {
    fn visit(theme: &str, dirs: &[PathBuf], chain: &mut Vec<String>) {
        if chain.iter().any(|seen| seen == theme) || chain.len() > 16 {
            return;
        }
        chain.push(theme.to_owned());
        if let Some(index) = index_theme(theme, dirs) {
            for parent in index.inherits {
                visit(&parent, dirs, chain);
            }
        }
    }
    let mut chain = Vec::new();
    visit(theme, dirs, &mut chain);
    if !chain.iter().any(|seen| seen == FALLBACK_THEME) {
        chain.push(FALLBACK_THEME.to_owned());
    }
    chain
}

struct IndexTheme {
    inherits: Vec<String>,
    /// Sub-directories, the stereo ones first.
    directories: Vec<String>,
}

/// The theme's `index.theme`, from the first sounds directory that has it.
fn index_theme(theme: &str, dirs: &[PathBuf]) -> Option<IndexTheme> {
    let text = dirs
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(theme).join("index.theme")).ok())?;
    let mut group = String::new();
    let mut inherits = Vec::new();
    let mut directories = Vec::new();
    let mut profiles: Vec<(String, String)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            name.clone_into(&mut group);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let list = || {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        match (group.as_str(), key.trim()) {
            ("Sound Theme", "Inherits") => inherits = list(),
            ("Sound Theme", "Directories") => directories = list(),
            (_, "OutputProfile") => profiles.push((group.clone(), value.trim().to_owned())),
            _ => {}
        }
    }
    let stereo = |directory: &String| {
        profiles
            .iter()
            .any(|(group, profile)| group == directory && profile == "stereo")
    };
    directories.sort_by_key(|directory| !stereo(directory));
    Some(IndexTheme {
        inherits,
        directories,
    })
}

fn in_theme(name: &str, theme: &str, dirs: &[PathBuf]) -> Found {
    let directories = index_theme(theme, dirs).map_or_else(
        // A theme without an index keeps its sounds in `stereo`.
        || vec!["stereo".to_owned()],
        |index| index.directories,
    );
    for dir in dirs {
        let root = dir.join(theme);
        for directory in &directories {
            let place = root.join(directory);
            if place.join(format!("{name}.disabled")).exists() {
                return Found::Disabled;
            }
            if let Some(path) = with_extension(&place, name) {
                return Found::File(path);
            }
        }
    }
    Found::Nothing
}

fn with_extension(dir: &Path, name: &str) -> Option<PathBuf> {
    EXTENSIONS
        .iter()
        .map(|extension| dir.join(format!("{name}.{extension}")))
        .find(|path| path.is_file())
}
