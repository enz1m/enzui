//! Writing-direction resolution from widget overrides and locale environment variables.

/// Writing direction, which decides what `Start`/`End` alignment means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    /// Left-to-right: `Start` is the left/top edge.
    #[default]
    LeftToRight,
    /// Right-to-left: `Start` is the right edge (cross-axis mirroring applies
    /// to vertically-stacked popups, whose cross axis is horizontal).
    RightToLeft,
}

/// Returns the scoped localization direction, or resolves process environment variables outside a scope.
///
/// Outside a localization scope, `ENZUI_TEXT_DIRECTION` takes precedence over `LC_ALL`, `LC_MESSAGES`, and
/// `LANG`; absent or unrecognized values default to left-to-right. The
/// environment is read once per process.
#[must_use]
pub fn ambient_direction() -> Direction {
    if let Some(direction) = crate::localization::scoped_locale().map(|locale| locale.direction()) {
        return direction;
    }
    // Read once: layout asks on every pass, and reading four variables each
    // time was a measurable share of rebuilding a busy page.
    static FROM_ENV: std::sync::OnceLock<Direction> = std::sync::OnceLock::new();
    *FROM_ENV.get_or_init(|| direction_from_env(|key| std::env::var(key).ok()))
}

/// Environment variables consulted by [`ambient_direction`], in precedence order.
pub const ENV_KEYS: [&str; 4] = ["ENZUI_TEXT_DIRECTION", "LC_ALL", "LC_MESSAGES", "LANG"];

/// Resolves direction with the [`ambient_direction`] precedence using an injected variable lookup.
#[must_use]
pub fn direction_from_env(get: impl Fn(&str) -> Option<String>) -> Direction {
    if let Some(dir) = get(ENV_KEYS[0]).as_deref().and_then(parse_direction) {
        return dir;
    }

    for key in &ENV_KEYS[1..] {
        if let Some(locale) = get(key) {
            // An empty or `C`/`POSIX` locale carries no language, so it must
            // not shadow a lower-priority variable that does. Skip to the next
            // key rather than committing to LTR here.
            if let Some(dir) = direction_from_locale(&locale) {
                return dir;
            }
        }
    }

    Direction::LeftToRight
}

/// Returns the explicit direction when present, otherwise [`ambient_direction`].
#[must_use]
pub fn resolve_direction(over: Option<Direction>) -> Direction {
    over.unwrap_or_else(ambient_direction)
}

/// Parses `rtl`, `right-to-left`, `righttoleft`, and their left-to-right equivalents, case-insensitively.
///
/// Returns `None` for other values so a lower-priority source can be consulted.
#[must_use]
pub fn parse_direction(s: &str) -> Option<Direction> {
    match s.trim().to_ascii_lowercase().as_str() {
        "rtl" | "right-to-left" | "righttoleft" => Some(Direction::RightToLeft),
        "ltr" | "left-to-right" | "lefttoright" => Some(Direction::LeftToRight),
        _ => None,
    }
}

/// Resolves the language subtag of a POSIX locale.
///
/// Returns `None` for empty, `C`, or `POSIX` locales, a right-to-left value for
/// known RTL languages, and left-to-right for other named languages.
#[must_use]
pub fn direction_from_locale(locale: &str) -> Option<Direction> {
    let lang = locale
        .split(['_', '.', '@', '-'])
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    if lang.is_empty() || lang == "c" || lang == "posix" {
        return None;
    }

    // This fallback is language-only; scripts not represented by these locale language codes require an explicit override.
    const RTL_LANGS: &[&str] = &[
        "ar", "arc", "ckb", "dv", "fa", "he", "iw", "ji", "ks", "ps", "sd", "ug", "ur", "yi",
    ];
    // glibc `ha` and `ku` locales use Latin scripts, while `ks` uses Perso-Arabic.

    if RTL_LANGS.contains(&lang.as_str()) {
        Some(Direction::RightToLeft)
    } else {
        Some(Direction::LeftToRight)
    }
}
