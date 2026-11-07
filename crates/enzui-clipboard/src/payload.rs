//! What is offered to, and what arrives from, the selection.
//!
//! A Wayland selection is not a value the compositor stores; it is a promise by
//! the owning client to produce bytes for any of the MIME names it advertised.
//! [`Payload`] is therefore the whole promise — every alias and the bytes each
//! alias resolves to — held for as long as the application owns the selection.

use std::collections::HashSet;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use enzui_security::secret::SecretText;

use crate::mime::{HTML_MIME_TYPES, MimeType, TEXT_MIME_TYPES};

/// Whether a file selection was placed for copying or for moving.
///
/// Desktop file managers embed this intent in the `x-special/gnome-copied-files`
/// MIME payload so that a paste into another window can decide between
/// duplicating the files (copy) and relocating them (cut). Plain
/// `text/uri-list` does not carry intent; when only that type is available the
/// framework defaults to [`FileIntent::Copy`] rather than guessing cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileIntent {
    /// Request copying the files at the destination; performs no filesystem operation.
    Copy,
    /// Request moving the files at the destination; performs no filesystem operation.
    Cut,
}

impl fmt::Display for FileIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Copy => "copy",
            Self::Cut => "cut",
        })
    }
}

/// A file selection with its copy/cut intent.
///
/// [`Pasted::file_transfer`] validates incoming paths; [`Self::new`] trusts the
/// caller's paths. Debug output exposes the intent and count, not file names.
#[derive(Clone, PartialEq, Eq)]
pub struct FileTransfer {
    intent: FileIntent,
    paths: Vec<PathBuf>,
}

impl fmt::Debug for FileTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileTransfer")
            .field("intent", &self.intent)
            .field("path_count", &self.paths.len())
            .finish_non_exhaustive()
    }
}

impl FileTransfer {
    /// Store an intent and paths without validation or filesystem access.
    ///
    /// Accepts empty lists, relative paths and paths containing control bytes.
    pub fn new(intent: FileIntent, paths: Vec<PathBuf>) -> Self {
        Self { intent, paths }
    }

    /// The intent the source declared when placing the files.
    pub fn intent(&self) -> FileIntent {
        self.intent
    }

    /// The stored paths; constructor-supplied paths are not validated.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Consumes the transfer and returns the paths.
    pub fn into_paths(self) -> Vec<PathBuf> {
        self.paths
    }
}

/// Why a file URI was rejected during clipboard transfer validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileUriRejection {
    /// The URI does not use the `file` scheme.
    NotFileScheme,
    /// The host part names a machine other than `localhost`.
    RemoteHost,
    /// A percent-escape is incomplete or uses a non-hex digit.
    MalformedEncoding,
    /// The decoded path contains NUL or an ASCII control character.
    ControlCharacter,
    /// The decoded path is not absolute.
    RelativePath,
    /// The URI exceeds the per-entry byte limit.
    TooLong,
}

impl fmt::Display for FileUriRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFileScheme => "not a file:// URI",
            Self::RemoteHost => "remote host in file URI",
            Self::MalformedEncoding => "malformed percent encoding",
            Self::ControlCharacter => "NUL or control character in decoded path",
            Self::RelativePath => "decoded path is not absolute",
            Self::TooLong => "URI exceeds per-entry byte limit",
        })
    }
}

/// Maximum file URIs admitted from a single clipboard transfer.
pub const MAX_FILE_URI_COUNT: usize = 16_384;

/// Maximum byte length of a single file URI line.
pub const MAX_FILE_URI_BYTES: usize = 8_192;

#[derive(Clone)]
struct Entry {
    mime: MimeType,
    bytes: OfferedBytes,
}

#[derive(Clone)]
pub(crate) enum OfferedBytes {
    Ordinary(Arc<[u8]>),
    Sensitive(SecretText),
    Deferred(Arc<crate::deferred::Shared>),
}

impl OfferedBytes {
    pub(crate) fn immediate(&self) -> Option<&[u8]> {
        match self {
            Self::Ordinary(bytes) => Some(bytes),
            Self::Sensitive(text) => Some(text.expose_secret().as_bytes()),
            Self::Deferred(_) => None,
        }
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut entry = f.debug_struct("Entry");
        entry.field("mime", &self.mime);
        match self.bytes.immediate() {
            Some(bytes) => entry.field("bytes", &format_args!("[redacted; {}]", bytes.len())),
            None => entry.field("bytes", &"[deferred]"),
        };
        entry.finish()
    }
}

/// The content an application offers while it owns the selection.
///
/// MIME aliases share one allocation. Cloning a payload shares its byte buffers.
/// Debug output reports MIME names and sizes while redacting content.
#[derive(Clone, Default)]
#[must_use]
pub struct Payload {
    entries: Vec<Entry>,
    expires: Option<Instant>,
}

/// Lists the advertised MIME names and their sizes, never the offered bytes.
impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Payload")
            .field("entries", &self.entries)
            .finish()
    }
}

impl Payload {
    pub(crate) fn deferred(source: Arc<crate::deferred::Shared>) -> Self {
        Self {
            entries: TEXT_MIME_TYPES
                .iter()
                .map(|mime| Entry {
                    mime: MimeType::new(*mime),
                    bytes: OfferedBytes::Deferred(Arc::clone(&source)),
                })
                .collect(),
            expires: Some(source.expires),
        }
    }

    pub(crate) fn deferred_source(&self) -> Option<&Arc<crate::deferred::Shared>> {
        self.entries.iter().find_map(|entry| match &entry.bytes {
            OfferedBytes::Deferred(source) => Some(source),
            _ => None,
        })
    }

    pub(crate) fn revoke_deferred(&self) {
        if let Some(source) = self.deferred_source() {
            source.revoke();
        }
    }

    pub(crate) fn confirm_deferred(&self) -> Result<(), crate::ClipboardError> {
        self.deferred_source()
            .map_or(Ok(()), |source| source.claimed())
    }

    /// Offer protected UTF-8 text for a positive lifetime of at most 120 seconds.
    ///
    /// Plain-text MIME aliases share the protected allocation. The password-manager
    /// hint requests exclusion from clipboard history. This is a peer hint, not
    /// enforcement over readers that already received the text. Sensitive offers
    /// never enter the exit keeper and cannot be offered as primary selection or drag data.
    ///
    /// # Errors
    /// Refuses zero, over-limit or unrepresentable lifetimes, and text above 64 KiB.
    pub fn sensitive_text(
        text: SecretText,
        lifetime: Duration,
    ) -> Result<Self, crate::ClipboardError> {
        if lifetime.is_zero() || lifetime > Duration::from_secs(120) {
            return Err(crate::ClipboardError::Unavailable {
                reason: "sensitive clipboard lifetime must be positive and at most 120 seconds"
                    .into(),
            });
        }
        if text.expose_secret().len() > 64 * 1024 {
            return Err(crate::ClipboardError::TransferTooLarge { limit: 64 * 1024 });
        }
        let expires = Instant::now().checked_add(lifetime).ok_or_else(|| {
            crate::ClipboardError::Unavailable {
                reason: "sensitive clipboard deadline cannot be represented".into(),
            }
        })?;
        let mut payload = Self {
            entries: TEXT_MIME_TYPES
                .iter()
                .map(|mime| Entry {
                    mime: MimeType::new(*mime),
                    bytes: OfferedBytes::Sensitive(text.clone()),
                })
                .collect(),
            expires: Some(expires),
        };
        payload.insert(
            MimeType::new("x-kde-passwordManagerHint"),
            Arc::from(&b"secret"[..]),
        );
        Ok(payload)
    }

    /// Whether this offer is protected, expires and must not be kept after exit.
    pub fn is_sensitive(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry.bytes, OfferedBytes::Sensitive(_)))
    }

    pub(crate) fn expires_at(&self) -> Option<Instant> {
        self.expires
    }

    pub(crate) fn offered_bytes(&self, mime: &MimeType) -> Option<OfferedBytes> {
        self.entries
            .iter()
            .find(|entry| entry.mime.normalized() == mime.normalized())
            .map(|entry| entry.bytes.clone())
    }

    /// An empty payload. Offering it is refused; use [`crate::WaylandClipboard::clear`]
    /// to give up the selection instead.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one MIME name with its own bytes.
    ///
    /// A normalized duplicate replaces its bytes while preserving the original
    /// wire spelling and advertisement position. No MIME or byte-size validation is performed.
    pub fn with(mut self, mime: impl Into<MimeType>, bytes: impl Into<Arc<[u8]>>) -> Self {
        self.insert(mime.into(), bytes.into());
        self
    }

    /// Adds several MIME names that all resolve to the same bytes.
    pub fn with_aliases<M>(
        mut self,
        mimes: impl IntoIterator<Item = M>,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self
    where
        M: Into<MimeType>,
    {
        let bytes = bytes.into();

        for mime in mimes {
            self.insert(mime.into(), Arc::clone(&bytes));
        }

        self
    }

    /// Offer UTF-8 text using this crate's plain-text MIME aliases.
    pub fn text(text: impl AsRef<str>) -> Self {
        let bytes: Arc<[u8]> = Arc::from(text.as_ref().as_bytes());

        Self::new().with_aliases(TEXT_MIME_TYPES.iter().copied(), bytes)
    }

    /// HTML plus a plain-text fallback, so a peer that cannot read HTML still
    /// gets the text rather than an empty paste.
    pub fn html(html: impl AsRef<str>, plain_text: impl AsRef<str>) -> Self {
        let html_bytes: Arc<[u8]> = Arc::from(html.as_ref().as_bytes());

        Self::text(plain_text).with_aliases(HTML_MIME_TYPES.iter().copied(), html_bytes)
    }

    /// An encoded image under its own MIME name, for example `image/png`.
    pub fn image(mime: impl Into<MimeType>, encoded: impl Into<Arc<[u8]>>) -> Self {
        Self::new().with(mime, encoded)
    }

    /// A file selection as `file://` URIs with [`FileIntent::Copy`] intent.
    ///
    /// Equivalent to `Payload::file_transfer(paths, FileIntent::Copy)`. The
    /// payload advertises `x-special/gnome-copied-files` with a `copy` intent
    /// header and `text/uri-list` with standard RFC 2483 bytes.
    ///
    /// Paths are percent-encoded byte by byte, so a name that is not valid
    /// UTF-8 survives the round trip instead of being replaced by U+FFFD.
    pub fn uris<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Self {
        Self::file_transfer(paths, FileIntent::Copy)
    }

    /// A file selection with explicit copy/cut intent.
    ///
    /// Publishes two independent representations:
    ///
    /// * `x-special/gnome-copied-files` — the GNOME desktop format with an
    ///   intent header (`copy` or `cut`) followed by LF-separated `file://`
    ///   URIs, with no empty or trailing line.
    /// * `text/uri-list` — the RFC 2483 format with CRLF-terminated lines
    ///   and no intent header.
    ///
    /// Paths that are relative, empty, or contain NUL/control characters are
    /// silently excluded. Duplicates under filesystem-path equality are removed.
    /// At most [`MAX_FILE_URI_COUNT`] unique valid paths are kept. No filesystem
    /// existence check or per-URI byte-length limit is applied to outgoing paths.
    /// Incoming validated parsing can therefore reject a long outgoing URI.
    pub fn file_transfer<P: AsRef<Path>>(
        paths: impl IntoIterator<Item = P>,
        intent: FileIntent,
    ) -> Self {
        let mut seen = HashSet::new();
        let mut uris: Vec<String> = Vec::new();

        for path in paths {
            let path = path.as_ref();
            if !is_valid_local_path(path) {
                continue;
            }
            let owned = path.to_path_buf();
            if !seen.insert(owned) {
                continue;
            }
            if uris.len() >= MAX_FILE_URI_COUNT {
                break;
            }
            uris.push(file_uri(path));
        }

        if uris.is_empty() {
            return Self::new();
        }

        let mut uri_list = String::new();
        for uri in &uris {
            uri_list.push_str(uri);
            uri_list.push_str("\r\n");
        }

        // Nautilus rejects empty entries, including one created by a trailing LF.
        let header = match intent {
            FileIntent::Copy => "copy",
            FileIntent::Cut => "cut",
        };
        let mut gnome = String::with_capacity(header.len() + 1 + uri_list.len());
        gnome.push_str(header);
        for uri in &uris {
            gnome.push('\n');
            gnome.push_str(uri);
        }
        Self::new()
            .with("x-special/gnome-copied-files", gnome.into_bytes())
            .with("text/uri-list", uri_list.into_bytes())
    }

    /// Whether every representation is ordinary bytes with no selection expiry.
    /// Protected and deferred offers remain selection-only even if an alias is
    /// subsequently replaced. MIME syntax and transfer bounds are checked separately.
    pub fn is_ordinary(&self) -> bool {
        self.expires.is_none()
            && self
                .entries
                .iter()
                .all(|entry| matches!(&entry.bytes, OfferedBytes::Ordinary(_)))
    }

    /// Every advertised name, in advertisement order.
    pub fn mime_types(&self) -> Vec<MimeType> {
        self.entries
            .iter()
            .map(|entry| entry.mime.clone())
            .collect()
    }

    /// Ordinary bytes for one advertised name, comparing names in normalised form.
    /// Sensitive or deferred text returns `None`; it is served only by the live selection source.
    pub fn bytes_for(&self, mime: &MimeType) -> Option<Arc<[u8]>> {
        let wanted = mime.normalized();

        self.entries
            .iter()
            .find(|entry| entry.mime.normalized() == wanted)
            .and_then(|entry| match &entry.bytes {
                OfferedBytes::Ordinary(bytes) => Some(Arc::clone(bytes)),
                OfferedBytes::Sensitive(_) | OfferedBytes::Deferred(_) => None,
            })
    }

    /// Whether the payload advertises nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert(&mut self, mime: MimeType, bytes: Arc<[u8]>) {
        let normalized = mime.normalized();

        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.mime.normalized() == normalized)
        {
            existing.bytes = OfferedBytes::Ordinary(bytes);
        } else {
            self.entries.push(Entry {
                mime,
                bytes: OfferedBytes::Ordinary(bytes),
            });
        }
    }
}

/// Content received from the selection.
#[derive(Clone, PartialEq, Eq)]
pub struct Pasted {
    mime: MimeType,
    bytes: Vec<u8>,
}

/// Reports the negotiated MIME name and transfer size, never the content.
impl fmt::Debug for Pasted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pasted")
            .field("mime", &self.mime)
            .field("bytes", &format_args!("[redacted; {}]", self.bytes.len()))
            .finish()
    }
}

impl Pasted {
    /// Builds a result from the negotiated type and the transferred bytes.
    pub fn new(mime: MimeType, bytes: Vec<u8>) -> Self {
        Self { mime, bytes }
    }

    /// The type that was actually negotiated, which may be a fallback rather
    /// than the caller's first choice.
    pub fn mime(&self) -> &MimeType {
        &self.mime
    }

    /// The transferred bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the result and returns the bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The bytes as text, when they are valid UTF-8.
    ///
    /// Returns `None` for invalid UTF-8, regardless of the stored MIME name.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// Return trimmed nonempty, non-comment text lines without MIME or URI validation.
    ///
    /// Invalid UTF-8 returns an empty list; no entry-count or length limits are applied.
    pub fn uris(&self) -> Vec<String> {
        let Some(text) = self.text() else {
            return Vec::new();
        };

        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect()
    }

    /// Decode local file-URI lines using [`file_uri_path`], without checking MIME.
    ///
    /// This path permits decoded control bytes and has no size/count limits.
    /// Use [`Self::file_transfer`] for validated, bounded entry parsing.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.uris()
            .iter()
            .filter_map(|uri| file_uri_path(uri))
            .collect()
    }

    /// Parses the pasted bytes into a validated [`FileTransfer`] with intent.
    ///
    /// The negotiated MIME type determines the format:
    ///
    /// * `x-special/gnome-copied-files` — the first line is trimmed and matched
    ///   against `copy` or `cut` ignoring ASCII case; later lines are file URIs.
    /// * `text/uri-list` — standard RFC 2483 format; intent is always
    ///   [`FileIntent::Copy`] because plain URI lists carry no intent.
    ///
    /// Individual URIs are validated by [`validate_file_uri`]: remote hosts,
    /// non-file schemes, malformed percent encoding, NUL/control characters,
    /// and relative paths are silently excluded. Duplicate paths are removed.
    /// Returns `None` when the MIME type is unrecognised or no valid local
    /// path remains after filtering, or the bytes are not UTF-8. Validates at most
    /// [`MAX_FILE_URI_COUNT`] nonempty entries (excluding URI-list comments);
    /// invalid and duplicate entries count toward that limit. No filesystem access occurs.
    pub fn file_transfer(&self) -> Option<FileTransfer> {
        let gnome_mime = MimeType::new("x-special/gnome-copied-files");
        let uri_list_mime = MimeType::new("text/uri-list");

        if self.mime().matches(&gnome_mime) {
            parse_gnome_copied_files(&self.bytes)
        } else if self.mime().matches(&uri_list_mime) {
            parse_uri_list_as_transfer(&self.bytes)
        } else {
            None
        }
    }
}

/// Decode a local `file://` URI, preserving arbitrary decoded path bytes.
///
/// Trims surrounding whitespace and accepts empty authority or exactly lowercase
/// `localhost`. Returns `None` for other hosts, schemes or malformed escapes.
/// No length, control-byte or filesystem checks are applied; use
/// [`validate_file_uri`] when validating clipboard input.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let rest = uri.trim().strip_prefix("file://")?;

    // Everything up to the first slash is the host: empty or `localhost` means
    // this machine, and anything else names a file that is not here.
    let (host, path) = rest.split_at(rest.find('/')?);

    if !host.is_empty() && host != "localhost" {
        return None;
    }

    let mut decoded: Vec<u8> = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();

    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }

        let high = digit(bytes.next()?)?;
        let low = digit(bytes.next()?)?;
        decoded.push(high << 4 | low);
    }

    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Remove spans delimited by `<` and `>` from a string.
///
/// This is not an HTML parser: it does not decode entities, insert block spacing,
/// or exclude script/style content. An unmatched `<` discards the remaining text.
pub fn plain_text_from_html(html: &str) -> String {
    let mut plain = String::with_capacity(html.len());
    let mut inside_tag = false;

    for character in html.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => plain.push(character),
            _ => {}
        }
    }

    plain
}

fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");

    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'/' => uri.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char);
            }
            other => {
                let _ = write!(uri, "%{other:02X}");
            }
        }
    }

    uri
}

fn is_valid_local_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || !path.is_absolute() {
        return false;
    }
    bytes.iter().all(|&b| b >= 0x20 && b != 0x7F)
}

/// Validates a single `file://` URI and returns the local path it names.
///
/// Rejects remote hosts, non-file schemes, malformed percent encoding,
/// NUL/control characters in the decoded path, relative paths, and URIs
/// longer than [`MAX_FILE_URI_BYTES`].
///
/// Trims surrounding whitespace; the `file://` prefix is case-sensitive while
/// `localhost` is ASCII-case-insensitive. Preserves non-UTF-8 decoded bytes and
/// does not check existence, resolve symlinks, or restrict directory traversal.
///
/// # Errors
///
/// Returns [`FileUriRejection`] for excessive length, scheme/host mismatch,
/// malformed escapes, decoded control bytes or a non-absolute path.
pub fn validate_file_uri(uri: &str) -> Result<PathBuf, FileUriRejection> {
    use std::os::unix::ffi::OsStringExt;

    if uri.len() > MAX_FILE_URI_BYTES {
        return Err(FileUriRejection::TooLong);
    }

    let rest = uri
        .trim()
        .strip_prefix("file://")
        .ok_or(FileUriRejection::NotFileScheme)?;

    let slash = rest.find('/').ok_or(FileUriRejection::RelativePath)?;
    let (host, path_str) = rest.split_at(slash);

    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return Err(FileUriRejection::RemoteHost);
    }

    let mut decoded: Vec<u8> = Vec::with_capacity(path_str.len());
    let mut bytes = path_str.bytes();

    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes
                .next()
                .and_then(digit)
                .ok_or(FileUriRejection::MalformedEncoding)?;
            let low = bytes
                .next()
                .and_then(digit)
                .ok_or(FileUriRejection::MalformedEncoding)?;
            decoded.push(high << 4 | low);
        } else {
            decoded.push(byte);
        }
    }

    for &byte in &decoded {
        if byte < 0x20 || byte == 0x7F {
            return Err(FileUriRejection::ControlCharacter);
        }
    }

    let path = PathBuf::from(std::ffi::OsString::from_vec(decoded));

    if !path.is_absolute() {
        return Err(FileUriRejection::RelativePath);
    }

    Ok(path)
}

fn parse_gnome_copied_files(bytes: &[u8]) -> Option<FileTransfer> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();

    let header = lines.next()?.trim();
    let intent = match header {
        h if h.eq_ignore_ascii_case("copy") => FileIntent::Copy,
        h if h.eq_ignore_ascii_case("cut") => FileIntent::Cut,
        _ => return None,
    };

    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut count: usize = 0;

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        count = count.saturating_add(1);
        if count > MAX_FILE_URI_COUNT {
            break;
        }
        if let Ok(path) = validate_file_uri(line)
            && seen.insert(path.clone())
        {
            paths.push(path);
        }
    }

    if paths.is_empty() {
        return None;
    }

    Some(FileTransfer { intent, paths })
}

fn parse_uri_list_as_transfer(bytes: &[u8]) -> Option<FileTransfer> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut count: usize = 0;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        count = count.saturating_add(1);
        if count > MAX_FILE_URI_COUNT {
            break;
        }
        if let Ok(path) = validate_file_uri(line)
            && seen.insert(path.clone())
        {
            paths.push(path);
        }
    }

    if paths.is_empty() {
        return None;
    }

    Some(FileTransfer {
        intent: FileIntent::Copy,
        paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gnome_copy_header() {
        let bytes = b"copy\nfile:///tmp/a\nfile:///tmp/b\n";
        let ft = parse_gnome_copied_files(bytes).expect("valid GNOME copy payload");
        assert_eq!(ft.intent(), FileIntent::Copy);
        assert_eq!(
            ft.paths(),
            &[PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }
}
