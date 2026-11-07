//! The app shell follows how the desktop decorates its window.

use enzui_core::collect::collect_frame;
use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use iced::window::{Decorations, Id};
use iced::{Element, Size};

#[derive(Debug, Clone)]
enum Msg {
    Close,
    Minimize,
    Maximize,
}

/// Builds the shell as the view of a window the desktop reported `desktop`
/// for, and lists the labels of what it publishes with their centres.
fn placed_labels(desktop: Decorations) -> Vec<(String, f32)> {
    let id = Id::unique();
    iced::window::set_decorations(id, desktop);
    let shell: Element<'static, Msg> = iced::window::viewing(id, || {
        enz_app_shell(|| iced::widget::text("content").into())
            .layout(ShellLayout::TitlebarContent)
            .titlebar(|| iced::widget::text("Title").into())
            .on_close(Msg::Close)
            .on_minimize(Msg::Minimize)
            .on_maximize(Msg::Maximize, false)
            .into()
    });
    collect_frame(shell, Size::new(640.0, 400.0))
        .collected()
        .iter()
        .filter_map(|node| Some((node.semantics.label.clone()?, node.bounds.center_x())))
        .collect()
}

fn labels(desktop: Decorations) -> Vec<String> {
    placed_labels(desktop)
        .into_iter()
        .map(|(label, _)| label)
        .collect()
}

#[test]
fn window_frame_policy_follows_layout_and_desktop_capabilities() {
    // window_controls_give_way_to_server_side_decorations
    {
        let own = labels(Decorations::default());
        assert!(own.iter().any(|label| label == "Close"), "{own:?}");
        let server = labels(Decorations {
            server_side: true,
            ..Decorations::default()
        });
        assert!(!server.iter().any(|label| label == "Close"), "{server:?}");
    }

    // window_controls_follow_the_desktop_button_layout
    {
        use enzui_core::desktop_settings::{ButtonLayout, DesktopSettings, with_settings};
        let under = |layout: &str| {
            let settings = DesktopSettings {
                button_layout: Some(ButtonLayout::parse(layout)),
                ..DesktopSettings::default()
            };
            with_settings(settings, || placed_labels(Decorations::default()))
        };
        let x = |placed: &[(String, f32)], label: &str| {
            placed
                .iter()
                .find(|(l, _)| l == label)
                .unwrap_or_else(|| panic!("no {label:?} in {placed:?}"))
                .1
        };

        let gnome = under("appmenu:close");
        assert!(
            !gnome.iter().any(|(label, _)| label == "Minimize"),
            "{gnome:?}"
        );

        let mac = under("close,minimize,maximize:");
        let (close, minimize, maximize) =
            (x(&mac, "Close"), x(&mac, "Minimize"), x(&mac, "Maximize"));
        assert!(close < minimize && minimize < maximize, "{mac:?}");
        assert!(maximize < x(&mac, "Title"), "controls lead: {mac:?}");

        let kde = under(":minimize,maximize,close");
        assert!(x(&kde, "Minimize") > x(&kde, "Title"), "{kde:?}");
    }

    // a_layout_without_close_still_draws_one_close
    {
        use enzui_core::desktop_settings::{ButtonLayout, DesktopSettings, with_settings};
        for layout in ["appmenu:", ":", "minimize,maximize:"] {
            let settings = DesktopSettings {
                button_layout: Some(ButtonLayout::parse(layout)),
                ..DesktopSettings::default()
            };
            let placed = with_settings(settings, || placed_labels(Decorations::default()));
            let closes = placed.iter().filter(|(label, _)| label == "Close").count();
            assert_eq!(closes, 1, "{layout:?}: {placed:?}");
        }
    }

    // native_fullscreen_omits_shell_chrome_unless_explicitly_retained
    {
        use enzui_widgets::{enz_button, enz_text};
        let id = Id::unique();
        for (fullscreen, retain) in [(false, false), (true, false), (true, true), (false, false)] {
            iced::window::set_decorations(
                id,
                Decorations {
                    fullscreen,
                    ..Default::default()
                },
            );
            let controls: Element<'static, Msg> = iced::window::viewing(id, || {
                assert_eq!(
                    enzui_widgets::enz_title_bar::<Msg>().renders_controls(),
                    !fullscreen
                );
                enzui_widgets::enz_window_controls(Msg::Close).into()
            });
            let controls_frame = collect_frame(controls, Size::new(800.0, 600.0));
            assert_eq!(
                controls_frame
                    .collected()
                    .iter()
                    .any(|n| n.semantics.label.as_deref() == Some("Close")),
                !fullscreen
            );
            let shell: Element<'static, Msg> = iced::window::viewing(id, || {
                enzui_widgets::enz_app_shell_sized(|width| {
                    enz_button("Movie", Msg::Close).width(width).into()
                })
                .tokens(enzui_widgets::ShellTokens {
                    page_inset: 19.0,
                    ..Default::default()
                })
                .layout(ShellLayout::TitlebarSidebarContent)
                .titlebar(|| enz_text("Window title").into())
                .sidebar(|| enz_button("Navigation", Msg::Close).into())
                .footer(|| enz_text("Footer").into())
                .on_toggle_sidebar(Msg::Close)
                .fullscreen_chrome_visible(retain)
                .into()
            });
            let frame = collect_frame(shell, Size::new(800.0, 600.0));
            let labels: Vec<_> = frame
                .collected()
                .iter()
                .filter_map(|n| n.semantics.label.as_deref())
                .collect();
            assert!(labels.contains(&"Movie"));
            assert_eq!(
                labels.contains(&"Navigation"),
                !fullscreen || retain,
                "{labels:?}"
            );
            assert_eq!(
                labels.contains(&"Footer"),
                !fullscreen || retain,
                "{labels:?}"
            );
            if fullscreen && !retain {
                let main = frame
                    .collected()
                    .iter()
                    .find(|n| n.semantics.role == enzui_core::semantics::Role::Main)
                    .expect("main landmark");
                assert_eq!(
                    main.bounds,
                    iced::Rectangle::with_size(Size::new(800.0, 600.0))
                );
                let movie = frame
                    .collected()
                    .iter()
                    .find(|n| n.semantics.label.as_deref() == Some("Movie"))
                    .unwrap();
                assert_eq!(movie.bounds.x, 0.0);
                assert_eq!(movie.bounds.y, 0.0);
                assert_eq!(movie.bounds.width, 800.0);
            }
        }
    }
}
