#[test]
fn high_contrast_palettes_meet_aaa() {
    for base in [Palette::light(), Palette::dark()] {
        let hc = base.clone().high_contrast();
        let page = hc.background;
        assert!(wcag_contrast(hc.text, page) >= 15.0);
        assert_eq!(hc.text_muted, hc.text, "secondary text is drawn as text");
        assert_eq!(hc.text_subtle, hc.text);
        assert_eq!(hc.control_border, hc.text, "edges in the text colour");
        assert_eq!(hc.surface, page, "surfaces are the page");
        for (name, color) in [
            ("accent", hc.accent),
            ("focus ring", hc.focus_ring),
            ("danger", hc.danger),
            ("success", hc.success),
            ("warning", hc.warning),
            ("error", hc.error),
        ] {
            assert!(
                wcag_contrast(color, page) >= 7.0 - 0.05,
                "{name}: {:.2}",
                wcag_contrast(color, page)
            );
        }
        assert!(wcag_contrast(hc.disabled, page) >= 4.5 - 0.05);
        assert!(
            wcag_contrast(hc.surface_selected, page) > 1.3,
            "selection still shows"
        );
        assert_eq!(hc.is_dark(), base.is_dark(), "the mode is kept");
        assert!(hc.is_high_contrast() && !base.is_high_contrast());
    }
}
