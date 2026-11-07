//! Fluent message catalogs, locale selection, and scoped defaults for framework widgets.
//!
//! Build a [`Catalog`], create a [`Localizer`] with the user's preferred locales,
//! and build root views with [`Localizer::view`]. Apps also use
//! the localizer directly for their own messages. [`Localizer::select`] changes
//! the shared selection; the caller rebuilds its view and invalidates any cached
//! translated content using [`Localizer::generation`]. No environment is mutated.

mod argument;
mod catalog;
mod collation;
mod context;
mod coverage;
#[cfg(feature = "locale-formatters")]
mod formatters;
mod formatting;
mod locale;
mod pseudo;
mod widget;

pub use argument::MessageArgument;
pub(crate) use catalog::english_text;
pub use catalog::{Catalog, CatalogBuilder, LocalizationError, LocalizedMessage, Localizer};
pub use collation::{Collation, CollationStrength, Collator};
pub(crate) use context::scoped_locale;
pub use context::{current, scoped, text, text_with};
pub use coverage::{Coverage, coverage, framework_coverage};
pub use fluent_bundle::{FluentArgs as Arguments, FluentValue as Value};
#[cfg(feature = "locale-formatters")]
pub use formatters::DurationStyle;
pub use formatting::{DateStyle, Decimal, LocaleCalendarLabels, LocalizedCalendarLabels};
pub use locale::{
    Locale, LocaleCategory, category_locale_from_env, preferred_locales, preferred_locales_from_env,
};
pub use pseudo::{PSEUDO_LOCALE, pseudolocalize};

/// The built-in English widget catalog, also usable as a translator's template.
pub const ENGLISH: &str = include_str!("locales/en-US.ftl");

/// The framework's own translations of [`ENGLISH`], by language tag. A
/// [`Localizer`] uses them after the app's catalog for the same language and
/// before the app's fallback language, so framework widgets speak the user's
/// language without the app shipping these strings.
pub const FRAMEWORK_TRANSLATIONS: &[(&str, &str)] = &[
    ("de", include_str!("locales/de.ftl")),
    ("es", include_str!("locales/es.ftl")),
    ("fr", include_str!("locales/fr.ftl")),
    ("hu", include_str!("locales/hu.ftl")),
];

pub(crate) fn safe_text(key: &str) -> String {
    text(key)
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

/// Formats a catalog message with named Fluent arguments through the current localizer.
#[macro_export]
macro_rules! localized {
    ($key:expr $(,)?) => { $crate::localization::text($key) };
    ($key:expr $(, $name:literal => $value:expr)* $(,)?) => {{
        let mut arguments = $crate::localization::Arguments::new();
        $(let value = &$value;
          arguments.set($name, $crate::localization::MessageArgument::message_value(value));)*
        $crate::localization::text_with($key, &arguments)
    }};
}

#[cfg(all(test, feature = "rust-source-audit"))]
mod catalog_contract;
#[cfg(test)]
mod consumer_contract;
#[cfg(test)]
mod tests;
