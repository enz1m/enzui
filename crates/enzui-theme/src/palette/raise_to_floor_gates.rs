use super::*;

// OkLCh sampling distributes fixed-chroma hues around the circle; an RGB-primary
// grid would cluster samples near the primaries and miss hue drift elsewhere.
fn hue_grid() -> Vec<Color> {
    let mut inks = Vec::new();
    for step in 0..24 {
        let h = step as f32 / 24.0 * std::f32::consts::TAU;
        for &(l, c) in &[(0.55_f32, 0.14_f32), (0.70, 0.10), (0.45, 0.09)] {
            let col = oklab_to_srgb(l, c * h.cos(), c * h.sin());
            if col.r >= 0.0
                && col.r <= 1.0
                && col.g >= 0.0
                && col.g <= 1.0
                && col.b >= 0.0
                && col.b <= 1.0
            {
                inks.push(quantise_srgb(col));
            }
        }
    }
    inks
}

#[test]
fn oklab_roundtrip_is_accurate() {
    for c in hue_grid() {
        let (l, a, b) = srgb_to_oklab(c);
        let back = quantise_srgb(oklab_to_srgb(l, a, b));
        for (x, y) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
            assert!(
                (x - y).abs() <= 2.0 / 255.0,
                "OkLab round-trip drifted: {c:?} -> {back:?}",
            );
        }
    }
}
