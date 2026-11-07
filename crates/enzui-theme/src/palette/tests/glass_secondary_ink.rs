/// Secondary ink on a glass palette's translucent fills is judged on what the
/// fill shows (the glass's dark ground under light ink), not on the fill's
/// colour taken as opaque: a faint white wash is not a white backdrop.
#[test]
fn secondary_ink_on_translucent_glass_fills_stays_readable() {
    let glass = Palette::dark().on_glass_ink(1.0);
    for fill in [glass.surface, glass.surface_alt, glass.surface_hover] {
        assert!(fill.a < 1.0, "glass fills are translucent: {fill:?}");
        let solid = glass.solid_backdrop(fill);
        let muted = over(glass.text_muted_on(fill), solid);
        assert!(
            wcag_contrast(muted, solid) >= 4.5,
            "muted {muted:?} on {solid:?}"
        );
        // Light ink on dark glass: secondary ink stays light, never greyed down.
        assert!(color_math::wcag_luminance(muted) > color_math::wcag_luminance(solid));
        let subtle = over(glass.text_subtle_on(fill), solid);
        assert!(wcag_contrast(subtle, solid) >= 4.5);
    }
    // Opaque backdrops behave as before.
    let dark = Palette::dark();
    assert_eq!(
        dark.text_muted_on(dark.surface),
        lift_brand_to_text_contrast(dark.text_muted, dark.surface, 4.5)
    );
}

/// A caller-chosen floor lifts accent ink further than the 4.5:1 default, and
/// a floor below 4.5 never darkens ink that already clears it.
#[test]
fn brand_text_on_at_honours_its_floor() {
    let glass = Palette::dark().on_glass_ink(1.0);
    let ground = glass.solid_backdrop(glass.surface);
    let ink = Palette::brand_text_on_at(ground, glass.accent, 6.0);
    assert!(wcag_contrast(ink, ground) >= 6.0, "{ink:?} on {ground:?}");
    assert_eq!(
        Palette::brand_text_on_at(ground, glass.accent, 4.5),
        Palette::brand_text_on(ground, glass.accent)
    );
}
