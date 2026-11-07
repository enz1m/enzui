//! Sorting text the way the user's language sorts it.
//!
//! [`Locale::collator`] builds a [`Collator`] from the Unicode Collation
//! Algorithm with the locale's tailoring: accented letters sort beside their
//! base letter (`é` with `e`, not after `z`), Swedish puts `ä` after `z`,
//! German phone-book order and the like come from the locale. Numeric
//! ordering sorts digit runs by value (`file2` before `file10`), as a file
//! manager does. Use the locale of [`LocaleCategory::Collate`]
//! (`Localizer::locale_for`) so `LC_COLLATE` is honoured.
//!
//! [`LocaleCategory::Collate`]: super::LocaleCategory::Collate

use super::{Locale, LocalizationError};
use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use std::cmp::Ordering;

/// Which differences tell two strings apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CollationStrength {
    /// Base letters only: `a`, `á` and `A` are equal.
    Base,
    /// Base letters and accents: `a` and `A` are equal, `á` is not.
    Accents,
    /// Letters, accents and case, lower case first by default.
    #[default]
    Full,
}

/// How a [`Collator`] compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Collation {
    /// Sort runs of digits by their value.
    pub numeric: bool,
    /// Which differences count.
    pub strength: CollationStrength,
}

impl Collation {
    /// Locale order for names a user typed: digit runs by value, full
    /// strength, as a file manager sorts.
    pub const NATURAL: Self = Self {
        numeric: true,
        strength: CollationStrength::Full,
    };
}

/// Compares strings in a locale's order; build one with
/// [`Locale::collator`] and reuse it.
pub struct Collator(CollatorBorrowed<'static>);

impl std::fmt::Debug for Collator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Collator")
    }
}

impl Collator {
    /// The order of `a` and `b`.
    #[must_use]
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        self.0.compare(a, b)
    }

    /// Sorts `items` by the text `key` gives, stably.
    pub fn sort_by_key<T>(&self, items: &mut [T], key: impl Fn(&T) -> &str) {
        items.sort_by(|a, b| self.compare(key(a), key(b)));
    }
}

impl Locale {
    /// A collator for this locale.
    ///
    /// # Errors
    /// The collation data for the locale could not be loaded.
    pub fn collator(&self, collation: Collation) -> Result<Collator, LocalizationError> {
        let mut preferences = CollatorPreferences::from(&self.0);
        if collation.numeric {
            preferences.numeric_ordering = Some(CollationNumericOrdering::True);
        }
        let mut options = CollatorOptions::default();
        options.strength = Some(match collation.strength {
            CollationStrength::Base => Strength::Primary,
            CollationStrength::Accents => Strength::Secondary,
            CollationStrength::Full => Strength::Tertiary,
        });
        CollatorBorrowed::try_new(preferences, options)
            .map(Collator)
            .map_err(|error| LocalizationError::Format(error.to_string()))
    }
}
