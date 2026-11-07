//! `Palette::tone_on(SemanticTone, backdrop)` is the tone counterpart
//! of `text_muted_on` / `subtle_on` / `disabled_on`, resolving the
//! status-text foreground against the surface a widget actually paints on.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_theme::{Palette, RaisedInk, SemanticTone};
use iced_core::Color;

fn rgb_u8(c: Color) -> (u8, u8, u8) {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    (q(c.r), q(c.g), q(c.b))
}

fn lum_u8(c: Color) -> f64 {
    let ch = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = rgb_u8(c);
    0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
}

/// WCAG ratio between two colours, measured at the 8-bit pixels they will
/// paint (matching how `raise_to_floor` reports its own success).
fn ratio(a: Color, b: Color) -> f64 {
    let (la, lb) = (lum_u8(a), lum_u8(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

const TEXT_FLOOR: f64 = 4.5;

fn raw_tone(p: &Palette, t: SemanticTone) -> Color {
    match t {
        SemanticTone::Success => p.tone_success_fg,
        SemanticTone::Warning => p.tone_warning_fg,
        SemanticTone::Error => p.tone_error_fg,
    }
}

/// A round of the same claim over the shipped presets themselves: every
/// passing raw role stays byte-identical.
#[test]
fn default_preservation_returns_passing_shipped_roles_byte_identical() {
    for (mode, p) in [("light", Palette::light()), ("dark", Palette::dark())] {
        for (name, bg) in [
            ("background", p.background),
            ("surface", p.surface),
            ("surface_elevated", p.surface_elevated),
        ] {
            for tone in [
                SemanticTone::Success,
                SemanticTone::Warning,
                SemanticTone::Error,
            ] {
                let raw = raw_tone(&p, tone);
                if ratio(raw, bg) < TEXT_FLOOR {
                    continue; // rows where the raise SHOULD fire
                }
                let ink = p.tone_on(tone, bg);
                assert!(
                    matches!(ink, RaisedInk::AlreadyLegible(_)),
                    "{mode} {tone:?} on {name}: passing raw role \
                     unexpectedly {ink:?}",
                );
                assert_eq!(
                    rgb_u8(ink.color()),
                    rgb_u8(raw),
                    "{mode} {tone:?} on {name}: passing raw role must \
                     round-trip byte-identical",
                );
            }
        }
    }
}
