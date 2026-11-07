//! The kebab overflow menu of
//! [`enzui_widgets::titlebar_actions`] has a dismissal contract, and
//! it is the one [`enzui_widgets::action_bar`] has.

use enzui_core::action_overflow::Descriptor;
use enzui_core::semantics::{Role, Semantics};
use enzui_widgets::focus_button::focus_button;
use enzui_widgets::semantics_container::SemanticsContainer;
use enzui_widgets::titlebar_actions::{ActionItem, enz_titlebar_actions};
use iced::{Element, Size};

const VIEWPORT: Size = Size::new(375.0, 812.0);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Run(&'static str),
    ToggleOverflow,
    Dismiss,
    Item(&'static str),
}

fn act(id: &'static str) -> ActionItem<'static, Msg> {
    ActionItem::new(
        Descriptor::new(id),
        id,
        move || {
            SemanticsContainer::leaf(
                focus_button(iced::widget::text(id), Msg::Run(id)),
                Semantics::new(Role::Button).label(id),
            )
            .into()
        },
        Msg::Item(id),
    )
}

/// A compact cluster with a dedicated dismiss message.
fn cluster(open: bool) -> Element<'static, Msg> {
    enz_titlebar_actions(
        vec![act("Save"), act("Find"), act("Settings")],
        Msg::ToggleOverflow,
    )
    .force_compact(true)
    .overflow_open(open)
    .on_dismiss(Msg::Dismiss)
    .trigger_id(iced::advanced::widget::Id::new("kebab-trigger"))
    .menu_id_prefix("kebab")
    .into()
}

fn count(msgs: &[Msg], want: &Msg) -> usize {
    msgs.iter().filter(|m| *m == want).count()
}

/// Path 2: Escape dismisses, wherever focus sits.
#[test]
fn escape_dismisses_the_open_kebab_menu() {
    let mut sim =
        iced_test::Simulator::with_size(iced::Settings::default(), VIEWPORT, cluster(true));
    let _ = sim.tap_key(iced::keyboard::key::Named::Escape);
    let msgs: Vec<Msg> = sim.into_messages().collect();
    assert_eq!(
        count(&msgs, &Msg::Dismiss),
        1,
        "Escape must dismiss the open kebab menu exactly once: {msgs:?}"
    );
}
