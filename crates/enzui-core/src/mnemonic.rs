//! Access keys (mnemonics): the underlined letter of a label that Alt plus
//! that letter activates.
//!
//! A label marks its access key with an underscore before the letter, as
//! GTK and Qt (with `&`) do: `"_Save"`, `"Save _As…"`. `"__"` is a literal
//! underscore. [`Mnemonic::parse`] splits the shown text from the key; the
//! widgets underline the letter while Alt is held and activate on Alt and the
//! key.

use iced_core::keyboard::Key;
use std::ops::Range;

/// A label with its access key taken out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mnemonic {
    /// The label as shown, without the marker.
    pub text: String,
    /// The access key, lower-cased; `None` when the label marks none.
    pub key: Option<char>,
    /// Where the marked letter is in [`Self::text`], in bytes.
    pub at: Option<Range<usize>>,
}

impl Mnemonic {
    /// Splits `label` into its text and access key; the first marked
    /// letter counts.
    #[must_use]
    pub fn parse(label: &str) -> Self {
        let mut text = String::with_capacity(label.len());
        let mut key = None;
        let mut at = None;
        let mut chars = label.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '_' {
                text.push(c);
                continue;
            }
            match chars.next() {
                Some('_') => text.push('_'),
                Some(marked) => {
                    if key.is_none() && marked.is_alphanumeric() {
                        key = marked.to_lowercase().next();
                        at = Some(text.len()..text.len() + marked.len_utf8());
                    }
                    text.push(marked);
                }
                None => text.push('_'),
            }
        }
        Self { text, key, at }
    }

    /// Whether `key` (as pressed, with Alt) is this access key.
    #[must_use]
    pub fn matches(&self, key: &Key) -> bool {
        let Some(own) = self.key else {
            return false;
        };
        match key {
            Key::Character(pressed) => {
                let mut chars = pressed.chars().flat_map(char::to_lowercase);
                chars.next() == Some(own) && chars.next().is_none()
            }
            _ => false,
        }
    }
}
