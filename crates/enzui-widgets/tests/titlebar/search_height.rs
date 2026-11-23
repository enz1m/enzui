use enzui_theme::text_scale::{TextScale, TextScaleScope};
use enzui_widgets::search_field::enz_titlebar_search;
use enzui_widgets::window_control_button::enz_window_control_button;
use iced::advanced::layout::Limits;
use iced::advanced::widget::Tree;
use iced::{Element, Size};

fn height(mut view: Element<'_, ()>) -> f32 {
    let renderer = crate::honest_frame_driver::new_renderer();
    let mut tree = Tree::new(view.as_widget());
    tree.diff(view.as_widget_mut());
    view.as_widget_mut().layout(
        &mut tree,
        &renderer,
        &Limits::new(Size::ZERO, Size::new(600.0, 120.0)),
    );
    tree.size.height
}

#[test]
fn titlebar_search_matches_the_close_control_at_smaller_text_sizes() {
    for percent in [87, 100] {
        let _scale = TextScaleScope::enter(TextScale::from_percent(percent));
        let close = enz_window_control_button("action.close", ())
            .aria_label("Close")
            .into_element();
        let close_height = height(close);
        for width in [28, 240] {
            let search = enz_titlebar_search("", |_| ()).width(width).into_element();
            assert_eq!(height(search), close_height);
        }
    }
}
