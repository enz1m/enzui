#[test]
fn coloured_fills_take_white_or_a_tinted_dark_ink_never_black() {
    use super::color_math::{apca_lc, wcag_contrast};
    for (name, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
        for (role, fill, ink) in [
            ("accent", p.accent, p.on_accent()),
            ("danger", p.danger, p.on_danger()),
            ("success", p.success, p.on_success()),
            ("warning", p.warning, p.on_warning()),
            ("info", p.info, p.on_info()),
        ] {
            let at = format!("{name} {role}");
            assert_eq!(ink.a, 1.0, "{at}");
            assert_ne!(rgb_u8(ink), (0, 0, 0), "{at}: pure black ink");
            if ink == Color::WHITE {
                assert!(wcag_contrast(ink, fill) >= 3.0, "{at}");
            } else {
                assert!(wcag_contrast(ink, fill) >= 4.5, "{at}");
                assert!(
                    apca_lc(ink, fill).abs() > apca_lc(Color::WHITE, fill).abs()
                        || wcag_contrast(Color::WHITE, fill) < 3.0,
                    "{at}: dark ink where white reads better"
                );
            }

            // A labelled control on this fill reads at 4.5:1 or better.
            let pair = p.filled_color(fill);
            assert!(wcag_contrast(pair.text, pair.color) >= 4.5, "{at} filled");
            assert_ne!(rgb_u8(pair.text), (0, 0, 0), "{at} filled: pure black");
        }
    }

    // The case that looked broken: a light-mode accent button kept its blue
    // and got a black label. It now keeps a white label on a deeper blue.
    let light = Palette::light();
    let pair = light.filled_color(light.accent);
    assert_eq!(pair.text, Color::WHITE);
    assert_eq!(light.on_accent(), Color::WHITE);
    // A dark-mode destructive button deepens its red for a white label
    // rather than keeping a salmon fill under dark text.
    let dark = Palette::dark();
    assert_eq!(dark.filled_color(dark.danger).text, Color::WHITE);
}

#[test]
fn a_fill_too_light_for_white_takes_a_dark_ink_in_its_own_hue() {
    use super::color_math::wcag_contrast;
    let yellow = Color::from_rgb8(0xf6, 0xd3, 0x2d);
    let ink = Palette::ink_on(yellow);
    assert_ne!(ink, Color::WHITE);
    assert!(wcag_contrast(ink, yellow) >= 4.5);
    // Warm, not neutral: red above blue, like the fill.
    assert!(ink.r > ink.b, "{ink:?}");
    // And the fill is kept as it is.
    assert_eq!(Palette::light().filled_color(yellow).color, yellow);
}
