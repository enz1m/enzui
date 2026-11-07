#[path = "native_file_widgets.rs"]
mod file_widgets;

use super::{AppProcess, PrivateWayland, report};
use enzui_clipboard::{ClipboardError, Kind, MimeType, Payload, SeatSelector};
use enzui_clipboard_runtime::native_drag::{
    self, DragAction, DragActions, DragEvent, DragOutcome, NativeDragRuntime, ReceivedDrop,
    SourceDrag,
};
use iced::{Element, Subscription, Task, event, mouse, window};
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const BUDGET: Duration = Duration::from_secs(5);

pub(super) fn copy_and_move_both_ways() {
    file_widgets::both_ways();
    for action in ["copy", "move"] {
        for outgoing in [false, true] {
            let display = compositor();
            let mut input = Input::new(&display);
            let mode = format!(
                "native-drag-{}-{action}",
                if outgoing { "send" } else { "receive" }
            );
            let app = display.app(&mode, None);
            app.expect_line("native-ready");
            let peer = gtk(
                &display,
                if outgoing { "destination" } else { "source" },
                action,
            );
            peer.expect_line("peer-ready");
            let (source, destination) = positions(&display, outgoing);
            input.move_to(source);
            if outgoing {
                app.expect_line("native-pointer-entered");
            } else {
                peer.expect_line("peer-entered");
            }
            input.button(true);
            if outgoing {
                app.expect_line("native-started");
                assert_no_terminal(&app, Duration::from_millis(2200));
            } else {
                peer.expect_line("peer-pressed");
                input.move_to((source.0 + 24, source.1));
                peer.expect_line("peer-started");
            }
            input.move_to(destination);
            if !outgoing {
                app.expect_line("native-accepted");
            } else {
                std::thread::sleep(Duration::from_millis(150));
            }
            input.button(false);
            if outgoing {
                peer.expect_line("peer-received");
                assert_no_terminal(&app, Duration::from_millis(300));
                input.move_to(source);
                app.expect_line("native-pointer-entered");
                input.button(true);
                app.expect_line("native-secondary-press");
                input.button(false);
                app.expect_line("native-secondary-release");
                assert_no_terminal(&app, Duration::from_millis(300));
                std::fs::write(display.runtime.path().join("commit-peer-drop"), b"commit").unwrap();
                peer.expect_line("peer-committed");
                app.expect_line(&format!("native-finished {action}"));
            } else {
                app.expect_line("native-received");
                assert_no_terminal(&peer, Duration::from_millis(300));
                std::fs::write(display.runtime.path().join("commit-native-drop"), b"commit")
                    .unwrap();
                app.expect_line("native-committed");
                peer.expect_line("peer-ended");
            }
        }
    }
}

pub(super) fn rejections_and_cancellation() {
    for reason in ["mime", "action"] {
        let display = compositor();
        let mut input = Input::new(&display);
        let app = display.app(&format!("native-drag-receive-reject-{reason}"), None);
        app.expect_line("native-ready");
        let peer = gtk(&display, "source", "move");
        peer.expect_line("peer-ready");
        let (source, destination) = positions(&display, false);
        input.move_to(source);
        peer.expect_line("peer-entered");
        input.button(true);
        peer.expect_line("peer-pressed");
        input.move_to((source.0 + 24, source.1));
        peer.expect_line("peer-started");
        input.move_to(destination);
        app.expect_line("native-refused");
        input.button(false);
        peer.expect_line("peer-cancelled");
        peer.expect_line("peer-ended");
    }
    for reason in ["mime", "action"] {
        let display = compositor();
        let mut input = Input::new(&display);
        let app = display.app("native-drag-send-copy", None);
        app.expect_line("native-ready");
        let peer = gtk(&display, &format!("destination-reject-{reason}"), "copy");
        peer.expect_line("peer-ready");
        let (source, destination) = positions(&display, true);
        input.move_to(source);
        app.expect_line("native-pointer-entered");
        input.button(true);
        app.expect_line("native-started");
        input.move_to(destination);
        std::thread::sleep(Duration::from_millis(150));
        input.button(false);
        app.expect_line("native-cancelled");
    }
    let display = compositor();
    let mut input = Input::new(&display);
    let app = display.app("native-drag-send-cancel", None);
    app.expect_line("native-ready");
    let peer = gtk(&display, "destination", "copy");
    peer.expect_line("peer-ready");
    input.move_to(positions(&display, true).0);
    app.expect_line("native-pointer-entered");
    input.button(true);
    app.expect_line("native-started");
    app.expect_line("native-cancelled");
    input.button(false);
}

pub(super) fn abandoned_drop_cancels_without_source_success() {
    cancelled_destination("native-drag-receive-abandon-move");
}

pub(super) fn destination_teardown_cancels_retained_offer() {
    cancelled_destination("native-drag-receive-teardown-move");
}

fn cancelled_destination(mode: &str) {
    let display = compositor();
    let mut input = Input::new(&display);
    let app = display.app(mode, None);
    app.expect_line("native-ready");
    let peer = gtk(&display, "source", "move");
    peer.expect_line("peer-ready");
    let (source, destination) = positions(&display, false);
    input.move_to(source);
    peer.expect_line("peer-entered");
    input.button(true);
    peer.expect_line("peer-pressed");
    input.move_to((source.0 + 24, source.1));
    peer.expect_line("peer-started");
    input.move_to(destination);
    app.expect_line("native-accepted");
    input.button(false);
    app.expect_line("native-received");
    assert_no_terminal(&peer, Duration::from_millis(300));
    std::fs::write(
        display.runtime.path().join("commit-native-drop"),
        b"abandon",
    )
    .unwrap();
    app.expect_line("native-abandoned");
    peer.expect_line("peer-cancelled");
    peer.expect_line("peer-ended");
}

pub(super) fn released_pointer_has_no_drag_authority() {
    let display = compositor();
    let mut input = Input::new(&display);
    let app = display.app("native-drag-stale-copy", None);
    app.expect_line("native-ready");
    let peer = gtk(&display, "destination", "copy");
    peer.expect_line("peer-ready");
    input.move_to(positions(&display, true).0);
    app.expect_line("native-pointer-entered");
    input.button(true);
    app.expect_line("native-pressed");
    input.button(false);
    app.expect_line("native-stale-refused");
}

fn assert_no_terminal(peer: &AppProcess, budget: Duration) {
    let deadline = Instant::now() + budget;
    loop {
        match peer
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(line) => assert!(
                line != "peer-ended"
                    && line != "peer-cancelled"
                    && !line.starts_with("native-finished")
                    && line != "native-cancelled",
                "source completed before destination commit: {line}"
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return,
            Err(error) => panic!("GTK source stopped before destination commit: {error}"),
        }
    }
}

fn compositor() -> PrivateWayland {
    PrivateWayland::start_with_compositor(|runtime| {
        let config = runtime.join("sway.conf");
        std::fs::write(&config, "xwayland disable\noutput * mode 640x480\nseat seat0 fallback true\ndefault_border none\nfor_window [title=\"native-enzui\"] floating enable, resize set 240 240, move position 0 0\nfor_window [title=\"native-gtk\"] floating enable, resize set 240 240, move position 320 0\n").unwrap();
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

fn gtk(display: &PrivateWayland, role: &str, action: &str) -> AppProcess {
    let mut command = Command::new("python3");
    command
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/app_setup_clipboard/native_drag_peer.py"
        ))
        .args([role, action])
        .env("XDG_RUNTIME_DIR", display.runtime.path())
        .env("WAYLAND_DISPLAY", &display.socket)
        .env("DBUS_SESSION_BUS_ADDRESS", &display.bus.address)
        .env("GDK_BACKEND", "wayland")
        .env("NO_AT_BRIDGE", "1")
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("AT_SPI_BUS_ADDRESS")
        .stdin(Stdio::null());
    AppProcess::spawn(command, "GTK native peer")
}

fn positions(display: &PrivateWayland, outgoing: bool) -> ((u32, u32), (u32, u32)) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let tree = ipc(display, 4, "");
        let mut app = None;
        let mut peer = None;
        fn visit(
            node: &serde_json::Value,
            app: &mut Option<(u32, u32)>,
            peer: &mut Option<(u32, u32)>,
        ) {
            if let Some(name) = node["name"].as_str() {
                let rect = &node["rect"];
                if let (Some(x), Some(y), Some(w), Some(h)) = (
                    rect["x"].as_u64(),
                    rect["y"].as_u64(),
                    rect["width"].as_u64(),
                    rect["height"].as_u64(),
                ) {
                    let position = (
                        u32::try_from(x + w / 2).unwrap(),
                        u32::try_from(y + h / 2).unwrap(),
                    );
                    match name {
                        "native-enzui" if x == 0 && y == 0 && w == 240 && h == 240 => {
                            *app = Some(position);
                        }
                        "native-gtk" if x == 320 && y == 0 && w == 240 && h == 240 => {
                            *peer = Some(position);
                        }
                        _ => {}
                    }
                }
            }
            for key in ["nodes", "floating_nodes"] {
                if let Some(nodes) = node[key].as_array() {
                    for child in nodes {
                        visit(child, app, peer);
                    }
                }
            }
        }
        visit(&tree, &mut app, &mut peer);
        if let (Some(app), Some(peer)) = (app, peer) {
            return if outgoing { (app, peer) } else { (peer, app) };
        }
        assert!(
            Instant::now() < deadline,
            "native windows did not map: {tree}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn ipc(display: &PrivateWayland, kind: u32, command: &str) -> serde_json::Value {
    let path = std::fs::read_dir(display.runtime.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with("sway-ipc."))
        .unwrap()
        .path();
    let mut stream = UnixStream::connect(path).unwrap();
    stream.set_read_timeout(Some(BUDGET)).unwrap();
    stream.set_write_timeout(Some(BUDGET)).unwrap();
    stream.write_all(b"i3-ipc").unwrap();
    stream
        .write_all(&u32::try_from(command.len()).unwrap().to_le_bytes())
        .unwrap();
    stream.write_all(&kind.to_le_bytes()).unwrap();
    stream.write_all(command.as_bytes()).unwrap();
    let mut header = [0; 14];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(&header[..6], b"i3-ipc");
    let len = u32::from_le_bytes(header[6..10].try_into().unwrap());
    assert!(len < 1024 * 1024);
    let mut bytes = vec![0; len as usize];
    stream.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Input {
    connection: Connection,
    queue: wayland_client::EventQueue<InputState>,
    state: InputState,
    pointer: ZwlrVirtualPointerV1,
}

struct InputState;
delegate_noop!(InputState: ignore WlSeat);
delegate_noop!(InputState: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(InputState: ignore ZwlrVirtualPointerV1);
impl Dispatch<WlRegistry, GlobalListContents> for InputState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Input {
    fn new(display: &PrivateWayland) -> Self {
        let socket = UnixStream::connect(display.runtime.path().join(&display.socket)).unwrap();
        socket.set_read_timeout(Some(BUDGET)).unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<InputState>(&connection).unwrap();
        let qh = queue.handle();
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).unwrap();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
        let mut state = InputState;
        queue.roundtrip(&mut state).unwrap();
        let mut input = Self {
            connection,
            queue,
            state,
            pointer,
        };
        input.move_to((630, 470));
        input
    }
    fn move_to(&mut self, (x, y): (u32, u32)) {
        self.pointer.motion_absolute(0, x, y, 640, 480);
        self.pointer.frame();
        self.sync();
    }
    fn button(&mut self, pressed: bool) {
        self.pointer.button(
            0,
            272,
            if pressed {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            },
        );
        self.pointer.frame();
        self.sync();
    }
    fn sync(&mut self) {
        self.connection.flush().unwrap();
        self.queue.roundtrip(&mut self.state).unwrap();
    }
}

#[derive(Debug)]
enum Message {
    Event(iced::Event, window::Id),
    Connected(Option<Result<NativeDragRuntime, ClipboardError>>),
    Surface(Option<u32>),
    NoGrab(Option<Result<SourceDrag, ClipboardError>>),
    StaleGrab(Option<Result<SourceDrag, ClipboardError>>),
    Started(Option<Result<SourceDrag, ClipboardError>>),
    Accepted(Option<Result<MimeType, ClipboardError>>),
    Received(Option<Result<ReceivedDrop, ClipboardError>>),
    Commit,
    Outcome(Result<DragOutcome, ClipboardError>),
}

struct App {
    mode: String,
    native: Option<NativeDragRuntime>,
    surface: Option<u32>,
    checked_grab: bool,
    ready: bool,
    received: Option<ReceivedDrop>,
    source_active: bool,
}

impl App {
    fn new() -> Self {
        Self {
            mode: std::env::var(super::MODE).unwrap(),
            native: None,
            surface: None,
            checked_grab: false,
            ready: false,
            received: None,
            source_active: false,
        }
    }
    fn action(&self) -> DragAction {
        if self.mode.contains("move") || self.mode.contains("reject") {
            DragAction::Move
        } else {
            DragAction::Copy
        }
    }
    fn actions(&self) -> DragActions {
        if self.action() == DragAction::Move {
            DragActions::MOVE
        } else {
            DragActions::COPY
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        let task = match message {
            Message::Event(iced::Event::Window(window::Event::Opened { .. }), id) => Task::batch([
                Task::perform(NativeDragRuntime::connect(BUDGET), Message::Connected),
                window::run(id, |window| native_drag::surface_id(window)).map(Message::Surface),
            ]),
            Message::Connected(result) => {
                let native = result
                    .expect("default runtime registration exists")
                    .unwrap();
                self.native = Some(native.clone());
                Task::perform(
                    async move {
                        native
                            .start(
                                Payload::text("no-grab"),
                                DragActions::COPY,
                                SeatSelector::Focused,
                                BUDGET,
                            )
                            .await
                    },
                    Message::NoGrab,
                )
            }
            Message::NoGrab(result) => {
                assert!(matches!(result, Some(Err(ClipboardError::NotFocused))));
                self.checked_grab = true;
                Task::none()
            }
            Message::StaleGrab(result) => {
                assert!(
                    matches!(result, Some(Err(ClipboardError::NotFocused))),
                    "a released pointer serial cannot start a drag: {result:?}"
                );
                report("native-stale-refused");
                Task::none()
            }
            Message::Event(
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                _,
            ) if self.mode.contains("stale") => {
                report("native-pressed");
                Task::none()
            }
            Message::Event(
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                _,
            ) if self.mode.contains("stale") => {
                let native = self.native.clone().unwrap();
                Task::perform(
                    async move {
                        native
                            .start(
                                Payload::text("stale"),
                                DragActions::COPY,
                                SeatSelector::Focused,
                                BUDGET,
                            )
                            .await
                    },
                    Message::StaleGrab,
                )
            }
            Message::Surface(surface) => {
                assert!(surface.is_some());
                self.surface = surface;
                Task::none()
            }
            Message::Event(iced::Event::Mouse(mouse::Event::CursorEntered), _) => {
                report("native-pointer-entered");
                Task::none()
            }
            Message::Event(
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                _,
            ) if self.source_active => {
                report("native-secondary-press");
                Task::none()
            }
            Message::Event(
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                _,
            ) if self.source_active => {
                report("native-secondary-release");
                Task::none()
            }
            Message::Event(
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                _,
            ) if self.mode.contains("send") => {
                let native = self.native.clone().unwrap();
                let actions = self.actions();
                let payload = if self.action() == DragAction::Move {
                    Payload::uris([std::path::PathBuf::from("/tmp/native-from-enzui")])
                } else {
                    Payload::text("native-from-enzui")
                };
                Task::perform(
                    async move {
                        native
                            .start(payload, actions, SeatSelector::Focused, BUDGET)
                            .await
                    },
                    Message::Started,
                )
            }
            Message::Started(result) => {
                let source = result.unwrap().unwrap();
                self.source_active = true;
                report("native-started");
                if self.mode.contains("cancel") {
                    source.cancel();
                }
                Task::perform(native_drag::source_outcome(source), Message::Outcome)
            }
            Message::Accepted(result) => {
                if self.mode.contains("reject") {
                    assert!(result.unwrap().is_err());
                    report("native-refused");
                } else {
                    assert!(result.unwrap().is_ok());
                    report("native-accepted");
                }
                Task::none()
            }
            Message::Received(result) => {
                let received = result.unwrap().unwrap();
                if self.action() == DragAction::Move {
                    assert_eq!(
                        received.data().file_transfer().unwrap().paths(),
                        &[std::path::PathBuf::from("/tmp/native-from-gtk")]
                    );
                } else {
                    assert_eq!(received.data().text(), Some("native-from-gtk"));
                }
                assert_eq!(received.action(), self.action());
                self.received = Some(received);
                report("native-received");
                Task::perform(
                    async {
                        let path =
                            std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap())
                                .join("commit-native-drop");
                        let deadline = Instant::now() + BUDGET;
                        while !path.exists() {
                            assert!(
                                Instant::now() < deadline,
                                "destination commit was not requested"
                            );
                            futures_timer::Delay::new(Duration::from_millis(10)).await;
                        }
                    },
                    |()| Message::Commit,
                )
            }
            Message::Commit => {
                let received = self.received.take().unwrap();
                if self.mode.contains("teardown") {
                    self.native = None;
                    self.received = Some(received);
                    report("native-abandoned");
                } else if self.mode.contains("abandon") {
                    drop(received);
                    report("native-abandoned");
                } else {
                    received.finish();
                    report("native-committed");
                }
                Task::none()
            }
            Message::Outcome(result) => {
                self.source_active = false;
                match result.unwrap() {
                    DragOutcome::Finished(action) => {
                        assert_eq!(action, self.action());
                        report(if action == DragAction::Copy {
                            "native-finished copy"
                        } else {
                            "native-finished move"
                        });
                    }
                    DragOutcome::Cancelled => report("native-cancelled"),
                }
                Task::none()
            }
            Message::Event(_, _) => Task::none(),
        };
        if self.native.is_some() && self.surface.is_some() && self.checked_grab && !self.ready {
            self.ready = true;
            report("native-ready");
        }
        task
    }
    fn native_event(&mut self, event: DragEvent) -> Task<Message> {
        match event {
            DragEvent::Entered { id, surface, .. } if self.mode.contains("receive") => {
                assert_eq!(Some(surface), self.surface);
                let native = self.native.clone().unwrap();
                let preferences = if self.mode.contains("reject-mime") {
                    vec![MimeType::new("application/x-refused")]
                } else if self.action() == DragAction::Move {
                    Kind::Uris.preferences()
                } else {
                    Kind::Text.preferences()
                };
                let (actions, preferred) = if self.mode.contains("reject-action") {
                    (DragActions::COPY, DragAction::Copy)
                } else {
                    (self.actions(), self.action())
                };
                Task::perform(
                    async move {
                        native
                            .accept(id, preferences, actions, preferred, BUDGET)
                            .await
                    },
                    Message::Accepted,
                )
            }
            DragEvent::Dropped { id, .. } => {
                let native = self.native.clone().unwrap();
                Task::perform(
                    async move {
                        let result = native.receive(id, BUDGET).await;
                        assert!(
                            matches!(native.receive(id, BUDGET).await, Some(Err(_))),
                            "a drop is consumed once"
                        );
                        assert!(
                            matches!(
                                native
                                    .accept(
                                        id,
                                        Kind::Text.preferences(),
                                        DragActions::BOTH,
                                        DragAction::Copy,
                                        BUDGET
                                    )
                                    .await,
                                Some(Err(_))
                            ),
                            "a dropped offer cannot be renegotiated"
                        );
                        result
                    },
                    Message::Received,
                )
            }
            _ => Task::none(),
        }
    }
    fn subscription(&self) -> Subscription<AppMessage> {
        Subscription::batch([
            event::listen_raw(|event, _, id| Some(AppMessage::App(Message::Event(event, id)))),
            self.native
                .as_ref()
                .map_or_else(Subscription::none, |native| {
                    native.subscription().map(AppMessage::Native)
                }),
        ])
    }
    fn view(&self) -> Element<'_, AppMessage> {
        iced::widget::text("Native Wayland framework fixture").into()
    }
}

#[derive(Debug)]
enum AppMessage {
    App(Message),
    Native(DragEvent),
}

pub(super) fn application() -> ! {
    if std::env::var(super::MODE).is_ok_and(|mode| mode.starts_with("native-drag-widgets-")) {
        file_widgets::application();
    }
    let result = enzui_widgets::app_setup::run(
        iced::application(
            App::new,
            |app: &mut App, message| {
                match message {
                    AppMessage::App(message) => app.update(message),
                    AppMessage::Native(event) => app.native_event(event),
                }
                .map(AppMessage::App)
            },
            App::view,
        )
        .title("native-enzui")
        .subscription(App::subscription)
        .window_size((240.0, 240.0))
        .decorations(false),
    );
    if let Err(error) = result {
        panic!("native fixture failed: {error}");
    }
    std::process::exit(0)
}
