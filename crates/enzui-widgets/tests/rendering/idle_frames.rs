//! A page of settled widgets asks for no frames that redraw it. Anything that
//! keeps moving does so by track, which the render lane presents without
//! the UI thread: here a breathing and a rippling status dot.
//!
//! A determinate progress bar once asked for a full redraw on every frame for
//! its sweep, so a page with one redrew at the display rate forever.

use enzui_frame_check::FrameCheck;
use enzui_widgets::StatusDotMotion;
use enzui_widgets::status_dot::StatusKind;
use enzui_widgets::{
    enz_avatar, enz_badge, enz_card, enz_checkbox, enz_gauge, enz_progress_bar, enz_slider,
    enz_status_dot, enz_text_field, enz_toggle,
};
use iced::widget::{column, container, row};
use iced::{Element, Size};

fn view() -> Element<'static, ()> {
    page(StatusDotMotion::Breathe, StatusDotMotion::Ripple)
}

fn page(breathing: StatusDotMotion, rippling: StatusDotMotion) -> Element<'static, ()> {
    container(enz_card(
        column![
            row![
                enz_avatar("Ada Lovelace").into_element(),
                enz_badge("3 new").into_element(),
                enz_status_dot(StatusKind::Warning).into_element(),
                enz_status_dot(StatusKind::Running)
                    .motion(breathing)
                    .into_element(),
                enz_status_dot(StatusKind::Error)
                    .motion(rippling)
                    .into_element(),
            ]
            .spacing(12),
            enz_progress_bar(0.45).into_element(),
            enz_gauge(0.6).into_element(),
            enzui_widgets::sparkline::enz_sparkline(vec![3.0_f32, 5.0, 4.0, 8.0, 6.0])
                .label("Load"),
            enz_toggle(true, "Autoscale", |_| ()).into_element(),
            enz_checkbox(true, "Notify me", |_| ()).into_element(),
            enz_slider(0.0..=1.0, 0.3, |_| ()).into_element(),
            enz_text_field("Note", "Type here", "", |_| ()).into_element(),
        ]
        .spacing(12)
        .width(320),
    ))
    .padding(16)
    .into()
}

#[test]
fn a_settled_page_asks_for_no_redraws() {
    let Some(mut check) = FrameCheck::new(
        Size::new(380.0, 520.0),
        enzui_theme::EnzTheme::dark().iced_theme(),
        view,
    ) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(30);
    let settling = check.content_frames();
    check.frames(300);
    assert_eq!(
        check.content_frames() - settling,
        0,
        "a settled page asked for full redraws"
    );
    check.assert_replays_match();
}

#[test]
fn moving_status_dots_wake_the_ui_thread_no_more_than_still_ones() {
    let requests = |motion: [StatusDotMotion; 2]| {
        let mut check = FrameCheck::new(
            Size::new(380.0, 520.0),
            enzui_theme::EnzTheme::dark().iced_theme(),
            move || page(motion[0], motion[1]),
        )?;
        check.frames(30);
        let settling = check.frame_requests();
        check.frames(300);
        Some(check.frame_requests() - settling)
    };
    let Some(still) = requests([StatusDotMotion::Still; 2]) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    let moving = requests([StatusDotMotion::Breathe, StatusDotMotion::Ripple])
        .expect("an adapter for the first page serves the second");
    assert_eq!(
        moving, still,
        "the render lane plays the dots' motion; the UI thread wakes only between stretches"
    );
}
