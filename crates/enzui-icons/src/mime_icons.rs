//! The icon-theme names that draw a MIME type, from shared-mime-info.
//!
//! [`MimeIconNames::names_for_mime`] answers the candidate list a themed
//! lookup ([`crate::theme_index::ThemeIndex::find`]) walks, in this order:
//!
//! 1. the type's own `icons` entry, when the database names one;
//! 2. the MIME icon name, `/` replaced by `-` (`image/png` → `image-png`);
//! 3. the type's shared-mime-info `generic-icon` (`x-office-document` for
//!    `application/vnd.oasis.opendocument.text`);
//! 4. the family generic, `<media>-x-generic` (`image-x-generic`);
//! 5. `application-x-generic`, the icon for any file.
//!
//! An alias (`application/x-pdf`) is first replaced by its canonical type, and
//! parameters (`; charset=utf-8`) and case are ignored. `inode/directory`
//! leads with `folder`.
//!
//! [`MimeIconNames::host`] reads `mime/generic-icons`, `mime/icons` and
//! `mime/aliases` from the XDG data directories once per process; an earlier
//! directory's entry wins, as for `globs2`.
//!
//! # Example
//! ```
//! use enzui_icons::mime_icons::MimeIconNames;
//!
//! let table = MimeIconNames::from_text("application/vnd.oasis.opendocument.text:x-office-document\n", "", "");
//! assert_eq!(
//!     table.names_for_mime("application/vnd.oasis.opendocument.text"),
//!     [
//!         "application-vnd.oasis.opendocument.text",
//!         "x-office-document",
//!         "application-x-generic",
//!     ]
//! );
//! ```

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::mime::xdg::Globs2Env;

/// Longest database file read; the real ones are tens of kilobytes.
const FILE_BYTES: u64 = 4 * 1024 * 1024;

/// The icon for any file, the last name of every candidate list.
pub const GENERIC_FILE_ICON: &str = "application-x-generic";

/// The theme icon for a directory.
pub const FOLDER_ICON: &str = "folder";

/// shared-mime-info's icon tables: per-type icons, generic icons and aliases.
#[derive(Clone, Debug, Default)]
pub struct MimeIconNames {
    generic: HashMap<String, String>,
    icons: HashMap<String, String>,
    aliases: HashMap<String, String>,
}

impl MimeIconNames {
    /// The host's tables, read from the XDG data directories once per process.
    /// Empty on a host without shared-mime-info: names then derive from the
    /// type alone.
    pub fn host() -> &'static MimeIconNames {
        static HOST: OnceLock<MimeIconNames> = OnceLock::new();
        HOST.get_or_init(|| Self::load(&mime_dirs(&Globs2Env::from_env())))
    }

    /// Read the tables from `mime` directories in precedence order (an
    /// earlier directory's entry wins).
    #[must_use]
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut table = Self::default();
        for dir in dirs {
            let read = |name| read_bounded(&dir.join(name)).unwrap_or_default();
            table.merge(&read("generic-icons"), &read("icons"), &read("aliases"));
        }
        table
    }

    /// Tables from document text: `generic-icons` and `icons` are
    /// `type:icon-name` lines, `aliases` is `alias canonical` lines.
    #[must_use]
    pub fn from_text(generic_icons: &str, icons: &str, aliases: &str) -> Self {
        let mut table = Self::default();
        table.merge(generic_icons, icons, aliases);
        table
    }

    fn merge(&mut self, generic_icons: &str, icons: &str, aliases: &str) {
        fn pairs(text: &str, sep: char) -> impl Iterator<Item = (String, &str)> {
            text.lines().filter_map(move |line| {
                let line = line.trim();
                if line.starts_with('#') {
                    return None;
                }
                let (key, value) = line.split_once(sep)?;
                let (key, value) = (key.trim(), value.trim());
                (!key.is_empty() && !value.is_empty()).then(|| (key.to_ascii_lowercase(), value))
            })
        }
        for (mime, icon) in pairs(generic_icons, ':') {
            self.generic.entry(mime).or_insert_with(|| icon.to_owned());
        }
        for (mime, icon) in pairs(icons, ':') {
            self.icons.entry(mime).or_insert_with(|| icon.to_owned());
        }
        for (alias, canonical) in pairs(aliases, ' ') {
            self.aliases
                .entry(alias)
                .or_insert_with(|| canonical.to_ascii_lowercase());
        }
    }

    /// The canonical form of `mime`: lower case, without parameters, an alias
    /// replaced by the type it names.
    #[must_use]
    pub fn canonical(&self, mime: &str) -> String {
        let base = mime
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        self.aliases.get(&base).cloned().unwrap_or(base)
    }

    /// The shared-mime-info `generic-icon` of `mime`, if the database names
    /// one.
    #[must_use]
    pub fn generic_icon(&self, mime: &str) -> Option<&str> {
        self.generic.get(&self.canonical(mime)).map(String::as_str)
    }

    /// The theme icon names for `mime`, most specific first, always ending at
    /// [`GENERIC_FILE_ICON`] (see the module docs for the order).
    #[must_use]
    pub fn names_for_mime(&self, mime: &str) -> Vec<String> {
        let mime = self.canonical(mime);
        let mut out: Vec<String> = Vec::with_capacity(5);
        let mut push = |name: &str| {
            if !name.is_empty() && !out.iter().any(|n| n == name) {
                out.push(name.to_owned());
            }
        };
        if mime == "inode/directory" {
            push(FOLDER_ICON);
        }
        if let Some(icon) = self.icons.get(&mime) {
            push(icon);
        }
        let (media, sub) = mime.split_once('/').unwrap_or((mime.as_str(), ""));
        if !media.is_empty() && !sub.is_empty() {
            push(&format!("{media}-{sub}"));
        }
        if let Some(icon) = self.generic.get(&mime) {
            push(icon);
        }
        if !media.is_empty()
            && media
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            push(&format!("{media}-x-generic"));
        }
        push(GENERIC_FILE_ICON);
        out
    }
}

/// `<root>/mime` for every XDG data root, in precedence order.
fn mime_dirs(env: &Globs2Env) -> Vec<PathBuf> {
    env.candidates()
        .into_iter()
        .filter_map(|globs| globs.parent().map(Path::to_path_buf))
        .collect()
}

fn read_bounded(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(FILE_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}
