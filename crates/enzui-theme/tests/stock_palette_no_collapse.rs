//! Under a stock iced theme (Dracula, Nord, plain Dark, ...) the
//! extended-palette `.text` fields on `weakest / weaker / weak` all collapse
//! to a single `readable()`-processed near-white against the near-black
//! backgrounds, so a naive fallback in `Palette::from_iced` gives
//! `text_muted == text_subtle == disabled` — three distinct enzUI roles
//! reduced to one.
//!
//! And `accent_hover` had no dedicated iced slot in the forward transport, so
//! for a stock theme it kept the STRUCTURAL preset's value (a hardcoded blue
//! shade). When the theme's primary is a different hue (Dracula purple), hover
//! jumped to an unrelated colour.

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

use enzui_theme::Palette;
use iced_core::{Color, Theme};

fn rgb_u8(c: Color) -> (u8, u8, u8) {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    (q(c.r), q(c.g), q(c.b))
}

const DISTINCT: i32 = 8;

fn abs_delta(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    (i32::from(a.0) - i32::from(b.0)).abs()
        + (i32::from(a.1) - i32::from(b.1)).abs()
        + (i32::from(a.2) - i32::from(b.2)).abs()
}

/// After `Palette::from_iced` the three roles must be materially
/// distinct on every stock theme.
#[test]
fn stock_theme_text_tiers_are_distinct_after_from_iced() {
    for (name, t) in [
        ("Dracula", Theme::Dracula),
        ("Nord", Theme::Nord),
        ("Dark", Theme::Dark),
        ("SolarizedLight", Theme::SolarizedLight),
    ] {
        let p = Palette::from_iced(&t);
        let text = rgb_u8(p.text);
        let muted = rgb_u8(p.text_muted);
        let subtle = rgb_u8(p.text_subtle);
        let disabled = rgb_u8(p.disabled);
        assert!(
            abs_delta(text, muted) >= DISTINCT,
            "{name}: text {text:?} and text_muted {muted:?} must be materially \
             distinct (delta >= {DISTINCT})"
        );
        assert!(
            abs_delta(muted, subtle) >= DISTINCT,
            "{name}: text_muted {muted:?} and text_subtle {subtle:?} must be \
             materially distinct (delta >= {DISTINCT})"
        );
        assert!(
            abs_delta(subtle, disabled) >= DISTINCT,
            "{name}: text_subtle {subtle:?} and disabled {disabled:?} must be \
             materially distinct (delta >= {DISTINCT})"
        );
    }
}
