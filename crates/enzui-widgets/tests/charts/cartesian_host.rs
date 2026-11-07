//! Line, area and scatter share one Cartesian host/axis path.
//!
//! The compatibility presets remain public, but their static draw calls now
//! adapt into `cartesian_chart::draw_static`. These gates exercise the real
//! Canvas path rather than inspecting builder fields.

use enzui_core::chart::{DatumStep, Series, SeriesId};
use enzui_widgets::cartesian_chart::{
    CartesianMark, CartesianProgram, CartesianState, PointMark, enz_cartesian_chart,
};
use enzui_widgets::line_chart::{DatumSelection, TraversalKey};
use iced::advanced::renderer::Headless;
use iced::widget::canvas::Program;
use iced::{Rectangle, Size, Theme, mouse};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Selected,
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(<Renderer as Headless>::new(
        iced::advanced::renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer")
}

fn fixture() -> Series {
    Series::from_points(
        SeriesId(7),
        [(0.0, -3.0), (2.0, 5.0), (7.0, 8.0), (9.0, 2.0)],
    )
}

#[test]
fn common_point_marks_expose_selection_traversal_semantics_and_paint() {
    let series = fixture();
    let first = series.points[0].key;
    let second = series.points[1].key;
    let chart = enz_cartesian_chart::<Msg>("interactive")
        .mark(CartesianMark::Points(PointMark::default()))
        .selection(Some(DatumSelection::new(SeriesId(7), first)))
        .push(series);

    let semantics = chart.semantics();
    assert_eq!(
        semantics.selected,
        Some(true),
        "selected datum must reach semantics"
    );
    assert_eq!(semantics.value.as_deref(), Some("0, -3"));
    assert_eq!(
        chart.traverse(TraversalKey::Datum(DatumStep::Next)),
        Some(DatumSelection::new(SeriesId(7), second))
    );

    let renderer = renderer();
    let state = CartesianState::default();
    let geometry = <CartesianProgram as Program<Msg>>::draw(
        &chart.program(),
        &state,
        &renderer,
        &Theme::Light,
        Rectangle::with_size(Size::new(480.0, 240.0)),
        mouse::Cursor::Unavailable,
    );
    assert_eq!(
        geometry.len(),
        2,
        "selection must add a real uncached paint layer"
    );
}

// ---------------------------------------------------------------------------
// Modifier chord policy at the Cartesian route
// ---------------------------------------------------------------------------
//
// `EnzCartesianChart::into_element` is the second consumer of
// `line_chart::event_for_key`. The shared decoder rejects Ctrl/Alt/Logo
// (with or without Shift) so those application chords keep bubbling, and
// these tests drive real `Event::Keyboard`s through the built Cartesian
// element to prove the routing honours it.
//
// A minimal `UserInterface` harness lives here rather than being shared
// with `line_chart_keyboard.rs`: cross-file harnesses in this integration
// binary would require another `#[path = ...]` include next to the shared
// probes in `main.rs`, which the derived matrix does not need.
