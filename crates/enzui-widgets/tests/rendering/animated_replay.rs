//! `animated` children travel to where each view puts them on the render
//! lane: replayed frames draw what a full redraw draws, with no message from
//! the application while they move.

use enzui_frame_check::FrameCheck;
use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use enzui_widgets::card::enz_card;
use enzui_widgets::motion::animated;
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::sidebar_content::enz_sidebar_content;
use enzui_widgets::sidebar_row::enz_sidebar_row;
use enzui_widgets::text::enz_text;
use iced::widget::{Space, column, container, scrollable};
use iced::{Element, Length, Point, Size, Vector};

const SIZE: Size = Size::new(700.0, 440.0);

/// Where the blocks are: `0` at rest, `1` pushed left past the sidebar and
/// faded out, `2` pushed right.
fn view(place: u8) -> Element<'static, ()> {
    let block = move |i: usize| -> Element<'static, ()> {
        let (offset, opacity) = match place {
            1 => (Vector::new(-500.0 - 40.0 * i as f32, 0.0), 0.0),
            2 => (Vector::new(160.0, 12.0 * i as f32), 0.6),
            _ => (Vector::ZERO, 1.0),
        };
        let card = enz_card(
            container(enz_text(format!("Block {i}")))
                .padding(12)
                .width(180),
        );
        let moving = animated(card).offset(offset);
        let moving = if i.is_multiple_of(2) {
            moving.opacity(opacity)
        } else {
            moving
        };
        container(moving).height(60).into()
    };
    enz_app_shell(move || {
        container(
            scrollable(
                column![Space::new().height(330)]
                    .extend((0..5).map(block))
                    .push(Space::new().height(500))
                    .spacing(8)
                    .width(Length::Fill),
            )
            .height(Length::Fill),
        )
        .padding(24)
        .into()
    })
    .layout(ShellLayout::TitlebarSidebarContent)
    .state(NavSplitState::new())
    .on_sidebar_state(|_| ())
    .sidebar(|| {
        ["Inbox", "Drafts", "Sent", "Archive"]
            .into_iter()
            .fold(enz_sidebar_content(), |content, name| {
                content.push(enz_sidebar_row(name, ()))
            })
            .into()
    })
    .into()
}

#[test]
fn animated_children_travel_on_replay_and_match_a_full_redraw() {
    let Some(mut check) =
        FrameCheck::new(SIZE, enzui_theme::EnzTheme::dark().iced_theme(), || view(0))
    else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(4);
    check.scroll(Point::new(600.0, 300.0), 300.0);
    check.frames(30);
    let settled = check.changed();
    // Out past the sidebar, back in from there, across, and a reversal.
    for place in [1, 0, 2, 0] {
        check.view(move || view(place));
        check.frames(90);
    }
    check.view(|| view(1));
    check.frames(10);
    check.view(|| view(0));
    check.frames(90);

    check.assert_replays_match();
    assert!(
        check.replays() > 300,
        "only {} of {} frames were replayed",
        check.replays(),
        check.presented()
    );
    assert!(
        check.changed() - settled > 100,
        "the blocks moved on only {} frames: they jumped instead of travelling",
        check.changed() - settled
    );
}
