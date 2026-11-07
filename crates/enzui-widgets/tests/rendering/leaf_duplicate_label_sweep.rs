//! Sweep: does a `SemanticsContainer::leaf` widget publish its own accessible
//! name a *second* time as a nested `Role::Label`?

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use iced::Element;
use iced::advanced::widget::Operation;

#[derive(Debug, Clone)]
enum Msg {}

fn collect(view: Element<'_, Msg>) -> Vec<CollectedNode> {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        view,
        iced::Size::new(400.0, 200.0),
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );

    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    collector.into_collected()
}

/// How many times does `text` appear as a node label in the collected tree?
fn label_count(nodes: &[CollectedNode], text: &str) -> usize {
    nodes
        .iter()
        .filter(|n| n.semantics.label.as_deref() == Some(text))
        .count()
}

#[test]
fn a_badge_publishes_its_text_once() {
    let el: Element<'_, Msg> = enzui_widgets::badge::enz_badge("Beta").into();
    let nodes = collect(el);
    let n = label_count(&nodes, "Beta");
    assert_eq!(n, 1, "badge must announce 'Beta' once, got {n}: {nodes:?}");
}
