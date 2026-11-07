//! A busy progress bar's sweep runs on replay: the band moves by track, every
//! replayed frame draws what a full redraw draws, and between passes the bar
//! asks for no frames. A bar that shows a level does not sweep at all.

use enzui_frame_check::FrameCheck;
use enzui_widgets::enz_progress_bar;
use iced::widget::{column, container};
use iced::{Element, Size};

/// Bars at `value`, still or sweeping.
fn bars(value: f32) -> Element<'static, ()> {
    container(
        column![
            enz_progress_bar(value).busy(true).into_element(),
            enz_progress_bar(1.0 - value).busy(true).into_element(),
            enz_progress_bar(value)
                .busy(true)
                .bar_height(14.0)
                .into_element(),
        ]
        .spacing(16)
        .width(360),
    )
    .padding(24)
    .into()
}

#[test]
fn a_value_change_glides_on_replay() {
    let mut theme = enzui_theme::EnzTheme::dark();
    theme.motion.reduced_motion = false;
    let Some(mut check) =
        FrameCheck::new(Size::new(420.0, 160.0), theme.iced_theme(), || bars(0.1))
    else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(10);
    let mut glides = 0;
    for value in [0.35_f32, 0.8, 0.5, 0.95, 0.05] {
        check.view(move || bars(value));
        let before = check.content_frames();
        // A new value is drawn once; its 180 ms glide runs on replay.
        check.frames(40);
        glides += check.content_frames() - before;
    }
    check.assert_replays_match();
    assert!(
        glides <= 5 * 2,
        "glides asked for {glides} full redraws over five value changes"
    );
    assert!(
        check.changed() > 100,
        "the bars moved on only {} frames",
        check.changed()
    );
}

/// A bar that shows a level (usage, how much was read) stays still at rest:
/// after its value lands, no frame changes a pixel.
///
/// Sabotage: sweep every bar between empty and full, busy or not -- the
/// band keeps moving and frames keep changing.
#[test]
fn a_level_bar_at_rest_changes_no_frames() {
    let mut theme = enzui_theme::EnzTheme::dark();
    theme.motion.reduced_motion = false;
    let levels = || -> Element<'static, ()> {
        container(
            column![
                enz_progress_bar(0.45).into_element(),
                enz_progress_bar(0.7).bar_height(14.0).into_element(),
            ]
            .spacing(16)
            .width(360),
        )
        .padding(24)
        .into()
    };
    let Some(mut check) = FrameCheck::new(Size::new(420.0, 120.0), theme.iced_theme(), levels)
    else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(30);
    let settled = check.changed();
    check.frames(400);
    assert_eq!(
        check.changed() - settled,
        0,
        "a level bar kept changing pixels at rest"
    );
}

/// A level bar's glide to a new value is handed to the render lane whole:
/// the UI thread draws the change and wakes again where the glide lands,
/// not on every frame of it.
///
/// Sabotage: publish only each frame's sampled track, not the travel -- the
/// lane has nothing to play and every moving frame needs the UI thread.
#[test]
fn a_glide_asks_the_ui_thread_for_no_frame_per_frame() {
    let mut theme = enzui_theme::EnzTheme::dark();
    theme.motion.reduced_motion = false;
    let level = |value: f32| -> Element<'static, ()> {
        container(enz_progress_bar(value).into_element())
            .padding(24)
            .width(420)
            .into()
    };
    let Some(mut check) = FrameCheck::new(Size::new(420.0, 80.0), theme.iced_theme(), move || {
        level(0.1)
    }) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(30);
    let mut requests = 0;
    let mut moved = 0;
    let uncarried = check.uncarried_tracks();
    for value in [0.35_f32, 0.8, 0.5, 0.95, 0.05] {
        check.view(move || level(value));
        let (before, changed) = (check.frame_requests(), check.changed());
        check.frames(60);
        requests += check.frame_requests() - before;
        moved += check.changed() - changed;
    }
    check.assert_replays_match();
    assert!(moved > 5 * 10, "the glides moved on only {moved} frames");
    assert!(
        requests <= 5 * 4,
        "five glides asked the UI thread for {requests} frames"
    );
    assert_eq!(
        check.uncarried_tracks() - uncarried,
        0,
        "a glide moved a node with no travel the render lane could play"
    );
}

/// A new value the fill's edge would cross in under two pixels lands at
/// once: a glide that short moves the edge a fraction of a pixel per frame
/// and keeps the render lane busy for nothing. A meter fed a reading every
/// second mostly takes such steps.
///
/// Sabotage: glide every change, however small -- each step changes the
/// picture on dozens of frames.
#[test]
fn a_step_under_two_pixels_lands_at_once() {
    let mut theme = enzui_theme::EnzTheme::dark();
    theme.motion.reduced_motion = false;
    let level = |value: f32| -> Element<'static, ()> {
        container(enz_progress_bar(value).into_element())
            .padding(24)
            .width(420)
            .into()
    };
    let Some(mut check) = FrameCheck::new(Size::new(420.0, 80.0), theme.iced_theme(), move || {
        level(0.5)
    }) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(30);
    // 0.4 % of a 372 px bar is 1.5 px.
    for value in [0.504_f32, 0.508, 0.512] {
        check.view(move || level(value));
        let changed = check.changed();
        check.frames(60);
        assert!(
            check.changed() - changed <= 1,
            "a step of 1.5 px changed {} frames",
            check.changed() - changed
        );
    }
    check.assert_replays_match();
}

#[test]
fn progress_text_keeps_its_moving_ink_on_glass() {
    use enzui_widgets::{frame_clock::frame_clock_phase, progress_text::enz_progress_text};
    use iced::advanced::renderer::Headless as _;
    use iced_test::runtime::user_interface::{Cache, UserInterface};
    let mut renderer = iced_test::futures::futures::executor::block_on(iced::Renderer::new(
        Default::default(),
        Some("wgpu"),
    ))
    .expect("GPU renderer for optical-material qualification");
    for (name, theme) in [
        ("light", enzui_theme::EnzTheme::light().iced_theme()),
        ("dark", enzui_theme::EnzTheme::dark().iced_theme()),
        ("stock", iced::Theme::Light),
    ] {
        for glass in [false, true] {
            let mut cache = Cache::default();
            let mut capture = |at: f32| {
                let phase = frame_clock_phase();
                phase.set(at);
                let mut pixels = Vec::new();
                for _ in 0..4 {
                    let label = enz_progress_text("Avery is typing…")
                        .size(enzui_widgets::text::TextSize::Headline)
                        .phase(std::rc::Rc::clone(&phase))
                        .reduced_motion(false)
                        .into_element();
                    let label: Element<'_, ()> = if glass {
                        enzui_widgets::surface::enz_surface(label)
                            .material(enzui_theme::MaterialStyle::FlatFrosted)
                            .padding(8)
                            .into_element()
                    } else {
                        container(label).padding(8).into()
                    };
                    let view: Element<'_, ()> = container(label).padding(16).into();
                    let mut ui = UserInterface::build(
                        view,
                        Size::new(280.0, 80.0),
                        std::mem::take(&mut cache),
                        &mut renderer,
                    );
                    ui.draw(
                        &mut renderer,
                        &theme,
                        &iced::advanced::renderer::Style {
                            text_color: iced::Color::BLACK,
                        },
                        iced::mouse::Cursor::Unavailable,
                    );
                    cache = ui.into_cache();
                    let base =
                        <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
                    pixels = renderer.screenshot(Size::new(280, 80), 1.0, base.background_color);
                }
                pixels
            };
            let a = capture(0.15);
            let zero = capture(0.15);
            let b = capture(0.75);
            let changed = |left: &[u8], right: &[u8]| {
                (24..52)
                    .flat_map(|y| (24..256).map(move |x| (y * 280 + x) * 4))
                    .filter(|&i| (0..3).any(|c| left[i + c].abs_diff(right[i + c]) > 3))
                    .count()
            };
            assert!(
                changed(&a, &zero) <= 2,
                "same phase must remain still on {name}, glass={glass}"
            );
            assert!(
                changed(&a, &b) > 20,
                "moving ink disappeared on {name}, glass={glass}: {} changed pixels",
                changed(&a, &b)
            );
        }
    }
}
