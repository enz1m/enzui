//! Reveal effects on the render lane draw what a full redraw draws, frame by
//! frame, inside the structure an application gives them: an app shell with
//! a sidebar, a scrolled page, and a row of panels on a curve, an expressive
//! curve and a spring. Every presented frame of the whole window is compared,
//! so ink leaking over the sidebar counts as much as a missing panel.

use enzui_frame_check::FrameCheck;
use enzui_theme::ExpressiveMotion;
use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use enzui_widgets::card::enz_card;
use enzui_widgets::motion::{MotionEffect, SlideDirection, motion};
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::sidebar_content::enz_sidebar_content;
use enzui_widgets::sidebar_row::enz_sidebar_row;
use enzui_widgets::text::enz_text;
use iced::widget::{Space, column, container, row, scrollable};
use iced::{Element, Length, Point, Size};

const SIZE: Size = Size::new(700.0, 440.0);
const PAGES: [&str; 8] = [
    "Buttons",
    "Inputs",
    "Pickers",
    "Navigation",
    "Feedback",
    "Charts",
    "Tables",
    "Motion",
];

fn panel(label: &'static str) -> Element<'static, ()> {
    enz_card(container(enz_text(label)).padding(12).width(150)).into()
}

fn reveal(
    label: &'static str,
    effect: MotionEffect,
    curve: u8,
    shown: bool,
) -> Element<'static, ()> {
    let wrapped = motion(panel(label), shown, effect);
    let wrapped = match curve {
        1 => wrapped.expressive(Some(ExpressiveMotion::DefaultSpatial)),
        2 => wrapped.spring(Some(ExpressiveMotion::FastSpatial.spring())),
        _ => wrapped,
    };
    container(wrapped).width(170).height(64).into()
}

fn page(shown: bool) -> Element<'static, ()> {
    let left = MotionEffect::Slide(SlideDirection::Left);
    column![
        Space::new().height(420),
        enz_text("Reveal effects"),
        row![
            reveal("Slide", left, 0, shown),
            reveal("Slide", left, 1, shown),
            reveal("Slide", left, 2, shown),
        ]
        .spacing(16),
        row![
            reveal("Fade", MotionEffect::Fade, 2, shown),
            reveal("Scale", MotionEffect::Scale, 2, shown),
            reveal(
                "Slide up",
                MotionEffect::Slide(SlideDirection::Down),
                2,
                shown
            ),
        ]
        .spacing(16),
        Space::new().height(600),
    ]
    .spacing(12)
    .into()
}

fn view(shown: bool) -> Element<'static, ()> {
    enz_app_shell(move || {
        container(
            scrollable(container(page(shown)).width(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .padding(24)
        .into()
    })
    .layout(ShellLayout::TitlebarSidebarContent)
    .state(NavSplitState::new())
    .on_sidebar_state(|_| ())
    .sidebar(|| {
        PAGES
            .into_iter()
            .fold(enz_sidebar_content(), |content, name| {
                content.push(enz_sidebar_row(name, ()).selected(name == "Motion"))
            })
            .into()
    })
    .into()
}

#[test]
fn reveal_panels_replay_what_a_full_redraw_draws() {
    let Some(mut check) = FrameCheck::new(SIZE, enzui_theme::EnzTheme::dark().iced_theme(), || {
        view(true)
    }) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(4);
    // Scrolled so the panels sit mid-window, as on the gallery page.
    check.scroll(Point::new(600.0, 300.0), 300.0);
    check.frames(60);
    for shown in [false, true, false, true] {
        check.view(move || view(shown));
        check.frames(110);
    }
    // A reversal mid-travel.
    check.view(|| view(false));
    check.frames(12);
    check.view(|| view(true));
    check.frames(110);

    check.assert_replays_match();
    assert!(
        check.replays() > 200,
        "only {} frames were replayed; the reveals do not reach the lane",
        check.replays()
    );
}
