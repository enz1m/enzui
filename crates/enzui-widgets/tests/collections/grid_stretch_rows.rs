//! `EnzGrid::stretch_rows` gives every child its row's full height.

use enzui_widgets::grid::enz_grid;
use iced::widget::{Space, container};
use iced::{Element, Length, Size};
use iced_test::Simulator;
use iced_test::selector::id as by_id;

fn cell(id: &'static str, content_height: f32) -> Element<'static, ()> {
    container(Space::new().height(Length::Fixed(content_height)))
        .id(iced::widget::Id::new(id))
        .width(Length::Fill)
        .into()
}

fn heights(stretch: bool) -> (f32, f32, f32) {
    let grid = enz_grid(
        2,
        vec![cell("short", 20.0), cell("tall", 60.0), cell("next", 10.0)],
    )
    .stretch_cells(true)
    .stretch_rows(stretch)
    .into_element();
    let mut ui = Simulator::with_size(
        iced_test::core::Settings::default(),
        Size::new(300.0, 400.0),
        grid,
    );
    let mut height = |id: &'static str| {
        ui.find(by_id(iced::widget::Id::new(id)))
            .expect("cell exists")
            .bounds()
            .height
    };
    (height("short"), height("tall"), height("next"))
}

#[test]
fn a_stretched_row_ends_level_and_rows_stay_their_own_height() {
    assert_eq!(heights(false), (20.0, 60.0, 10.0));
    assert_eq!(heights(true), (60.0, 60.0, 10.0));
}
