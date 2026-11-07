//! Which messages a translation lacks or keeps after the reference dropped them.

use super::LocalizationError;
use fluent_syntax::ast::Entry;
use std::collections::BTreeSet;

/// How a translation compares with its reference catalog, by message key.
///
/// Keys are message identifiers, and `message.attribute` for each attribute,
/// as [`Localizer::text`](super::Localizer::text) takes them. Terms are
/// private to a catalog and are not compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Keys the reference has and the translation does not; these display
    /// in the fallback language.
    pub missing: Vec<String>,
    /// Keys the translation has and the reference no longer does.
    pub obsolete: Vec<String>,
    /// How many keys the reference has.
    pub total: usize,
}

impl Coverage {
    /// Whether the translation has exactly the reference's keys.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.obsolete.is_empty()
    }

    /// Translated share of the reference's keys, from 0 to 100.
    #[must_use]
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        let translated = self.total.saturating_sub(self.missing.len());
        #[allow(
            clippy::cast_precision_loss,
            reason = "message counts are far below 2^52"
        )]
        let share = translated as f64 * 100.0 / self.total as f64;
        share
    }
}

/// Compares the keys of a translation with those of its reference (usually
/// the English catalog).
///
/// # Errors
/// Returns [`LocalizationError::InvalidResource`] when either text is not
/// valid Fluent.
pub fn coverage(reference: &str, translation: &str) -> Result<Coverage, LocalizationError> {
    let reference = keys(reference)?;
    let translation = keys(translation)?;
    Ok(Coverage {
        missing: reference.difference(&translation).cloned().collect(),
        obsolete: translation.difference(&reference).cloned().collect(),
        total: reference.len(),
    })
}

/// The framework's shipped translations compared with its English catalog,
/// by language tag.
#[must_use]
pub fn framework_coverage() -> Vec<(&'static str, Coverage)> {
    super::FRAMEWORK_TRANSLATIONS
        .iter()
        .map(|&(locale, source)| {
            let report = coverage(super::ENGLISH, source).unwrap_or_else(|error| Coverage {
                missing: vec![error.to_string()],
                ..Coverage::default()
            });
            (locale, report)
        })
        .collect()
}

fn keys(source: &str) -> Result<BTreeSet<String>, LocalizationError> {
    let resource = fluent_syntax::parser::parse(source)
        .map_err(|(_, errors)| LocalizationError::InvalidResource(format!("{errors:?}")))?;
    let mut keys = BTreeSet::new();
    for entry in resource.body {
        if let Entry::Message(message) = entry {
            let id = message.id.name;
            if message.value.is_some() {
                keys.insert(id.to_owned());
            }
            for attribute in message.attributes {
                keys.insert(format!("{id}.{}", attribute.id.name));
            }
        }
    }
    Ok(keys)
}
