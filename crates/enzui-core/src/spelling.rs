//! Spell checking: which words of a text are misspelled, what to suggest,
//! and the words the user has accepted.
//!
//! A [`SpellChecker`] is a dictionary for one language; enzUI ships one over
//! Enchant (`enzui_services::spell_check`, feature `spell-check`), which
//! reaches Hunspell, Nuspell, Aspell and Voikko dictionaries the desktop
//! already has. [`SpellCheck`] wraps a checker for the text widgets: it
//! splits text into words by Unicode word boundaries, remembers each word's
//! verdict, and keeps the words the user ignored for the session. Give it
//! to `EnzTextArea::spell_check`, which underlines what it finds and offers
//! suggestions, "Add to Dictionary" and "Ignore" in the edit menu.
//!
//! Words with digits, all-capital abbreviations, and links and e-mail
//! addresses are never flagged.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};
use unicode_segmentation::UnicodeSegmentation as _;

/// A dictionary for one language.
pub trait SpellChecker: Send + Sync {
    /// Whether `word` is spelled correctly.
    fn check(&self, word: &str) -> bool;
    /// Corrections for `word`, best first.
    fn suggest(&self, word: &str) -> Vec<String>;
    /// Adds `word` to the user's personal dictionary, for every app.
    fn add_to_dictionary(&self, word: &str);
    /// The language, as a tag such as `en_US`.
    fn language(&self) -> &str;
}

/// The most suggestions [`SpellCheck::suggestions`] returns.
pub const MAX_SUGGESTIONS: usize = 5;

/// A [`SpellChecker`] with word verdicts remembered and session-ignored
/// words; cheap to clone, and clones share both.
#[derive(Clone)]
pub struct SpellCheck {
    checker: Arc<dyn SpellChecker>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    verdicts: HashMap<String, bool>,
    ignored: HashSet<String>,
}

impl std::fmt::Debug for SpellCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpellCheck")
            .field("language", &self.checker.language())
            .finish_non_exhaustive()
    }
}

impl SpellCheck {
    /// Checks with `checker`.
    pub fn new(checker: impl SpellChecker + 'static) -> Self {
        Self::shared(Arc::new(checker))
    }

    /// Checks with a shared `checker`.
    #[must_use]
    pub fn shared(checker: Arc<dyn SpellChecker>) -> Self {
        Self {
            checker,
            state: Arc::default(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The checker's language.
    #[must_use]
    pub fn language(&self) -> &str {
        self.checker.language()
    }

    /// Whether `word` counts as correct: the dictionary knows it, the user
    /// ignored it, or it is not a word to check.
    #[must_use]
    pub fn is_correct(&self, word: &str) -> bool {
        if !checkable(word) {
            return true;
        }
        let state = self.state();
        if state.ignored.contains(word) {
            return true;
        }
        if let Some(&verdict) = state.verdicts.get(word) {
            return verdict;
        }
        drop(state);
        let verdict = self.checker.check(word);
        self.state().verdicts.insert(word.to_owned(), verdict);
        verdict
    }

    /// The byte ranges of the misspelled words in `text`.
    #[must_use]
    pub fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        for (start, word) in words(text) {
            if !self.is_correct(word) {
                ranges.push(start..start + word.len());
            }
        }
        ranges
    }

    /// Corrections for `word`, best first, at most [`MAX_SUGGESTIONS`].
    #[must_use]
    pub fn suggestions(&self, word: &str) -> Vec<String> {
        let mut suggestions = self.checker.suggest(word);
        suggestions.retain(|suggestion| suggestion != word);
        suggestions.truncate(MAX_SUGGESTIONS);
        suggestions
    }

    /// Accepts `word` for the rest of the session, in every widget sharing
    /// this check.
    pub fn ignore(&self, word: &str) {
        self.state().ignored.insert(word.to_owned());
    }

    /// Adds `word` to the user's personal dictionary.
    pub fn add_to_dictionary(&self, word: &str) {
        self.checker.add_to_dictionary(word);
        self.state().verdicts.insert(word.to_owned(), true);
    }
}

/// The words of `text` with their byte offsets, by Unicode word
/// boundaries; `don't` and `co-op`'s halves are words, punctuation is not.
fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut skip_until = 0;
    text.split_word_bound_indices()
        .filter_map(move |(start, word)| {
            if start < skip_until {
                return None;
            }
            // A link or address runs to the next whitespace.
            let rest = text.get(start..).unwrap_or_default();
            let token_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let token = rest.get(..token_end).unwrap_or_default();
            if token.contains("://") || token.starts_with("www.") || token.contains('@') {
                skip_until = start + token_end;
                return None;
            }
            word.chars()
                .any(char::is_alphabetic)
                .then_some((start, word))
        })
}

/// Whether `word` is one to check: not a number-bearing token or an
/// all-capital abbreviation.
fn checkable(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    if letters == 0 || word.chars().any(char::is_numeric) {
        return false;
    }
    let all_caps = letters > 1
        && word
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(char::is_uppercase);
    !all_caps
}

/// The dictionary language for the user's locale: the language and region
/// of `LC_ALL`, `LC_MESSAGES` or `LANG` (`de_DE.UTF-8` gives `de_DE`), or
/// `en_US`.
#[must_use]
pub fn locale_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .map(|value| {
            value
                .split(['.', '@'])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .filter(|tag| !tag.is_empty())
        .unwrap_or_else(|| "en_US".to_owned())
}

#[cfg(test)]
#[allow(
    clippy::string_slice,
    reason = "the tests slice ASCII text at word boundaries"
)]
mod tests {
    use super::*;

    struct Tiny;
    impl SpellChecker for Tiny {
        fn check(&self, word: &str) -> bool {
            ["the", "cat", "sat", "don't", "on", "mat", "co", "op"]
                .contains(&word.to_lowercase().as_str())
        }
        fn suggest(&self, word: &str) -> Vec<String> {
            vec![word.to_owned(), "cat".into(), "sat".into()]
        }
        fn add_to_dictionary(&self, _word: &str) {}
        fn language(&self) -> &str {
            "en_US"
        }
    }

    #[test]
    fn misspelled_words_are_found_by_byte_range() {
        let check = SpellCheck::new(Tiny);
        let text = "The catt sat, don't on the co-op matt.";
        let found: Vec<&str> = check
            .misspelled(text)
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(found, ["catt", "matt"]);
    }

    #[test]
    fn numbers_abbreviations_links_and_addresses_are_skipped() {
        let check = SpellCheck::new(Tiny);
        let text = "zzz 3rd NASA https://exmple.org/pth ada@exmple.org www.exmple.org Qq";
        let found: Vec<&str> = check
            .misspelled(text)
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(found, ["zzz", "Qq"]);
    }

    #[test]
    #[allow(
        clippy::redundant_clone,
        reason = "the test is that a clone shares the session"
    )]
    fn ignored_words_pass_and_the_word_itself_is_not_suggested() {
        let check = SpellCheck::new(Tiny);
        assert!(!check.is_correct("catt"));
        check.clone().ignore("catt");
        assert!(check.is_correct("catt"), "clones share the session");
        assert_eq!(check.suggestions("catt"), ["cat", "sat"]);
    }
}
