//! Matching text the way people search: without case or accents.
//!
//! [`fold`] reduces text to a search key: canonical decomposition, combining
//! marks removed, full lower case (`ß` becomes `ss`, `Σ` becomes `σ`), and
//! compatibility forms (`ﬁ`, full-width letters) unfolded. `cafe` then finds
//! `Café`, `STRASSE` finds `Straße`. [`contains`] tests a folded haystack for
//! a folded needle. The framework's filters (file browser, command palette,
//! font chooser, emoji search) match through it.

use unicode_normalization::UnicodeNormalization as _;
use unicode_normalization::char::is_combining_mark;

/// The search key of `text`; see the module docs.
#[must_use]
pub fn fold(text: &str) -> String {
    // ASCII is its own NFKD form, has no combining marks, and lowercases
    // byte for byte, so the Unicode route would produce exactly this.
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    fold_unicode(text)
}

fn fold_unicode(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for c in text.nfkd().filter(|c| !is_combining_mark(*c)) {
        match c {
            'ß' | 'ẞ' => folded.push_str("ss"),
            'ς' => folded.push('σ'),
            c => folded.extend(c.to_lowercase()),
        }
    }
    folded
}

/// Whether `haystack` contains `needle`, both folded.
#[must_use]
pub fn contains(haystack: &str, needle: &str) -> bool {
    fold(haystack).contains(&fold(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ascii_route_matches_the_unicode_route() {
        let all: String = (0u8..=0x7f).map(char::from).collect();
        assert_eq!(fold(&all), fold_unicode(&all));
        assert_eq!(fold("Report-2026 FINAL.PDF"), "report-2026 final.pdf");
    }

    #[test]
    fn case_accents_and_compatibility_forms_fold_away() {
        assert_eq!(fold("Café"), "cafe");
        assert_eq!(fold("Straße"), fold("STRASSE"));
        assert_eq!(fold("ΟΔΟΣ"), fold("οδος"), "final sigma");
        assert_eq!(fold("ﬁle"), "file");
        assert_eq!(fold("Ｆｕｌｌ"), "full");
        assert_eq!(fold("Ångström"), "angstrom");
        assert!(contains("Résumé final.pdf", "resume"));
        assert!(contains("Łódź", "Łodz"), "Ł has no decomposition; ó does");
        assert!(!contains("cafe", "café au lait"));
    }
}
