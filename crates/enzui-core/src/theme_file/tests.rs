use super::registry::TokenValue;
use super::*;
use enzui_theme::EnzThemeMode;

#[test]
fn theme_file_layers_validated_app_and_desktop_preferences_without_losing_unknown_keys_1() {
    // set_refuses_an_out_of_range_or_unknown_token
    {
        let mut doc = ThemeDocument::empty();
        assert!(!doc.set(
            "spacing.space6",
            EnzThemeMode::Dark,
            TokenValue::Number(-1.0)
        ));
        assert!(!doc.set(
            "spacing.space6",
            EnzThemeMode::Dark,
            TokenValue::Number(f32::NAN)
        ));
        assert!(!doc.set("nope.nope", EnzThemeMode::Dark, TokenValue::Number(1.0)));
        assert!(!doc.set(
            "motion.easing",
            EnzThemeMode::Dark,
            TokenValue::Enum("springy".into())
        ));
        assert_eq!(doc.override_count(), 0);
    }

    // desktop_accent_replaces_the_built_in_accent_but_not_the_theme_file
    {
        use iced_core::Color;
        let violet = Color::from_rgb(0.57, 0.25, 0.85);
        let resolve = |document: &ThemeDocument, accent| {
            resolve_theme(
                document,
                EnzThemeMode::Dark,
                TextScalePolicy::new(),
                &FontPreferencePolicy::default(),
                ReducedMotionPolicy::new(),
                PlatformAppearance {
                    accent,
                    high_contrast: false,
                },
            )
            .palette
        };
        let empty = ThemeDocument::empty();
        let built_in = resolve(&empty, None);
        let desktop = resolve(&empty, Some(violet));
        assert_eq!(desktop.accent, violet);
        assert_eq!(desktop.focus_ring, violet);
        assert_ne!(desktop.accent_hover, built_in.accent_hover);

        let mut themed = ThemeDocument::empty();
        let teal = Color::from_rgb(0.0, 0.5, 0.5);
        assert!(themed.set(
            "palette.accent",
            EnzThemeMode::Dark,
            TokenValue::Color(teal)
        ));
        assert_eq!(resolve(&themed, Some(violet)).accent, teal);

        let mut runtime = ThemeRuntime::new(ProbeOverride::Dark, EnzThemeMode::Dark);
        assert!(runtime.set_platform_accent(Some(violet)));
        assert_eq!(runtime.theme().palette.accent, violet);
        assert!(!runtime.set_platform_accent(Some(violet)));
        assert!(runtime.set_platform_accent(None));
        assert_eq!(runtime.theme().palette.accent, built_in.accent);
    }

    // the_desktop_contrast_preference_selects_the_high_contrast_palette
    {
        use iced_core::Color;
        for mode in [EnzThemeMode::Light, EnzThemeMode::Dark] {
            let probe = match mode {
                EnzThemeMode::Light => ProbeOverride::Light,
                EnzThemeMode::Dark => ProbeOverride::Dark,
            };
            let mut runtime = ThemeRuntime::new(probe, mode);
            let normal = (
                runtime.theme().palette.background,
                runtime.theme().palette.text,
                runtime.theme().palette.border,
            );
            assert!(runtime.set_platform_high_contrast(true));
            let hc = &runtime.theme().palette;
            let (page, ink) = match mode {
                EnzThemeMode::Light => (Color::WHITE, Color::BLACK),
                EnzThemeMode::Dark => (Color::BLACK, Color::WHITE),
            };
            assert_eq!((hc.background, hc.text), (page, ink), "{mode:?}");
            assert!(!runtime.set_platform_high_contrast(true));
            assert!(runtime.set_platform_high_contrast(false));
            let palette = &runtime.theme().palette;
            assert_eq!((palette.background, palette.text, palette.border), normal);
        }
    }

    // a_window_table_names_the_window_buttons
    {
        use enzui_theme::WindowButtons;
        let document = ThemeDocument::parse(
            "version = 1\n[window]\nbuttons = \"minimize_close\"\nradius = 16\n",
        )
        .expect("a valid theme");
        assert_eq!(
            document.resolve(EnzThemeMode::Dark).window_buttons,
            WindowButtons::MinimizeClose
        );
        assert_eq!(document.resolve(EnzThemeMode::Dark).window_radius, 16.0);
        assert_eq!(
            ThemeDocument::empty()
                .resolve(EnzThemeMode::Dark)
                .window_radius,
            enzui_theme::DEFAULT_WINDOW_RADIUS
        );
        assert_eq!(
            ThemeDocument::empty()
                .resolve(EnzThemeMode::Dark)
                .window_buttons,
            WindowButtons::Desktop,
            "a theme without the key follows the desktop"
        );
    }
}

#[test]
fn theme_file_layers_validated_app_and_desktop_preferences_without_losing_unknown_keys_2() {
    // a_grain_table_sets_the_theme_grain
    {
        use enzui_theme::{GrainPlacement, GrainTone};
        use iced_core::Color;
        let document = ThemeDocument::parse(
            "version = 1\n\
         [grain]\n\
         intensity = 0.3\n\
         density = 0.6\n\
         scale = 2\n\
         tone = \"dark\"\n\
         placement = \"over\"\n\
         motion_fps = 24\n\
         dark_ink = \"#102030\"\n",
        )
        .expect("a valid theme");
        let grain = document.resolve(EnzThemeMode::Light).grain;
        assert_eq!(grain.intensity, 0.3);
        assert_eq!(grain.density, 0.6);
        assert_eq!(grain.scale, 2.0);
        assert_eq!(grain.tone, GrainTone::Dark);
        assert_eq!(grain.placement, GrainPlacement::Over);
        assert_eq!(grain.motion, Some(24.0));
        assert_eq!(grain.dark_ink, Color::from_rgb8(0x10, 0x20, 0x30));
        let mut still = ThemeDocument::empty();
        assert!(still.set(
            "grain.motion_fps",
            EnzThemeMode::Dark,
            TokenValue::Number(0.0)
        ));
        assert_eq!(
            still.resolve(EnzThemeMode::Dark).grain.motion,
            None,
            "0 fps keeps it still"
        );
    }

    // writing_back_keeps_what_this_build_does_not_understand
    {
        let text = "# my theme\nversion = 2\nmode = \"twilight\"\n\n[window]\n# rounder\nradius = 16\nbuttons = \"everything\"\nshadow = true\n\n[shape]\nbutton = \"md\"\n";
        let mut document = ThemeDocument::parse(text).expect("a valid theme");
        assert!(document.set(
            "window.radius",
            EnzThemeMode::Dark,
            TokenValue::Number(20.0)
        ));
        assert!(document.clear("shape.button", EnzThemeMode::Dark));
        assert!(document.set(
            "palette.accent",
            EnzThemeMode::Light,
            TokenValue::Color(iced_core::Color::from_rgb8(0x12, 0x34, 0x56))
        ));

        let written = document.to_toml();
        for kept in [
            "# my theme",
            "version = 2",
            "mode = \"twilight\"",
            "# rounder",
            "buttons = \"everything\"",
            "shadow = true",
        ] {
            assert!(written.contains(kept), "lost {kept:?}:\n{written}");
        }
        assert!(
            !written.contains("button = \"md\""),
            "a cleared key stays:\n{written}"
        );
        let again = ThemeDocument::parse(&written).expect("the written file parses");
        assert_eq!(
            again.get("window.radius", EnzThemeMode::Dark),
            Some(&TokenValue::Number(20.0))
        );
        assert_eq!(again.get("shape.button", EnzThemeMode::Dark), None);
        assert_eq!(
            again.get("palette.accent", EnzThemeMode::Light),
            Some(&TokenValue::Color(iced_core::Color::from_rgb8(
                0x12, 0x34, 0x56
            )))
        );
    }

    // a_shape_takes_every_radius_step_the_theme_has
    {
        let names: Vec<&str> = enzui_theme::RadiusStep::ALL
            .iter()
            .map(|step| step.name())
            .collect();
        assert_eq!(registry::RADIUS_STEP_NAMES, names.as_slice());

        let document = ThemeDocument::parse(
        "version = 1\n[window]\nradius = 34\n[shape]\npopover = \"window\"\ndialog = \"window\"\n",
    )
    .expect("a valid theme");
        let theme = document.resolve(EnzThemeMode::Light);
        assert_eq!(theme.shape.popover, enzui_theme::RadiusStep::Window);
        assert_eq!(theme.shape.dialog, enzui_theme::RadiusStep::Window);
        let again = ThemeDocument::parse(&document.to_toml()).expect("the written file parses");
        assert_eq!(
            again.resolve(EnzThemeMode::Light).shape.popover,
            enzui_theme::RadiusStep::Window
        );
    }

    // an_app_theme_overrides_what_it_sets_and_inherits_the_rest
    {
        let system = ThemeDocument::parse(
        "version = 1\nmode = \"dark\"\n[window]\nradius = 14\n[dark.palette]\naccent = \"#112233\"\nbackground = \"#000000\"\n",
    )
    .unwrap();
        let app = ThemeDocument::parse(
            "version = 1\nmode = \"light\"\n[light.palette]\naccent = \"#ff8800\"\n",
        )
        .unwrap();
        let layered = system.layered(&app);
        assert_eq!(layered.mode_preference(), ModePreference::Light);
        let light = layered.resolve(EnzThemeMode::Light);
        assert_eq!(
            light.palette.accent,
            iced_core::Color::from_rgb8(0xff, 0x88, 0x00)
        );
        assert_eq!(light.window_radius, 14.0, "the system's radius stays");
        let dark = layered.resolve(EnzThemeMode::Dark);
        assert_eq!(
            dark.palette.accent,
            iced_core::Color::from_rgb8(0x11, 0x22, 0x33)
        );

        let accent_only =
            ThemeDocument::parse("version = 1\n[dark.palette]\naccent = \"#00ff00\"\n").unwrap();
        let layered = system.layered(&accent_only);
        assert_eq!(
            layered.mode_preference(),
            ModePreference::Dark,
            "no mode in the app file keeps the system's"
        );
        let dark = layered.resolve(EnzThemeMode::Dark);
        assert_eq!(dark.palette.accent, iced_core::Color::from_rgb8(0, 0xff, 0));
        assert_eq!(dark.palette.background, iced_core::Color::BLACK);
    }
}

// A hermetic config root with the given files under `enzui/`.
fn config_root(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("enzui-app-theme-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(THEME_DIR)).unwrap();
    for (file, text) in files {
        std::fs::write(root.join(THEME_DIR).join(file), text).unwrap();
    }
    root
}

#[test]
fn theme_file_layers_validated_app_and_desktop_preferences_without_losing_unknown_keys_3() {
    // the_loader_layers_the_apps_file_from_the_same_folder
    {
        let app = crate::app_data::AppId::new("org.example.Reader").unwrap();
        let root = config_root(
            "layers",
            &[
                ("theme.toml", "version = 1\n[window]\nradius = 9\n"),
                (
                    "org.example.Reader.toml",
                    "version = 1\nmode = \"light\"\n[light.palette]\naccent = \"#ff8800\"\n",
                ),
            ],
        );
        let paths = ThemePaths::new(
            Some(&root),
            Vec::<std::path::PathBuf>::new(),
            None::<std::path::PathBuf>,
        )
        .with_app(app.clone());
        let app_file = root.join(THEME_DIR).join("org.example.Reader.toml");
        assert!(
            paths.watched().contains(&app_file),
            "the app's file is watched"
        );
        assert_eq!(paths.app_user_path(&app), Some(app_file.clone()));
        let load = ThemeLoadOp::new(&paths, 1).execute();
        assert_eq!(load.app_path, Some(app_file.clone()));
        let document = load.outcome.unwrap();
        assert_eq!(document.mode_preference(), ModePreference::Light);
        let theme = document.resolve(EnzThemeMode::Light);
        assert_eq!(
            theme.palette.accent,
            iced_core::Color::from_rgb8(0xff, 0x88, 0x00)
        );
        assert_eq!(theme.window_radius, 9.0);

        // Another app, or none, reads the shared theme alone.
        let other = ThemePaths::new(
            Some(&root),
            Vec::<std::path::PathBuf>::new(),
            None::<std::path::PathBuf>,
        )
        .with_app(crate::app_data::AppId::new("org.example.Other").unwrap());
        let load = ThemeLoadOp::new(&other, 1).execute();
        assert_eq!(load.app_path, None);
        assert_eq!(
            load.outcome.unwrap().mode_preference(),
            ModePreference::System
        );

        // A broken app file leaves the shared theme, with a diagnostic.
        std::fs::write(&app_file, "version = 1\n[light.palette\n").unwrap();
        let document = ThemeLoadOp::new(&paths, 2).execute().outcome.unwrap();
        assert_eq!(document.resolve(EnzThemeMode::Light).window_radius, 9.0);
        assert!(
            document
                .diagnostics()
                .iter()
                .any(|d| d.kind == DiagnosticKind::AppThemeUnusable)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // an_app_theme_names_a_mode_only_when_one_is_chosen
    {
        let shared = ThemeDocument::parse("version = 1\nmode = \"dark\"\n").unwrap();
        let mut app = ThemeDocument::empty();
        assert!(app.set(
            "palette.accent",
            EnzThemeMode::Dark,
            TokenValue::Color(iced_core::Color::from_rgb8(1, 2, 3))
        ));
        let written = ThemeDocument::parse(&app.to_toml()).unwrap();
        assert!(!written.declares_mode());
        assert_eq!(
            shared.layered(&written).mode_preference(),
            ModePreference::Dark
        );

        let mut light = ThemeDocument::parse(&app.to_toml()).unwrap();
        light.set_mode_preference(ModePreference::Light);
        let light = ThemeDocument::parse(&light.to_toml()).unwrap();
        assert_eq!(
            shared.layered(&light).mode_preference(),
            ModePreference::Light
        );

        let mut cleared = light;
        cleared.clear_mode_preference();
        let cleared = ThemeDocument::parse(&cleared.to_toml()).unwrap();
        assert!(!cleared.declares_mode(), "{}", cleared.to_toml());
        assert_eq!(
            shared.layered(&cleared).mode_preference(),
            ModePreference::Dark
        );
    }

    // a_declared_application_reads_its_own_theme_by_default
    {
        fn reader() -> Option<crate::app_data::AppId> {
            crate::app_data::AppId::new("org.example.Declared").ok()
        }
        fn undeclared() -> Option<crate::app_data::AppId> {
            None
        }
        let root = config_root(
            "declared",
            &[
                ("theme.toml", "version = 1\n[window]\nradius = 9\n"),
                (
                    "org.example.Declared.toml",
                    "version = 1\nmode = \"light\"\n",
                ),
            ],
        );
        let base = ThemePaths::new(
            Some(&root),
            Vec::<std::path::PathBuf>::new(),
            None::<std::path::PathBuf>,
        );
        let mode = |paths: &ThemePaths| {
            ThemeLoadOp::new(paths, 1)
                .execute()
                .outcome
                .unwrap()
                .mode_preference()
        };
        assert_eq!(
            mode(&base.clone().with_app_source(reader)),
            ModePreference::Light
        );
        assert_eq!(
            mode(&base.clone().with_app_source(undeclared)),
            ModePreference::System
        );
        assert_eq!(
            mode(&base.clone().with_app_source(reader).without_app()),
            ModePreference::System,
            "opted out"
        );
        assert_eq!(
            mode(&base),
            ModePreference::System,
            "plain paths read the shared theme, as an editor does"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // desktop_fonts_follow_live_metrics_and_per_app_family_overrides
    {
        use enzui_theme::{DesktopFontPreferences, FontFamily, FontRole};
        let _scale = enzui_theme::text_scale::TextScaleScope::enter(
            enzui_theme::text_scale::TextScale::BASELINE,
        );
        let shared = ThemeDocument::parse("version = 1\n[fonts]\nui = \"Shared Font\"\n").unwrap();
        let app = ThemeDocument::parse("version = 1\n[fonts]\nui = \"App Font\"\n").unwrap();
        let mut policy = FontPreferencePolicy::new();
        policy.set_platform(Some(
            DesktopFontPreferences::new()
                .with(FontRole::Ui, "Desktop Sans Light 12 @wght=300 #tnum=1"),
        ));
        let resolve = |document: &ThemeDocument, policy: &FontPreferencePolicy| {
            resolve_theme(
                document,
                EnzThemeMode::Dark,
                TextScalePolicy::new(),
                policy,
                ReducedMotionPolicy::new(),
                PlatformAppearance::default(),
            )
        };
        let first = resolve(&shared, &policy);
        assert_eq!(
            first.font_catalog.family(FontRole::Ui),
            &FontFamily::Named("Desktop Sans".into())
        );
        assert!((first.typography.body_size - 16.0).abs() < 0.01);
        assert_eq!(first.typography.weight_regular, 300);
        assert_eq!(first.typography.weight_semibold, 500);
        let _scope = enzui_theme::text_defaults::TextDefaultsScope::enter(&first);
        assert!((enzui_theme::text_scale::ui_px(13.0) - 16.0).abs() < 0.01);
        assert!((enzui_theme::TypographyScale::ambient().body_size - 16.0).abs() < 0.01);

        assert_eq!(enzui_theme::TypographyScale::ambient().weight_regular, 300);
        let private = resolve(&shared.layered(&app), &policy);
        assert_eq!(
            private.font_catalog.family(FontRole::Ui),
            &FontFamily::Named("App Font".into())
        );
        assert!((private.typography.body_size - 16.0).abs() < 0.01);
        policy.set_platform(Some(
            DesktopFontPreferences::new().with(FontRole::Ui, "Desktop Sans Bold 15"),
        ));
        let mut runtime = ThemeRuntime::new(ProbeOverride::Dark, EnzThemeMode::Dark);
        assert!(runtime.set_font_policy(policy.clone()));
        policy.set_platform(Some(
            DesktopFontPreferences::new().with(FontRole::Ui, "Desktop Sans Bold 18"),
        ));
        assert!(
            runtime.set_font_policy(policy.clone()),
            "a size-only delivery must republish the theme"
        );
        policy.set_platform(Some(
            DesktopFontPreferences::new().with(FontRole::Ui, "Desktop Sans Light 18"),
        ));
        assert!(
            runtime.set_font_policy(policy.clone()),
            "a weight-only delivery must republish the theme"
        );
        policy.set_platform(Some(
            DesktopFontPreferences::new().with(FontRole::Ui, "Desktop Sans Bold 15"),
        ));
        let changed = resolve(&shared, &policy);
        assert!((changed.typography.body_size - 20.0).abs() < 0.01);
        assert_eq!(changed.typography.weight_regular, 700);
        policy.set_override(
            FontRole::Ui,
            Some(FontFamily::Named("Explicit Font".into())),
        );
        assert_eq!(
            resolve(&shared.layered(&app), &policy)
                .font_catalog
                .family(FontRole::Ui),
            &FontFamily::Named("Explicit Font".into())
        );
        let authored = ThemeDocument::parse(
            "version = 1\n[typography]\nbody_size = 17\ntext_scale = 1.5\nweight_regular = 450\n",
        )
        .unwrap();
        let pinned = resolve(&authored, &policy);
        assert!((pinned.typography.body_size - 25.5).abs() < 0.01);
        assert_eq!(pinned.typography.weight_regular, 450);
        let scaled = ThemeDocument::parse("version = 1\n[typography]\ntext_scale = 1.5\n").unwrap();
        assert!((resolve(&scaled, &policy).typography.body_size - 30.0).abs() < 0.01);
        policy.set_override(FontRole::Ui, None);
        policy.set_platform(None);
        assert_eq!(
            resolve(&shared, &policy).font_catalog.family(FontRole::Ui),
            &FontFamily::Named("Shared Font".into())
        );
        assert_eq!(
            enzui_theme::RawFontDescription::new("Monospace 11").validate(),
            Some(FontRole::Monospace.generic_font_family())
        );
    }
}
