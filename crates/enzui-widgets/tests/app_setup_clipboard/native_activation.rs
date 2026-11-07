use super::{AppProcess, OwnedChild, PrivateWayland, PrivateX11, app_command, report};
use enzui_core::activation::{
    ActivateWindowError, ActivationBroker, ActivationPlatform, ActivationResponse,
    InboundActivationToken, NativeActivationError,
};
use iced::{Element, Subscription, Task, event, window};
use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) fn x11_startup_and_generated_context_keep_their_protocol() {
    let display = PrivateX11::start();
    let mut command = app_command(
        "native-activation-x11",
        &display.bus.address,
        display.runtime.path(),
    );
    command
        .env("DISPLAY", &display.display)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env("DESKTOP_STARTUP_ID", "x11-startup-context")
        .env("XDG_ACTIVATION_TOKEN", "wayland-startup-context");
    let app = AppProcess::spawn(command, "native X11 activation");
    let xid = app.expect_prefix("native-window ");
    let xid = xid.strip_prefix("native-window ").unwrap();
    let mut command = Command::new("xprop");
    command
        .args(["-id", xid, "_NET_STARTUP_ID"])
        .env("DISPLAY", &display.display)
        .env_remove("WAYLAND_DISPLAY");
    let property = peer_output(command);
    assert!(property.contains("x11-startup-context"), "{property}");
    assert!(!property.contains("wayland-startup-context"), "{property}");
    app.expect_line("native-child X11");
}

pub(super) fn wayland_selects_its_field_and_rejects_x11_context() {
    let display = PrivateWayland::start_for_activation();
    let log_path = display.runtime.path().join("activation-protocol.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = app_command(
        "native-activation-wayland",
        &display.bus.address,
        display.runtime.path(),
    );
    command
        .env("WAYLAND_DISPLAY", &display.socket)
        .env("WAYLAND_DEBUG", "client")
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env("DESKTOP_STARTUP_ID", "x11-startup-context")
        .env("XDG_ACTIVATION_TOKEN", "wayland-startup-context");
    let app = AppProcess::spawn_with_stderr(command, "native Wayland activation", Stdio::from(log));
    let progress = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.expect_line("native-selected");
        app.expect_line("native-untyped");
        app.expect_line("native-refused");
    }));
    assert!(
        progress.is_ok(),
        "native application failed: {}",
        std::fs::read_to_string(&log_path).unwrap()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let protocol = std::fs::read_to_string(&log_path).unwrap();
        if [
            "wayland-startup-context",
            "wayland-explicit-context",
            "native-notification-context",
        ]
        .iter()
        .all(|token| {
            protocol
                .lines()
                .any(|line| line.contains(".activate(") && line.contains(token))
        }) {
            assert!(
                !protocol
                    .lines()
                    .any(|line| line.contains(".activate(") && line.contains("x11-")),
                "{protocol}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "activation did not reach the native protocol: {protocol}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn peer_output(mut command: Command) -> String {
    let mut child = OwnedChild::spawn(
        command.stdout(Stdio::piped()).stderr(Stdio::inherit()),
        "native activation peer",
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.child_mut().try_wait().unwrap() {
            assert!(status.success());
            let mut output = String::new();
            child
                .child_mut()
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut output)
                .unwrap();
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "native activation peer did not exit"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Debug)]
enum Message {
    Opened(window::Id),
    RawId(u64),
    Token(ActivationResponse),
    Submitted(&'static str, Result<(), ActivateWindowError>),
}

struct State {
    broker: ActivationBroker,
    window: Option<window::Id>,
}

impl State {
    fn new() -> Self {
        Self {
            broker: ActivationBroker::default(),
            window: None,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Opened(window) if self.window.is_none() => {
                self.window = Some(window);
                if std::env::var(super::MODE).as_deref() == Ok("native-activation-x11") {
                    window::raw_id::<()>(window).map(Message::RawId)
                } else {
                    let now = Instant::now();
                    let paired = InboundActivationToken::from_platform_data(
                        Some("wayland-explicit-context".into()),
                        Some("x11-explicit-context".into()),
                    )
                    .unwrap()
                    .unwrap();
                    let paired = self.broker.admit_token(paired, now).unwrap();
                    paired
                        .activate(window, now)
                        .map(|result| Message::Submitted("selected", result))
                }
            }
            Message::Opened(_) => Task::none(),
            Message::RawId(id) => {
                report(&format!("native-window {id}"));
                let (_, request) = self
                    .broker
                    .request(self.window.unwrap(), Instant::now())
                    .unwrap();
                request.map(Message::Token)
            }
            Message::Token(response) => {
                let context = self.broker.finish(response, Instant::now()).unwrap();
                let child = enzui_core::activation::ChildLaunch::new("/bin/sh")
                    .args(["-c", "test -n \"${DESKTOP_STARTUP_ID-}\" && test \"${XDG_ACTIVATION_TOKEN+set}\" != set"])
                    .activation(context).spawn(Instant::now()).unwrap();
                let mut child = OwnedChild { child };
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Some(status) = child.child_mut().try_wait().unwrap() {
                        assert!(
                            status.success(),
                            "the native X11 request was relabeled as Wayland"
                        );
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
                report("native-child X11");
                Task::none()
            }
            Message::Submitted(kind, result) => {
                if kind == "refused" {
                    assert_eq!(
                        result,
                        Err(ActivateWindowError::Native(
                            NativeActivationError::Unsupported
                        ))
                    );
                } else {
                    assert_eq!(result, Ok(()));
                }
                report(&format!("native-{kind}"));
                let now = Instant::now();
                match kind {
                    "selected" => self
                        .broker
                        .admit_token(
                            InboundActivationToken::native("native-notification-context".into()),
                            now,
                        )
                        .unwrap()
                        .activate(self.window.unwrap(), now)
                        .map(|result| Message::Submitted("untyped", result)),
                    "untyped" => self
                        .broker
                        .admit_token(
                            InboundActivationToken::for_platform(
                                ActivationPlatform::X11,
                                "x11-mismatched-context".into(),
                            ),
                            now,
                        )
                        .unwrap()
                        .activate(self.window.unwrap(), now)
                        .map(|result| Message::Submitted("refused", result)),
                    _ => Task::none(),
                }
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        event::listen_raw(|event, _, window| {
            matches!(event, iced::Event::Window(window::Event::Opened { .. }))
                .then_some(Message::Opened(window))
        })
    }

    fn view(&self) -> Element<'_, Message> {
        iced::widget::text("native activation fixture").into()
    }
}

pub(super) fn application() -> ! {
    enzui_widgets::app_setup::run(
        iced::application(State::new, State::update, State::view)
            .subscription(State::subscription)
            .window_size((320.0, 240.0)),
    )
    .unwrap();
    std::process::exit(0)
}
