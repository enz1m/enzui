//! Select an installed upright monospace family for the process font system.
//!
//! Preferred names precede a lexicographic fallback among installed families with
//! an upright face marked monospaced. This selects a database family, not universal
//! glyph coverage. If none qualifies, the generic remains unchanged. Installation
//! is disabled on macOS and Windows.
//!
//! [`monospace_font`] and [`ensure_monospace_fallback`] share a once gate and install
//! at first use. Direct calls to [`install_monospace_fallback`] resolve and update
//! again. The selected generic applies to raw monospace font consumers too.

use iced::advanced::graphics::text::cosmic_text::fontdb;

/// Preferred family names, matched case-insensitively in this order.
///
/// Each candidate must own an upright face marked monospaced in the font database.
pub const MONOSPACE_PREFERENCES: &[&str] = &[
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "JetBrains Mono",
    "Fira Code",
    "Source Code Pro",
    "Ubuntu Mono",
    "FreeMono",
];

fn family_has_upright_mono_face(db: &fontdb::Database, family: &str) -> bool {
    db.faces().any(|face| {
        face.monospaced
            && face.style == fontdb::Style::Normal
            && face
                .families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
    })
}

/// Choose an installed upright monospace family without mutating the database.
///
/// Returns the first qualifying preference using the database’s canonical spelling.
/// Otherwise returns the lexicographically smallest primary family name on a qualifying
/// face, or `None` when no upright monospaced face exists.
pub fn resolve_monospace_family(db: &fontdb::Database) -> Option<String> {
    for pref in MONOSPACE_PREFERENCES {
        if family_has_upright_mono_face(db, pref)
            && let Some(canonical) = canonical_family_name(db, pref)
        {
            return Some(canonical);
        }
    }

    let mut fallback: Option<&str> = None;
    for face in db.faces() {
        if face.monospaced
            && face.style == fontdb::Style::Normal
            && let Some((name, _)) = face.families.first()
        {
            match fallback {
                None => fallback = Some(name.as_str()),
                Some(cur) if name.as_str() < cur => fallback = Some(name.as_str()),
                _ => {}
            }
        }
    }
    fallback.map(str::to_owned)
}

fn canonical_family_name(db: &fontdb::Database, family: &str) -> Option<String> {
    db.faces().find_map(|face| {
        face.families
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(family))
            .map(|(name, _)| name.clone())
    })
}

/// Resolve and install the process monospace generic, returning the selected family.
///
/// Returns `None` without mutation on macOS and Windows, or without a qualifying
/// installed face. Each direct call resolves again; use [`ensure_monospace_fallback`]
/// for once-only installation.
///
/// # Panics
///
/// On platforms that install, panics if the process font-system lock is poisoned.
pub fn install_monospace_fallback() -> Option<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use iced::advanced::graphics::text::font_system;

        let mut system = font_system().write().expect("font system poisoned");
        let db = system.raw().db();
        let chosen = resolve_monospace_family(db)?;
        system.raw().db_mut().set_monospace_family(chosen.clone());
        Some(chosen)
    }
}

/// Return a default-weight/style monospace font after ensuring fallback installation.
///
/// Shares the process-wide installation gate with [`ensure_monospace_fallback`].
///
/// # Panics
///
/// Propagates an installation panic or a previously poisoned installation gate.
pub fn monospace_font() -> iced::Font {
    ensure_monospace_fallback();

    iced::Font {
        family: iced::font::Family::Monospace,
        ..iced::Font::DEFAULT
    }
}

/// Run process monospace installation once without constructing a font.
///
/// Use at construction sites that supply their own weight or style. The gate is
/// shared with [`monospace_font`]; later calls do not acquire the font-system lock.
///
/// # Panics
///
/// Propagates an installation panic, including a poisoned font-system lock. Subsequent
/// calls panic if a prior installation panic poisoned the once gate.
pub fn ensure_monospace_fallback() {
    use std::sync::Once;

    static INSTALL_ONCE: Once = Once::new();
    INSTALL_ONCE.call_once(|| {
        let _ = install_monospace_fallback();
    });
}
