//! Select an installed proportional sans-serif fallback for the process font system.
//!
//! Preferred families must own an upright, non-monospaced face. Candidates are
//! shaped at Normal and Bold using a Latin probe, favoring those whose glyphs avoid
//! monospace and emoji-family faces. Proportional fallback to another family is
//! allowed. This checks two weights and one probe string, not universal glyph or
//! weight coverage. If every shaping check fails, the first structurally suitable
//! candidate remains the best-effort choice.
//!
//! [`ensure_sans_serif_fallback`] installs once on first framework text construction.
//! Installation is disabled on macOS and Windows. A product renderer’s font feature
//! graph may differ from the headless test graph, so headless success alone does
//! not prove deployed font selection. [`sans_serif_resolves`] checks font-database
//! resolution, which can succeed even when actual shaping substitutes another face.

use iced::advanced::graphics::text::cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight, fontdb,
};

/// Preferred installed family names, in selection order.
///
/// Candidates must own an upright proportional face and are favored when the
/// Normal/Bold shaping probe remains proportional. Other installed families follow
/// in lexicographic order. Names are matched case-insensitively.
pub const SANS_SERIF_PREFERENCES: &[&str] = &[
    "Fira Sans",
    "Open Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Noto Sans",
    "Cantarell",
    "Adwaita Sans",
    "Nimbus Sans",
    "FreeSans",
];

fn family_has_upright_proportional_face(db: &fontdb::Database, family: &str) -> bool {
    db.faces().any(|face| {
        !face.monospaced
            && face.style == fontdb::Style::Normal
            && face
                .families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
    })
}

fn cosmic_weight(weight: iced::font::Weight) -> u16 {
    use iced::font::Weight as W;
    match weight {
        W::Thin => 100,
        W::ExtraLight => 200,
        W::Light => 300,
        W::Normal => 400,
        W::Medium => 500,
        W::Semibold => 600,
        W::Bold => 700,
        W::ExtraBold => 800,
        W::Black => 900,
    }
}

const SPAN_PROBE_TEXT: &str = "Settings hjiWQ";

fn family_renders_span_proportionally(system: &mut FontSystem, family: &str) -> bool {
    // Query matching alone cannot expose the shaper’s cross-family substitutions.
    for anchor in [iced::font::Weight::Normal, iced::font::Weight::Bold] {
        let mut buffer = Buffer::new(system, Metrics::new(13.0, 16.0));
        buffer.set_text(
            SPAN_PROBE_TEXT,
            &Attrs::new()
                .family(Family::Name(family))
                .weight(Weight(cosmic_weight(anchor))),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(system, false);

        let mut shaped_any = false;
        for glyph in buffer.layout_runs().flat_map(|run| run.glyphs.iter()) {
            shaped_any = true;
            let Some(face) = system.db().face(glyph.font_id) else {
                return false;
            };
            if face.monospaced {
                return false;
            }
            if face
                .families
                .iter()
                .any(|(name, _)| name.to_ascii_lowercase().contains("emoji"))
            {
                return false;
            }
        }
        if !shaped_any {
            return false;
        }
    }
    true
}

fn canonical_family_name(db: &fontdb::Database, family: &str) -> Option<String> {
    db.faces().find_map(|face| {
        face.families
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(family))
            .map(|(name, _)| name.clone())
    })
}

/// Choose a fallback against the supplied font database without mutating it.
///
/// Installed preferences precede other upright proportional families in lexicographic
/// order. A cloned database is used to shape a Latin probe at Normal and Bold; the
/// first candidate avoiding monospace and emoji-family glyphs wins. Other proportional
/// families may supply glyphs. If every probe fails, return the first structurally
/// suitable candidate; return `None` only when no candidate exists.
///
/// Each call constructs a probe font system and can shape multiple candidates.
pub fn resolve_sans_serif_family(db: &fontdb::Database) -> Option<String> {
    let mut probe = FontSystem::new_with_locale_and_db("en-US".to_owned(), db.clone());

    let mut candidates: Vec<String> = Vec::new();
    for pref in SANS_SERIF_PREFERENCES {
        if family_has_upright_proportional_face(db, pref)
            && let Some(canonical) = canonical_family_name(db, pref)
            && !candidates
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&canonical))
        {
            candidates.push(canonical);
        }
    }
    let mut lex = std::collections::BTreeSet::new();
    for face in db.faces() {
        if !face.monospaced
            && face.style == fontdb::Style::Normal
            && let Some((name, _)) = face.families.first()
        {
            lex.insert(name.clone());
        }
    }
    for name in lex {
        if !candidates.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
            candidates.push(name);
        }
    }

    candidates
        .iter()
        .find(|name| family_renders_span_proportionally(&mut probe, name))
        .or_else(|| candidates.first())
        .cloned()
}

/// Check whether the process sans-serif generic resolves at weights 400, 500, 600 and 700.
///
/// This queries the font database, not rendered glyphs. A positive result does not
/// guarantee that shaping stays proportional or that every glyph paints.
///
/// # Panics
///
/// Panics if the process font-system lock is poisoned.
pub fn sans_serif_resolves() -> bool {
    use iced::advanced::graphics::text::font_system;

    let mut system = font_system().write().expect("font system poisoned");
    let db = system.raw().db();
    [400u16, 500, 600, 700].iter().all(|w| {
        db.query(&fontdb::Query {
            families: &[fontdb::Family::SansSerif],
            weight: fontdb::Weight(*w),
            style: fontdb::Style::Normal,
            ..Default::default()
        })
        .is_some()
    })
}

/// Install a selected family into the process font system and return its name.
///
/// Returns `None` without mutation on macOS and Windows, or when no upright
/// proportional candidate exists. Other platforms resolve on each direct call.
/// Use [`ensure_sans_serif_fallback`] for process-wide once-only installation.
///
/// # Panics
///
/// On platforms that install, panics if the process font-system lock is poisoned.
pub fn install_sans_serif_fallback() -> Option<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use iced::advanced::graphics::text::font_system;

        let mut system = font_system().write().expect("font system poisoned");
        let db = system.raw().db();
        let chosen = resolve_sans_serif_family(db)?;
        system.raw().db_mut().set_sans_serif_family(chosen.clone());
        Some(chosen)
    }
}

static SEAM_REACHED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Return the process-wide number of fallback-seam calls, including calls after installation.
///
/// This diagnostic tally has no ordering relation to other state. Concurrent callers
/// can contribute to a delta measured around a particular widget construction.
pub fn seam_reached_count() -> u64 {
    SEAM_REACHED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Run fallback installation once per process and count every seam call.
///
/// Framework text constructors call this automatically. Subsequent calls increment
/// the diagnostic tally without acquiring the font-system lock or resolving again.
///
/// # Panics
///
/// Propagates an installation panic, including a poisoned font-system lock. Subsequent
/// calls panic if a prior installation panic poisoned the once gate.
pub fn ensure_sans_serif_fallback() {
    use std::sync::Once;

    // Count outside Once so diagnostics can observe each construction seam.
    SEAM_REACHED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    static INSTALL_ONCE: Once = Once::new();
    INSTALL_ONCE.call_once(|| {
        let _ = install_sans_serif_fallback();
    });
}
