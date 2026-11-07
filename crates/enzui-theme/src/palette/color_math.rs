use iced::Color;

pub(crate) fn hex(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    )
}

// Sum of absolute quantized channel differences (0..=765) in gamma-encoded
// sRGB. `Palette::from_iced` uses this simple channel metric to detect collapsed
// or inverted transported emphasis tiers; it is not a perceptual distance.
pub(crate) fn srgb_distance(a: Color, b: Color) -> i32 {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as i32;
    (q(a.r) - q(b.r)).abs() + (q(a.g) - q(b.g)).abs() + (q(a.b) - q(b.b)).abs()
}

// Interpolate gamma-encoded sRGB channels at `factor` (0 = `a`, 1 = `b`); this
// is not a perceptually uniform mix.
pub(crate) fn mix_srgb(a: Color, b: Color, factor: f32) -> Color {
    let f = factor.clamp(0.0, 1.0);
    let g = 1.0 - f;
    Color {
        r: a.r * g + b.r * f,
        g: a.g * g + b.g * f,
        b: a.b * g + b.b * f,
        a: a.a,
    }
}

pub(super) fn wcag_luminance(c: Color) -> f32 {
    0.2126 * linear_channel(c.r) + 0.7152 * linear_channel(c.g) + 0.0722 * linear_channel(c.b)
}

/// One sRGB channel in linear light.
///
/// Style closures solve contrast per widget per frame, and nearly every colour
/// they see is an 8-bit value, so those read a table built with the same
/// formula (bit-identical) instead of a `powf` each.
fn linear_channel(v: f32) -> f32 {
    let s = v.clamp(0.0, 1.0);
    let scaled = s * 255.0;
    let index = scaled.round();
    if scaled == index {
        return LINEAR_TABLE[index as usize];
    }
    srgb_channel_to_linear(s)
}

fn srgb_channel_to_linear(s: f32) -> f32 {
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

static LINEAR_TABLE: std::sync::LazyLock<[f32; 256]> =
    std::sync::LazyLock::new(|| std::array::from_fn(|i| srgb_channel_to_linear(i as f32 / 255.0)));

pub(crate) fn wcag_contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (wcag_luminance(a), wcag_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

// Use the accent when it clears 3:1 against page and surface; otherwise choose
// the first quantized step toward black or white that clears both.
pub(crate) fn focus_ring_with_contrast(accent: Color, page: Color, surface: Color) -> Color {
    const FLOOR: f32 = 3.0;
    const N: usize = 256;

    let accent_opaque = Color { a: 1.0, ..accent };
    let backdrops = [quantise_srgb(page), quantise_srgb(surface)];
    let passes = |candidate: Color| {
        let candidate = quantise_srgb(candidate);
        backdrops
            .iter()
            .all(|backdrop| wcag_contrast(candidate, *backdrop) >= FLOOR)
    };

    if passes(accent_opaque) {
        return accent_opaque;
    }

    let first_passing = |endpoint: Color| {
        (1..=N).find_map(|step| {
            let factor = step as f32 / N as f32;
            let candidate = quantise_srgb(mix_srgb(accent_opaque, endpoint, factor));
            passes(candidate).then_some((step, candidate))
        })
    };

    match (first_passing(Color::WHITE), first_passing(Color::BLACK)) {
        (Some(white), Some(black)) => {
            if white.0 <= black.0 {
                white.1
            } else {
                black.1
            }
        }
        (Some((_, white)), None) => white,
        (None, Some((_, black))) => black,
        (None, None) => {
            // If neither arm clears both surfaces, return the stronger worst-case endpoint.
            let minimum = |candidate: Color| {
                backdrops
                    .iter()
                    .map(|backdrop| wcag_contrast(candidate, *backdrop))
                    .fold(f32::INFINITY, f32::min)
            };
            if minimum(Color::WHITE) >= minimum(Color::BLACK) {
                Color::WHITE
            } else {
                Color::BLACK
            }
        }
    }
}

/// Pure white or black, whichever has the higher WCAG 2 ratio on
/// `background`: the endpoint to mix toward when a colour must gain contrast
/// on a surface. Text on a coloured fill uses [`legible_ink_on`] instead.
pub(crate) fn extreme_ink_on(background: Color) -> Color {
    if wcag_contrast(Color::WHITE, background) >= wcag_contrast(Color::BLACK, background) {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

/// The ink for labels and icons on a coloured `fill`: white, or a dark ink
/// in the fill's own hue on fills too light for white.
///
/// The choice of polarity follows APCA, not the WCAG 2 ratio. WCAG 2
/// overrates black on saturated mid tones (black scores 5.4:1 on the
/// `#3584e4` accent, white 3.9:1), so "higher ratio wins" puts black labels
/// on blue buttons, which reads worse than white and looks broken. APCA
/// scores white clearly ahead there, as readers and every major design
/// system do (GNOME, Apple, Material and Fluent put white on their blues).
/// White also needs at least 3:1 under WCAG 2, so a mid grey still takes
/// dark ink. A fill that must carry text at 4.5:1 is darkened for white by
/// [`super::Palette::filled_color`] rather than flipped to black.
///
/// The dark ink is never pure black: it is the fill's hue at very low
/// lightness and low chroma (the near-black, hue-tinted foreground Radix and
/// libadwaita pair with yellow and amber fills), darkened further only as
/// far as 4.5:1 needs.
pub(crate) fn legible_ink_on(fill: Color) -> Color {
    let fill = Color { a: 1.0, ..fill };
    memo::color(
        &memo::LEGIBLE_INK,
        memo::key([fill, Color::TRANSPARENT], 0.0),
        || legible_ink_on_uncached(fill),
    )
}

fn legible_ink_on_uncached(fill: Color) -> Color {
    let dark = tinted_dark_ink(fill);
    let white_reads = wcag_contrast(Color::WHITE, fill) >= 3.0;
    if white_reads && apca_lc(Color::WHITE, fill).abs() >= apca_lc(dark, fill).abs() {
        Color::WHITE
    } else {
        dark
    }
}

/// `fill` deepened until white text reaches `target` on it: OkLab lightness
/// goes down with the hue kept, giving up only the chroma the sRGB gamut
/// forces at each lightness, so a vivid blue stays a blue rather than
/// greying the way mixing toward black does.
pub(crate) fn deepen_for_white(fill: Color, target: f32) -> Color {
    memo::color(
        &memo::DEEPEN,
        memo::key([fill, Color::WHITE], target),
        || deepen_for_white_uncached(fill, target),
    )
}

fn deepen_for_white_uncached(fill: Color, target: f32) -> Color {
    shift_to_contrast(fill, Color::WHITE, target)
}

/// `color` moved in OkLab lightness away from `against` until the two reach
/// `target`: darker against a light surface, lighter against a dark one. The
/// hue is kept and only the chroma the sRGB gamut forces is given up, so a
/// vivid colour keeps its identity where a fixed-chroma search runs out of
/// gamut. Returns the endpoint (black or white) when nothing on the path
/// reaches the target.
pub(crate) fn shift_to_contrast(color: Color, against: Color, target: f32) -> Color {
    let fill = Color { a: 1.0, ..color };
    if wcag_contrast(against, fill) >= target {
        return fill;
    }
    let darker = wcag_contrast(Color::BLACK, against) >= wcag_contrast(Color::WHITE, against);
    let (l0, a, b) = super::raise::srgb_to_oklab(fill);
    let in_gamut = |c: Color| {
        const E: f32 = 1.0 / 512.0;
        [c.r, c.g, c.b].iter().all(|v| (-E..=1.0 + E).contains(v))
    };
    let step = if darker { -0.005 } else { 0.005 };
    let mut lightness = l0;
    while (0.0..=1.0).contains(&(lightness + step)) {
        lightness += step;
        let mut scale = 1.0_f32;
        while scale > 0.0 && !in_gamut(super::raise::oklab_to_srgb(lightness, a * scale, b * scale))
        {
            scale -= 0.02;
        }
        let scale = scale.max(0.0);
        let candidate = quantise_srgb(super::raise::oklab_to_srgb(lightness, a * scale, b * scale));
        if wcag_contrast(against, candidate) >= target {
            return Color {
                a: 1.0,
                ..candidate
            };
        }
    }
    if darker { Color::BLACK } else { Color::WHITE }
}

/// Whether white text reads better than [`legible_ink_on`]'s dark ink on
/// `fill` by APCA alone: the polarity for a fill that may be deepened until
/// white reaches its ratio.
pub(crate) fn prefers_white(fill: Color) -> bool {
    let fill = Color { a: 1.0, ..fill };
    memo::lookup(
        &memo::PREFERS_WHITE,
        memo::key([fill, Color::TRANSPARENT], 0.0),
        || apca_lc(Color::WHITE, fill).abs() >= apca_lc(tinted_dark_ink(fill), fill).abs(),
    )
}

/// A near-black in `fill`'s hue with at least 4.5:1 on it (pure black when
/// nothing tinted reaches that).
fn tinted_dark_ink(fill: Color) -> Color {
    // The counter distinguishes cache hits from recomputation.
    #[cfg(test)]
    tinted_dark_ink_calls::bump();
    let (_, a, b) = super::raise::srgb_to_oklab(fill);
    let chroma = (a * a + b * b).sqrt();
    // Keep a hint of the hue, never a coloured ink.
    let scale = if chroma > 0.0 {
        chroma.min(0.04) / chroma
    } else {
        0.0
    };
    let mut lightness = 0.26_f32;
    while lightness > 0.0 {
        let ink = quantise_srgb(super::raise::oklab_to_srgb(lightness, a * scale, b * scale));
        if wcag_contrast(ink, fill) >= 4.5 {
            return Color { a: 1.0, ..ink };
        }
        lightness -= 0.02;
    }
    Color::BLACK
}

/// APCA (W3 0.0.98G) lightness contrast of `text` on `background`, as Lc:
/// positive for dark text on a light background, negative for light text on
/// a dark one.
pub(crate) fn apca_lc(text: Color, background: Color) -> f32 {
    fn screen_luminance(c: Color) -> f32 {
        let y = 0.212_672_9 * c.r.clamp(0.0, 1.0).powf(2.4)
            + 0.715_152_2 * c.g.clamp(0.0, 1.0).powf(2.4)
            + 0.072_175 * c.b.clamp(0.0, 1.0).powf(2.4);
        // Soft clamp near black.
        if y < 0.022 {
            y + (0.022 - y).powf(1.414)
        } else {
            y
        }
    }
    let (txt, bg) = (screen_luminance(text), screen_luminance(background));
    if (bg - txt).abs() < 0.000_5 {
        return 0.0;
    }
    let lc = if bg > txt {
        let s = (bg.powf(0.56) - txt.powf(0.57)) * 1.14;
        if s < 0.1 { 0.0 } else { s - 0.027 }
    } else {
        let s = (bg.powf(0.65) - txt.powf(0.62)) * 1.14;
        if s > -0.1 { 0.0 } else { s + 0.027 }
    };
    lc * 100.0
}

pub(crate) fn mix_to_contrast(ink: Color, bg: Color, target: f32) -> Color {
    // The counter distinguishes cache hits from recomputation.
    #[cfg(test)]
    mix_to_contrast_calls::bump();
    memo::color(&memo::MIX_TO_CONTRAST, memo::key([ink, bg], target), || {
        mix_to_contrast_uncached(ink, bg, target)
    })
}

fn mix_to_contrast_uncached(ink: Color, bg: Color, target: f32) -> Color {
    if wcag_contrast(ink, bg) <= target {
        return Color { a: 1.0, ..ink };
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if wcag_contrast(mix_srgb(ink, bg, mid), bg) >= target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Color {
        a: 1.0,
        ..mix_srgb(ink, bg, lo)
    }
}

pub(crate) fn lift_brand_to_text_contrast(brand: Color, surface: Color, floor: f32) -> Color {
    memo::color(
        &memo::LIFT_BRAND,
        memo::key([brand, surface], floor),
        || lift_brand_to_text_contrast_uncached(brand, surface, floor),
    )
}

fn lift_brand_to_text_contrast_uncached(brand: Color, surface: Color, floor: f32) -> Color {
    let brand_opaque = Color { a: 1.0, ..brand };
    if wcag_contrast(brand_opaque, surface) >= floor {
        return brand_opaque;
    }
    let ink = extreme_ink_on(surface);
    if wcag_contrast(ink, surface) < floor {
        // Even pure black or white cannot reach the floor — give the caller
        // the best foreground available on this surface.
        return ink;
    }
    // A 1/256 step is finer than an 8-bit channel increment.
    const N: usize = 256;
    for i in 0..=N {
        let f = i as f32 / N as f32;
        let mix = quantise_srgb(mix_srgb(brand_opaque, ink, f));
        if wcag_contrast(mix, surface) >= floor {
            return Color { a: 1.0, ..mix };
        }
    }
    // Avoid panic on floating-point pathology; endpoint ink remains the best fallback.
    ink
}

pub(crate) fn quantise_srgb(c: Color) -> Color {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round()) / 255.0;
    Color {
        r: q(c.r),
        g: q(c.g),
        b: q(c.b),
        a: c.a,
    }
}

/// Small per-thread memo tables for the contrast solvers.
///
/// The solvers are pure functions of a few colours that change only with the
/// theme, yet style closures call them for every styled widget every frame.
/// Each table is direct-mapped: a key hashes to one slot and a different key
/// simply replaces it, so a table never grows and a miss is one recompute.
pub(crate) mod memo {
    use iced::Color;
    use std::cell::RefCell;
    use std::thread::LocalKey;

    const SLOTS: usize = 128;

    /// Two colours and a target, as exact bits.
    pub(crate) type Key = [u32; 9];

    pub(crate) type Table<V> = RefCell<[Option<(Key, V)>; SLOTS]>;

    thread_local! {
        pub(crate) static MIX_TO_CONTRAST: Table<Color> = const { RefCell::new([None; SLOTS]) };
        pub(crate) static LIFT_BRAND: Table<Color> = const { RefCell::new([None; SLOTS]) };
        pub(crate) static DEEPEN: Table<Color> = const { RefCell::new([None; SLOTS]) };
        pub(crate) static LEGIBLE_INK: Table<Color> = const { RefCell::new([None; SLOTS]) };
        pub(crate) static PREFERS_WHITE: Table<bool> = const { RefCell::new([None; SLOTS]) };
        pub(crate) static RAISE: Table<super::super::raise::RaisedInk> =
            const { RefCell::new([None; SLOTS]) };
    }

    pub(crate) fn key(colors: [Color; 2], target: f32) -> Key {
        let [a, b] = colors;
        [
            a.r.to_bits(),
            a.g.to_bits(),
            a.b.to_bits(),
            a.a.to_bits(),
            b.r.to_bits(),
            b.g.to_bits(),
            b.b.to_bits(),
            b.a.to_bits(),
            target.to_bits(),
        ]
    }

    fn slot(key: &Key) -> usize {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for word in key {
            h = (h ^ u64::from(*word)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        (h >> 32) as usize % SLOTS
    }

    pub(crate) fn lookup<V: Copy + 'static>(
        table: &'static LocalKey<Table<V>>,
        key: Key,
        compute: impl FnOnce() -> V,
    ) -> V {
        let index = slot(&key);
        if let Some(hit) =
            table.with(|t| t.borrow()[index].and_then(|(k, v)| (k == key).then_some(v)))
        {
            return hit;
        }
        let value = compute();
        table.with(|t| t.borrow_mut()[index] = Some((key, value)));
        value
    }

    pub(crate) fn color(
        table: &'static LocalKey<Table<Color>>,
        key: Key,
        compute: impl FnOnce() -> Color,
    ) -> Color {
        lookup(table, key, compute)
    }
}

#[cfg(test)]
pub(super) mod mix_to_contrast_calls {
    use std::cell::Cell;
    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }
    pub(crate) fn bump() {
        COUNT.with(|c| c.set(c.get() + 1));
    }
}

#[cfg(test)]
pub(super) mod tinted_dark_ink_calls {
    use std::cell::Cell;
    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }
    pub(crate) fn bump() {
        COUNT.with(|c| c.set(c.get() + 1));
    }
    pub(crate) fn count() -> u64 {
        COUNT.with(Cell::get)
    }
}

#[cfg(test)]
mod ink_memo_tests {
    use super::*;

    /// Ink on a fill is worked out once per fill and thread: styles ask for
    /// it on every frame. A cached answer is the computed one.
    ///
    /// Sabotage: compute `legible_ink_on` or `prefers_white` without the
    /// table -- the second asks search the dark ink again.
    #[test]
    fn ink_on_a_fill_is_searched_once() {
        let fills = [
            hex(0x35, 0x84, 0xe4),
            hex(0xf6, 0xd3, 0x2d),
            hex(0x77, 0x77, 0x77),
        ];
        let first: Vec<(Color, bool)> = fills
            .iter()
            .map(|fill| (legible_ink_on(*fill), prefers_white(*fill)))
            .collect();
        let searched = tinted_dark_ink_calls::count();
        for (fill, (ink, white)) in fills.iter().zip(&first) {
            assert_eq!(legible_ink_on(*fill), *ink);
            assert_eq!(prefers_white(*fill), *white);
            assert_eq!(*ink, legible_ink_on_uncached(*fill));
        }
        assert_eq!(
            tinted_dark_ink_calls::count(),
            searched + 3,
            "only the uncached reference searched"
        );
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    /// The table is the formula: every 8-bit channel reads back bit for bit.
    ///
    /// Sabotage: build the table with `powf(2.2)` and every entry past the
    /// linear toe differs.
    #[test]
    fn the_linear_table_matches_the_formula_bit_for_bit() {
        for i in 0..=255u32 {
            let v = i as f32 / 255.0;
            assert_eq!(
                linear_channel(v).to_bits(),
                srgb_channel_to_linear(v).to_bits(),
                "channel {i}"
            );
        }
        // Off-grid values still take the formula.
        assert_eq!(linear_channel(0.5001), srgb_channel_to_linear(0.5001));
    }
}
