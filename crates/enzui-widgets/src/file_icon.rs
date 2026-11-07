//! [`enz_file_icon`] — a file's icon from the desktop icon theme.
//!
//! A file list, an attachment chip or a download row shows what a file *is*
//! with the artwork the rest of the desktop uses for it: the icon theme's
//! `image-png`, `x-office-document` or `folder`. Ask with a
//! [`FileIconQuery`] — a MIME type, a file name (classified by the host's MIME
//! globs) or a directory — and a size:
//!
//! ```no_run
//! use enzui_widgets::file_icon::{FileIconQuery, enz_file_icon};
//! # #[derive(Clone)] enum Message {}
//! let icon: iced::Element<'_, Message> = enz_file_icon(FileIconQuery::file_name("report.pdf"))
//!     .size(24.0)
//!     .into_element();
//! ```
//!
//! # Resolution
//!
//! The theme is searched for, in order, the MIME icon name (`image-png`),
//! shared-mime-info's `generic-icon` for the type, the family generic
//! (`image-x-generic`) and `application-x-generic`
//! ([`enzui_icons::mime_icons`]). The selected theme is searched for every
//! name before an inherited theme is, so one list never mixes two themes'
//! artwork ([`enzui_icons::theme_index`]). Theme artwork renders in its own
//! colours. When nothing resolves — no theme configured, a theme with no file
//! icons, or [`themed(false)`](EnzFileIcon::themed) — the icon is the bundled
//! Lucide glyph for the type ([`enzui_icons::file_type::icon_for_mime`]),
//! tinted like any [`icon`](crate::icon).
//!
//! [`themed_icon`] is the same lookup for a list of plain theme names, for
//! places and devices (`folder-documents`, `drive-harddisk`, `user-trash`).
//!
//! # Which theme
//!
//! The first of: [`set_icon_theme`]; the `ENZUI_ICON_THEME` environment
//! variable (empty or `none` turns themed icons off); GSettings'
//! `org.gnome.desktop.interface icon-theme` (read through `gsettings`); GTK's
//! `gtk-icon-theme-name` in `settings.ini`; KDE's `[Icons] Theme`. It is read
//! once per process; [`reload_icon_theme`] reads it again and drops cached
//! artwork.
//!
//! # Cost
//!
//! Resolutions are cached per type and pixel size, so a view rebuilt every
//! frame pays one map lookup per icon. The first lookup reads the theme's
//! `index.theme` files and lists the selected theme's directories, a few
//! milliseconds; [`prewarm_icon_theme`] does that on a background thread at
//! startup so the first frame does not. The artwork itself is decoded by the
//! renderer, which caches the rasterized pixels.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use enzui_icons::file_type::{FileTypeTable, icon_for_mime};
use enzui_icons::mime_icons::MimeIconNames;
use enzui_icons::theme_index::{ThemeIndex, icon_theme_roots, is_valid_name};
use iced::widget::{image as iced_image, svg};
use iced::{Color, Element};

use crate::icon::{DEFAULT_ICON_SIZE, EnzIcon, IconSource, enz_icon};

/// The environment variable that names the icon theme, ahead of the desktop's
/// setting. Empty or `none` turns themed icons off.
pub const ICON_THEME_ENV: &str = "ENZUI_ICON_THEME";

/// Most cached resolutions before the cache starts over.
const CACHE_ENTRIES: usize = 4096;

/// What a file icon is asked for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileIconQuery {
    /// A MIME type, such as `image/png` or a `Content-Type` value.
    Mime(String),
    /// A file name or path, classified by the host's MIME globs.
    FileName(String),
    /// A directory.
    Directory,
}

impl FileIconQuery {
    /// A query for a MIME type.
    pub fn mime(mime: impl Into<String>) -> Self {
        Self::Mime(mime.into())
    }

    /// A query for a file name, classified by its name alone.
    pub fn file_name(name: impl Into<String>) -> Self {
        Self::FileName(name.into())
    }

    /// A query for a directory.
    #[must_use]
    pub fn directory() -> Self {
        Self::Directory
    }

    /// The MIME type the query stands for.
    #[must_use]
    pub fn mime_type(&self) -> String {
        match self {
            Self::Mime(mime) => mime.clone(),
            Self::FileName(name) => FileTypeTable::host().classify(name).mime,
            Self::Directory => "inode/directory".to_owned(),
        }
    }

    /// The theme icon names tried, most specific first.
    #[must_use]
    pub fn theme_names(&self) -> Vec<String> {
        MimeIconNames::host().names_for_mime(&self.mime_type())
    }

    /// The bundled Lucide glyph drawn when the theme has no icon.
    #[must_use]
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::Mime(mime) => icon_for_mime(mime),
            Self::FileName(name) => FileTypeTable::host().icon_name(name),
            Self::Directory => FileTypeTable::DIRECTORY_ICON,
        }
    }
}

/// The icon-theme artwork for `query` at `size` logical pixels, or `None`
/// when the theme has none (see the module docs for the order).
#[must_use]
pub fn themed_file_icon(query: &FileIconQuery, size: f32) -> Option<IconSource> {
    let key = match query {
        FileIconQuery::Mime(mime) => CacheKey::Mime(mime.clone()),
        FileIconQuery::FileName(name) => CacheKey::Mime(query_mime_for_name(name)),
        FileIconQuery::Directory => CacheKey::Mime("inode/directory".to_owned()),
    };
    resolve(key, size)
}

/// The icon for `query`: the theme's artwork, else the bundled glyph as
/// [`IconSource::Named`].
#[must_use]
pub fn file_icon_source(query: &FileIconQuery, size: f32) -> IconSource {
    themed_file_icon(query, size).unwrap_or_else(|| IconSource::Named(query.glyph().to_owned()))
}

/// The theme artwork for the first of `names` the theme has, at `size`
/// logical pixels: `["folder-documents", "folder"]`, `["drive-harddisk"]`.
#[must_use]
pub fn themed_icon<S: AsRef<str>>(names: &[S], size: f32) -> Option<IconSource> {
    let names: Vec<String> = names.iter().map(|n| n.as_ref().to_owned()).collect();
    resolve(CacheKey::Names(names), size)
}

/// The icon theme in use, or `None` when themed icons are off or no theme is
/// configured.
#[must_use]
pub fn icon_theme() -> Option<String> {
    lock().theme().map(str::to_owned)
}

/// Use `theme` instead of the desktop's setting (`None` turns themed icons
/// off), dropping cached artwork. An invalid name turns them off too.
pub fn set_icon_theme(theme: Option<&str>) {
    let theme = theme.filter(|t| is_valid_name(t)).map(str::to_owned);
    *lock() = Resolver::new(Some(theme), icon_theme_roots());
}

/// Read the desktop's icon theme again (after it changed) and drop cached
/// artwork. A theme chosen with [`set_icon_theme`] stays chosen.
pub fn reload_icon_theme() {
    let mut state = lock();
    let explicit = state.explicit.clone();
    *state = Resolver::new(explicit, icon_theme_roots());
}

/// Find the theme and list its directories on a background thread, so the
/// first frame that draws a file icon does not. Call once at startup;
/// lookups before it finishes wait for it.
pub fn prewarm_icon_theme() {
    let _ = std::thread::Builder::new()
        .name("file-icon-theme".into())
        .spawn(|| {
            let _ = themed_file_icon(&FileIconQuery::mime("application/octet-stream"), 24.0);
            let _ = MimeIconNames::host();
            let _ = FileTypeTable::host();
        });
}

/// A file's icon at a size: theme artwork, or a tinted bundled glyph.
#[must_use]
pub struct EnzFileIcon<'a> {
    query: FileIconQuery,
    size: f32,
    themed: bool,
    fallback_tint: Option<Color>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// A file icon for `query`, 16 px, from the icon theme when it has one.
pub fn enz_file_icon<'a>(query: FileIconQuery) -> EnzFileIcon<'a> {
    EnzFileIcon {
        query,
        size: DEFAULT_ICON_SIZE,
        themed: true,
        fallback_tint: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a> EnzFileIcon<'a> {
    /// The icon's size in logical pixels.
    pub fn size(mut self, px: f32) -> Self {
        self.size = if px.is_finite() { px.max(0.0) } else { 0.0 };
        self
    }

    /// Whether to use the icon theme's artwork (default `true`); `false`
    /// always draws the bundled glyph, for a monochrome list.
    pub fn themed(mut self, themed: bool) -> Self {
        self.themed = themed;
        self
    }

    /// The ink of the bundled glyph when it is drawn (default: the muted
    /// icon ink). Theme artwork keeps its own colours.
    pub fn fallback_tint(mut self, color: Color) -> Self {
        self.fallback_tint = Some(color);
        self
    }

    /// The source this icon draws: theme artwork, or the glyph's name.
    #[must_use]
    pub fn source(&self) -> IconSource {
        if self.themed {
            file_icon_source(&self.query, self.size)
        } else {
            IconSource::Named(self.query.glyph().to_owned())
        }
    }

    /// Whether this icon draws theme artwork rather than the bundled glyph.
    #[must_use]
    pub fn is_themed(&self) -> bool {
        self.themed && themed_file_icon(&self.query, self.size).is_some()
    }

    /// Build the element.
    pub fn into_element<Message: 'a>(self) -> Element<'a, Message> {
        let size = self.size;
        match self.source() {
            IconSource::Named(name) => {
                let icon: EnzIcon<'a> = enz_icon(&name).size(size);
                match self.fallback_tint {
                    Some(color) => icon.tint(color).into_element(),
                    None => icon.into_element(),
                }
            }
            artwork => artwork.into_app_element(size),
        }
    }
}

impl<'a, Message: 'a> From<EnzFileIcon<'a>> for Element<'a, Message> {
    fn from(icon: EnzFileIcon<'a>) -> Self {
        icon.into_element()
    }
}

/// The MIME type of a file name, cached: classifying walks the glob table.
fn query_mime_for_name(name: &str) -> String {
    static NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);
    // Only the extension decides for most names; key by the lowercased base
    // name so a folder of photos classifies once per distinct name shape.
    let key = base.to_ascii_lowercase();
    let cache = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(mime) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return mime.clone();
    }
    let mime = FileTypeTable::host().classify(base).mime;
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= CACHE_ENTRIES {
        cache.clear();
    }
    cache.insert(key, mime.clone());
    mime
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CacheKey {
    Mime(String),
    Names(Vec<String>),
}

/// The theme, its index and the cached answers.
struct Resolver {
    /// `Some` when [`set_icon_theme`] chose the theme.
    explicit: Option<Option<String>>,
    roots: Vec<PathBuf>,
    /// The theme in use, discovered on first lookup.
    theme: OnceLock<Option<String>>,
    index: OnceLock<Option<ThemeIndex>>,
    cache: HashMap<(CacheKey, u16), Option<IconSource>>,
}

impl Resolver {
    fn new(explicit: Option<Option<String>>, roots: Vec<PathBuf>) -> Self {
        Self {
            explicit,
            roots,
            theme: OnceLock::new(),
            index: OnceLock::new(),
            cache: HashMap::new(),
        }
    }

    fn theme(&self) -> Option<&str> {
        self.theme
            .get_or_init(|| match &self.explicit {
                Some(theme) => theme.clone(),
                None => discover_icon_theme(|key| std::env::var_os(key)),
            })
            .as_deref()
    }

    fn index(&self) -> Option<&ThemeIndex> {
        self.index
            .get_or_init(|| {
                let theme = self.theme()?;
                let index = ThemeIndex::load(theme, &self.roots);
                index.has_selected().then_some(index)
            })
            .as_ref()
    }

    fn resolve(&mut self, key: CacheKey, size: f32) -> Option<IconSource> {
        let px = pixel_size(size);
        if let Some(hit) = self.cache.get(&(key.clone(), px)) {
            return hit.clone();
        }
        let found = self.index().and_then(|index| {
            let names = match &key {
                CacheKey::Mime(mime) => MimeIconNames::host().names_for_mime(mime),
                CacheKey::Names(names) => names.clone(),
            };
            index.find(&names, px, 1)
        });
        let source = found.as_deref().and_then(artwork);
        if self.cache.len() >= CACHE_ENTRIES {
            self.cache.clear();
        }
        self.cache.insert((key, px), source.clone());
        source
    }
}

fn pixel_size(size: f32) -> u16 {
    if size.is_finite() {
        // Clamped to the u16 range first, so the cast cannot truncate.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 1..=4096 just above"
        )]
        let px = size.ceil().clamp(1.0, 4096.0) as u16;
        px
    } else {
        16
    }
}

/// An icon source drawing the file at `path`, by its extension.
fn artwork(path: &Path) -> Option<IconSource> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("svg") => Some(IconSource::Svg(svg::Handle::from_path(path))),
        Some("png") => Some(IconSource::Image(iced_image::Handle::from_path(path))),
        _ => None,
    }
}

fn state() -> &'static Mutex<Resolver> {
    static STATE: OnceLock<Mutex<Resolver>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Resolver::new(None, icon_theme_roots())))
}

fn lock() -> std::sync::MutexGuard<'static, Resolver> {
    state().lock().unwrap_or_else(PoisonError::into_inner)
}

fn resolve(key: CacheKey, size: f32) -> Option<IconSource> {
    lock().resolve(key, size)
}

/// The desktop's icon theme name: [`ICON_THEME_ENV`], then GSettings, GTK's
/// `settings.ini` and KDE's `kdeglobals`. `None` when themed icons are turned
/// off or nothing names a theme.
#[must_use]
pub fn discover_icon_theme(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<String> {
    if let Some(value) = var(ICON_THEME_ENV) {
        let value = value.to_string_lossy();
        let value = value.trim();
        return (!value.is_empty() && value != "none" && is_valid_name(value))
            .then(|| value.to_owned());
    }
    gsettings_icon_theme()
        .or_else(|| {
            enzui_core::desktop_settings::gtk_settings_files(&var)
                .iter()
                .find_map(|path| ini_value(path, "Settings", "gtk-icon-theme-name"))
        })
        .or_else(|| {
            enzui_core::desktop_settings::kdeglobals_files(&var)
                .iter()
                .find_map(|path| ini_value(path, "Icons", "Theme"))
        })
        .filter(|name| is_valid_name(name))
}

/// `gsettings get org.gnome.desktop.interface icon-theme`, waiting at most a
/// second for it.
fn gsettings_icon_theme() -> Option<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    parse_gsettings_string(&out)
}

/// The string inside a `gsettings get` reply such as `'enzim'`.
fn parse_gsettings_string(reply: &str) -> Option<String> {
    let reply = reply.trim();
    let inner = reply
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| reply.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(reply);
    (!inner.is_empty()).then(|| inner.to_owned())
}

/// The value of `key` in `[group]` of a small INI file.
fn ini_value(path: &Path, group: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_group = name == group;
        } else if in_group
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            let v = v.trim().trim_matches('"');
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    None
}
