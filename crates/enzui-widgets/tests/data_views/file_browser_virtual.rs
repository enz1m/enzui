//! A file browser lent a scroll handle builds only the rows near its viewport,
//! in list and grid mode; without one it builds every entry.

use enzui_core::collect::SemanticsCollector;
use enzui_core::file_browser::{FileBrowserModel, FileEntry};
use enzui_widgets::file_browser::{FileBrowserView, enz_file_browser};
use enzui_widgets::scroll_view::ScrollTranslation;
use iced::Size;
use iced::advanced::widget::Operation;
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

const ENTRIES: usize = 2_000;

fn model() -> &'static FileBrowserModel {
    let mut model = FileBrowserModel::new();
    model.set_entries(
        (0..ENTRIES)
            .map(|i| FileEntry::file(format!("/w/f{i:05}"), format!("f{i:05}"), 1))
            .collect(),
    );
    Box::leak(Box::new(model))
}

/// How many entry rows or tiles reach the accessibility tree.
fn built_entries(view: FileBrowserView, scroll: Option<ScrollTranslation>) -> usize {
    let mut browser = enz_file_browser::<()>(model()).view_mode(view);
    if let Some(scroll) = scroll {
        browser = browser.scroll(scroll);
    }
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut ui = UserInterface::build(
        browser.into_element(),
        Size::new(700.0, 400.0),
        Cache::default(),
        &mut renderer,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    collector
        .into_collected()
        .iter()
        .filter(|node| {
            node.semantics
                .label
                .as_deref()
                .is_some_and(|label| label.starts_with('f') && label.len() == 6)
        })
        .count()
}

#[test]
fn file_browser_bounds_realization_and_preserves_scroll() {
    // a_scroll_handle_bounds_the_list_to_its_viewport
    {
        let eager = built_entries(FileBrowserView::List, None);
        let bounded = built_entries(FileBrowserView::List, Some(ScrollTranslation::new()));
        assert_eq!(eager, ENTRIES, "without a handle every row is built");
        assert!(
            (1..60).contains(&bounded),
            "a 400 px list builds its visible rows and overscan: {bounded}"
        );
    }

    // scrolling_moves_the_built_window_and_survives_a_rebuild
    {
        let model = model();
        let scroll = ScrollTranslation::new();
        let view = || {
            enz_file_browser::<()>(model)
                .scroll(scroll.clone())
                .into_element()
        };
        let labels = |ui: &mut iced_test::Simulator<'_, ()>| {
            let mut collector = SemanticsCollector::new();
            ui.operate(&mut collector as &mut dyn Operation);
            collector
                .into_collected()
                .into_iter()
                .filter_map(|node| node.semantics.label)
                .filter(|label| label.starts_with('f') && label.len() == 6)
                .collect::<Vec<_>>()
        };
        let mut ui = iced_test::Simulator::with_size(
            iced::Settings::default(),
            Size::new(700.0, 400.0),
            view(),
        );
        ui.point_at(iced::Point::new(300.0, 200.0));
        let _ = ui.simulate([iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
            delta: iced::mouse::ScrollDelta::Pixels {
                x: 0.0,
                y: -3_000.0,
            },
        })]);
        let mut ui = ui.rebuild(view());
        let shown = labels(&mut ui);
        assert!(
            scroll.get().y > 2_000.0,
            "the handle holds the scrolled offset: {:?}",
            scroll.get()
        );
        assert!(
            !shown.iter().any(|label| label == "f00000")
                && shown.iter().any(|label| label == "f00100"),
            "the rebuilt list shows the rows at the offset: {shown:?}"
        );
    }
}
