//! Clicks controls by their accessible names and returns the messages sent.
//!
//! Names are compared without the Unicode isolation marks Fluent puts around
//! the values it substitutes.

use enzui_core::collect::SemanticsCollector;
use iced::advanced::widget::Operation;
use iced::{Element, Event, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

/// The accessible names of the nodes `view` publishes, in tree order.
pub fn labels<'a, Message: 'a>(view: impl Fn() -> Element<'a, Message>, size: Size) -> Vec<String> {
    let mut renderer = super::honest_frame_driver::new_renderer();
    let mut ui = UserInterface::build(view(), size, Cache::default(), &mut renderer);
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    collector
        .into_collected()
        .into_iter()
        .filter_map(|node| node.semantics.label.as_deref().map(plain))
        .collect()
}

/// `label` without bidi isolation marks.
pub fn plain(label: &str) -> String {
    label.replace(['\u{2068}', '\u{2069}'], "")
}

/// Clicks the node named `label` in each fresh build of `view`, in order,
/// and returns every message the clicks sent.
pub fn click_labels<'a, Message: 'a>(
    view: impl Fn() -> Element<'a, Message>,
    size: Size,
    clicks: &[&str],
) -> Vec<Message> {
    let mut renderer = super::honest_frame_driver::new_renderer();
    let mut sent = Vec::new();
    for label in clicks {
        let mut ui = UserInterface::build(view(), size, Cache::default(), &mut renderer);
        let mut collector = SemanticsCollector::new();
        ui.operate(&renderer, &mut collector as &mut dyn Operation);
        let point = collector
            .into_collected()
            .into_iter()
            .find(|node| node.semantics.label.as_deref().map(plain).as_deref() == Some(*label))
            .unwrap_or_else(|| panic!("no control named {label:?}"))
            .bounds
            .center();
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[
                Event::Mouse(mouse::Event::CursorMoved { position: point }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            mouse::Cursor::Available(point),
            &mut renderer,
            &mut messages,
        );
        sent.extend(messages);
    }
    sent
}
