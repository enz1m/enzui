//! Right-to-left mirroring of widgets that lay out along a row.
//!
//! Each case builds the widget twice through its `.direction(..)` override
//! and compares measured positions: a published node's bounds, painted ink,
//! or the value a pointer press maps to.

use crate::corner_probe::{ink_bbox, render};
use enzui_core::collect::{CollectedNode, collect_frame};
use enzui_core::placement::Direction::{self, LeftToRight, RightToLeft};
use enzui_core::semantics::{Role, Semantics};
use enzui_widgets::focus_button::focus_button;
use iced::{Element, Point, Size};
use iced_test::Simulator;
use iced_test::simulator::click;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Noop,
    Value(u32),
    Toggle(usize),
}

const WIDTH: f32 = 600.0;

fn nodes(element: Element<'_, Msg>) -> Vec<CollectedNode> {
    collect_frame(element, Size::new(WIDTH, 300.0))
        .collected()
        .to_vec()
}

/// The horizontal centre of the node labelled `label`.
fn centre_x(element: Element<'_, Msg>, label: &str) -> f32 {
    let nodes = nodes(element);
    nodes
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some(label))
        .unwrap_or_else(|| {
            let labels: Vec<_> = nodes.iter().map(|node| &node.semantics.label).collect();
            panic!("no node labelled {label:?} among {labels:?}")
        })
        .bounds
        .center_x()
}

fn button(label: &str) -> Element<'static, Msg> {
    focus_button(iced::widget::text(label.to_owned()), Msg::Noop)
        .semantics(Semantics::new(Role::Button).label(label.to_owned()))
        .into()
}

/// The horizontal centre of everything `element` paints in a 300 px band.
fn ink_centre_x(element: Element<'static, Msg>) -> f32 {
    let buf = render(element, 300, 40);
    let (x, _, w, _) = ink_bbox(&buf, 300, 40).expect("the widget paints something");
    x + w / 2.0
}

#[test]
fn rtl_layout_navigation_and_paint_follow_the_locale_1() {
    // row_widgets_put_their_start_on_the_right
    {
        let sidebar = |direction: Direction| {
            enzui_widgets::sidebar_row::enz_sidebar_row("Inbox", Msg::Noop)
                .trailing(button("Remove"))
                .direction(direction)
                .into()
        };
        let (ltr, rtl) = (
            centre_x(sidebar(LeftToRight), "Remove"),
            centre_x(sidebar(RightToLeft), "Remove"),
        );
        assert!(
            ltr > WIDTH / 2.0 && rtl < WIDTH / 2.0,
            "sidebar trailing control: {ltr} / {rtl}"
        );

        let field = |direction: Direction| {
            enzui_widgets::text_field::enz_text_field("", "", "", |_| Msg::Noop)
                .trailing(button("Clear"))
                .direction(direction)
                .into()
        };
        let (ltr, rtl) = (
            centre_x(field(LeftToRight), "Clear"),
            centre_x(field(RightToLeft), "Clear"),
        );
        assert!(
            ltr > WIDTH / 2.0 && rtl < WIDTH / 2.0,
            "text field trailing slot: {ltr} / {rtl}"
        );

        let toolbar = |direction: Direction| {
            enzui_widgets::toolbar_row::enz_toolbar_row()
                .start(button("Back"))
                .end(button("Menu"))
                .direction(direction)
                .into_element()
        };
        for (direction, back_first) in [(LeftToRight, true), (RightToLeft, false)] {
            let back = centre_x(toolbar(direction), "Back");
            let menu = centre_x(toolbar(direction), "Menu");
            assert_eq!(
                back < menu,
                back_first,
                "toolbar {direction:?}: back {back}, menu {menu}"
            );
        }

        let crumbs = |direction: Direction| {
            enzui_widgets::breadcrumb::enz_breadcrumb(
                vec!["Home".into(), "Documents".into(), "Report".into()],
                |_| Msg::Noop,
            )
            .direction(direction)
            .into()
        };
        for (direction, home_first) in [(LeftToRight, true), (RightToLeft, false)] {
            let home = centre_x(crumbs(direction), "Home");
            let report = centre_x(crumbs(direction), "Report");
            assert_eq!(
                home < report,
                home_first,
                "breadcrumb {direction:?}: {home} / {report}"
            );
        }
    }

    // title_bar_moves_window_controls_to_the_left
    {
        let bar = |direction: Direction| {
            enzui_widgets::title_bar::enz_title_bar()
                .title("Document")
                .on_gesture(|_| Msg::Noop)
                .direction(direction)
                .into_element()
        };
        let (ltr, rtl) = (
            centre_x(bar(LeftToRight), "Close"),
            centre_x(bar(RightToLeft), "Close"),
        );
        assert!(
            ltr > WIDTH / 2.0 && rtl < WIDTH / 2.0,
            "close button: {ltr} / {rtl}"
        );
    }

    // leading_icons_paint_on_the_right
    {
        let menu = |direction: Direction| {
            enzui_widgets::menu_row::enz_menu_row("", Msg::Noop)
                .icon("action.copy")
                .direction(direction)
                .into()
        };
        let (ltr, rtl) = (
            ink_centre_x(menu(LeftToRight)),
            ink_centre_x(menu(RightToLeft)),
        );
        assert!(ltr < 150.0 && rtl > 150.0, "menu row icon: {ltr} / {rtl}");

        let disclosure = |direction: Direction| {
            enzui_widgets::disclosure::enz_disclosure("", false, |_| Msg::Noop)
                .direction(direction)
                .into()
        };
        let (ltr, rtl) = (
            ink_centre_x(disclosure(LeftToRight)),
            ink_centre_x(disclosure(RightToLeft)),
        );
        assert!(
            ltr < 150.0 && rtl > 150.0,
            "disclosure chevron: {ltr} / {rtl}"
        );

        // A label alone starts at the start edge too.
        let menu_label = |direction: Direction| {
            enzui_widgets::menu_row::enz_menu_row("Copy", Msg::Noop)
                .direction(direction)
                .into()
        };
        let (ltr, rtl) = (
            ink_centre_x(menu_label(LeftToRight)),
            ink_centre_x(menu_label(RightToLeft)),
        );
        assert!(ltr < 150.0 && rtl > 150.0, "menu row label: {ltr} / {rtl}");
        let sidebar_label = |direction: Direction| {
            enzui_widgets::sidebar_row::enz_sidebar_row("Inbox", Msg::Noop)
                .direction(direction)
                .into()
        };
        let (ltr, rtl) = (
            ink_centre_x(sidebar_label(LeftToRight)),
            ink_centre_x(sidebar_label(RightToLeft)),
        );
        assert!(
            ltr < 150.0 && rtl > 150.0,
            "sidebar row label: {ltr} / {rtl}"
        );
    }

    // a_right_to_left_slider_has_its_minimum_on_the_right
    {
        let press_near_right = |direction: Direction| {
            let slider: Element<'_, Msg> =
                enzui_widgets::slider::enz_slider(0.0..=100.0, 50.0, |v| {
                    Msg::Value(v.round() as u32)
                })
                .direction(direction)
                .into();
            let mut ui =
                Simulator::with_size(iced::Settings::default(), Size::new(WIDTH, 60.0), slider);
            ui.point_at(Point::new(WIDTH - 4.0, 10.0));
            let _ = ui.simulate(click());
            ui.into_messages().last()
        };
        assert_eq!(press_near_right(LeftToRight), Some(Msg::Value(100)));
        assert_eq!(press_near_right(RightToLeft), Some(Msg::Value(0)));
    }
}

#[test]
fn rtl_layout_navigation_and_paint_follow_the_locale_2() {
    // a_right_to_left_tree_expands_with_the_left_arrow
    {
        use enzui_widgets::tree_view::{EnzTreeItem, enz_tree_view};
        let toggled = |direction: Direction, arrow: iced::keyboard::key::Named| {
            let tree: Element<'_, Msg> = enz_tree_view(vec![
                EnzTreeItem::branch("Folder", 0, false),
                EnzTreeItem::leaf("File", 1),
            ])
            .on_select(|_| Msg::Noop)
            .on_toggle(Msg::Toggle)
            .direction(direction)
            .into();
            let mut ui =
                Simulator::with_size(iced::Settings::default(), Size::new(WIDTH, 200.0), tree);
            ui.point_at(Point::new(WIDTH / 2.0, 12.0));
            let _ = ui.simulate(click());
            let _ = ui.tap_key(iced::keyboard::Key::Named(arrow));
            ui.into_messages().any(|message| message == Msg::Toggle(0))
        };
        use iced::keyboard::key::Named::{ArrowLeft, ArrowRight};
        assert!(toggled(LeftToRight, ArrowRight), "positive calibration");
        assert!(!toggled(LeftToRight, ArrowLeft));
        assert!(toggled(RightToLeft, ArrowLeft));
        assert!(!toggled(RightToLeft, ArrowRight));
    }

    // start_aligned_text_follows_the_locale
    {
        use enzui_core::localization::{CatalogBuilder, Locale, Localizer};
        use enzui_widgets::TextAlign;
        use iced::alignment::Horizontal;
        let localizer = Localizer::new(
            CatalogBuilder::new().build(),
            vec![Locale::default()],
            Locale::default(),
        );
        localizer.scope(|| {
            localizer.select(vec!["en".parse().unwrap()]);
            assert_eq!(TextAlign::Start.into_horizontal(), Horizontal::Left);
            localizer.select(vec!["ar".parse().unwrap()]);
            assert_eq!(TextAlign::Start.into_horizontal(), Horizontal::Right);
            assert_eq!(TextAlign::End.into_horizontal(), Horizontal::Left);
            assert_eq!(
                TextAlign::Left.into_horizontal(),
                Horizontal::Left,
                "physical stays put"
            );
        });
    }

    // a_right_to_left_slider_steps_up_with_the_left_arrow
    {
        let after_left_arrow = |direction: Direction| {
            let slider: Element<'_, Msg> =
                enzui_widgets::slider::enz_slider(0.0..=100.0, 50.0, |v| {
                    Msg::Value(v.round() as u32)
                })
                .step(10.0)
                .direction(direction)
                .into();
            let mut ui =
                Simulator::with_size(iced::Settings::default(), Size::new(WIDTH, 60.0), slider);
            ui.point_at(Point::new(WIDTH / 2.0, 10.0));
            let _ = ui.simulate(click());
            let _ = ui.tap_key(iced::keyboard::Key::Named(
                iced::keyboard::key::Named::ArrowLeft,
            ));
            ui.into_messages().last()
        };
        assert_eq!(after_left_arrow(LeftToRight), Some(Msg::Value(40)));
        assert_eq!(after_left_arrow(RightToLeft), Some(Msg::Value(60)));
    }

    // tab_strips_run_from_the_right
    {
        use enzui_widgets::view_switcher::{SwitcherPage, enz_view_switcher};
        let switcher = || -> Element<'static, Msg> {
            enz_view_switcher(
                vec![
                    SwitcherPage::new("one", "One"),
                    SwitcherPage::new("two", "Two"),
                    SwitcherPage::new("three", "Three"),
                ],
                0,
                Msg::Toggle,
            )
            .into_element()
        };
        for rtl in [false, true] {
            let (one, two) = in_locale(rtl, || {
                (centre_x(switcher(), "One"), centre_x(switcher(), "Two"))
            });
            assert_eq!(one > two, rtl, "rtl={rtl}: one {one}, two {two}");

            // The arrow pointing at the second tab selects it.
            let arrow = if rtl {
                iced::keyboard::key::Named::ArrowLeft
            } else {
                iced::keyboard::key::Named::ArrowRight
            };
            let selected = in_locale(rtl, || {
                let mut ui = Simulator::with_size(
                    iced::Settings::default(),
                    Size::new(WIDTH, 80.0),
                    switcher(),
                );
                let _ = ui.click("One");
                let _ = ui.tap_key(iced::keyboard::Key::Named(arrow));
                ui.into_messages().any(|message| message == Msg::Toggle(1))
            });
            assert!(selected, "rtl={rtl}: {arrow:?} selects the second tab");
        }
    }
}

/// Runs `test` with an Arabic (right-to-left) or English locale in scope.
fn in_locale<R>(rtl: bool, test: impl FnOnce() -> R) -> R {
    use enzui_core::localization::{CatalogBuilder, Locale, Localizer};
    let localizer = Localizer::new(
        CatalogBuilder::new().build(),
        vec![Locale::default()],
        Locale::default(),
    );
    localizer.select(vec![if rtl { "ar" } else { "en" }.parse().unwrap()]);
    localizer.scope(test)
}

#[test]
fn rtl_layout_navigation_and_paint_follow_the_locale_3() {
    // a_pane_header_leads_from_the_right_in_a_right_to_left_locale
    {
        let header = |direction: Option<Direction>| -> Element<'static, Msg> {
            let mut header = enzui_widgets::pane_header::enz_pane_header("Mailbox")
                .leading(button("Menu"))
                .subtitle("12 unread")
                .trailing(button("Compose"));
            if let Some(direction) = direction {
                header = header.direction(direction);
            }
            header.into()
        };
        let order = |element: Element<'static, Msg>| -> Vec<f32> {
            let nodes = nodes(element);
            ["Menu", "Mailbox", "Compose"]
                .iter()
                .map(|label| {
                    nodes
                        .iter()
                        .find(|node| node.semantics.label.as_deref() == Some(*label))
                        .unwrap_or_else(|| panic!("no {label}"))
                        .bounds
                        .center_x()
                })
                .collect()
        };
        // The locale decides by default; the override wins over it.
        let ltr = in_locale(false, || order(header(None)));
        let rtl = in_locale(true, || order(header(None)));
        let forced_ltr = in_locale(true, || order(header(Some(LeftToRight))));
        assert!(ltr[0] < ltr[1] && ltr[1] < ltr[2], "left to right: {ltr:?}");
        assert!(rtl[0] > rtl[1] && rtl[1] > rtl[2], "right to left: {rtl:?}");
        assert!(
            rtl[0] > WIDTH / 2.0 && rtl[2] < WIDTH / 2.0,
            "edges: {rtl:?}"
        );
        assert_eq!(forced_ltr, ltr, "the override wins over the locale");

        // Reading and focus order stay logical: Menu, title, Compose.
        let labels: Vec<String> = in_locale(true, || nodes(header(None)))
            .into_iter()
            .filter_map(|node| node.semantics.label)
            .filter(|label| ["Menu", "Mailbox", "Compose"].contains(&label.as_str()))
            .collect();
        assert_eq!(labels, ["Menu", "Mailbox", "Compose"]);
    }

    // inbox_rows_and_tab_cards_run_from_the_start_edge
    {
        use enzui_core::notification_inbox::Inbox;
        use enzui_widgets::notification_inbox::enz_notification_inbox;
        use enzui_widgets::tab_overview::{TabCard, enz_tab_overview};
        let inbox_view = || -> Element<'static, Msg> {
            let mut inbox = Inbox::default();
            inbox.push(
                "Backup finished",
                Some("1,204 files".into()),
                std::time::SystemTime::now(),
            );
            enz_notification_inbox(&inbox, |_| Msg::Noop)
        };
        let cards = || -> Element<'static, Msg> {
            enz_tab_overview(
                vec![TabCard {
                    title: "Inbox".to_owned(),
                    preview: iced::widget::text("preview").into(),
                    selected: true,
                }],
                |_| Msg::Noop,
            )
        };
        for rtl in [false, true] {
            let (title, dismiss, body) = in_locale(rtl, || {
                let nodes = nodes(inbox_view());
                let find = |prefix: &str| {
                    nodes
                        .iter()
                        .find(|node| {
                            node.semantics
                                .label
                                .as_deref()
                                .is_some_and(|label| label.starts_with(prefix))
                        })
                        .unwrap_or_else(|| panic!("no {prefix}"))
                        .bounds
                };
                (
                    find("Backup finished"),
                    find("Dismiss"),
                    find("1,204 files"),
                )
            });
            assert_eq!(
                dismiss.center_x() < title.center_x(),
                rtl,
                "rtl={rtl}: dismiss {dismiss:?} beside title {title:?}"
            );
            assert_eq!(
                body.center_x() > WIDTH / 2.0,
                rtl,
                "rtl={rtl}: the body starts at the start edge: {body:?}"
            );
            let (card_title, close) = in_locale(rtl, || {
                let nodes = nodes(cards());
                let find = |prefix: &str| {
                    nodes
                        .iter()
                        .find(|node| {
                            node.semantics
                                .label
                                .as_deref()
                                .is_some_and(|label| label.starts_with(prefix))
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "no {prefix}: {:?}",
                                nodes.iter().map(|n| &n.semantics.label).collect::<Vec<_>>()
                            )
                        })
                        .bounds
                };
                (find("Inbox"), find("Close"))
            });
            assert_eq!(
                close.center_x() < card_title.center_x(),
                rtl,
                "rtl={rtl}: card close {close:?}"
            );
        }
    }

    // preferences_rows_and_groups_lead_from_the_right_in_right_to_left_text
    {
        let row = |direction: Direction| -> Vec<CollectedNode> {
            nodes(
                enzui_widgets::preferences_row::enz_preferences_row("Title")
                    .entry("Value", "Title", |_| Msg::Noop)
                    .direction(direction)
                    .into_element(),
            )
        };
        let positions = |direction: Direction| {
            let nodes = row(direction);
            let entry = nodes
                .iter()
                .find(|node| node.semantics.role == Role::TextInput)
                .expect("the entry is published")
                .bounds
                .center_x();
            let label = nodes
                .iter()
                .find(|node| {
                    node.semantics.role != Role::TextInput
                        && node.semantics.label.as_deref() == Some("Title")
                })
                .expect("the title is published")
                .bounds;
            (label, entry)
        };
        let (label, entry) = positions(LeftToRight);
        assert!(
            label.center_x() < entry,
            "left to right: the label leads at the left"
        );
        let (label, entry) = positions(RightToLeft);
        assert!(
            label.center_x() > entry,
            "right to left: the label leads at the right"
        );
        assert!(
            label.x + label.width > WIDTH - 40.0,
            "right to left: the label starts at the right edge: {label:?}"
        );

        // The group's header is decorative, so its placement is read from ink.
        let heading = |rtl: bool| {
            in_locale(rtl, || {
                ink_centre_x(
                    enzui_widgets::preferences_group::enz_preferences_group(
                        iced::widget::Space::new().width(iced::Length::Fill),
                    )
                    .title("Heading")
                    .into_element(),
                )
            })
        };
        assert!(
            heading(false) < 150.0,
            "left to right: the heading at the left"
        );
        assert!(
            heading(true) > 150.0,
            "right to left: the heading at the right"
        );
    }
}
