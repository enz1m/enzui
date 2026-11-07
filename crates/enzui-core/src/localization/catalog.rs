use super::{Arguments, ENGLISH, Locale};
use fluent_bundle::FluentResource;
use fluent_bundle::concurrent::FluentBundle;
use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};
use unic_langid::LanguageIdentifier;

type Bundle = FluentBundle<Arc<FluentResource>>;

pub(crate) fn english_text(key: &str) -> String {
    static ENGLISH_LOCALIZER: LazyLock<Localizer> =
        LazyLock::new(|| Localizer::new(Catalog::default(), Vec::new(), Locale::default()));
    ENGLISH_LOCALIZER.text(key)
}

/// Catalog admission or message resolution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalizationError {
    /// A locale tag or POSIX modifier is invalid.
    InvalidLocale(String),
    /// Fluent syntax is invalid; the resource is rejected in its entirety.
    InvalidResource(String),
    /// A catalog repeats a message, term or function identifier.
    DuplicateMessage(String),
    /// No locale in the fallback chain supplies the requested value or attribute.
    MissingMessage(String),
    /// A pattern contains an unresolved reference, argument or function.
    Format(String),
}

impl fmt::Display for LocalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, detail) = match self {
            Self::InvalidLocale(detail) => ("invalid locale", detail),
            Self::InvalidResource(detail) => ("invalid Fluent resource", detail),
            Self::DuplicateMessage(detail) => ("duplicate Fluent entry", detail),
            Self::MissingMessage(detail) => ("missing message", detail),
            Self::Format(detail) => ("message formatting failed", detail),
        };
        write!(formatter, "{kind}: {detail}")
    }
}

impl std::error::Error for LocalizationError {}

/// Validates Fluent resources before publishing an immutable catalog.
#[derive(Default)]
pub struct CatalogBuilder {
    resources: BTreeMap<LanguageIdentifier, Vec<Arc<FluentResource>>>,
}

fn make_bundle(locale: &LanguageIdentifier) -> Bundle {
    let mut bundle = Bundle::new_concurrent(vec![locale.clone()]);
    let _ = bundle.add_function("NUMBER", super::formatting::number_value);
    bundle.set_formatter(Some(super::formatting::fluent_number));
    bundle
}

impl CatalogBuilder {
    /// Starts an empty app catalog; built-in English widget messages remain available.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a complete Fluent resource for one language, ignoring formatting extensions.
    ///
    /// Resources for the same locale share terms and references. Duplicates are
    /// rejected transactionally; rejected resources never alter the builder.
    ///
    /// # Errors
    /// Returns locale, parse, or duplicate-entry errors.
    pub fn add_resource(
        &mut self,
        locale: &str,
        source: impl Into<String>,
    ) -> Result<&mut Self, LocalizationError> {
        let locale: Locale = locale.parse()?;
        let id = locale.language_id();
        let resource = Arc::new(FluentResource::try_new(source.into()).map_err(
            |(_, errors)| LocalizationError::InvalidResource(format!("{id}: {errors:?}")),
        )?);
        let mut candidate = make_bundle(&id);
        for existing in self.resources.get(&id).into_iter().flatten() {
            candidate
                .add_resource(Arc::clone(existing))
                .map_err(|errors| {
                    LocalizationError::DuplicateMessage(format!("{id}: {errors:?}"))
                })?;
        }
        candidate
            .add_resource(Arc::clone(&resource))
            .map_err(|errors| LocalizationError::DuplicateMessage(format!("{id}: {errors:?}")))?;
        self.resources.entry(id).or_default().push(resource);
        Ok(self)
    }

    /// Freezes validated resources into a thread-safe, cheaply cloned catalog.
    #[must_use]
    pub fn build(self) -> Catalog {
        let bundles = self
            .resources
            .into_iter()
            .map(|(locale, resources)| {
                let mut bundle = make_bundle(&locale);
                for resource in resources {
                    let _ = bundle.add_resource(resource);
                }
                (locale, bundle)
            })
            .collect();
        Catalog(Arc::new(bundles))
    }
}

/// Immutable app and widget translations, shareable between windows and threads.
#[derive(Clone, Default)]
pub struct Catalog(Arc<BTreeMap<LanguageIdentifier, Bundle>>);

impl Catalog {
    /// Lists the catalog's language tags in stable lexical order.
    #[must_use]
    pub fn locales(&self) -> Vec<String> {
        let mut locales: Vec<_> = self.0.keys().map(ToString::to_string).collect();
        locales.sort();
        locales
    }
}

#[derive(Clone)]
struct Selection {
    preferred: Vec<Locale>,
    /// App catalog languages: those matching the preferences, then those
    /// matching the fallback locale.
    chain: Vec<LanguageIdentifier>,
    /// How many entries of `chain` match the preferences.
    preferred_len: usize,
    /// Framework translations matching the preferences.
    framework: Vec<LanguageIdentifier>,
    /// The first preference is the pseudo-locale.
    pseudo: bool,
    generation: u64,
}

/// A resolved value with the language that supplied it and recoverable diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedMessage {
    /// Opaque display text, including Fluent's bidirectional isolation marks.
    pub value: String,
    /// The actual source language; absent when the message identifier is displayed.
    pub locale: Option<String>,
    /// Failures encountered before finding a usable fallback.
    pub errors: Vec<LocalizationError>,
}

/// Shared language selection over an immutable catalog.
///
/// Clones observe [`Self::select`] together. Create another localizer over the
/// same [`Catalog`] for independently localized windows. Selection schedules no
/// UI work; the caller rebuilds views and keys cached content by generation.
#[derive(Clone)]
pub struct Localizer {
    catalog: Catalog,
    fallback: Locale,
    /// Replaced whole on change, so a lookup clones one pointer.
    selection: Arc<RwLock<Arc<Selection>>>,
    /// Locales set apart from the preferred one for formatting and sorting.
    categories: Arc<RwLock<BTreeMap<super::LocaleCategory, Locale>>>,
}

impl fmt::Debug for Localizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Localizer")
            .field("locale", &self.locale())
            .field("fallback", &self.fallback)
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl Localizer {
    pub(super) fn snapshot(&self) -> Self {
        let selection = self
            .selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let categories = self
            .categories
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Self {
            catalog: self.catalog.clone(),
            fallback: self.fallback.clone(),
            selection: Arc::new(RwLock::new(selection)),
            categories: Arc::new(RwLock::new(categories)),
        }
    }

    pub(super) fn identity(&self) -> usize {
        Arc::as_ptr(&self.selection) as usize
    }

    /// Selects preferred languages, then `fallback`, then built-in English widgets.
    ///
    /// Negotiation uses Fluent's filtering strategy. Empty preferences use the
    /// fallback locale. Formatting and layout use the first preference even if
    /// no message catalog exists for it.
    #[must_use]
    pub fn new(catalog: Catalog, preferred: Vec<Locale>, fallback: Locale) -> Self {
        let selection = Arc::new(Self::selection(&catalog, &preferred, &fallback, 0));
        Self {
            catalog,
            fallback,
            selection: Arc::new(RwLock::new(selection)),
            categories: Arc::default(),
        }
    }

    fn selection(
        catalog: &Catalog,
        preferred: &[Locale],
        fallback: &Locale,
        generation: u64,
    ) -> Selection {
        let preferred = if preferred.is_empty() {
            vec![fallback.clone()]
        } else {
            preferred.to_vec()
        };
        let requested: Vec<_> = preferred.iter().map(Locale::language_id).collect();
        let available: Vec<_> = catalog.0.keys().cloned().collect();
        let mut chain: Vec<_> =
            negotiate_languages(&requested, &available, None, NegotiationStrategy::Filtering)
                .into_iter()
                .cloned()
                .collect();
        let preferred_len = chain.len();
        let framework_available: Vec<_> = FRAMEWORK_BUNDLES.keys().cloned().collect();
        let framework = negotiate_languages(
            &requested,
            &framework_available,
            None,
            NegotiationStrategy::Filtering,
        )
        .into_iter()
        .cloned()
        .collect();
        let pseudo = preferred
            .first()
            .is_some_and(|locale| locale.language_id() == super::PSEUDO_LOCALE);
        let fallback_id = fallback.language_id();
        for id in negotiate_languages(
            &[fallback_id],
            &available,
            None,
            NegotiationStrategy::Filtering,
        ) {
            if !chain.contains(id) {
                chain.push(id.clone());
            }
        }
        Selection {
            preferred,
            chain,
            preferred_len,
            framework,
            pseudo,
            generation,
        }
    }

    /// Changes every clone's language selection and advances its cache generation.
    ///
    /// Returns whether the preferences changed; repeating the current selection
    /// neither advances the generation nor requires a view rebuild.
    pub fn select(&self, preferred: Vec<Locale>) -> bool {
        let mut selection = self
            .selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let preferred = if preferred.is_empty() {
            vec![self.fallback.clone()]
        } else {
            preferred
        };
        if selection.preferred == preferred {
            return false;
        }
        *selection = Arc::new(Self::selection(
            &self.catalog,
            &preferred,
            &self.fallback,
            selection.generation.wrapping_add(1),
        ));
        true
    }

    /// The preferred locale used for direction and formatting.
    #[must_use]
    pub fn locale(&self) -> Locale {
        self.selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .preferred[0]
            .clone()
    }

    /// The locale for `category`: the one set for it with
    /// [`Self::set_category_locale`] (the default localizer reads `LC_TIME`,
    /// `LC_NUMERIC`, `LC_COLLATE` and `LC_MONETARY`), or else
    /// [`Self::locale`]. Format dates with `Time`, numbers with `Numeric`,
    /// sort with `Collate`.
    #[must_use]
    pub fn locale_for(&self, category: super::LocaleCategory) -> Locale {
        self.categories
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&category)
            .cloned()
            .unwrap_or_else(|| self.locale())
    }

    /// Sets the locale for `category` in every clone, or with `None` lets it
    /// follow [`Self::locale`]. Returns whether it changed; a change
    /// advances the generation like [`Self::select`].
    pub fn set_category_locale(
        &self,
        category: super::LocaleCategory,
        locale: Option<Locale>,
    ) -> bool {
        let changed = {
            let mut categories = self
                .categories
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match locale {
                Some(locale) => categories.insert(category, locale.clone()) != Some(locale),
                None => categories.remove(&category).is_some(),
            }
        };
        if changed {
            let mut selection = self
                .selection
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::make_mut(&mut selection).generation = selection.generation.wrapping_add(1);
        }
        changed
    }

    /// A revision for translated views and cached subtrees owned by this localizer.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    /// Resolves a message value or `message.attribute`, falling back on missing or broken patterns.
    ///
    /// Sources are tried in order: the app catalog's languages that match the
    /// preferences, the framework's own translations for those languages
    /// ([`FRAMEWORK_TRANSLATIONS`](super::FRAMEWORK_TRANSLATIONS)), the app
    /// catalog's fallback language, then built-in English. Under the
    /// pseudo-locale ([`PSEUDO_LOCALE`](super::PSEUDO_LOCALE)) the result is
    /// [pseudolocalized](super::pseudolocalize).
    ///
    /// A failure in one locale does not discard otherwise valid translations.
    /// Explicit Fluent error arguments are rejected before evaluation.
    /// Missing messages display their identifier. Diagnostics retain failures
    /// even when a fallback succeeds; no partially formatted text is returned.
    #[must_use]
    pub fn resolve(&self, key: &str, arguments: Option<&Arguments<'_>>) -> LocalizedMessage {
        if arguments.is_some_and(|arguments| {
            arguments
                .iter()
                .any(|(_, value)| matches!(value, super::Value::Error))
        }) {
            return LocalizedMessage {
                value: key.to_owned(),
                locale: None,
                errors: vec![LocalizationError::Format(format!(
                    "{key}: arguments contain a Fluent error value"
                ))],
            };
        }
        let selection = self
            .selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (id, attribute) = key
            .split_once('.')
            .map_or((key, None), |(id, attr)| (id, Some(attr)));
        let mut errors = Vec::new();
        fn app<'a, B>(
            catalog: &'a BTreeMap<LanguageIdentifier, B>,
            locales: &'a [LanguageIdentifier],
        ) -> impl Iterator<Item = (&'a LanguageIdentifier, &'a B)> {
            locales
                .iter()
                .filter_map(move |locale| catalog.get(locale).map(|bundle| (locale, bundle)))
        }
        let catalog = &self.catalog.0;
        let (preferred_app, fallback_app) = selection.chain.split_at(selection.preferred_len);
        let framework = selection
            .framework
            .iter()
            .filter_map(|locale| FRAMEWORK_BUNDLES.get(locale).map(|bundle| (locale, bundle)));
        let bundles = app(catalog, preferred_app)
            .chain(framework)
            .chain(app(catalog, fallback_app))
            .chain(std::iter::once((&*ENGLISH_ID, &*ENGLISH_BUNDLE)));
        for (locale, bundle) in bundles {
            let Some(message) = bundle.get_message(id) else {
                continue;
            };
            let pattern = match attribute {
                Some(name) => message.get_attribute(name).map(|attr| attr.value()),
                None => message.value(),
            };
            let Some(pattern) = pattern else {
                continue;
            };
            let mut failures = Vec::new();
            let (value, invalid_number) = super::formatting::capture_number_failure(|| {
                bundle.format_pattern(pattern, arguments, &mut failures)
            });
            if failures.is_empty() && !invalid_number {
                let value = if selection.pseudo {
                    super::pseudolocalize(&value)
                } else {
                    value.into_owned()
                };
                return LocalizedMessage {
                    value,
                    locale: Some(locale.to_string()),
                    errors,
                };
            }
            errors.push(LocalizationError::Format(format!(
                "{locale}/{key}: {failures:?}; invalid NUMBER arguments: {invalid_number}"
            )));
        }
        errors.push(LocalizationError::MissingMessage(key.to_owned()));
        LocalizedMessage {
            value: key.to_owned(),
            locale: None,
            errors,
        }
    }

    /// Formats a value, reporting malformed patterns even when fallback succeeds.
    ///
    /// # Errors
    /// Returns the first resolution diagnostic, or a missing-message error.
    pub fn try_text(
        &self,
        key: &str,
        arguments: Option<&Arguments<'_>>,
    ) -> Result<String, LocalizationError> {
        let result = self.resolve(key, arguments);
        match result.errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(result.value),
        }
    }

    /// Formats a message with safe fallback; use [`Self::resolve`] for diagnostics.
    #[must_use]
    pub fn text(&self, key: &str) -> String {
        self.resolve(key, None).value
    }

    /// Formats a parameterized message with safe fallback.
    #[must_use]
    pub fn text_with(&self, key: &str, arguments: &Arguments<'_>) -> String {
        self.resolve(key, Some(arguments)).value
    }
}

impl Default for Localizer {
    /// The preferred locales and the category locales of the environment.
    fn default() -> Self {
        let localizer = Self::new(
            Catalog::default(),
            super::preferred_locales(),
            Locale::default(),
        );
        for category in super::LocaleCategory::ALL {
            let locale = super::category_locale_from_env(category, |key| std::env::var(key).ok());
            let _ = localizer.set_category_locale(category, locale);
        }
        localizer
    }
}

/// The framework's own translations, one bundle per language.
static FRAMEWORK_BUNDLES: LazyLock<BTreeMap<LanguageIdentifier, Bundle>> = LazyLock::new(|| {
    super::FRAMEWORK_TRANSLATIONS
        .iter()
        .map(|&(tag, source)| {
            let locale: LanguageIdentifier = tag
                .parse()
                .expect("a framework translation's tag must be a language identifier");
            let mut bundle = make_bundle(&locale);
            let resource = FluentResource::try_new(source.to_owned())
                .expect("a framework translation must parse");
            bundle
                .add_resource(Arc::new(resource))
                .expect("a framework translation must have unique entries");
            (locale, bundle)
        })
        .collect()
});

/// The language of the bundled English catalog, the last fallback.
static ENGLISH_ID: LazyLock<LanguageIdentifier> = LazyLock::new(|| Locale::default().language_id());

static ENGLISH_BUNDLE: LazyLock<Bundle> = LazyLock::new(|| {
    let mut bundle = make_bundle(&ENGLISH_ID);
    let resource = FluentResource::try_new(ENGLISH.to_owned())
        .expect("the bundled English Fluent catalog must parse");
    bundle
        .add_resource(Arc::new(resource))
        .expect("the bundled English Fluent catalog must have unique entries");
    bundle
});
