//! A page hosted through `confirm_dialog_host` keeps its widget state as a
//! dialog opens over it and closes: a scrolled page stays where it was
//! scrolled to.

use enzui_core::confirm::ConfirmState;
use enzui_widgets::confirm_dialog::{ConfirmDialogEvent, enz_confirm_dialog};
use enzui_widgets::confirm_dialog_host;
use enzui_widgets::scroll_view::{QueryScrollGeometry, enz_scroll_view};
use iced::advanced::widget::Id;
use iced::widget::{column, text};
use iced::{Element, Event, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const SIZE: Size = Size::new(400.0, 300.0);

fn page_id() -> Id {
    Id::new("dialog-host-page")
}

fn view(state: Option<&ConfirmState>) -> Element<'_, ConfirmDialogEvent> {
    let mut rows = column![];
    for i in 0..80 {
        rows = rows.push(text(format!("row {i}")));
    }
    let page = enz_scroll_view(rows).id(page_id());
    let dialog = state.map(|state| enz_confirm_dialog("Delete?", "It goes.", state, |e| e));
    confirm_dialog_host(page, dialog, |e| e)
}

fn offset(
    ui: &mut UserInterface<'_, ConfirmDialogEvent, iced::Theme, iced::Renderer>,
    renderer: &iced::Renderer,
) -> f32 {
    let mut query = QueryScrollGeometry::new(page_id());
    ui.operate(renderer, &mut query);
    -query.result().expect("the page scroll view").translation.y
}

#[test]
fn opening_a_dialog_keeps_the_page_scrolled() {
    use iced::advanced::renderer::Headless;
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as Headless>::new(iced::advanced::renderer::Settings::default(), None),
    )
    .expect("headless renderer");

    let mut ui = UserInterface::build(view(None), SIZE, Cache::default(), &mut renderer);
    let cursor = mouse::Cursor::Available(iced::Point::new(100.0, 100.0));
    let _ = ui.update(
        &iced::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -200.0 },
        })],
        cursor,
        &mut renderer,
        &mut iced::advanced::shell::Bus::new(),
    );
    let scrolled = offset(&mut ui, &renderer);
    assert!(
        scrolled.abs() > 50.0,
        "precondition: the page scrolled ({scrolled})"
    );
    let cache = ui.into_cache();

    // The open host withdraws the page from operations, so look again once
    // the dialog has closed: a page re-mounted by either change is back at
    // the top.
    let state = ConfirmState::new(None);
    let ui = UserInterface::build(view(Some(&state)), SIZE, cache, &mut renderer);
    let cache = ui.into_cache();
    let mut ui = UserInterface::build(view(None), SIZE, cache, &mut renderer);
    let after = offset(&mut ui, &renderer);
    assert_eq!(
        after, scrolled,
        "the page keeps its scroll offset through the dialog"
    );
}
