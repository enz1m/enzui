//! A spinner turns by track: a replay turns its painted dots, every replayed
//! frame draws what a full redraw draws, and the UI thread draws nothing
//! while it spins.

use enzui_frame_check::FrameCheck;
use enzui_widgets::enz_spinner;
use enzui_widgets::motion::animated;
use iced::widget::{column, container, row};
use iced::{Element, Size, Vector};

fn view(shifted: bool) -> Element<'static, ()> {
    container(
        column![
            row![
                enz_spinner().into_element(),
                enz_spinner().size(40.0).into_element(),
                enz_spinner().size(16.0).into_element(),
            ]
            .spacing(24),
            // A spinner inside a moving parent turns about where it is shown.
            animated(enz_spinner().size(32.0).into_element())
                .offset(Vector::new(if shifted { 120.0 } else { 0.0 }, 0.0)),
        ]
        .spacing(24),
    )
    .padding(24)
    .into()
}

#[test]
fn a_spinner_turns_on_replay_without_redrawing() {
    let Some(mut check) = FrameCheck::new(
        Size::new(320.0, 200.0),
        enzui_theme::EnzTheme::dark().iced_theme(),
        || view(false),
    ) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(10);
    let settling = check.content_frames();
    check.frames(150);
    let spinning = check.content_frames() - settling;
    // A new view is drawn once; the parent then carries the spinner along.
    check.view(|| view(true));
    check.frames(5);
    let moved = check.content_frames();
    check.frames(145);
    let carried = check.content_frames() - moved;
    check.assert_replays_match();
    assert_eq!(spinning, 0, "a still spinner asked for full redraws");
    // The parent draws once where it comes to rest, so the recording stops
    // needing its track; a node that stops publishing after a drawn frame is
    // drawn once more, since the runtime cannot tell resting where it was
    // drawn from moving since.
    assert!(
        carried <= 2,
        "a carried spinner asked for {carried} full redraws"
    );
    assert!(
        check.replays() > 250,
        "only {} of {} frames were replayed",
        check.replays(),
        check.presented()
    );
    assert!(
        check.changed() > 250,
        "the dots turned on only {} frames",
        check.changed()
    );
}
