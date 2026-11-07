//! Derived WCAG 2.2 SC 2.5.8 hit-target sweep.

use crate::layout_spill_widget_sweep::Msg;
use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::{Role, Semantics};
use enzui_widgets::semantics_container::SemanticsContainer;
use iced::advanced::widget::Operation;
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const VIEWPORT_HEIGHT: f32 = 812.0;

fn renderer() -> iced_test::renderer::Renderer {
    iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn nodes_at(view: Element<'static, Msg>, width: u32) -> Vec<CollectedNode> {
    let mut renderer = renderer();
    let size = Size::new(width as f32, VIEWPORT_HEIGHT);
    let mut ui = enzui_widgets::app_shell::without_sidebar_toggle_assert(|| {
        UserInterface::build(view, size, Cache::default(), &mut renderer)
    });
    ui = enzui_widgets::app_shell::without_sidebar_toggle_assert(|| {
        ui.relayout(size, &mut renderer)
    });
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    collector.into_collected()
}

fn titlebar_actions_subject_with_icon_size(icon_size: Option<f32>) -> Element<'static, Msg> {
    use enzui_core::action_overflow::Descriptor;
    use enzui_widgets::focus_button::focus_button;
    use enzui_widgets::titlebar_actions::{ActionItem, enz_titlebar_actions};
    use enzui_widgets::toolbar_icon::ToolbarIconTokens;

    let action = ActionItem::new(
        Descriptor::new("save"),
        "Save",
        || focus_button(iced::widget::text("Save"), Msg::Noop).into(),
        Msg::Noop,
    );
    let mut subject = enz_titlebar_actions(vec![action], Msg::Noop).force_compact(true);
    if let Some(icon_size) = icon_size {
        let mut tokens = ToolbarIconTokens::from_palette(&enzui_theme::Palette::dark());
        tokens.icon_size = icon_size;
        subject = subject.trigger_tokens(tokens);
    }
    subject.into()
}

fn wrapped_titlebar_subject() -> Element<'static, Msg> {
    let row = SemanticsContainer::new(
        titlebar_actions_subject_with_icon_size(None),
        Semantics::new(Role::Group).label("compact reservation"),
    );
    iced::widget::container(row).padding(20).into()
}

#[test]
fn pointer_activation_reaches_the_expanded_area_outside_the_compact_reservation() {
    let nodes = nodes_at(wrapped_titlebar_subject(), 320);
    let trigger = nodes
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some("More actions"))
        .expect("expanded trigger");
    let point = iced::Point::new(trigger.bounds.x + 1.0, trigger.bounds.center_y());
    assert!(
        trigger.bounds.contains(point),
        "calibration: {point:?} must be inside the standard toolbar target {:?}",
        trigger.bounds
    );

    let mut simulator = iced_test::Simulator::with_size(
        iced::Settings::default(),
        Size::new(320.0, VIEWPORT_HEIGHT),
        wrapped_titlebar_subject(),
    );
    simulator.point_at(point);
    let _ = simulator.simulate(iced_test::simulator::click());
    assert_eq!(
        simulator.into_messages().collect::<Vec<_>>(),
        vec![Msg::Noop],
        "the standard toolbar box must be a real pointer target"
    );
}
