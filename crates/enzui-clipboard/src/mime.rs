//! MIME names and the negotiation between what a caller wants and what the
//! current selection actually offers.
//!
//! Wayland selections carry a list of MIME names chosen by whoever copied, and
//! nothing normalises them: the same UTF-8 text is spelled
//! `text/plain;charset=utf-8` by one toolkit, `text/plain;charset=UTF-8` by the
//! next and `UTF8_STRING` by an X11-era application bridged into the session.
//! Negotiation therefore compares normalised names against an ordered
//! preference list, and a caller that gets no match is told what was on offer.

use std::fmt;

/// A MIME name as it travels over the wire.
///
/// The stored spelling is preserved exactly, because it is what gets sent back
/// to the owner in `wl_data_offer::receive`; only comparison is normalised.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MimeType(String);

impl MimeType {
    /// Wrap a wire spelling without validating its syntax or length.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The exact spelling to put on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Return an ASCII-lowercase comparison form with each semicolon-separated
    /// part trimmed. Parameter order, internal whitespace and quoting remain significant.
    ///
    /// `text/plain; charset=UTF-8` and `text/plain;charset=utf-8` are the same
    /// type, and a selection that only offers the first spelling must still
    /// satisfy a caller that asked for the second.
    pub fn normalized(&self) -> String {
        let mut out = String::with_capacity(self.0.len());

        for part in self.0.split(';') {
            if !out.is_empty() {
                out.push(';');
            }
            out.push_str(part.trim());
        }

        out.make_ascii_lowercase();
        out
    }

    /// Compare names using this crate's normalization; derived equality preserves wire spelling.
    pub fn matches(&self, other: &MimeType) -> bool {
        self.normalized() == other.normalized()
    }
}

impl fmt::Display for MimeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for MimeType {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for MimeType {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// A broad content family, used to build a preference list without spelling out
/// every alias a desktop might use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// Plain UTF-8 text.
    Text,
    /// Rich text as HTML, with the plain-text aliases as fallback.
    Html,
    /// A raster image in any commonly offered encoding.
    Image,
    /// A file selection, carried as a `text/uri-list` of `file://` URIs.
    Uris,
}

/// The preference order for plain text, best first.
pub const TEXT_MIME_TYPES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

/// The preference order for HTML, best first.
pub const HTML_MIME_TYPES: &[&str] = &["text/html;charset=utf-8", "text/html"];

/// The preference order for images, best first.
pub const IMAGE_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/webp",
    "image/tiff",
    "image/jpeg",
    "image/bmp",
];

/// The preference order for file offers, best first.
///
/// GNOME's `x-special/gnome-copied-files` is first because it carries a
/// copy/cut intent header that `text/uri-list` cannot represent. A reader
/// that prefers this order therefore preserves the transfer intent when the
/// source advertises both types.
pub const URI_MIME_TYPES: &[&str] = &["x-special/gnome-copied-files", "text/uri-list"];

impl Kind {
    /// The MIME names this kind accepts, best first.
    ///
    /// [`Kind::Html`] deliberately ends with the plain-text names: an
    /// application asking for HTML would rather have the text than nothing, and
    /// the returned [`Pasted`](crate::Pasted) still reports which type actually
    /// arrived.
    pub fn preferences(self) -> Vec<MimeType> {
        let names: Vec<&str> = match self {
            Self::Text => TEXT_MIME_TYPES.to_vec(),
            Self::Html => HTML_MIME_TYPES
                .iter()
                .chain(TEXT_MIME_TYPES.iter())
                .copied()
                .collect(),
            Self::Image => IMAGE_MIME_TYPES.to_vec(),
            Self::Uris => URI_MIME_TYPES.to_vec(),
        };

        names.into_iter().map(MimeType::new).collect()
    }
}

/// Picks the best offered type for an ordered preference list.
///
/// Preference order wins over offer order: the owner lists its types in its own
/// order and a receiver that simply took the first intersection would end up
/// with whatever the owner happened to list first.
pub fn negotiate(preferences: &[MimeType], offered: &[MimeType]) -> Option<MimeType> {
    for wanted in preferences {
        let wanted = wanted.normalized();

        if let Some(found) = offered.iter().find(|offer| offer.normalized() == wanted) {
            return Some(found.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_follows_caller_preference_not_offer_order() {
        let offered = vec![
            MimeType::new("STRING"),
            MimeType::new("text/plain; charset=utf-8"),
        ];

        let chosen = negotiate(&Kind::Text.preferences(), &offered).expect("a text type matches");

        assert_eq!(chosen.as_str(), "text/plain; charset=utf-8");
    }
}
