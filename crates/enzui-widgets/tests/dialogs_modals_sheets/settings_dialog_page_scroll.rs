//! Who scrolls a settings page: the preset by default, the page when it owns
//! its viewport; dialog commands stay pinned in the footer either way.

use enzui_core::identity::derive_id;
use enzui_widgets::button::enz_button;
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::settings_dialog::{
    SettingsDialogEvent, SettingsPage, enz_settings_dialog, footer_action_id, settings_action,
};
use iced::widget::column;
use iced::{Element, Length, Size, mouse};
use iced_test::Simulator;

const PREFIX: &str = "page-scroll";
const WINDOW: Size = Size::new(900.0, 600.0);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Save,
    Header,
    Other,
}

fn id(name: &str) -> iced::advanced::widget::Id {
    derive_id(PREFIX, name)
}

fn rows() -> Vec<Element<'static, Msg>> {
    (0..60)
        .map(|i| {
            enz_button(format!("Row {i}"), Msg::Other)
                .id(id(&format!("row{i}")))
                .into_element()
        })
        .collect()
}

fn dialog(page: SettingsPage, body: Element<'static, Msg>) -> Element<'static, Msg> {
    enz_settings_dialog(true, WINDOW)
        .title("Settings")
        .page(page, body)
        .page(SettingsPage::new("other", "Other"), iced::widget::text("x"))
        .on_select(|_| Msg::Other)
        .page_id_prefix(PREFIX)
        .action(settings_action("save", "Save", || Msg::Save))
        .into_host_with(
            iced::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill),
            |_: SettingsDialogEvent| Msg::Other,
        )
}

fn scroll_down(sim: &mut Simulator<'_, Msg>, over: iced::Point) {
    sim.point_at(over);
    for _ in 0..20 {
        let _ = sim.scroll(mouse::ScrollDelta::Lines { x: 0.0, y: -10.0 });
    }
}

#[test]
fn a_natural_height_page_scrolls_under_a_pinned_save() {
    let body = column(rows()).spacing(8).into();
    let mut sim = Simulator::with_size(
        iced_test::core::Settings::default(),
        WINDOW,
        dialog(SettingsPage::new("general", "General"), body),
    );
    let first = sim.find(id("row0")).unwrap().bounds();
    assert!(sim.find(id("row59")).unwrap().visible_bounds().is_none());
    scroll_down(&mut sim, first.center());
    assert!(
        sim.find(id("row59")).unwrap().visible_bounds().is_some(),
        "the preset scrolls the page to its end"
    );
    assert!(
        sim.find(footer_action_id(PREFIX, "save"))
            .unwrap()
            .visible_bounds()
            .is_some(),
        "Save stays in view"
    );
    sim.click(iced_test::selector::id(footer_action_id(PREFIX, "save")))
        .unwrap();
    assert_eq!(sim.into_messages().collect::<Vec<_>>(), [Msg::Save]);
}

#[test]
fn a_page_that_owns_its_scroll_keeps_its_own_header_in_place() {
    let body = column![
        enz_button("Header", Msg::Header)
            .id(id("header"))
            .into_element(),
        enz_scroll_column(rows()).height(Length::Fill),
    ]
    .height(Length::Fill)
    .into();
    let mut sim = Simulator::with_size(
        iced_test::core::Settings::default(),
        WINDOW,
        dialog(SettingsPage::new("list", "List").owns_scroll(true), body),
    );
    let header = sim.find(id("header")).unwrap().bounds();
    let first = sim.find(id("row0")).unwrap().bounds();
    scroll_down(&mut sim, first.center());
    assert!(
        sim.find(id("row59")).unwrap().visible_bounds().is_some(),
        "the page's own list scrolls to its end"
    );
    let after = sim.find(id("header")).unwrap();
    assert_eq!(
        after.bounds(),
        header,
        "the header above the list stays put"
    );
    assert!(after.visible_bounds().is_some());
    sim.click(iced_test::selector::id(id("header"))).unwrap();
    sim.click(iced_test::selector::id(footer_action_id(PREFIX, "save")))
        .unwrap();
    assert_eq!(
        sim.into_messages().collect::<Vec<_>>(),
        [Msg::Header, Msg::Save]
    );
}
