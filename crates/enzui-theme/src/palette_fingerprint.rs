//! Palette fingerprint: a cheap content hash of every [`Palette`] colour field,
//! used to detect whether two palettes differ without `PartialEq`.
//!
//! The digest is stable across platforms and Rust versions while the ordered
//! 8-bit RGBA values of both built-in palettes remain unchanged.

use iced_core::Color;

use crate::palette::Palette;

/// Expected fingerprint for the committed built-in palettes.
pub const COMMITTED_FINGERPRINT: &str = include_str!("../PALETTE_FINGERPRINT").trim_ascii_end();

/// Returns a deterministic 16-hex-character FNV-1a hash of the ordered,
/// quantized RGBA fields in both built-in palettes.
///
/// Equal inputs produce the same value across supported platforms and Rust
/// versions. As a 64-bit hash it can collide, so it is a change detector rather
/// than a proof that every channel is identical.
pub fn palette_fingerprint() -> String {
    let mut h = Fnv1a64::new();
    // Palette order and separators are part of the digest format.
    h.write(b"enzui-palette-v1\n");
    h.write(b"light:");
    walk_palette(&Palette::light(), &mut h);
    h.write(b"\ndark:");
    walk_palette(&Palette::dark(), &mut h);
    h.write(b"\n");
    format!("{:016x}", h.finish())
}

// Field order is part of the digest format; exhaustive destructuring forces new
// `Palette` fields to be considered before compilation succeeds.
fn walk_palette(p: &Palette, h: &mut Fnv1a64) {
    let Palette {
        background,
        surface,
        surface_alt,
        surface_elevated,
        text,
        text_muted,
        text_subtle,
        surface_hover,
        surface_selected,
        image_canvas,
        video_canvas,
        border,
        border_strong,
        control_border,
        control_border_strong,
        field,
        field_border,
        field_border_strong,
        accent,
        accent_hover,
        accent_pressed,
        accent_soft,
        danger,
        danger_soft,
        destructive,
        success,
        success_soft,
        warning,
        warning_soft,
        rating,
        info,
        info_soft,
        tone_success_fg,
        tone_warning_fg,
        tone_error_fg,
        overlay,
        shadow,
        focus_ring,
        disabled,
        selected,
        error,
        button_primary_bg,
        button_primary_hover_bg,
        button_primary_pressed_bg,
    } = p;
    let fields: &[(&[u8], &Color)] = &[
        (b"|background=", background),
        (b"|surface=", surface),
        (b"|surface_alt=", surface_alt),
        (b"|surface_elevated=", surface_elevated),
        (b"|text=", text),
        (b"|text_muted=", text_muted),
        (b"|text_subtle=", text_subtle),
        (b"|surface_hover=", surface_hover),
        (b"|surface_selected=", surface_selected),
        (b"|image_canvas=", image_canvas),
        (b"|video_canvas=", video_canvas),
        (b"|border=", border),
        (b"|border_strong=", border_strong),
        (b"|control_border=", control_border),
        (b"|control_border_strong=", control_border_strong),
        (b"|field=", field),
        (b"|field_border=", field_border),
        (b"|field_border_strong=", field_border_strong),
        (b"|accent=", accent),
        (b"|accent_hover=", accent_hover),
        (b"|accent_pressed=", accent_pressed),
        (b"|accent_soft=", accent_soft),
        (b"|danger=", danger),
        (b"|danger_soft=", danger_soft),
        (b"|destructive=", destructive),
        (b"|success=", success),
        (b"|success_soft=", success_soft),
        (b"|warning=", warning),
        (b"|warning_soft=", warning_soft),
        (b"|rating=", rating),
        (b"|info=", info),
        (b"|info_soft=", info_soft),
        (b"|tone_success_fg=", tone_success_fg),
        (b"|tone_warning_fg=", tone_warning_fg),
        (b"|tone_error_fg=", tone_error_fg),
        (b"|overlay=", overlay),
        (b"|shadow=", shadow),
        (b"|focus_ring=", focus_ring),
        (b"|disabled=", disabled),
        (b"|selected=", selected),
        (b"|error=", error),
        (b"|button_primary_bg=", button_primary_bg),
        (b"|button_primary_hover_bg=", button_primary_hover_bg),
        (b"|button_primary_pressed_bg=", button_primary_pressed_bg),
    ];
    for (name, c) in fields {
        h.write(name);
        let (r, g, b, a) = quantise_rgba8(**c);
        h.write(&[r, g, b, a]);
    }
}

fn quantise_rgba8(c: Color) -> (u8, u8, u8, u8) {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    (q(c.r), q(c.g), q(c.b), q(c.a))
}

struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    fn new() -> Self {
        Self(Self::OFFSET)
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(Self::PRIME);
        }
        self.0 = h;
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// Assertions for keeping the compiled palette fingerprint equal to the
/// committed fingerprint consumed by app baseline checks.
pub mod snapshot {
    use super::{COMMITTED_FINGERPRINT, palette_fingerprint};

    /// Verifies that the compiled palette matches the committed fingerprint.
    ///
    /// # Panics
    ///
    /// Panics with update instructions when the fingerprints differ.
    pub fn assert_committed_matches_current() {
        let current = palette_fingerprint();
        let committed = COMMITTED_FINGERPRINT;
        assert_eq!(
            current, committed,
            "\nPALETTE FINGERPRINT MOVED — a built-in palette token changed.\n\
             \n\
             committed (PALETTE_FINGERPRINT): {committed}\n\
             current  (Palette::light+dark):  {current}\n\
             \n\
             This is not a bug in this test. A colour on Palette::light() or\n\
             Palette::dark() moved, so any screenshot baseline that records the\n\
             OLD fingerprint is now stale. To bless the move, update\n\
             crates/enzui-theme/PALETTE_FINGERPRINT to:\n\
                  {current}\n\
             \n\
             See crates/enzui-theme/src/palette_fingerprint.rs.\n"
        );
    }
}
