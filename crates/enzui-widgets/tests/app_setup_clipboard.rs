//! Production bootstrap, clipboard transfers and activation on private displays.
//!
//! Each clipboard application is a separate process entering through
//! `app_setup::run`. The clipboard fixture's Weston starts without XWayland or
//! data-control. Native activation uses Xvfb and headless Sway, and the exit
//! keeper headless Sway with `wl-copy`/`wl-paste`; the software renderer's
//! buffer format is read from headless Sway's protocol log. A private D-Bus
//! daemon and XDG directories isolate the applications from host desktop services.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

#[path = "app_setup_clipboard/exit_keeper.rs"]
mod exit_keeper;
#[path = "app_setup_clipboard/native_activation.rs"]
mod native_activation;
#[path = "app_setup_clipboard/native_drag.rs"]
mod native_drag;
#[path = "../../enzui-clipboard/tests/support/runner.rs"]
mod runner;
#[path = "app_setup_clipboard/software_alpha.rs"]
mod software_alpha;
#[path = "../../enzui-test-support/src/test_scratch.rs"]
mod test_scratch;

use std::io::{BufRead, BufReader, Write as _};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use iced::{Element, Subscription, Task, event, window};
use runner::{Case, run};
use test_scratch::ScratchDir;

const ROLE: &str = "ENZUI_APP_SETUP_CLIPBOARD_ROLE";
const MODE: &str = "ENZUI_APP_SETUP_CLIPBOARD_MODE";
const WAYLAND_SOCKET: &str = "wayland-enzui-clipboard-test";
const PAYLOAD: &str = "ordinary Wayland clipboard from app_setup";
const PROCESS_BUDGET: Duration = Duration::from_secs(20);

fn main() {
    // Links the keeper's constructor, which serves before `main` runs.
    std::hint::black_box(enzui_clipboard::keeper::hand_off as fn() -> std::io::Result<bool>);
    if std::env::var(ROLE).as_deref() == Ok("app") {
        if std::env::var(MODE).is_ok_and(|mode| mode.starts_with("native-drag-")) {
            native_drag::application();
        }
        if std::env::var(MODE).is_ok_and(|mode| mode.starts_with("native-activation-")) {
            native_activation::application();
        }
        if std::env::var(MODE).as_deref() == Ok("software-alpha") {
            software_alpha::application();
        }
        clipboard_app();
    }
    if std::env::var(ROLE).as_deref() == Ok("startup-stall") {
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(0);
    }

    run(&[
        Case {
            name: "native_drag_copy_and_move_cross_the_default_runtime_and_gtk_both_ways",
            run: native_drag::copy_and_move_both_ways,
        },
        Case {
            name: "native_drag_destination_teardown_cancels_retained_offer",
            run: native_drag::destination_teardown_cancels_retained_offer,
        },
        Case {
            name: "native_drag_released_pointer_has_no_drag_authority",
            run: native_drag::released_pointer_has_no_drag_authority,
        },
        Case {
            name: "native_drag_abandoned_drop_cancels_without_source_success",
            run: native_drag::abandoned_drop_cancels_without_source_success,
        },
        Case {
            name: "native_drag_rejects_mime_actions_and_source_cancellation",
            run: native_drag::rejections_and_cancellation,
        },
        Case {
            name: "native_activation_x11_startup_and_generated_context_keep_their_protocol",
            run: native_activation::x11_startup_and_generated_context_keep_their_protocol,
        },
        Case {
            name: "native_activation_wayland_selects_its_field_and_rejects_x11_context",
            run: native_activation::wayland_selects_its_field_and_rejects_x11_context,
        },
        Case {
            name: "software_transparent_window_buffers_carry_alpha",
            run: software_alpha::transparent_window_buffers_carry_alpha,
        },
        Case {
            name: "default_app_setup_copies_and_pastes_without_data_control_or_xwayland",
            run: default_app_setup_copies_and_pastes_without_data_control_or_xwayland,
        },
        Case {
            name: "explicit_default_override_keeps_the_runtime_clipboard",
            run: explicit_default_override_keeps_the_runtime_clipboard,
        },
        Case {
            name: "explicit_code_default_survives_automatic_app_setup",
            run: explicit_code_default_survives_automatic_app_setup,
        },
        Case {
            name: "x11_app_setup_keeps_the_runtime_clipboard_path",
            run: x11_app_setup_keeps_the_runtime_clipboard_path,
        },
        Case {
            name: "unfocused_app_with_an_input_serial_cannot_replace_the_selection",
            run: unfocused_app_with_an_input_serial_cannot_replace_the_selection,
        },
        Case {
            name: "exit_keeper_never_replaces_a_newer_selection",
            run: exit_keeper::never_replaces_a_newer_selection,
        },
        Case {
            name: "startup_timeout_reaps_its_exact_child",
            run: startup_timeout_reaps_its_exact_child,
        },
    ])
}

struct OwnedChild {
    child: Child,
}

impl OwnedChild {
    fn spawn(command: &mut Command, description: &str) -> Self {
        let child = command
            .spawn()
            .unwrap_or_else(|error| panic!("{description}: {error}"));
        Self { child }
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct PrivateBus {
    _child: OwnedChild,
    address: String,
}

impl PrivateBus {
    fn start(root: &std::path::Path) -> Self {
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        let config = root.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig>\n  <type>session</type>\n  <listen>{address}</listen>\n  \
                 <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    \
                 <allow receive_sender=\"*\"/>\n    <allow own=\"*\"/>\n  \
                 </policy>\n</busconfig>\n"
            ),
        )
        .expect("the private bus configuration is written");

        let child = OwnedChild::spawn(
            Command::new("dbus-daemon")
                .arg(format!("--config-file={}", config.display()))
                .arg("--nofork")
                .arg("--nopidfile")
                .env_remove("DBUS_SESSION_BUS_ADDRESS")
                .stdout(Stdio::null())
                .stderr(Stdio::inherit()),
            "dbus-daemon is available",
        );

        wait_for_path(&socket, Duration::from_secs(5), "private D-Bus socket")
            .unwrap_or_else(|message| panic!("{message}"));
        Self {
            _child: child,
            address,
        }
    }
}

struct PrivateWayland {
    _child: OwnedChild,
    runtime: ScratchDir,
    bus: PrivateBus,
    socket: String,
}

impl PrivateWayland {
    fn start() -> Self {
        Self::start_with_compositor(|_| {
            let mut command = Command::new("weston");
            command.args([
                "--backend=headless",
                "--renderer=pixman",
                "--fake-seat",
                "--shell=kiosk-shell.so",
                "--no-config",
                "--idle-time=0",
                "--width=320",
                "--height=240",
                &format!("--socket={WAYLAND_SOCKET}"),
            ]);
            command
        })
    }

    fn start_for_activation() -> Self {
        Self::start_with_compositor(|runtime| {
            let config = runtime.join("sway.conf");
            std::fs::write(
                &config,
                "xwayland disable\noutput * mode 320x240\nseat seat0 fallback true\n",
            )
            .unwrap();
            let mut command = Command::new("sway");
            command
                .arg("--config")
                .arg(config)
                .env("WLR_BACKENDS", "headless")
                .env("WLR_RENDERER", "pixman")
                .env("WLR_LIBINPUT_NO_DEVICES", "1")
                .env("WLR_HEADLESS_OUTPUTS", "1")
                .env_remove("SWAYSOCK")
                .env_remove("I3SOCK");
            command
        })
    }

    fn start_with_compositor(build: impl FnOnce(&std::path::Path) -> Command) -> Self {
        use std::os::unix::fs::FileTypeExt as _;
        let runtime = ScratchDir::new_restricted("appclip");
        let bus = PrivateBus::start(runtime.path());
        let log_path = runtime.path().join("compositor.log");
        let log = std::fs::File::create(&log_path).unwrap();
        let mut command = build(runtime.path());
        let mut child = OwnedChild::spawn(
            command
                .env("XDG_RUNTIME_DIR", runtime.path())
                .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
                .env_remove("DISPLAY")
                .env_remove("XAUTHORITY")
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("WAYLAND_SOCKET")
                .stdout(Stdio::null())
                .stderr(Stdio::from(log)),
            "private compositor",
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let socket = loop {
            let socket = std::fs::read_dir(runtime.path())
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| {
                    entry.file_name().to_string_lossy().starts_with("wayland-")
                        && entry.file_type().is_ok_and(|kind| kind.is_socket())
                });
            if let Some(socket) = socket {
                break socket.file_name().to_str().unwrap().to_owned();
            }
            assert!(
                child.child_mut().try_wait().unwrap().is_none() && Instant::now() < deadline,
                "private compositor did not start: {}",
                std::fs::read_to_string(&log_path).unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        Self {
            _child: child,
            runtime,
            bus,
            socket,
        }
    }

    /// `program` as a client of this compositor only.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus.address)
            .env("WAYLAND_DISPLAY", &self.socket)
            .env_remove("WAYLAND_SOCKET")
            .env_remove("DISPLAY")
            .env_remove("XAUTHORITY");
        command
    }

    fn app(&self, mode: &str, policy: Option<&str>) -> AppProcess {
        let mut command = app_command(mode, &self.bus.address, self.runtime.path());
        command
            .env("WAYLAND_DISPLAY", &self.socket)
            .env_remove("DISPLAY")
            .env_remove("XAUTHORITY");
        apply_policy(&mut command, policy);
        AppProcess::spawn(command, mode)
    }
}

struct PrivateX11 {
    _child: OwnedChild,
    display: String,
    runtime: ScratchDir,
    bus: PrivateBus,
}

impl PrivateX11 {
    fn start() -> Self {
        let runtime = ScratchDir::new_restricted("appx11");
        let bus = PrivateBus::start(runtime.path());
        let mut child = OwnedChild::spawn(
            Command::new("Xvfb")
                .args([
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    "320x240x24",
                    "-nolisten",
                    "tcp",
                    "-ac",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit()),
            "Xvfb is available",
        );
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .expect("Xvfb reports its display");
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let number = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("Xvfb reports its display before the deadline");
        let number = number.trim();
        assert!(!number.is_empty(), "Xvfb returned an empty display number");

        Self {
            _child: child,
            display: format!(":{number}"),
            runtime,
            bus,
        }
    }

    fn app(&self, mode: &str) -> AppProcess {
        let mut command = app_command(mode, &self.bus.address, self.runtime.path());
        command
            .env("DISPLAY", &self.display)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("XAUTHORITY")
            .env_remove("ENZUI_CLIPBOARD");
        AppProcess::spawn(command, mode)
    }
}

struct AppProcess {
    label: String,
    _child: OwnedChild,
    lines: Receiver<String>,
}

impl AppProcess {
    fn spawn(command: Command, label: &str) -> Self {
        Self::spawn_with_stderr(command, label, Stdio::inherit())
    }

    fn spawn_with_stderr(mut command: Command, label: &str, stderr: Stdio) -> Self {
        let mut child = OwnedChild::spawn(
            command.stdout(Stdio::piped()).stderr(stderr),
            &format!("{label} application starts"),
        );
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .expect("application stdout is piped");
        let (sender, lines) = channel();
        let reader_label = label.to_owned();
        std::thread::Builder::new()
            .name(format!("app-clipboard-{reader_label}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            })
            .expect("the application output reader starts");

        Self {
            label: label.to_owned(),
            _child: child,
            lines,
        }
    }

    fn expect_line(&self, expected: &str) {
        let deadline = Instant::now() + PROCESS_BUDGET;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) if line == expected => return,
                Ok(line) => assert!(
                    Instant::now() < deadline,
                    "{} did not report {expected:?}; last line was {line:?}",
                    self.label
                ),
                Err(RecvTimeoutError::Timeout) => {
                    panic!(
                        "{} did not report {expected:?} before the deadline",
                        self.label
                    )
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("{} exited before reporting {expected:?}", self.label)
                }
            }
        }
    }

    fn expect_prefix(&self, expected: &str) -> String {
        let deadline = Instant::now() + PROCESS_BUDGET;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) if line.starts_with(expected) => return line,
                Ok(line) => assert!(
                    Instant::now() < deadline,
                    "{} did not report prefix {expected:?}; last line was {line:?}",
                    self.label
                ),
                Err(RecvTimeoutError::Timeout) => panic!(
                    "{} did not report prefix {expected:?} before the deadline",
                    self.label
                ),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("{} exited before reporting prefix {expected:?}", self.label)
                }
            }
        }
    }
}

fn app_command(mode: &str, bus: &str, runtime: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test executable is known"));
    command
        .env(ROLE, "app")
        .env(MODE, mode)
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("ICED_BACKEND", "tiny-skia")
        .env_remove("AT_SPI_BUS_ADDRESS")
        .stdin(Stdio::null());
    command
}

fn apply_policy(command: &mut Command, policy: Option<&str>) {
    match policy {
        Some(value) => {
            command.env("ENZUI_CLIPBOARD", value);
        }
        None => {
            command.env_remove("ENZUI_CLIPBOARD");
        }
    }
}

fn wait_for_path(
    path: &std::path::Path,
    budget: Duration,
    description: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + budget;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(format!("{description} did not appear"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn process_exists(pid: u32) -> bool {
    std::path::Path::new("/proc").join(pid.to_string()).exists()
}

fn startup_timeout_reaps_its_exact_child() {
    let (pid, timed_out) = {
        let mut command =
            Command::new(std::env::current_exe().expect("the test executable is known"));
        command
            .env(ROLE, "startup-stall")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = OwnedChild::spawn(&mut command, "startup stall fixture starts");
        let pid = child.id();
        assert!(process_exists(pid), "the exact fixture child must be live");

        let missing = std::path::Path::new("/proc")
            .join(pid.to_string())
            .join("never-created");
        let result = wait_for_path(&missing, Duration::from_millis(30), "missing startup path");
        (pid, result)
    };
    assert!(timed_out.is_err(), "the startup wait must time out");

    let deadline = Instant::now() + Duration::from_secs(2);
    while process_exists(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !process_exists(pid),
        "startup timeout left exact child {pid} running"
    );
}

fn default_app_setup_copies_and_pastes_without_data_control_or_xwayland() {
    let display = PrivateWayland::start();
    let writer = display.app("writer", None);
    writer.expect_line("opened");
    writer.expect_line("write ok");

    let reader = display.app("reader", None);
    reader.expect_line("opened");
    reader.expect_line(&format!("read ok {PAYLOAD}"));
}

fn explicit_default_override_keeps_the_runtime_clipboard() {
    let display = PrivateWayland::start();
    let writer = display.app("writer", Some("default"));
    writer.expect_line("opened");
    let outcome = writer.expect_prefix("write err ");
    assert!(
        outcome.contains("ClipboardUnavailable"),
        "the data-control-free compositor should expose iced's unavailable fallback: {outcome}"
    );
}

fn explicit_code_default_survives_automatic_app_setup() {
    let display = PrivateWayland::start();
    let writer = display.app("writer-explicit-default", None);
    writer.expect_line("opened");
    let outcome = writer.expect_prefix("write err ");
    assert!(
        outcome.contains("ClipboardUnavailable"),
        "the explicit code policy must keep iced's unavailable fallback: {outcome}"
    );
}

fn x11_app_setup_keeps_the_runtime_clipboard_path() {
    let display = PrivateX11::start();
    let writer = display.app("writer");
    writer.expect_line("opened");
    writer.expect_line("write ok");

    let reader = display.app("reader");
    reader.expect_line("opened");
    reader.expect_line(&format!("read ok {PAYLOAD}"));
}

fn unfocused_app_with_an_input_serial_cannot_replace_the_selection() {
    let display = PrivateWayland::start();
    let refused = display.app("refuse-after-focus", None);
    refused.expect_line("focused");

    let holder = display.app("holder", None);
    holder.expect_line("focused");
    refused.expect_line("unfocused");
    let outcome = refused.expect_prefix("write err ");
    assert!(
        outcome.contains("no focused seat with a valid input serial owns this window"),
        "a previously focused client must be refused after losing focus: {outcome}"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Writer,
    Reader,
    RefuseAfterFocus,
    Holder,
}

#[derive(Debug, Clone)]
enum Message {
    Event(iced::Event),
    Written(Result<(), iced::clipboard::Error>),
    Read(Result<std::sync::Arc<String>, iced::clipboard::Error>),
}

struct State {
    mode: AppMode,
    started: bool,
}

impl State {
    fn new() -> Self {
        let mode = match std::env::var(MODE).as_deref() {
            Ok("writer" | "writer-explicit-default") => AppMode::Writer,
            Ok("reader") => AppMode::Reader,
            Ok("refuse-after-focus") => AppMode::RefuseAfterFocus,
            Ok("holder") => AppMode::Holder,
            value => panic!("unknown clipboard app mode: {value:?}"),
        };
        Self {
            mode,
            started: false,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Event(iced::Event::Window(window::Event::Opened { .. })) => {
                if matches!(self.mode, AppMode::RefuseAfterFocus | AppMode::Holder) {
                    return Task::none();
                }
                report("opened");
                if self.started {
                    return Task::none();
                }
                match self.mode {
                    AppMode::Writer => {
                        self.started = true;
                        iced::clipboard::write(String::from(PAYLOAD)).map(Message::Written)
                    }
                    AppMode::Reader => {
                        self.started = true;
                        iced::clipboard::read_text().map(Message::Read)
                    }
                    AppMode::RefuseAfterFocus | AppMode::Holder => Task::none(),
                }
            }
            Message::Event(iced::Event::Window(window::Event::Focused)) => {
                report("focused");
                Task::none()
            }
            Message::Event(iced::Event::Window(window::Event::Unfocused)) => {
                report("unfocused");
                if self.mode == AppMode::RefuseAfterFocus && !self.started {
                    self.started = true;
                    iced::clipboard::write(String::from("must be refused")).map(Message::Written)
                } else {
                    Task::none()
                }
            }
            Message::Event(_) => Task::none(),
            Message::Written(result) => {
                match result {
                    Ok(()) => report("write ok"),
                    Err(error) => report(&format!("write err {error:?}")),
                }
                Task::none()
            }
            Message::Read(result) => {
                match result {
                    Ok(text) => report(&format!("read ok {text}")),
                    Err(error) => report(&format!("read err {error:?}")),
                }
                iced::exit()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        event::listen_raw(|event, _status, _window| Some(Message::Event(event)))
    }

    fn view(&self) -> Element<'_, Message> {
        iced::widget::text("clipboard fixture").into()
    }
}

fn report(line: &str) {
    println!("{line}");
    std::io::stdout()
        .flush()
        .expect("the parent receives application reports");
}

fn clipboard_app() -> ! {
    if std::env::var(MODE).as_deref() == Ok("writer-explicit-default") {
        let outcome = enzui_clipboard_runtime::adopt_process_clipboard_with(
            enzui_clipboard::bootstrap::Policy::KeepDefault,
        );
        assert!(matches!(
            outcome,
            enzui_clipboard::bootstrap::Outcome::KeptDefault { .. }
        ));
    }

    let result = enzui_widgets::app_setup::run(
        iced::application(State::new, State::update, State::view)
            .subscription(State::subscription)
            .window_size((320.0, 240.0)),
    );
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("clipboard fixture runtime failed: {error}");
            std::process::exit(2);
        }
    }
}
