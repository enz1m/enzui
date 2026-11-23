use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_theme::text_scale::{TextScale, TextScaleScope};
use enzui_widgets::view_switcher::{
    SwitcherPage, SwitcherPolicy, SwitcherTokens, enz_view_switcher,
};
use iced::advanced::layout::{Layout, Limits};
use iced::advanced::widget::Tree;
use iced::{Element, Rectangle, Size};

fn measure(mut view: Element<'_, ()>) -> (Rectangle, Vec<CollectedNode>) {
    let renderer = crate::honest_frame_driver::new_renderer();
    let viewport = Rectangle::with_size(Size::new(600.0, 120.0));
    let mut tree = Tree::new(view.as_widget());
    tree.diff(view.as_widget_mut());
    let widget = view.as_widget_mut();
    widget.layout(
        &mut tree,
        &renderer,
        &Limits::new(Size::ZERO, viewport.size()),
    );
    let layout = Layout::new(tree.size);
    let bounds = layout.bounds();
    let mut collector = SemanticsCollector::new();
    widget.operate(&mut tree, layout, &viewport, &renderer, &mut collector);
    (bounds, collector.into_collected())
}

#[test]
fn the_selected_pill_keeps_equal_insets_when_text_scales() {
    for percent in [87, 125] {
        let _scale = TextScaleScope::enter(TextScale::from_percent(percent));
        let tokens = SwitcherTokens::default();
        let switcher = enz_view_switcher(
            vec![
                SwitcherPage::new("overview", "Overview"),
                SwitcherPage::new("history", "History"),
            ],
            0,
            |_| (),
        )
        .policy(SwitcherPolicy {
            compact_fallback: false,
            ..SwitcherPolicy::PILL_STRIP
        })
        .tokens(tokens)
        .into_element();
        let (track, nodes) = measure(switcher);
        let tab = nodes
            .iter()
            .find(|node| node.semantics.role == Role::Tab && node.semantics.selected == Some(true))
            .unwrap()
            .bounds;
        let expected_height = tokens
            .tab_height
            .max(enzui_theme::text_scale::ui_px(tokens.tab_height));
        assert!((tab.height - expected_height).abs() < 0.01);
        assert!((tab.y - track.y - tokens.padding).abs() < 0.01);
        assert!((track.y + track.height - tab.y - tab.height - tokens.padding).abs() < 0.01);
    }
}
