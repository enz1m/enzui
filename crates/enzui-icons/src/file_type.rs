//! Filename to MIME to bundled glyph: the per-type icon for a file name.
//!
//! # Example
//!
//! ```
//! use enzui_icons::file_type::FileTypeTable;
//!
//! let table = FileTypeTable::host();
//!
//! assert_eq!(table.icon_name("main.rs"), "file-code");
//! assert_eq!(table.icon_name("photo.png"), "file-image");
//! assert_eq!(table.icon_name("Cargo.toml"), "file-sliders");
//! assert_eq!(FileTypeTable::DIRECTORY_ICON, "folder");
//! ```
//!
//! # Options
//!
//! * [`FileTypeTable::host`] — the host's `globs2` database, cached
//!   process-wide, or the built-in fallback table on a host with none.
//! * [`FileTypeTable::from_globs`] — an explicit [`GlobTable`].
//! * [`FileTypeTable::icon_name`] — the bundled Lucide name for a filename.
//! * [`FileTypeTable::classify`] — the same lookup, also reporting the MIME
//!   string and the [`Certainty`].
//! * [`icon_for_mime`] — the MIME half alone, for a caller that already holds
//!   a MIME type.
//!
//! # Hazards
//!
//! The answer is advisory: it derives from the filename, so a renamed file
//! misreports and [`FileTypeClass::certainty`] says how much to trust it.
//!
//! `icon_name` never answers `folder`. Directories and symlinks are decided by
//! the filesystem rather than the name, so the caller applies
//! [`FileTypeTable::DIRECTORY_ICON`] / [`FileTypeTable::SYMLINK_ICON`].
use crate::mime::{Certainty, GlobRule, GlobTable, MimeMatch};
use std::sync::OnceLock;

/// The result of classifying a filename: the MIME type, how certain that
/// determination is, and the bundled Lucide glyph to draw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTypeClass {
    /// The resolved MIME type, e.g. `text/rust`. Never empty — an unmatched
    /// filename yields `application/octet-stream`.
    pub mime: String,
    /// How the MIME type was determined. [`Certainty::Fallback`] means nothing
    /// matched and `mime` is the generic catch-all.
    pub certainty: Certainty,
    /// The bundled Lucide icon name; always resolvable through
    /// [`crate::bundled_icon`].
    pub icon: &'static str,
}

impl FileTypeClass {
    /// How much to trust [`Self::mime`].
    pub fn certainty(&self) -> Certainty {
        self.certainty
    }
}

/// A filename classifier over a freedesktop glob database.
///
/// Build one per app, or take the process-cached [`FileTypeTable::host`].
pub struct FileTypeTable {
    globs: GlobTable,
}

impl FileTypeTable {
    /// The glyph for a directory. The caller applies it; `icon_name` never
    /// returns it.
    pub const DIRECTORY_ICON: &'static str = "folder";

    /// The glyph for a symbolic link whose target is not a directory.
    pub const SYMLINK_ICON: &'static str = "file-symlink";

    /// The glyph for a symbolic link whose target *is* a directory.
    pub const DIRECTORY_SYMLINK_ICON: &'static str = "folder-symlink";

    /// The host's freedesktop `globs2` database, discovered once per process
    /// and shared thereafter.
    ///
    /// Falls back to [`Self::builtin`] on a host with no `shared-mime-info`.
    pub fn host() -> &'static FileTypeTable {
        static HOST: OnceLock<FileTypeTable> = OnceLock::new();
        HOST.get_or_init(|| {
            let (parsed, files_read) = crate::mime::xdg::discover_globs2();
            if files_read == 0 || parsed.rules.is_empty() {
                Self::builtin()
            } else {
                Self::from_globs(GlobTable::new(parsed.rules))
            }
        })
    }

    /// A classifier over an explicit glob table.
    pub fn from_globs(globs: GlobTable) -> Self {
        Self { globs }
    }

    /// A small built-in table for hosts with no `shared-mime-info` database.
    ///
    /// Covers only the common families (source, image, audio, video, archive,
    /// document), so [`Self::host`] reaches it only on a minimal system.
    pub fn builtin() -> Self {
        let rules = BUILTIN_GLOBS
            .iter()
            .map(|(mime, pattern)| GlobRule::glob(*mime, *pattern))
            .collect();
        Self::from_globs(GlobTable::new(rules))
    }

    /// The bundled Lucide icon name for `filename`.
    ///
    /// Total: every filename yields a name [`crate::bundled_icon`] resolves.
    /// Only the base name is consulted, so a path works as well as a name.
    pub fn icon_name(&self, filename: &str) -> &'static str {
        self.classify(filename).icon
    }

    /// Classify `filename`: MIME type, certainty, and glyph.
    pub fn classify(&self, filename: &str) -> FileTypeClass {
        let MimeMatch { mime, certainty } = self.globs.resolve_or_fallback(filename);
        let icon = icon_for_mime(&mime);
        FileTypeClass {
            mime,
            certainty,
            icon,
        }
    }
}

/// The bundled Lucide glyph for a MIME type.
///
/// Type and subtype tokens are matched ASCII case-insensitively, so a
/// `Content-Type` such as `IMAGE/PNG` resolves like `image/png`. Total by
/// construction: the media-type arm answers every input, malformed included.
pub fn icon_for_mime(mime: &str) -> &'static str {
    let normalized = NormalizedMime::new(mime);
    let base = normalized.base();
    let (media, sub) = normalized.parts();

    if let Some(icon) = EXACT_MIME_ICONS
        .iter()
        .find(|(candidate, _)| *candidate == base)
        .map(|(_, icon)| *icon)
    {
        return icon;
    }
    if let Some(icon) = SUFFIX_ICONS
        .iter()
        .find(|(suffix, _)| sub.ends_with(suffix))
        .map(|(_, icon)| *icon)
    {
        return icon;
    }
    MEDIA_TYPE_ICONS
        .iter()
        .find(|(candidate, _)| *candidate == media)
        .map_or("file", |(_, icon)| *icon)
}

/// The conventional colour family of a file-type icon.
///
/// One variant per *format family*, not per hue: two families that a user tells
/// apart at a glance carry different variants even when both are conventionally
/// "yellow", because a renderer that painted them identically would defeat the
/// point of colouring the glyph at all. Families a user groups together on
/// purpose — everything a text editor opens, every raster image format — share
/// one variant so they stay visually one group.
///
/// Resolve a paintable colour with
/// `enzui_widgets::tokens::file_type_ink`, which owns the palette roles
/// and the non-text contrast floor. The variants here carry no colour values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileTypeTone {
    /// No type-specific association: unknown data, containers, binaries, keys.
    Neutral,
    /// Prose documents.
    Document,
    /// Databases and structured stores.
    Data,
    /// PDF.
    Pdf,
    /// Slide decks.
    Presentation,
    /// Moving pictures.
    Video,
    /// Compressed archives.
    Archive,
    /// Installable distribution packages.
    Package,
    /// Audio.
    Audio,
    /// Ebooks.
    Ebook,
    /// Spreadsheets and tabular data.
    Spreadsheet,
    /// Transfer descriptors, such as a torrent metainfo file.
    Transfer,
    /// Raster and vector images.
    Image,
    /// Anything a code editor opens: sources, scripts, configuration.
    Code,
    /// Fonts.
    Font,
}

impl FileTypeTone {
    /// Every tone, in declaration order.
    ///
    /// `tone_inventory_is_exactly_the_tones_the_resolver_emits` holds this
    /// equal to the tones [`tone_for_mime`] can return, so a renderer can
    /// audit its own palette against the whole vocabulary without hand-listing
    /// it.
    pub const ALL: &'static [FileTypeTone] = &[
        Self::Neutral,
        Self::Document,
        Self::Data,
        Self::Pdf,
        Self::Presentation,
        Self::Video,
        Self::Archive,
        Self::Package,
        Self::Audio,
        Self::Ebook,
        Self::Spreadsheet,
        Self::Transfer,
        Self::Image,
        Self::Code,
        Self::Font,
    ];
}

/// Returns the conventional color family for a MIME type.
///
/// The tone follows [`icon_for_mime`], so every MIME value that resolves to the
/// same glyph also resolves to the same tone. Unknown and malformed values use
/// [`FileTypeTone::Neutral`].
#[must_use]
pub fn tone_for_mime(mime: &str) -> FileTypeTone {
    let icon = icon_for_mime(mime);
    ICON_TONES
        .iter()
        .find(|(candidate, _)| *candidate == icon)
        .map_or(FileTypeTone::Neutral, |(_, tone)| *tone)
}

const SUFFIX_ICONS: &[(&str, &str)] = &[
    ("+xml", "file-code"),
    ("+json", "file-code"),
    ("+zip", "file-archive"),
    ("+gzip", "file-archive"),
];

const MEDIA_TYPE_ICONS: &[(&str, &str)] = &[
    ("image", "file-image"),
    ("audio", "file-music"),
    ("video", "file-video-camera"),
    ("font", "file-type"),
    ("text", "file-text"),
    ("inode", FileTypeTable::DIRECTORY_ICON),
    ("model", "file-box"),
];

const ICON_TONES: &[(&str, FileTypeTone)] = &[
    ("file", FileTypeTone::Neutral),
    (FileTypeTable::DIRECTORY_ICON, FileTypeTone::Neutral),
    ("file-box", FileTypeTone::Neutral),
    ("file-cog", FileTypeTone::Neutral),
    ("file-key", FileTypeTone::Neutral),
    ("file-lock", FileTypeTone::Neutral),
    ("database", FileTypeTone::Data),
    ("file-text", FileTypeTone::Document),
    ("file-badge", FileTypeTone::Pdf),
    ("file-play", FileTypeTone::Presentation),
    ("file-video-camera", FileTypeTone::Video),
    ("book-text", FileTypeTone::Ebook),
    ("file-archive", FileTypeTone::Archive),
    ("package", FileTypeTone::Package),
    ("file-music", FileTypeTone::Audio),
    ("file-spreadsheet", FileTypeTone::Spreadsheet),
    ("file-down", FileTypeTone::Transfer),
    ("file-code", FileTypeTone::Code),
    ("file-image", FileTypeTone::Image),
    ("file-sliders", FileTypeTone::Code),
    ("file-terminal", FileTypeTone::Code),
    ("file-type", FileTypeTone::Font),
];

struct NormalizedMime<'a> {
    original: &'a str,
    lowercase: Option<String>,
}

impl<'a> NormalizedMime<'a> {
    fn new(mime: &'a str) -> Self {
        let original = mime.split(';').next().unwrap_or(mime).trim();
        let lowercase = original
            .bytes()
            .any(|byte| byte.is_ascii_uppercase())
            .then(|| original.to_ascii_lowercase());
        Self {
            original,
            lowercase,
        }
    }

    fn base(&self) -> &str {
        self.lowercase.as_deref().unwrap_or(self.original)
    }

    fn parts(&self) -> (&str, &str) {
        self.base()
            .split_once('/')
            .unwrap_or_else(|| (self.base(), ""))
    }
}

// MIME types that deserve a glyph their media type would not give them.
const EXACT_MIME_ICONS: &[(&str, &str)] = &[
    // ---- source code -------------------------------------------------
    ("text/rust", "file-code"),
    ("text/x-rust", "file-code"),
    ("text/x-csrc", "file-code"),
    ("text/x-chdr", "file-code"),
    ("text/x-c++src", "file-code"),
    ("text/x-c++hdr", "file-code"),
    ("text/x-python", "file-code"),
    ("text/x-python3", "file-code"),
    ("text/x-java", "file-code"),
    ("text/x-go", "file-code"),
    ("text/x-lua", "file-code"),
    ("text/x-perl", "file-code"),
    ("text/x-scala", "file-code"),
    ("text/x-kotlin", "file-code"),
    ("text/x-haskell", "file-code"),
    ("text/x-csharp", "file-code"),
    ("text/javascript", "file-code"),
    ("text/css", "file-code"),
    ("text/html", "file-code"),
    ("text/xml", "file-code"),
    ("application/xml", "file-code"),
    ("application/json", "file-code"),
    ("application/javascript", "file-code"),
    ("application/sql", "file-code"),
    ("application/x-ruby", "file-code"),
    ("application/x-php", "file-code"),
    ("application/x-perl", "file-code"),
    ("text/x-ruby", "file-code"),
    ("text/x-php", "file-code"),
    ("text/x-swift", "file-code"),
    ("text/x-dart", "file-code"),
    ("text/x-zig", "file-code"),
    ("text/x-typescript", "file-code"),
    ("application/typescript", "file-code"),
    ("text/x-sql", "file-code"),
    ("text/x-patch", "file-code"),
    // ---- shell scripts ----------------------------------------------
    ("text/x-shellscript", "file-terminal"),
    ("application/x-shellscript", "file-terminal"),
    ("text/x-makefile", "file-terminal"),
    // ---- configuration ------------------------------------------------
    ("application/toml", "file-sliders"),
    ("application/yaml", "file-sliders"),
    ("application/x-yaml", "file-sliders"),
    ("text/x-ini", "file-sliders"),
    ("application/x-desktop", "file-sliders"),
    ("text/x-cmake", "file-sliders"),
    ("text/x-meson", "file-sliders"),
    // ---- documents ----------------------------------------------------
    ("application/pdf", "file-badge"),
    ("text/markdown", "file-text"),
    ("text/x-markdown", "file-text"),
    ("application/rtf", "file-text"),
    ("application/msword", "file-text"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "file-text",
    ),
    ("application/vnd.oasis.opendocument.text", "file-text"),
    ("application/epub+zip", "book-text"),
    ("application/x-mobipocket-ebook", "book-text"),
    ("application/vnd.amazon.mobi8-ebook", "book-text"),
    ("application/x-fictionbook+xml", "book-text"),
    ("image/vnd.djvu", "book-text"),
    // ---- images ---------------------------------------------------------
    // Without this row the `+xml` suffix rule files a picture with the source
    // code. No other image type needs one, `image/svg+xml-compressed`
    // included: no suffix claims them, so their media type answers.
    ("image/svg+xml", "file-image"),
    // ---- spreadsheets --------------------------------------------------
    ("text/csv", "file-spreadsheet"),
    ("text/tab-separated-values", "file-spreadsheet"),
    ("application/vnd.ms-excel", "file-spreadsheet"),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "file-spreadsheet",
    ),
    (
        "application/vnd.oasis.opendocument.spreadsheet",
        "file-spreadsheet",
    ),
    // ---- presentations -------------------------------------------------
    ("application/vnd.ms-powerpoint", "file-play"),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "file-play",
    ),
    (
        "application/vnd.oasis.opendocument.presentation",
        "file-play",
    ),
    // ---- archives ------------------------------------------------------
    ("application/zip", "file-archive"),
    ("application/gzip", "file-archive"),
    ("application/x-gzip", "file-archive"),
    ("application/x-tar", "file-archive"),
    ("application/x-compressed-tar", "file-archive"),
    ("application/x-bzip", "file-archive"),
    ("application/x-bzip2", "file-archive"),
    ("application/x-bzip-compressed-tar", "file-archive"),
    ("application/x-xz", "file-archive"),
    ("application/x-xz-compressed-tar", "file-archive"),
    ("application/x-7z-compressed", "file-archive"),
    ("application/vnd.rar", "file-archive"),
    ("application/x-rar-compressed", "file-archive"),
    ("application/zstd", "file-archive"),
    ("application/x-lzma", "file-archive"),
    ("application/x-lz4", "file-archive"),
    ("application/x-java-archive", "file-archive"),
    ("application/x-apple-diskimage", "file-archive"),
    ("application/vnd.efi.iso", "file-archive"),
    ("application/x-cd-image", "file-archive"),
    // ---- distribution packages ------------------------------------------
    // An installable package is an archive a user never unpacks by hand, so it
    // reads as its own family rather than as one more `.tar.*`.
    ("application/vnd.debian.binary-package", "package"),
    ("application/x-deb", "package"),
    ("application/x-rpm", "package"),
    ("application/x-redhat-package-manager", "package"),
    ("application/vnd.flatpak", "package"),
    ("application/vnd.flatpak.ref", "package"),
    ("application/vnd.flatpak.repo", "package"),
    ("application/vnd.snap", "package"),
    ("application/vnd.appimage", "package"),
    ("application/vnd.android.package-archive", "package"),
    ("application/x-msi", "package"),
    ("application/x-xar", "package"),
    ("application/x-arch-package", "package"),
    // ---- executables and libraries -------------------------------------
    ("application/x-executable", "file-cog"),
    ("application/x-sharedlib", "file-cog"),
    ("application/x-object", "file-cog"),
    ("application/x-msdownload", "file-cog"),
    ("application/x-ms-dos-executable", "file-cog"),
    // ---- keys and certificates -----------------------------------------
    ("application/x-x509-ca-cert", "file-key"),
    ("application/pgp-keys", "file-key"),
    ("application/pgp-encrypted", "file-lock"),
    ("application/pkcs8", "file-key"),
    ("application/x-pem-file", "file-key"),
    // ---- transfer descriptors -------------------------------------------
    // A `.torrent` carries no payload: it is the instruction to fetch one, and
    // a file manager that painted it as generic data hid the one thing that
    // makes it recognisable.
    ("application/x-bittorrent", "file-down"),
    ("application/metalink+xml", "file-down"),
    ("application/metalink4+xml", "file-down"),
    // ---- data -----------------------------------------------------------
    ("application/vnd.sqlite3", "database"),
    ("application/x-sqlite3", "database"),
    // ---- explicitly generic --------------------------------------------
    // A deliberate row: `octet-stream` means unknown, which is the plain glyph.
    ("application/octet-stream", "file"),
];

// The minimal glob set `FileTypeTable::builtin` uses. freedesktop `globs2`
// syntax, so the host's `GlobTable` precedence applies: `*.tar.gz` beats
// `*.gz`, which an extension map could not express.
const BUILTIN_GLOBS: &[(&str, &str)] = &[
    ("text/rust", "*.rs"),
    ("text/x-csrc", "*.c"),
    ("text/x-chdr", "*.h"),
    ("text/x-c++src", "*.cpp"),
    ("text/x-python", "*.py"),
    ("text/x-go", "*.go"),
    ("text/x-java", "*.java"),
    ("text/javascript", "*.js"),
    ("text/css", "*.css"),
    ("text/html", "*.html"),
    ("text/html", "*.htm"),
    ("application/json", "*.json"),
    ("application/xml", "*.xml"),
    ("application/toml", "*.toml"),
    ("application/yaml", "*.yaml"),
    ("application/yaml", "*.yml"),
    ("text/x-shellscript", "*.sh"),
    ("text/x-makefile", "Makefile"),
    ("text/plain", "*.txt"),
    ("text/markdown", "*.md"),
    ("text/csv", "*.csv"),
    ("application/pdf", "*.pdf"),
    ("image/png", "*.png"),
    ("image/jpeg", "*.jpg"),
    ("image/jpeg", "*.jpeg"),
    ("image/gif", "*.gif"),
    ("image/webp", "*.webp"),
    ("image/svg+xml", "*.svg"),
    ("image/svg+xml-compressed", "*.svgz"),
    ("image/tiff", "*.tiff"),
    ("image/avif", "*.avif"),
    ("audio/mpeg", "*.mp3"),
    ("audio/flac", "*.flac"),
    ("audio/ogg", "*.ogg"),
    ("audio/wav", "*.wav"),
    ("video/mp4", "*.mp4"),
    ("video/matroska", "*.mkv"),
    ("video/webm", "*.webm"),
    ("application/zip", "*.zip"),
    ("application/x-tar", "*.tar"),
    ("application/gzip", "*.gz"),
    ("application/x-compressed-tar", "*.tar.gz"),
    ("application/x-xz", "*.xz"),
    ("application/x-7z-compressed", "*.7z"),
    ("application/zstd", "*.zst"),
    ("application/vnd.rar", "*.rar"),
    ("application/x-bittorrent", "*.torrent"),
    ("application/vnd.debian.binary-package", "*.deb"),
    ("application/x-rpm", "*.rpm"),
    ("application/vnd.appimage", "*.AppImage"),
    ("application/vnd.android.package-archive", "*.apk"),
    ("application/vnd.flatpak", "*.flatpak"),
    ("application/epub+zip", "*.epub"),
    ("application/x-mobipocket-ebook", "*.mobi"),
    ("application/vnd.sqlite3", "*.sqlite"),
    ("font/ttf", "*.ttf"),
    ("font/woff2", "*.woff2"),
];
