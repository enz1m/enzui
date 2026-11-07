//! Loading placeholders and loading avatars pulse by track: the compositor fades their painted
//! bars, every replayed frame draws what a full redraw draws, and the UI
//! thread draws nothing while they pulse.

use enzui_frame_check::FrameCheck;
use enzui_widgets::skeleton::{EnzSkeleton, SkeletonKind, SkeletonTokens, enz_skeleton};
use iced::widget::{column, container, row};
use iced::{Color, Element, Length, Size};

/// An opaque white fill pulsing from nothing to full, so each frame of the
/// pulse moves pixels by more than FrameCheck's one-level threshold.
fn bright(kind: SkeletonKind) -> EnzSkeleton {
    enz_skeleton(kind).tokens(SkeletonTokens {
        base_color: Color::WHITE,
        idle_opacity: 0.0,
        loading_opacity: 1.0,
        ..SkeletonTokens::dark()
    })
}

fn view() -> Element<'static, ()> {
    container(
        column![
            row![
                container(bright(SkeletonKind::Circle)).width(48),
                container(bright(SkeletonKind::Text { lines: 3 })).width(Length::Fill),
            ]
            .spacing(16),
            bright(SkeletonKind::Card).into_element(),
            // A loading avatar pulses its sheen the same way.
            enzui_widgets::enz_avatar("Ada Lovelace")
                .loading()
                .into_element(),
        ]
        .spacing(16),
    )
    .padding(24)
    .into()
}

#[test]
fn placeholders_pulse_on_replay_without_redrawing() {
    let Some(mut check) = FrameCheck::new(
        Size::new(360.0, 320.0),
        {
            let mut theme = enzui_theme::EnzTheme::dark();
            theme.motion.reduced_motion = false;
            theme.iced_theme()
        },
        view,
    ) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(10);
    let settling = check.content_frames();
    check.frames(200);
    let pulsing = check.content_frames() - settling;
    check.assert_replays_match();
    assert_eq!(pulsing, 0, "pulsing asked for {pulsing} full redraws");
    assert!(
        check.replays() > 180,
        "only {} of {} frames were replayed",
        check.replays(),
        check.presented()
    );
    assert!(
        check.changed() > 150,
        "the placeholders faded on only {} frames",
        check.changed()
    );
}
