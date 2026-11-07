//! A bar chart is keyboard navigable like a line chart: arrows move a
//! host-owned cursor between bars, the cursor's bar is announced and
//! outlined.

use enzui_core::chart::DatumStep;
use enzui_widgets::bar_chart::{BarSelection, BarSeries, EnzBarChart, enz_bar_chart};
use enzui_widgets::line_chart::TraversalKey;
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::Outcome;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::canvas::Program;
use iced::{Event, Point, Rectangle, Size, Theme, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Selected(BarSelection),
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn chart(selection: Option<BarSelection>) -> EnzBarChart<Msg> {
    enz_bar_chart("Spending")
        .bar("Jan", 4.0)
        .bar("Feb", 3.0)
        .bar("Mar", 5.0)
        .series(BarSeries::new("Rent", [7.0, 7.5, 8.0]))
        .selection(selection)
        .on_select(Msg::Selected)
}

fn at(category: usize, series: usize) -> Option<BarSelection> {
    Some(BarSelection { category, series })
}

#[test]
fn arrows_move_between_categories_and_series() {
    let next = |from, key| chart(from).traverse(key);
    let right = TraversalKey::Datum(DatumStep::Next);
    let left = TraversalKey::Datum(DatumStep::Previous);
    assert_eq!(next(None, right), at(0, 0), "a key enters the first bar");
    assert_eq!(next(at(0, 0), right), at(1, 0));
    assert_eq!(next(at(1, 0), TraversalKey::SeriesStep(1)), at(1, 1));
    assert_eq!(
        next(at(1, 1), TraversalKey::SeriesStep(1)),
        None,
        "the last series"
    );
    assert_eq!(next(at(0, 1), left), None, "the first category");
    assert_eq!(
        next(at(0, 1), TraversalKey::Datum(DatumStep::Last)),
        at(2, 1)
    );
}

#[test]
fn the_cursor_bar_is_announced_and_outlined() {
    let selected = chart(at(1, 1));
    let semantics = selected.semantics();
    assert_eq!(semantics.value.as_deref(), Some("Rent, Feb: 7.5"));
    assert_eq!(
        semantics.set_position.map(|p| (p.index, p.count)),
        Some((4, 6)),
        "bar 4 of 6"
    );

    let renderer = renderer();
    let draw = |chart: EnzBarChart<Msg>| {
        let program = chart.program();
        <_ as Program<Msg>>::draw(
            &program,
            &Default::default(),
            &renderer,
            &Theme::Light,
            Rectangle::with_size(Size::new(480.0, 240.0)),
            mouse::Cursor::Unavailable,
        )
        .len()
    };
    assert_eq!(draw(chart(None)), 1);
    assert_eq!(draw(selected), 2, "the cursor adds an outline layer");
}

#[test]
fn a_focused_chart_publishes_arrow_moves() {
    let mut renderer = renderer();
    let view = || chart(at(0, 0)).into_element();
    let mut ui = UserInterface::build(
        view(),
        Size::new(600.0, 300.0),
        Cache::default(),
        &mut renderer,
    );
    // Tab onto the chart: `focus_next` counts, then focuses on the chained pass.
    let mut op = iced::advanced::widget::operation::focusable::focus_next::<()>();
    ui.operate(&renderer, &mut op as &mut dyn Operation<()>);
    let mut outcome = op.finish();
    while let Outcome::Chain(mut next) = outcome {
        ui.operate(&renderer, &mut next);
        outcome = next.finish();
    }
    let mut ui = UserInterface::build(
        view(),
        Size::new(600.0, 300.0),
        ui.into_cache(),
        &mut renderer,
    );
    let press = |key: Named| {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(key),
            modified_key: Key::Named(key),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    };
    let mut messages = iced::advanced::shell::Bus::new();
    let _ = ui.update(
        &iced::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[press(Named::ArrowRight), press(Named::ArrowDown)],
        mouse::Cursor::Available(Point::new(-10.0, -10.0)),
        &mut renderer,
        &mut messages,
    );
    let messages: Vec<Msg> = messages.into_iter().collect();
    assert_eq!(
        messages,
        [
            Msg::Selected(BarSelection {
                category: 1,
                series: 0
            }),
            Msg::Selected(BarSelection {
                category: 0,
                series: 1
            }),
        ]
    );
}
