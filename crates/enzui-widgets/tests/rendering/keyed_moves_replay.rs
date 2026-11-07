//! A keyed column's moves travel on the render lane: when rows arrive at the
//! top of a list, leave it or change places, the rows they move slide there
//! on replayed frames that draw what a full redraw draws (enzUI-ICED #3667).

use enzui_frame_check::FrameCheck;
use enzui_widgets::card::enz_card;
use enzui_widgets::keyed_column::keyed_column;
use enzui_widgets::text::enz_text;
use iced::widget::{container, scrollable};
use iced::{Element, Length, Size};

const SIZE: Size = Size::new(420.0, 360.0);

fn view(keys: &'static [&'static str]) -> Element<'static, ()> {
    let rows = keys
        .iter()
        .map(|key| {
            let row: Element<'static, ()> = enz_card(
                container(enz_text(format!("Message {key}")))
                    .padding(10)
                    .width(Length::Fill),
            )
            .into();
            ((*key).to_owned(), row)
        })
        .collect();
    container(
        scrollable(
            keyed_column(rows)
                .spacing(8.0)
                .width(Length::Fill)
                .animate_moves(),
        )
        .height(Length::Fill),
    )
    .padding(16)
    .into()
}

#[test]
fn keyed_moves_travel_on_replay_and_match_a_full_redraw() {
    let Some(mut check) = FrameCheck::new(SIZE, enzui_theme::EnzTheme::dark().iced_theme(), || {
        view(&["c", "d", "e", "f", "g", "h"])
    }) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(10);
    let settled = check.changed();
    // An arrival at the top, another, a removal and a reorder.
    for keys in [
        &["b", "c", "d", "e", "f", "g", "h"][..],
        &["a", "b", "c", "d", "e", "f", "g", "h"][..],
        &["a", "b", "d", "e", "f", "g", "h"][..],
        &["h", "a", "b", "d", "e", "f", "g"][..],
    ] {
        check.view(move || view(keys));
        check.frames(60);
    }

    check.assert_replays_match();
    assert!(
        check.replays() > 150,
        "only {} of {} frames were replayed",
        check.replays(),
        check.presented()
    );
    assert!(
        check.changed() - settled > 60,
        "the rows moved on only {} frames: they jumped instead of sliding",
        check.changed() - settled
    );
}
