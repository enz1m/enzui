use super::LocalizationError;
use crate::placement::Direction;
use std::fmt;
use std::str::FromStr;

/// A Unicode locale, including optional script, region and formatting preferences.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Locale(pub(super) icu_locale::Locale);

impl Locale {
    /// Parses a POSIX locale such as `sr_RS.UTF-8@latin` or a Unicode locale tag.
    ///
    /// `C`, `C.UTF-8`, and `POSIX` select `en-US`. Supported POSIX modifiers are
    /// `latin`, `cyrillic`, `arabic`, and `euro`; unknown modifiers are rejected.
    ///
    /// # Errors
    /// Returns an error for an invalid tag or unsupported modifier.
    pub fn from_posix(value: &str) -> Result<Self, LocalizationError> {
        let value = value.trim();
        let (base, modifier) = value.split_once('@').unwrap_or((value, ""));
        let base = base.split('.').next().unwrap_or(base);
        if base.eq_ignore_ascii_case("c") || base.eq_ignore_ascii_case("posix") {
            return Ok(Self::default());
        }
        let mut normalized = base.replace('_', "-");
        let script = match modifier.to_ascii_lowercase().as_str() {
            "" | "euro" => None,
            "latin" => Some("Latn"),
            "cyrillic" => Some("Cyrl"),
            "arabic" => Some("Arab"),
            _ => return Err(LocalizationError::InvalidLocale(value.to_owned())),
        };
        if let Some(script) = script {
            let mut parsed: Self = normalized.parse()?;
            parsed.0.id.script = script.parse().ok();
            normalized = parsed.to_string();
        }
        normalized.parse()
    }

    /// Resolves direction from the explicit or likely script using CLDR data.
    #[must_use]
    pub fn direction(&self) -> Direction {
        if icu_locale::LocaleDirectionality::new_extended().is_right_to_left(&self.0.id) {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        }
    }

    pub(super) fn language_id(&self) -> unic_langid::LanguageIdentifier {
        self.0.id.to_string().parse().unwrap_or_default()
    }
}

impl Default for Locale {
    fn default() -> Self {
        Self(icu_locale::locale!("en-US"))
    }
}

impl FromStr for Locale {
    type Err = LocalizationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.contains(['_', '.', '@']) || value.trim() != value {
            return Err(LocalizationError::InvalidLocale(value.to_owned()));
        }
        let mut locale: icu_locale::Locale = value
            .parse()
            .map_err(|_| LocalizationError::InvalidLocale(value.to_owned()))?;
        icu_locale::LocaleCanonicalizer::new_extended().canonicalize(&mut locale);
        Ok(Self(locale))
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// What a locale is used for, as the POSIX `LC_*` categories split it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LocaleCategory {
    /// Dates and times (`LC_TIME`).
    Time,
    /// Numbers (`LC_NUMERIC`).
    Numeric,
    /// Sort order (`LC_COLLATE`).
    Collate,
    /// Money (`LC_MONETARY`).
    Monetary,
}

impl LocaleCategory {
    /// Every category.
    pub const ALL: [Self; 4] = [Self::Time, Self::Numeric, Self::Collate, Self::Monetary];

    /// The category's environment variable.
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::Time => "LC_TIME",
            Self::Numeric => "LC_NUMERIC",
            Self::Collate => "LC_COLLATE",
            Self::Monetary => "LC_MONETARY",
        }
    }
}

/// The locale the environment sets for `category`: `LC_ALL`, then the
/// category's own variable (`LC_TIME`...), then `LANG`, as POSIX resolves
/// it. `None` when none is set to a usable value.
#[must_use]
pub fn category_locale_from_env(
    category: LocaleCategory,
    read: impl Fn(&str) -> Option<String>,
) -> Option<Locale> {
    ["LC_ALL", category.variable(), "LANG"]
        .into_iter()
        .filter_map(&read)
        .find_map(|value| Locale::from_posix(&value).ok())
}

/// Reads preferred UI locales without changing process environment variables.
#[must_use]
pub fn preferred_locales() -> Vec<Locale> {
    preferred_locales_from_env(|key| std::env::var(key).ok())
}

/// Resolves GNU `LANGUAGE` preferences and the POSIX locale precedence.
///
/// `LC_ALL`, `LC_MESSAGES`, then `LANG` choose the base locale. Empty and invalid
/// values are skipped. A `C`/`POSIX` base disables `LANGUAGE`; otherwise its
/// colon-separated preferences precede the base. The final default is `en-US`.
#[must_use]
pub fn preferred_locales_from_env(read: impl Fn(&str) -> Option<String>) -> Vec<Locale> {
    let base = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(&read)
        .find_map(|value| {
            let locale = Locale::from_posix(&value).ok()?;
            Some((value, locale))
        });
    let Some((raw, base)) = base else {
        return vec![Locale::default()];
    };
    if raw
        .trim()
        .split(['.', '@'])
        .next()
        .is_some_and(|base| base.eq_ignore_ascii_case("c") || base.eq_ignore_ascii_case("posix"))
    {
        return vec![base];
    }
    let mut preferred = Vec::new();
    if let Some(languages) = read("LANGUAGE") {
        for raw in languages.split(':') {
            if let Ok(locale) = Locale::from_posix(raw)
                && !preferred.contains(&locale)
            {
                preferred.push(locale);
            }
        }
    }
    if !preferred.contains(&base) {
        preferred.push(base);
    }
    preferred
}
