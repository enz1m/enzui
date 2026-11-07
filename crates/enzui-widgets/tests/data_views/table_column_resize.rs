//! Resizable table columns: a drag on a header's trailing edge, the minimum
//! width, double-click to fit, the keyboard step and the resize cursor.

use enzui_core::table::{ColumnKey, TableProvider};
use enzui_core::virtual_sequence::{
    HeightModel, ItemProvider, KeyedProvider, SequenceKey, VariableHeightModel,
};
use enzui_widgets::table::{
    COLUMN_RESIZE_STEP, CellCoord, ColumnResize, SortCommand, TableKeyAction, enz_table,
    table_key_action,
};
use iced::keyboard::{self, Modifiers, key::Named};
use iced::mouse;
use iced::widget::text;
use iced::{Element, Event, Length, Point, Size};
use iced_test::Simulator;
use iced_test::simulator::click;

const LONG: &str = "a much longer value in the second column";

struct Rows;

impl ItemProvider<SequenceKey> for Rows {
    fn len(&self) -> u64 {
        3
    }
    fn item(&self, index: u64) -> Option<SequenceKey> {
        self.key(index)
    }
}

impl KeyedProvider<SequenceKey> for Rows {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        (index < 3).then(|| SequenceKey::new(format!("row-{index}")))
    }
}

impl TableProvider<String> for Rows {
    fn column_count(&self) -> usize {
        2
    }
    fn column_key(&self, column: usize) -> Option<ColumnKey> {
        (column < 2).then(|| ColumnKey::new(format!("col-{column}")))
    }
    fn cell(&self, row: u64, column: usize) -> Option<String> {
        match (row, column) {
            (1, 1) => Some(LONG.to_owned()),
            (0..3, 0..2) => Some(format!("r{row}c{column}")),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Resize(ColumnResize),
    Sort(SortCommand),
    Active(CellCoord),
}

fn table(widths: [f32; 2], active: Option<CellCoord>, min: f32) -> Element<'static, Msg> {
    let mut table = enz_table(
        &Rows,
        HeightModel::new(3, 24.0),
        VariableHeightModel::new(widths),
        |_row, _column, value: &String| text(value.clone()).into(),
    )
    .header(|index, _key| text(["Name", "Size"][index]).into())
    .on_column_sort(Msg::Sort)
    .on_active_cell_change(Msg::Active)
    .on_column_resize(Msg::Resize)
    .min_column_width(min)
    .width(Length::Fixed(600.0))
    .height(Length::Fixed(200.0));
    if let Some(active) = active {
        table = table.active_cell(active);
    }
    table.into_element()
}

fn resize(column: usize, width: f32, finished: bool) -> Msg {
    Msg::Resize(ColumnResize {
        column,
        key: ColumnKey::new(format!("col-{column}")),
        width,
        finished,
    })
}

fn moved(x: f32, y: f32) -> Event {
    Event::Mouse(mouse::Event::CursorMoved {
        position: Point::new(x, y),
    })
}

const PRESS: Event = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
const RELEASE: Event = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));

#[test]
fn column_resize_sort_fit_and_keyboard_paths_obey_constraints_1() {
    // dragging_a_header_edge_resizes_its_column
    {
        let mut ui = Simulator::new(table([100.0, 100.0], None, 40.0));
        ui.point_at(Point::new(97.0, 16.0));
        let _ = ui.simulate([PRESS, moved(130.0, 16.0), moved(147.0, 16.0), RELEASE]);
        let messages: Vec<Msg> = ui.into_messages().collect();
        assert_eq!(
            messages,
            [
                resize(0, 133.0, false),
                resize(0, 150.0, false),
                resize(0, 150.0, true)
            ],
            "live widths while moving, the last one again when the drag ends; no sort"
        );
    }

    // a_drag_stops_at_the_minimum_width
    {
        let mut ui = Simulator::new(table([100.0, 100.0], None, 60.0));
        ui.point_at(Point::new(197.0, 16.0));
        let _ = ui.simulate([PRESS, moved(20.0, 16.0), RELEASE]);
        let messages: Vec<Msg> = ui.into_messages().collect();
        assert_eq!(messages, [resize(1, 60.0, false), resize(1, 60.0, true)]);
    }

    // a_press_inside_the_header_still_sorts
    {
        let mut ui = Simulator::new(table([100.0, 100.0], None, 40.0));
        ui.point_at(Point::new(50.0, 16.0));
        let _ = ui.simulate(click());
        let messages: Vec<Msg> = ui.into_messages().collect();
        assert!(
            matches!(messages.as_slice(), [Msg::Sort(command)] if command.column.as_str() == "col-0"),
            "{messages:?}"
        );
    }

    // double_clicking_a_header_edge_fits_the_column
    {
        let fit = |column: usize| {
            let mut ui = Simulator::new(table([100.0, 100.0], None, 10.0));
            ui.point_at(Point::new(100.0 * (column as f32 + 1.0) - 3.0, 16.0));
            let _ = ui.simulate(click().chain(click()));
            ui.into_messages().collect::<Vec<Msg>>()
        };
        // The widest realised cell of column 1 is the long value.
        let long = natural_width(LONG).ceil();
        assert!(long > 150.0, "calibration: the long value is wide: {long}");
        assert_eq!(fit(1), [resize(1, long, true)]);
        // Column 0 holds short values: it fits narrower than it is.
        let short = natural_width("Name").max(natural_width("r0c0")).ceil();
        assert_eq!(fit(0), [resize(0, short, true)]);
    }
}

/// The natural width of `content`, laid out alone.
fn natural_width(content: &str) -> f32 {
    measure(text(content.to_owned()), content)
}

/// The natural width of `content` set at `size` pixels.
fn natural_width_at(content: &str, size: f32) -> f32 {
    measure(text(content.to_owned()).size(size), content)
}

fn measure(label: iced::widget::Text<'static>, content: &str) -> f32 {
    let mut ui: Simulator<'_, ()> = Simulator::new(
        iced::widget::row![label, iced::widget::Space::new().width(Length::Fill)]
            .width(Length::Fixed(1000.0)),
    );
    ui.find(content).expect("the text").bounds().width
}

fn key_press(named: Named, modifiers: Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        modified_key: keyboard::Key::Named(named),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        repeat: false,
        text: None,
    })
}

#[test]
fn column_resize_sort_fit_and_keyboard_paths_obey_constraints_2() {
    // alt_shift_arrows_resize_the_active_column
    {
        let step = |widths: [f32; 2], named: Named| {
            let mut ui = Simulator::new(table(widths, Some(CellCoord::new(0, 1)), 40.0));
            let _ = ui.simulate([key_press(named, Modifiers::ALT | Modifiers::SHIFT)]);
            ui.into_messages().collect::<Vec<Msg>>()
        };
        assert_eq!(
            step([100.0, 100.0], Named::ArrowRight),
            [resize(1, 100.0 + COLUMN_RESIZE_STEP, true)]
        );
        assert_eq!(
            step([100.0, 100.0], Named::ArrowLeft),
            [resize(1, 100.0 - COLUMN_RESIZE_STEP, true)]
        );
        assert_eq!(
            step([100.0, 40.0], Named::ArrowLeft),
            [],
            "at the minimum nothing moves"
        );
        // Calibration: the bare arrow still moves the active cell.
        let mut ui = Simulator::new(table([100.0, 100.0], Some(CellCoord::new(0, 1)), 40.0));
        let _ = ui.simulate([key_press(Named::ArrowLeft, Modifiers::empty())]);
        assert_eq!(
            ui.into_messages().collect::<Vec<Msg>>(),
            [Msg::Active(CellCoord::new(0, 0))]
        );
    }

    // the_resize_keys_are_alt_shift_arrows_only
    {
        let action = |named: Named, modifiers: Modifiers| {
            table_key_action(&keyboard::Key::Named(named), modifiers)
        };
        let alt_shift = Modifiers::ALT | Modifiers::SHIFT;
        assert_eq!(
            action(Named::ArrowLeft, alt_shift),
            Some(TableKeyAction::NarrowColumn)
        );
        assert_eq!(
            action(Named::ArrowRight, alt_shift),
            Some(TableKeyAction::WidenColumn)
        );
        assert_eq!(action(Named::ArrowLeft, Modifiers::ALT), None);
        assert_eq!(action(Named::ArrowUp, alt_shift), None);
        assert_eq!(action(Named::ArrowLeft, alt_shift | Modifiers::CTRL), None);
    }

    // the_header_edge_shows_the_resize_cursor
    {
        use iced::advanced::renderer::Headless;
        use iced_test::runtime::user_interface::{Cache, State, UserInterface};
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as Headless>::new(Default::default(), None),
        )
        .expect("renderer");
        let mut interaction_at = |point: Point| {
            let mut ui = UserInterface::build(
                table([100.0, 100.0], None, 40.0),
                Size::new(600.0, 200.0),
                Cache::default(),
                &mut renderer,
            );
            let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
            let (state, _) = ui.update(
                &iced::window::Headless,
                &iced_test::core::shell::Waker::noop(),
                &[moved(point.x, point.y)],
                mouse::Cursor::Available(point),
                &mut renderer,
                &mut messages,
            );
            match state {
                State::Updated {
                    mouse_interaction, ..
                } => mouse_interaction,
                State::Outdated => panic!("outdated"),
            }
        };
        assert_eq!(
            interaction_at(Point::new(97.0, 16.0)),
            mouse::Interaction::ResizingHorizontally
        );
        assert_ne!(
            interaction_at(Point::new(50.0, 16.0)),
            mouse::Interaction::ResizingHorizontally
        );
    }

    // fitting_measures_cells_that_fill_their_width
    {
        // The framework's text and header cells fill their column, so the fit
        // reads how much width their text needs, elided or not.
        use enzui_widgets::table_header_cell::enz_table_header_cell;
        use enzui_widgets::table_text_cell::enz_table_text_cell;
        let view = enz_table(
            &Rows,
            HeightModel::new(3, 24.0),
            VariableHeightModel::new([100.0, 100.0]),
            |_row, _column, value: &String| {
                enz_table_text_cell(value.clone())
                    .body_size(13.0)
                    .cell_padding_x(10.0)
                    .into_element()
            },
        )
        .header(|index, _key| enz_table_header_cell(["Name", "Size"][index]).into_element())
        .on_column_resize(Msg::Resize)
        .min_column_width(10.0)
        .width(Length::Fixed(600.0))
        .height(Length::Fixed(200.0));
        let mut ui = Simulator::new(view.into_element());
        ui.point_at(Point::new(197.0, 16.0));
        let _ = ui.simulate(click().chain(click()));
        let messages: Vec<Msg> = ui.into_messages().collect();
        let expected = (natural_width_at(LONG, 13.0) + 20.0).ceil();
        assert_eq!(messages, [resize(1, expected, true)]);
    }
}

#[test]
fn column_resize_sort_fit_and_keyboard_paths_obey_constraints_3() {
    // fitting_keeps_a_shrinking_neighbour_of_a_filling_label
    {
        // A filling label beside a shrinking badge: the badge's text leaves no
        // space unused, the label's does, and the fit is both side by side.
        let view = enz_table(
            &Rows,
            HeightModel::new(3, 24.0),
            VariableHeightModel::new([100.0, 100.0]),
            |_row, _column, value: &String| {
                iced::widget::row![
                    text(value.clone()).size(13.0).width(Length::Fill),
                    text("new").size(13.0)
                ]
                .spacing(6)
                .into()
            },
        )
        .header(|index, _key| text(["Name", "Size"][index]).into())
        .on_column_resize(Msg::Resize)
        .min_column_width(10.0)
        .width(Length::Fixed(600.0))
        .height(Length::Fixed(200.0));
        let mut ui = Simulator::new(view.into_element());
        ui.point_at(Point::new(197.0, 16.0));
        let _ = ui.simulate(click().chain(click()));
        let messages: Vec<Msg> = ui.into_messages().collect();
        let expected = (natural_width_at(LONG, 13.0) + 6.0 + natural_width_at("new", 13.0)).ceil();
        assert_eq!(messages, [resize(1, expected, true)]);
    }

    // hovering_a_header_edge_paints_the_column_edge
    {
        use iced::advanced::renderer::Headless;
        use iced_test::runtime::user_interface::{Cache, UserInterface};
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as Headless>::new(Default::default(), None),
        )
        .expect("renderer");
        let theme = enzui_theme::EnzTheme::dark().iced_theme();
        let base = <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
        let mut paint = |cursor: Point| {
            let mut ui = UserInterface::build(
                table([100.0, 100.0], None, 40.0),
                Size::new(600.0, 200.0),
                Cache::default(),
                &mut renderer,
            );
            let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
            let _ = ui.update(
                &iced::window::Headless,
                &iced_test::core::shell::Waker::noop(),
                &[moved(cursor.x, cursor.y)],
                mouse::Cursor::Available(cursor),
                &mut renderer,
                &mut messages,
            );
            ui.draw(
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: base.text_color,
                },
                mouse::Cursor::Available(cursor),
            );
            renderer.screenshot(Size::new(600, 200), 1.0, base.background_color)
        };
        let at = |pixels: &[u8], x: usize, y: usize| {
            pixels[(y * 600 + x) * 4..(y * 600 + x) * 4 + 3].to_vec()
        };
        let away = paint(Point::new(50.0, 16.0));
        let over = paint(Point::new(97.0, 16.0));
        assert_eq!(
            at(&away, 99, 16),
            at(&away, 99, 4),
            "no edge line away from the strip"
        );
        assert_ne!(
            at(&over, 99, 16),
            at(&away, 99, 16),
            "the edge line shows over the strip"
        );
    }
}
