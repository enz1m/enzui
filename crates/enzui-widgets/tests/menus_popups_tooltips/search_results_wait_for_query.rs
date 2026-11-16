//! A titlebar search presents its results once a query is typed, not on a
//! bare focus: an empty field would otherwise open on a "no matches" notice
//! before anything was typed. `results_open(true)` still presents results
//! (recent searches) for an empty query.

use enzui_widgets::search_field::enz_titlebar_search;
use iced::{Element, Point};
use iced_test::Simulator;
use iced_test::simulator::click;

const MARKER: &str = "RESULT-MARKER";

fn search(query: &'static str, open: Option<bool>) -> Element<'static, String> {
    let mut field = enz_titlebar_search(query, |q| q)
        .placeholder("Search")
        .results(iced::widget::text(MARKER));
    if let Some(open) = open {
        field = field.results_open(open);
    }
    iced::widget::container(field.into_element())
        .padding(8)
        .width(600)
        .into()
}

/// Whether the results show after focusing the field with a click.
fn shown_after_focus(query: &'static str, open: Option<bool>) -> bool {
    let mut ui = Simulator::with_size(
        iced::Settings::default(),
        (800.0, 400.0),
        search(query, open),
    );
    ui.point_at(Point::new(200.0, 22.0));
    let _ = ui.simulate(click());
    ui.find(MARKER).is_ok()
}

#[test]
fn an_empty_query_shows_no_results() {
    assert!(!shown_after_focus("", None));
    assert!(!shown_after_focus("   ", None));
}

#[test]
fn a_typed_query_shows_results() {
    assert!(shown_after_focus("e", None));
}

#[test]
fn results_open_presents_results_for_an_empty_query() {
    assert!(shown_after_focus("", Some(true)));
}
