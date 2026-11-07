use super::{AppProcess, PrivateWayland, app_command};
use iced::{Color, Element, Task, theme, widget::space};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// `wl_shm` format codes as the protocol log prints them.
const ARGB8888: &str = "0";

/// The software renderer's window buffers carry alpha, so a transparent window
/// shows what lies beneath it instead of black.
pub(super) fn transparent_window_buffers_carry_alpha() {
    let display = PrivateWayland::start_for_activation();
    let log_path = display.runtime.path().join("software-alpha.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = app_command(
        "software-alpha",
        &display.bus.address,
        display.runtime.path(),
    );
    command
        .env("WAYLAND_DISPLAY", &display.socket)
        .env("WAYLAND_DEBUG", "client")
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY");
    let _app = AppProcess::spawn_with_stderr(command, "software alpha", Stdio::from(log));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let protocol = std::fs::read_to_string(&log_path).unwrap();
        // Cursor images are small; the window's buffers span the tiled window.
        let formats: Vec<&str> = protocol
            .lines()
            .filter(|line| line.contains(" -> wl_shm_pool#") && line.contains(".create_buffer("))
            .filter_map(|line| {
                let args = line.split_once('(')?.1.trim_end_matches(')');
                let args: Vec<&str> = args.split(", ").collect();
                let width: u32 = args.get(2)?.parse().ok()?;
                (width >= 100).then_some(*args.get(5)?)
            })
            .collect();
        if !formats.is_empty() {
            assert!(
                formats.iter().all(|format| *format == ARGB8888),
                "window buffers must be argb8888, got formats {formats:?}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the software window never created a buffer: {protocol}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Transparent;

impl Transparent {
    fn view(&self) -> Element<'_, ()> {
        space().into()
    }
}

pub(super) fn application() -> ! {
    enzui_widgets::app_setup::run(
        iced::application(
            || (Transparent, Task::none()),
            |_: &mut Transparent, ()| Task::none(),
            Transparent::view,
        )
        .transparent(true)
        .style(|_, theme| theme::Style {
            background_color: Color::TRANSPARENT,
            text_color: theme.palette().background.base.text,
        })
        .window_size((320.0, 240.0)),
    )
    .unwrap();
    std::process::exit(0)
}
