# Localization

`enzui_core::localization`, also re-exported as
`enzui_widgets::localization`, supplies Fluent message catalogs, language
negotiation, scoped widget defaults, and ICU4X formatting backed by CLDR data.
Applications can translate their own messages and framework controls through
the same catalog. The framework's own messages ship in English, German,
Spanish, French and Hungarian (`ENGLISH` and `FRAMEWORK_TRANSLATIONS`). A
`Localizer` uses them after the app's catalog for the same language and
before the app's fallback language, so framework controls follow the user's
language without the app shipping their strings. Other languages are supplied
by the application.

## Standalone core consumers

A daemon or renderer that only needs localization can depend on `enzui-core`
without the iced facade or a windowing backend:

```toml
[dependencies]
enzui-core = { path = "../enzui/crates/enzui-core" }
```

The core crate directly pins the renderer-neutral iced dependencies. It needs
no iced source checkout or root patch table for this default-feature path.
GUI applications should follow [Dependencies](dependencies.md) and
[Building an app](building-an-app.md).

## Catalogs and language selection

Add UTF-8 Fluent resources with `CatalogBuilder::add_resource` and freeze the
builder with `build`. Resources for one locale share terms and message
references. Syntax errors and duplicate entries reject the entire resource
without changing the builder. Catalogs are immutable, thread-safe, and cheap
to clone. `add_resource` accepts source text and performs no filesystem or
network access; apps choose how to embed, read, or distribute their translation
files. Resource locale tags are canonicalized, and formatting extensions are
ignored when grouping resources into language bundles.

Create a `Localizer` with `Localizer::new(catalog, preferred, fallback)`, using
an ordered `Vec<Locale>` of preferences and an explicit fallback locale. Empty
preferences select the fallback for formatting and layout too.
`preferred_locales` reads Linux language preferences without modifying the
environment. The first valid value in `LC_ALL`, `LC_MESSAGES`, then `LANG`
chooses the base locale. Unless that base is `C` or `POSIX`, colon-separated
`LANGUAGE` preferences precede it. The final default is `en-US`.

`Locale::from_posix` accepts POSIX locale names; ordinary parsing accepts Unicode
locale tags, including script, region, and Unicode formatting extensions.
Locale aliases are canonicalized using CLDR data.

Languages are negotiated using Fluent's filtering strategy. Resolution checks
each negotiated language for the requested message value or attribute, then
languages negotiated from the configured fallback, then the built-in English
widget catalog. Fallback is per message and per attribute. References and terms
resolve within the candidate language's bundle; they do not borrow definitions
from a fallback language. The first preferred locale still controls layout and
typed `Locale` formatting when its translations are incomplete.

`Localizer::text` returns display text, while `text_with` supplies named
`Arguments`. Numeric arguments use cardinal plural rules by default;
`NUMBER($value, type: "ordinal")` selects ordinal rules. `message.attribute`
selects an attribute. The `enzui_core::localized!` macro, also re-exported as
`enzui_widgets::localized!`, formats named arguments through the current scope.

Use `resolve` when diagnostics matter: it reports the language that supplied
the text and any formatting failures encountered before fallback. A broken
pattern is skipped in its entirety. `try_text` returns the first diagnostic
even when fallback succeeds. A missing value in an earlier language is not
itself a diagnostic when fallback succeeds. To check translation coverage,
inspect the source language in `resolve` as well as its errors. If no candidate
supplies a usable value or attribute, resolution displays its identifier and
reports `MissingMessage` along with any formatting failures.

## The application's catalog everywhere

Give the localizer to `app_setup`:
`app_setup::run_with_services(program, AppServices::default().localizer(localizer.clone()))`.
`boot`, `update`, `view`, `title` and `subscription` then run in its scope,
so `localization::text("key")` and `localized!("key", "name" => value)` read
the application's catalog wherever the app needs a string, with no global or
`tr` helper of its own, and framework controls follow the same language. Keep
the clone in the application state: `select` on it changes the language, and
the next view shows it. Background work does not inherit the scope; pass it a
clone. A language-selection message calls `select` on the retained localizer
and rebuilds the view in its updated scope.

## Widget defaults and switching languages

Build each root view with `Localizer::view`. Its closure scopes widget
construction, and its wrapper retains the locale through layout, lazily built
overlays, event handling, painting and accessibility collection.
`Localizer::scope` can also surround a synchronous event loop when the whole
application shares one language. Default widget labels use the scoped catalog;
explicit labels supplied to a widget take precedence. Scope synchronous core
operations such as form validation when producing their messages outside the
view. The scope restores its predecessor on return or panic and never changes
another thread's scope.

Outside an explicit scope, `localization::current` and the free text functions
use a thread-local default initialized from the environment on first use. That
default has no application catalog, but framework widgets still format
numbers, sizes, dates, times and calendars in the environment's locale. Scope
the view so app text, framework text and formatting agree.

Cloned localizers share language selection. `select` replaces the preferred
locales and advances `generation` only when the preferences change. It returns
`true` for a changed selection and `false` for an identical one. The app handles
its language-selection message and rebuilds its view. Include the generation in
keys for translated cached subtrees and regenerate any app-owned strings stored
outside the view. Selection does not schedule frames, change
the operating system's locale, or rewrite existing strings.

For independently localized windows, create separate localizers over the same
catalog and use each localizer's `view` method for that window. A scope surrounds
a synchronous call; futures and background tasks do not inherit it. Pass a
localizer explicitly to background work and use its methods there.

Human-readable label getters return owned `String` values where catalog data
supplies their text. Keep canonical serialization values separate from display
labels. Use `Shortcut::localized_label` and `ShapeKind::localized_name` for
display; shortcut `Display` text and `ShapeKind::name` remain language-independent.

Keep control identity separate from translated text. Supply stable widget IDs
when using controls whose default identity derives from their label.
Framework-owned control roles, such as window close and search clear, retain
their IDs when their labels change.

Text, secure, multiline, autocomplete and combo fields accept owned labels,
placeholders and validation captions. Checkbox, toggle and radio-group labels,
date-picker labels, date-range placeholders, composer placeholders and
preference-row input placeholders also accept owned text. Pass
`Localizer::text` results directly when returning a view; controlled input
values and editor buffers remain borrowed. `enz_text_field_owned` additionally
accepts an owned value for retained subtrees. Pin field IDs explicitly when
translating labels; label-derived default IDs follow the displayed text. For
keyed radio options, `RadioChoice::with_label` accepts an owned label while
keeping the option key separate.

## Formatting and direction

| Need | API and behavior |
|---|---|
| Exact numbers | `Locale::format_decimal` formats `Decimal` values with CLDR grouping, separators and numbering systems. Decimal input preserves integers beyond binary floating-point precision. |
| Fluent numbers | Plain numeric arguments and `NUMBER` use the message's language, including when a fallback supplies it. Preferred-locale Unicode formatting extensions do not apply to these bundles. Decimal grouping, minimum integer digits, fraction digits, and significant digits are supported. |
| Dates | `Locale::format_date` supports short, medium, long and full Gregorian dates; `format_month_day` omits the year. The framework's civil date model remains Gregorian. |
| Times | `Locale::format_time` uses the locale's hour cycle, day-period placement and labels, with optional seconds. `Localizer::format_compact_hour` omits minutes and uses catalog day periods. Unicode `u-hc` and `u-nu` preferences apply. |
| Week conventions | `Locale::first_weekday` uses CLDR region data and Unicode region/first-weekday overrides. Pass it to the app-owned calendar controller. |
| Calendar widgets | Calendar and date-picker defaults use `LocalizedCalendarLabels`. Calendars use the current localizer's CLDR names, native numerals, and formatting plus catalog messages for announcements. Weekday rows use abbreviated names when they fit and narrow names otherwise; accessible names remain full. English defaults apply only if calendar formatters cannot be created. Explicit `CalendarLabels` implementations remain supported. |
| Stable calendar snapshots | `LocaleCalendarLabels::new` captures one selection and reusable formatters. Recreate it when the app changes languages. |
| Money, percent, relative time, durations | With `enzui-core`'s `locale-formatters` feature (forwarded by `enzui-widgets`): `Locale::format_currency(&amount, "EUR")`, `format_percent(&percent)` (42 gives `42 %` in German), `format_relative_time(seconds)` (`5 minutes ago`, `yesterday`, `in 2 hours`; negative is the past) and `format_duration(duration, DurationStyle)` (`1 hr, 5 min`, `1:05:00`), all from CLDR. File sizes: `enzui_widgets::file_browser::format_bytes`. |
| Sorting | `Locale::collator(Collation)` compares strings in the language's order: accented letters beside their base letter, Swedish `ä` after `z`, and with `Collation::NATURAL` digit runs by value (`file2` before `file10`). `CollationStrength` chooses whether case and accents count. The file browser sorts names with the `LC_COLLATE` locale's collator. |
| Searching | `enzui_core::text_fold::fold` and `text_fold::contains` match without case, accents or compatibility forms (`cafe` finds `Café`, `STRASSE` finds `Straße`). The file browser, command palette, searchable combo box and font chooser filter through it. |
| Layout | The scoped locale determines ambient direction from its explicit or likely script. Outside a scope, ambient direction uses the environment-based fallback. Per-widget direction overrides take precedence. Widgets with direction support use this ambient value. |

Fluent digit options are bounded to 100. Minimum integer and significant-digit
counts start at 1; fraction-digit counts can be 0. Significant-digit options
take precedence over fraction-digit options, and rounding ties go away from
zero. Unsupported styles, unknown options and invalid option combinations
report formatting errors.

Editable fields keep an explicit parsing grammar. `Localizer::time_format`
supplies a reversible time grammar with ASCII digits and a colon separator,
using the locale's hour cycle and translated AM/PM suffixes. Labels are trimmed,
must be nonempty and distinct after Unicode lowercasing, and cannot begin with
an ASCII digit or colon. Time fields use the current localizer's grammar by
default. If resolving it fails, they use H12 with English day-period labels. An explicit `.format(...)` takes precedence. Call
`Localizer::time_format` directly when these failures need to be reported.

`TimeFormat` uses the same grammar for both displayed values and entered text;
CLDR display output is not assumed to be parseable by that grammar. Reformat
app-owned time-field drafts when switching languages so their text matches the
new grammar. H24 is supported by the editable grammar; CLDR display formatting
returns an error for that cycle. Language changes do not alter dates,
timestamps, time zones, numeric values, or identifiers in the app model.

Fluent numeric arguments use binary floating-point values. Use exact `Decimal`
formatting for precise large integers and financial quantities, passing the
result as display text when necessary. `NUMBER` does not support currency or
percent styles, and `DATETIME` is unavailable. Format dates, times, money and
percentages through `Locale`, and pass the result to the message as text.

## A locale per category

The environment can set a different locale for each kind of formatting:
`LANG=en_US.UTF-8 LC_TIME=de_DE.UTF-8` shows English text with German dates.
`Localizer::locale_for(LocaleCategory)` answers for `Time`, `Numeric`,
`Collate` and `Monetary`: the locale set with `set_category_locale`, or else
the preferred locale. The default localizer reads `LC_ALL`, then the category's
own variable, then `LANG`, as POSIX does; a localizer built with
`Localizer::new` follows its preferred locale until told otherwise. The
framework formats dates and times with `Time`, numbers and sizes with
`Numeric`, and sorts with `Collate`. An application that switches its language
in its own settings clears the categories with
`set_category_locale(category, None)` if formats should follow.

## Right-to-left layout

In a right-to-left locale (Arabic, Hebrew, Persian, Urdu...) framework widgets
mirror on their own: the app shell, title bar (window controls move to the
left, close outermost), toolbar rows, sidebar and menu rows, breadcrumbs,
tab strips and view switchers, tree views (indented from the right, Left
expands), disclosures, text and search fields, sliders (minimum on the right,
Left increases) and switches. Each of these takes `.direction(..)` to
override the locale. Focus and accessibility order stay logical, so Tab still
moves from the start edge, now on the right.

The app's own layout mirrors where it opts in:

- `row![..].mirrored(rtl)` lays an iced row out from the right while keeping
  its children's logical order; `keyed_row(..).mirrored(rtl)` does the same for
  a keyed row. Read `rtl` from `localizer.locale().direction()` or
  `enzui_core::placement::ambient_direction()`.
- `enz_text` aligns `TextAlign::Start` by default, which is the right edge in
  a right-to-left locale. `TextAlign::Left` and `Right` stay physical.
- Directional icons named by meaning (`navigation.back`, `navigation.forward`,
  `navigation.sidebar_left`) point the other way in a right-to-left locale; a
  literal glyph name such as `chevron-right` is drawn as asked.
  `enz_icon_for_direction` takes an explicit direction.

Single-line fields move the caret on screen through right-to-left and mixed
text: Left moves left whatever the storage order, and Ctrl+Left jumps words
in the line's direction.

## Translating framework messages

The complete English catalog is
[`en-US.ftl`](../crates/enzui-core/src/localization/locales/en-US.ftl), also
available as `localization::ENGLISH`. The framework ships its own translations
in German (`de`), Spanish (`es`), French (`fr`) and Hungarian (`hu`), listed in
`localization::FRAMEWORK_TRANSLATIONS`. A localizer looks a message up in this
order:

1. the app's catalog for the languages matching the user's preferences;
2. the framework's translation for those languages;
3. the app's catalog for its fallback language;
4. built-in English.

So framework widgets speak a shipped language with no app code, an app
resource for that language can still override any framework message, and an
app's English fallback does not hide a framework translation. For other
languages, add a translated copy of `en-US.ftl` to the app's catalog for that
locale, preserving message IDs and argument names.

### Finding untranslated text

Select the pseudo-locale `en-XA` (`localization::PSEUDO_LOCALE`), for example
with `LANGUAGE=en_XA` or `localizer.select(vec!["en-XA".parse()?])`. Every
catalog message then shows accented and about 40 percent longer, in brackets:
`Close` becomes `[Çļöšé ~~]`. Plain English left on screen was never routed
through the catalog, and a missing closing bracket marks text that is clipped.
Inserted values stay as they are.

### Coverage

`localization::coverage(reference, translation)` lists the message keys a
translation is missing and the ones it keeps after the reference dropped them.
The `enzui-l10n-coverage` command prints that report and exits with status 1 when a
translation is incomplete:

```sh
cargo run --bin enzui-l10n-coverage                       # the framework's translations
cargo run --bin enzui-l10n-coverage -- en-US.ftl de.ftl   # an app's catalogs
```

### Contributing a framework translation

1. Copy `crates/enzui-core/src/localization/locales/en-US.ftl` to
   `<language>.ftl` in the same directory and translate every value. Keep the
   keys, the `{ $argument }` names and any `NUMBER(..)` calls; adapt plural
   variants to the language's CLDR plural categories.
2. Add the file to `FRAMEWORK_TRANSLATIONS` in
   `crates/enzui-core/src/localization/mod.rs`.
3. Run `cargo wt --lib shipped_translations_are_complete`. This gate fails when
   any shipped translation lacks a key from English or keeps an obsolete one,
   so a change that adds an English message also adds it to every shipped
   language.
4. Check the result in a running app with `LANGUAGE=<language>`.

Provide fonts that cover the application's supported scripts; translation
catalogs do not include font assets.

Keep inserted values as arguments so translators can choose word order and
plural forms. Fluent adds bidirectional isolation around inserted values where
needed. Treat formatted text as opaque display text: do not strip its isolation
marks, parse it back into model values, or use it as a control ID.

Validate catalogs with representative numeric values, every plural category
used by the language, and the arguments supplied by the app. Check rendered
views with longer translations and RTL text, including focus, overlays,
tooltips and accessibility labels. English fallback makes incomplete catalogs
usable; it does not demonstrate that a language is fully translated.
