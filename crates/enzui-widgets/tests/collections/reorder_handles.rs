//! Composite reorder-list rows dragged by a handle: the header drags the
//! group, its nested rows keep their clicks and start nothing, on the
//! standalone and the hosted path.

use enzui_core::identity::derive_id;
use enzui_widgets::button::enz_button;
use enzui_widgets::drag_drop_host::{DragHostChannel, enz_drag_drop_host};
use enzui_widgets::reorder_list::{enz_reorder_list, reorder_handle};
use iced::widget::{column, text};
use iced::{Element, Event, Point, Size, mouse};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Drop,
    Header(usize),
    Thread(usize),
}

fn id(name: &str) -> iced::advanced::widget::Id {
    derive_id("handles", name)
}

fn groups(hosted: Option<&DragHostChannel>) -> Element<'_, Msg> {
    let list = enz_reorder_list(
        (0..3).map(|group| {
            let body: Element<'_, Msg> = column![
                reorder_handle(
                    enz_button(format!("Workspace {group}"), Msg::Header(group))
                        .id(id(&format!("header{group}")))
                        .into_element()
                ),
                enz_button(format!("Thread {group}"), Msg::Thread(group))
                    .id(id(&format!("thread{group}")))
                    .into_element(),
                text("3 threads"),
            ]
            .spacing(4)
            .into();
            (format!("group{group}"), body)
        }),
        |_| Msg::Drop,
    )
    .drag_handles(true)
    .spacing(8.0);
    match hosted {
        Some(channel) => enz_drag_drop_host(channel, list.drag_host(channel))
            .on_drop(|_| Msg::Drop)
            .into(),
        None => list.into(),
    }
}

fn drag(sim: &mut Simulator<'_, Msg>, from: Point, to: Point) {
    sim.point_at(from);
    let _ = sim.simulate([Event::Mouse(mouse::Event::CursorMoved { position: from })]);
    let _ = sim.simulate([Event::Mouse(mouse::Event::ButtonPressed(
        mouse::Button::Left,
    ))]);
    for step in 1..=10 {
        let at = Point::new(
            from.x + (to.x - from.x) * step as f32 / 10.0,
            from.y + (to.y - from.y) * step as f32 / 10.0,
        );
        sim.point_at(at);
        let _ = sim.simulate([Event::Mouse(mouse::Event::CursorMoved { position: at })]);
    }
    let _ = sim.simulate([Event::Mouse(mouse::Event::ButtonReleased(
        mouse::Button::Left,
    ))]);
}

fn run(hosted: bool) {
    let channel = DragHostChannel::new();
    let host = hosted.then_some(&channel);
    let size = Size::new(400.0, 400.0);
    let mut sim = Simulator::with_size(iced_test::core::Settings::default(), size, groups(host));
    let header0 = sim.find(id("header0")).unwrap().bounds();
    let thread0 = sim.find(id("thread0")).unwrap().bounds();
    let last = sim.find(id("thread2")).unwrap().bounds();
    let below = Point::new(header0.center_x(), last.y + last.height + 4.0);

    // The nested row: its click is its own, and dragging it moves nothing.
    sim.click(iced_test::selector::id(id("thread0"))).unwrap();
    drag(&mut sim, thread0.center(), below);
    // A label in the group outside the handle drags nothing either.
    let label = Point::new(thread0.x + 4.0, thread0.y + thread0.height + 12.0);
    drag(&mut sim, label, below);
    // The header: a click is the header's, a drag moves the group.
    sim.click(iced_test::selector::id(id("header0"))).unwrap();
    drag(&mut sim, header0.center(), below);

    assert_eq!(
        sim.into_messages().collect::<Vec<_>>(),
        [Msg::Thread(0), Msg::Header(0), Msg::Drop],
        "hosted={hosted}"
    );
}

#[test]
fn reorder_requires_a_handle_with_and_without_a_drag_host() {
    // only_the_handle_drags_a_composite_row
    {
        run(false);
    }

    // only_the_handle_drags_a_composite_row_through_a_drag_host
    {
        run(true);
    }
}
