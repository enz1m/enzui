use enzui_core::calendar_nav::MonthController;
use enzui_core::civil_date::{Date, DateBounds, Weekday};
use enzui_core::collect::{CollectedNode, collect_frame};
use enzui_core::localization::{CatalogBuilder, Locale, Localizer};
use enzui_core::placement::Direction;
use enzui_widgets::app_shell::ShellTokens;
use enzui_widgets::button::enz_button;
use enzui_widgets::calendar::enz_calendar;
use enzui_widgets::search_field::enz_search_field;
use enzui_widgets::window_controls::enz_window_controls;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Id;
use iced::{Element, Size, Theme};
use iced_test::runtime::user_interface::{Cache, UserInterface};

fn localizer() -> Localizer {
    let mut builder = CatalogBuilder::new();
    builder
        .add_resource("en-US", "app-save = Save document")
        .unwrap();
    builder.add_resource("fr", "app-save = Enregistrer le document\nenzui-search-field-search = Rechercher…\nenzui-window-controls-close = Fermer\nenzui-window-controls-minimize = Réduire\nenzui-window-controls-maximize = Agrandir\nenzui-search-field-clear-search = Effacer la recherche\nenzui-search-field-search-preferences = Préférences de recherche\nenzui-search-field-results = Résultats de recherche\nenzui-date-selected = sélectionné\nenzui-date-today = aujourd’hui").unwrap();
    builder.add_resource("ar", "app-save = حفظ المستند\nenzui-window-controls-close = إغلاق\nenzui-search-field-clear-search = مسح البحث").unwrap();
    Localizer::new(builder.build(), vec![Locale::default()], Locale::default())
}

fn view(localizer: &Localizer) -> Element<'static, ()> {
    iced::widget::column![
        enz_button(localizer.text("app-save"), ()).id(Id::new("save-document")),
        enz_search_field("needle", |_| ())
            .id(Id::new("search"))
            .placeholder("An app-supplied placeholder")
            .responsive(false)
            .on_clear(())
            .filters(iced::widget::text("Filter scope"))
            .on_toggle_filters(()),
        enz_window_controls(())
            .id_prefix("localized-controls")
            .show_minimize(true, ())
            .show_maximize(true, (), false),
    ]
    .spacing(12)
    .into()
}

fn named<'a>(nodes: &'a [CollectedNode], label: &str) -> &'a CollectedNode {
    nodes
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some(label))
        .unwrap_or_else(|| {
            panic!(
                "missing {label:?}: {:?}",
                nodes
                    .iter()
                    .filter_map(|node| node.semantics.label.as_deref())
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn language_changes_update_retained_widgets_without_losing_identity_or_layout_1() {
    // localization_defaults_switch_in_a_retained_widget_tree_without_changing_control_ids
    {
        let localizer = localizer();
        localizer.scope(|| {
            let mut renderer = iced_test::futures::futures::executor::block_on(
                <iced_test::renderer::Renderer as Headless>::new(
                    iced::advanced::renderer::Settings::default(),
                    Some("software"),
                ),
            )
            .unwrap();
            let mut cache = Cache::default();
            let mut collect = || {
                let mut ui = UserInterface::build(
                    view(&localizer),
                    Size::new(720.0, 300.0),
                    std::mem::take(&mut cache),
                    &mut renderer,
                );
                let mut collector = enzui_core::collect::SemanticsCollector::new();
                ui.operate(&renderer, &mut collector);
                cache = ui.into_cache();
                collector.collected().to_vec()
            };
            let english = collect();
            let close_id = named(&english, "Close").id.clone().unwrap();
            let clear_id = named(&english, "Clear search").id.clone().unwrap();
            assert!(named(&english, "Save document").bounds.width > 0.0);
            assert!(localizer.select(vec!["fr".parse().unwrap()]));
            let french = collect();
            assert_eq!(named(&french, "Fermer").id.as_ref(), Some(&close_id));
            assert_eq!(
                named(&french, "Effacer la recherche").id.as_ref(),
                Some(&clear_id)
            );
            named(&french, "Réduire");
            named(&french, "Agrandir");
            named(&french, "Préférences de recherche");
            named(&french, "Enregistrer le document");
            named(&french, "An app-supplied placeholder");
            assert!(
                french
                    .iter()
                    .all(|node| node.semantics.label.as_deref() != Some("Clear search"))
            );
        });
    }

    // localization_calendar_uses_locale_names_and_translates_day_announcements
    {
        let localizer = localizer();
        localizer.select(vec!["fr".parse().unwrap()]);
        let date = Date::new(2026, 10, 19).unwrap();
        let controller = MonthController::new(date, Weekday::Monday, DateBounds::default());
        localizer.scope(|| {
            let element: Element<'_, (), Theme, iced_test::renderer::Renderer> =
                enz_calendar(&controller, |_| ())
                    .id(Id::new("localized-calendar"))
                    .selected(Some(date))
                    .today(Some(date))
                    .into();
            let frame = collect_frame(element, Size::new(360.0, 420.0));
            let nodes = frame.collected();
            named(nodes, "octobre 2026");
            named(nodes, "lundi");
            let day = named(nodes, "lundi 19 octobre 2026");
            let description = day.semantics.description.as_deref().unwrap();
            assert!(description.contains("sélectionné"), "{description}");
            assert!(description.contains("aujourd’hui"), "{description}");
            assert!(!description.contains("today"), "{description}");
        });
    }

    // localization_explicit_widget_labels_override_catalog_defaults
    {
        let localizer = localizer();
        localizer.select(vec!["fr".parse().unwrap()]);
        localizer.scope(|| {
            let element: Element<'_, (), Theme, iced_test::renderer::Renderer> =
                enzui_widgets::code_block::enz_code_block("let answer = 42;")
                    .copy_label("Copy this particular snippet")
                    .id_prefix("explicit-copy-label")
                    .into();
            let frame = collect_frame(element, Size::new(480.0, 160.0));
            named(frame.collected(), "Copy this particular snippet");
        });
    }

    // localization_arabic_switch_reaches_default_shell_direction_and_window_chrome
    {
        let localizer = localizer();
        localizer.scope(|| {
            assert_eq!(
                ShellTokens::default().effective_direction(),
                Direction::LeftToRight
            );
            localizer.select(vec!["ar".parse().unwrap()]);
            assert_eq!(
                ShellTokens::default().effective_direction(),
                Direction::RightToLeft
            );
            assert_eq!(
                ShellTokens::default()
                    .direction(Direction::LeftToRight)
                    .effective_direction(),
                Direction::LeftToRight
            );
            let frame = collect_frame(view(&localizer), Size::new(720.0, 300.0));
            named(frame.collected(), "إغلاق");
            named(frame.collected(), "حفظ المستند");
            named(frame.collected(), "مسح البحث");
        });
    }
}

#[test]
fn language_changes_update_retained_widgets_without_losing_identity_or_layout_2() {
    // localization_view_keeps_independent_locales_through_deferred_layout
    {
        let french = localizer();
        french.select(vec!["fr".parse().unwrap()]);
        let arabic = localizer();
        arabic.select(vec!["ar".parse().unwrap()]);
        let deferred = || -> Element<'static, ()> {
            iced::widget::responsive(|_| {
                enz_button(enzui_core::localization::text("app-save"), ()).into_element()
            })
            .into()
        };
        let element: Element<'static, ()> =
            iced::widget::column![french.view(deferred), arabic.view(deferred),].into();
        let frame = collect_frame(element, Size::new(720.0, 240.0));
        named(frame.collected(), "Enregistrer le document");
        named(frame.collected(), "حفظ المستند");
        assert!(enzui_core::localization::scoped().is_none());
    }

    // localization_view_carries_the_locale_into_late_search_overlay_semantics
    {
        let localizer = localizer();
        localizer.select(vec!["fr".parse().unwrap()]);
        let element = localizer.view(|| {
            enz_search_field("needle", |_| ())
                .id(Id::new("localized-overlay-search"))
                .responsive(false)
                .results(iced::widget::text("A real result"))
                .results_open(true)
                .into()
        });
        let frame = collect_frame(element, Size::new(720.0, 300.0));
        named(frame.collected(), "Résultats de recherche");
        named(frame.collected(), "A real result");
        assert!(enzui_core::localization::scoped().is_none());
    }

    // localization_default_search_placeholder_does_not_change_descendant_identity
    {
        let localizer = localizer();
        let collect = || {
            let element = localizer.view(|| {
                enz_search_field("needle", |_| ())
                    .id(Id::new("default-placeholder-search"))
                    .on_clear(())
                    .responsive(false)
                    .into()
            });
            collect_frame(element, Size::new(720.0, 160.0))
        };
        let english = collect();
        let clear_id = named(english.collected(), "Clear search")
            .id
            .clone()
            .unwrap();
        localizer.select(vec!["fr".parse().unwrap()]);
        let french = collect();
        named(french.collected(), "Rechercher…");
        assert_eq!(
            named(french.collected(), "Effacer la recherche")
                .id
                .as_ref(),
            Some(&clear_id)
        );
    }

    // localization_file_sort_control_identity_survives_translated_column_names
    {
        use enzui_core::file_browser::FileBrowserModel;
        use enzui_widgets::file_browser::enz_file_browser;
        let mut builder = CatalogBuilder::new();
        builder.add_resource("fr", "enzui-file-browser-name = Nom\nenzui-file-browser-size = Taille\nenzui-file-browser-modified = Modification\nenzui-file-browser-type = Type\nenzui-file-browser-directory-row = { $name } (dossier)").unwrap();
        let localizer = Localizer::new(builder.build(), vec![Locale::default()], Locale::default());
        let mut model = FileBrowserModel::new();
        model.set_entries(vec![
            enzui_core::file_browser::FileEntry::file("/report.txt", "report.txt", 2048),
            enzui_core::file_browser::FileEntry::directory("/docs", "docs"),
        ]);
        let collect = || {
            let element: Element<'_, ()> = localizer.view(|| {
                enz_file_browser(&model)
                    .id_prefix("localized-files")
                    .on_sort(|_| ())
                    .into()
            });
            collect_frame(element, Size::new(960.0, 400.0))
        };
        let english = collect();
        let name_id = named(english.collected(), "Name").id.clone().unwrap();
        localizer.select(vec!["fr".parse().unwrap()]);
        let french = collect();
        assert_eq!(named(french.collected(), "Nom").id.as_ref(), Some(&name_id));
        named(french.collected(), "\u{2068}docs\u{2069} (dossier)");
    }
}

#[test]
fn language_changes_update_retained_widgets_without_losing_identity_or_layout_3() {
    // localization_mixed_direction_clock_text_keeps_every_grapheme_inside_its_paragraph
    {
        use iced::advanced::text::{
            Alignment, Ellipsis, LineHeight, Paragraph as _, Shaping, Text, Wrapping,
        };
        for value in ["1:05 م", "1:05 PM", "١:٠٥ م", "م 1:05", "موعد 1:05 اليوم"] {
            let paragraph = iced::advanced::graphics::text::Paragraph::with_text(Text {
                content: value,
                bounds: Size::new(f32::INFINITY, 20.0),
                size: 13.0.into(),
                font: iced::Font::new("DejaVu Sans"),
                align_x: Alignment::Default,
                align_y: iced::alignment::Vertical::Center,
                line_height: LineHeight::default(),
                shaping: Shaping::Advanced,
                wrapping: Wrapping::None,
                ellipsis: Ellipsis::None,
                hint_factor: None,
            });
            let glyphs = paragraph.grapheme_bounds().unwrap();
            assert!(!glyphs.is_empty(), "{value}");
            for glyph in glyphs {
                assert!(glyph.bounds.x >= -0.01, "{value}: {glyph:?}");
                assert!(
                    glyph.bounds.x + glyph.bounds.width <= paragraph.min_width() + 0.01,
                    "{value}: {glyph:?}; {:?}",
                    paragraph.min_bounds()
                );
            }
        }
    }

    // localization_calendar_paints_compact_arabic_weekdays_and_keeps_fitting_english_abbreviations
    {
        for (language, narrow) in [("ar-EG", true), ("en-US", false)] {
            let localizer = Localizer::new(
                Default::default(),
                vec![language.parse().unwrap()],
                Locale::default(),
            );
            let labels = ExpectedWeekdayLabels {
                labels: enzui_core::localization::LocaleCalendarLabels::new(&localizer).unwrap(),
                narrow,
            };
            assert!(
                weekday_pixels(&localizer, None) == weekday_pixels(&localizer, Some(&labels)),
                "weekday header pixels differ for {language}"
            );
        }
    }

    // localization_owned_fields_return_views_and_keep_ids_across_language_changes
    {
        use enzui_core::semantics::Role;
        let mut builder = CatalogBuilder::new();
        builder
            .add_resource("en-US", "field-label = Name\nfield-caption = Required")
            .unwrap();
        builder
            .add_resource("fr", "field-label = Nom\nfield-caption = Obligatoire")
            .unwrap();
        let localizer = Localizer::new(builder.build(), vec![Locale::default()], Locale::default());
        let value = String::from("private value");
        let content = iced::widget::text_editor::Content::with_text(&value);
        for (locale, label, caption) in
            [("en-US", "Name", "Required"), ("fr", "Nom", "Obligatoire")]
        {
            localizer.select(vec![locale.parse().unwrap()]);
            for errors in [false, true] {
                for placeholders_only in [false, true] {
                    let frame = collect_frame(
                        owned_field_view(&localizer, &value, &content, errors, placeholders_only),
                        Size::new(720.0, 800.0),
                    );
                    for id in [
                        "owned-text",
                        "owned-secure",
                        "owned-area",
                        "owned-autocomplete",
                    ] {
                        let node = frame
                            .collected()
                            .iter()
                            .find(|node| node.id.as_ref() == Some(&Id::new(id)))
                            .unwrap_or_else(|| panic!("missing field {id}"));
                        assert_eq!(node.semantics.label.as_deref(), Some(label), "{id}");
                        assert_eq!(node.semantics.description.as_deref(), Some(caption), "{id}");
                        assert_eq!(
                            node.semantics.invalid,
                            errors || id == "owned-autocomplete",
                            "{id}"
                        );
                        if id == "owned-secure" {
                            assert_eq!(node.semantics.role, Role::PasswordInput);
                            assert!(node.semantics.value.is_none());
                        } else {
                            assert_eq!(
                                node.semantics.value.as_deref(),
                                Some(value.as_str()),
                                "{id}"
                            );
                        }
                    }
                }
            }
        }
    }

    // localization_owned_control_labels_cross_the_returned_view_boundary
    {
        let mut builder = CatalogBuilder::new();
        builder
            .add_resource("fr", "field-label = Nom\nfield-caption = Obligatoire")
            .unwrap();
        let localizer = Localizer::new(
            builder.build(),
            vec!["fr".parse().unwrap()],
            Locale::default(),
        );
        let value = String::from("value");
        let content = iced::widget::text_editor::Content::new();
        let today = Date::new(2026, 10, 19).unwrap();
        let date = enzui_widgets::date_picker::DatePickerState::new(
            today,
            Weekday::Monday,
            DateBounds::default(),
        );
        let range = enzui_widgets::date_range_picker::DateRangePickerState::new(
            today,
            Weekday::Monday,
            DateBounds::default(),
        );
        let frame = collect_frame(
            owned_control_view(&localizer, &value, &content, &date, &range),
            Size::new(720.0, 1200.0),
        );
        for id in [
            "owned-check",
            "owned-toggle",
            "owned-combo",
            "owned-radio",
            "owned-composer",
            "owned-date",
            "owned-range",
            "owned-pref-entry",
            "owned-pref-password",
        ] {
            let node = frame
                .collected()
                .iter()
                .find(|node| node.id.as_ref() == Some(&Id::new(id)))
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(node.semantics.label.as_deref(), Some("Nom"), "{id}");
        }
        for id in ["owned-check", "owned-toggle", "owned-combo"] {
            let node = frame
                .collected()
                .iter()
                .find(|node| node.id.as_ref() == Some(&Id::new(id)))
                .unwrap();
            assert_eq!(
                node.semantics.description.as_deref(),
                Some("Obligatoire"),
                "{id}"
            );
        }
    }
}

struct ExpectedWeekdayLabels {
    labels: enzui_core::localization::LocaleCalendarLabels,
    narrow: bool,
}

impl enzui_core::calendar_semantics::CalendarLabels for ExpectedWeekdayLabels {
    fn month_name(&self, month: u32) -> String {
        self.labels.month_name(month)
    }
    fn weekday_name(&self, weekday: Weekday) -> String {
        self.labels.weekday_name(weekday)
    }
    fn weekday_short(&self, weekday: Weekday) -> String {
        if self.narrow {
            self.labels.weekday_narrow(weekday)
        } else {
            self.labels.weekday_short(weekday)
        }
    }
}

fn weekday_pixels(
    localizer: &Localizer,
    labels: Option<&dyn enzui_core::calendar_semantics::CalendarLabels>,
) -> Vec<Vec<u8>> {
    let date = Date::new(2026, 10, 19).unwrap();
    let controller = MonthController::new(date, Weekday::Monday, DateBounds::default());
    let mut tokens = enzui_widgets::calendar::CalendarTokens::light();
    tokens.header = iced::Color::from_rgb(1.0, 0.0, 0.0);
    let element = localizer.view(|| {
        let calendar = enz_calendar(&controller, |_| ())
            .show_header(false)
            .tokens(tokens);
        match labels {
            Some(labels) => calendar.labels(labels).into(),
            None => calendar.into(),
        }
    });
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("software"),
        ),
    )
    .unwrap();
    let mut ui = UserInterface::build(
        element,
        Size::new(360.0, 300.0),
        Cache::default(),
        &mut renderer,
    );
    let mut collector = enzui_core::collect::SemanticsCollector::new();
    ui.operate(&renderer, &mut collector);
    let mut headers: Vec<_> = collector
        .collected()
        .iter()
        .filter(|node| node.semantics.role == enzui_core::semantics::Role::ColumnHeader)
        .map(|node| node.bounds)
        .collect();
    assert_eq!(headers.len(), 7);
    headers.sort_by(|left, right| left.x.total_cmp(&right.x));
    ui.draw(
        &mut renderer,
        &enzui_theme::EnzTheme::light().iced_theme(),
        &iced::advanced::renderer::Style {
            text_color: iced::Color::BLACK,
        },
        iced::advanced::mouse::Cursor::Unavailable,
    );
    let image = renderer.screenshot(Size::new(360, 300), 1.0, iced::Color::WHITE);
    headers
        .into_iter()
        .map(|bounds| {
            assert!(bounds.width >= 34.0 && bounds.height > 0.0);
            let mut pixels = Vec::new();
            for y in bounds.y.ceil() as usize..(bounds.y + bounds.height).floor() as usize {
                let start = 4 * (y * 360 + bounds.x.ceil() as usize);
                let end = 4 * (y * 360 + (bounds.x + bounds.width).floor() as usize);
                pixels.extend_from_slice(&image[start..end]);
            }
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| i16::from(pixel[0]) > i16::from(pixel[1]) + 30
                        && i16::from(pixel[0]) > i16::from(pixel[2]) + 30),
                "weekday heading must paint its configured ink"
            );
            pixels
        })
        .collect()
}

fn owned_field_view<'a>(
    localizer: &Localizer,
    value: &'a str,
    content: &'a iced::widget::text_editor::Content,
    errors: bool,
    placeholders_only: bool,
) -> Element<'a, ()> {
    use enzui_widgets::autocomplete_field::enz_autocomplete_field;
    use enzui_widgets::secure_field::enz_secure_field;
    use enzui_widgets::text_area::enz_text_area;
    use enzui_widgets::text_field::enz_text_field;
    let caption = || localizer.text("field-caption");
    let error = || if errors { caption() } else { String::new() };
    let label = || {
        if placeholders_only {
            String::new()
        } else {
            localizer.text("field-label")
        }
    };
    localizer.view(|| {
        iced::widget::column![
            enz_text_field(label(), localizer.text("field-label"), value, |_| ())
                .id(Id::new("owned-text"))
                .error(error())
                .description(caption()),
            enz_secure_field(value, |_| (), false, ())
                .id(Id::new("owned-secure"))
                .label(label())
                .placeholder(localizer.text("field-label"))
                .error(error())
                .description(caption()),
            enz_text_area(content, |_| ())
                .id(Id::new("owned-area"))
                .label(label())
                .placeholder(localizer.text("field-label"))
                .error(error())
                .description(caption()),
            enz_autocomplete_field(
                label(),
                value,
                Vec::<enzui_widgets::autocomplete_field::AutocompleteOption<()>>::new(),
                |_| (),
                |()| ()
            )
            .id(Id::new("owned-autocomplete"))
            .placeholder(localizer.text("field-label"))
            .error(caption())
            .responsive(false),
        ]
        .into()
    })
}

fn owned_control_view<'a>(
    localizer: &Localizer,
    value: &'a str,
    content: &'a iced::widget::text_editor::Content,
    date: &'a enzui_widgets::date_picker::DatePickerState,
    range: &'a enzui_widgets::date_range_picker::DateRangePickerState,
) -> Element<'a, ()> {
    localizer.view(|| {
        iced::widget::column![
            enzui_widgets::checkbox::enz_checkbox(false, localizer.text("field-label"), |_| ())
                .description(localizer.text("field-caption"))
                .id(Id::new("owned-check")),
            enzui_widgets::toggle::enz_toggle(false, localizer.text("field-label"), |_| ())
                .description(localizer.text("field-caption"))
                .id(Id::new("owned-toggle")),
            enzui_widgets::combo_box::enz_combo_box(
                localizer.text("field-label"),
                vec![],
                None,
                |_| ()
            )
            .error(localizer.text("field-caption"))
            .id(Id::new("owned-combo")),
            enzui_widgets::radio_group::enz_radio_group(
                localizer.text("field-label"),
                &["one", "two"],
                None,
                |_| ()
            )
            .id(Id::new("owned-radio")),
            enzui_widgets::composer::enz_composer(content, |_| ())
                .placeholder(localizer.text("field-label"))
                .id(Id::new("owned-composer")),
            enzui_widgets::date_picker::enz_date_picker(None, date, |_| ())
                .label(localizer.text("field-label"))
                .id(Id::new("owned-date")),
            enzui_widgets::date_range_picker::enz_date_range_picker(None, range, |_| ())
                .label(localizer.text("field-label"))
                .placeholder(localizer.text("field-caption"))
                .id(Id::new("owned-range")),
            enzui_widgets::preferences_row::enz_preferences_row(localizer.text("field-label"))
                .entry(value, localizer.text("field-caption"), |_| ())
                .id(Id::new("owned-pref-entry")),
            enzui_widgets::preferences_row::enz_preferences_row(localizer.text("field-label"))
                .password(value, localizer.text("field-caption"), false, |_| (), ())
                .id(Id::new("owned-pref-password")),
        ]
        .into()
    })
}

/// Framework strings that used to bypass the catalog: the confirm button,
/// file sizes, chart axis suffixes and the plain-text language name all
/// follow a French catalog and French number formatting.
#[test]
fn localization_sizes_ticks_and_dialog_defaults_follow_the_catalog() {
    let mut builder = CatalogBuilder::new();
    builder
        .add_resource(
            "fr",
            "enzui-confirm-dialog-ok = Valider\n\
             enzui-file-size-kilobytes = { $value } ko\n\
             enzui-file-size-bytes = { $value } o\n\
             enzui-chart-millions = { $value } M\n\
             enzui-syntax-plain-text = Texte brut\n\
             enzui-syntax-auto = Automatique",
        )
        .unwrap();
    let french: Locale = "fr-FR".parse().unwrap();
    let localizer = Localizer::new(builder.build(), vec![french.clone()], french);
    localizer.scope(|| {
        let strip = |s: String| s.replace(['\u{2068}', '\u{2069}'], "");
        assert_eq!(
            strip(enzui_widgets::file_browser::format_bytes(1_500)),
            "1,50 ko"
        );
        assert_eq!(strip(enzui_widgets::file_browser::format_bytes(12)), "12 o");
        assert_eq!(
            strip(enzui_widgets::chart_geometry::format_tick(
                2_500_000.0,
                500_000.0
            )),
            "2,5 M"
        );
        assert_eq!(
            enzui_widgets::syntax::Language::Plain.display_name(),
            "Texte brut"
        );
        assert_eq!(
            enzui_widgets::syntax::Language::override_display_name("Auto"),
            "Automatique"
        );
        let state = enzui_core::confirm::ConfirmState::new(None);
        let dialog = enzui_widgets::confirm_dialog::enz_confirm_dialog("t", "m", &state, |_| ());
        let frame = collect_frame(dialog.into_element(), Size::new(600.0, 400.0));
        named(frame.collected(), "Valider");
    });
}
