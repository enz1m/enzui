use super::*;
use crate::reorder_list::enz_reorder_list;
use iced::widget::{row, text};

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Drop(DropRequest),
    ItemDrop(ItemDropRequest),
}

// Distinct ids and lengths distinguish swapped collections and wrong insertion slots.
struct Fixture {
    channel: DragHostChannel,
}

impl Fixture {
    fn new() -> Self {
        Self {
            channel: DragHostChannel::new(),
        }
    }

    fn ui(&self, left_accepting: PayloadKinds, right_accepting: PayloadKinds) -> Element<'_, Msg> {
        let left: Element<'_, Msg> = enz_reorder_list(
            vec![
                ("i-one", text("one").into()),
                ("i-two", text("two").into()),
                ("i-three", text("three").into()),
                ("i-four", text("four").into()),
            ],
            Msg::Drop,
        )
        .collection_id("inbox")
        .accepting(left_accepting)
        .drag_host(&self.channel)
        .into();

        let right: Element<'_, Msg> = enz_reorder_list(
            vec![
                ("a-alpha", text("alpha").into()),
                ("a-beta", text("beta").into()),
                ("a-gamma", text("gamma").into()),
            ],
            Msg::Drop,
        )
        .collection_id("archive")
        .accepting(right_accepting)
        .drag_host(&self.channel)
        .into();

        enz_drag_drop_host(&self.channel, row![left, right])
            .on_drop(Msg::Drop)
            .deadband(4.0)
            .into()
    }
}

fn drops(msgs: &[Msg]) -> Vec<&DropRequest> {
    msgs.iter()
        .filter_map(|message| match message {
            Msg::Drop(request) => Some(request),
            Msg::ItemDrop(_) => None,
        })
        .collect()
}

#[test]
fn escape_cancels_the_host_session() {
    let f = Fixture::new();
    let el = f.ui(PayloadKinds::universal(), PayloadKinds::universal());
    let mut ui = iced_test::Simulator::new(el);
    ui.point_at(Point::new(5000.0, 5000.0));
    ui.point_at(Point::new(100.0, 30.0));
    let _ = ui.simulate([Event::Mouse(mouse::Event::ButtonPressed(
        mouse::Button::Left,
    ))]);
    ui.point_at(Point::new(700.0, 45.0));
    let _ = ui.simulate([Event::Mouse(mouse::Event::CursorMoved {
        position: Point::new(700.0, 45.0),
    })]);
    let _ = ui.simulate([iced_test::simulator::press_key(
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
        None,
    )]);
    let _ = ui.simulate([Event::Mouse(mouse::Event::ButtonReleased(
        mouse::Button::Left,
    ))]);
    let msgs: Vec<Msg> = ui.into_messages().collect();
    assert!(
        drops(&msgs).is_empty(),
        "Escape must cancel the host session, got {msgs:?}"
    );
}
