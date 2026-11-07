//! Minimal compositor fixture for focused-client selection tests.
//!
//! Advertises compositor, seat and ordinary data-device globals. Claims require
//! focus and any serial issued to that client; serial freshness is not checked.
//! Only focused clients receive selection offers, which this fixture explicitly
//! revokes on focus loss. Synthetic pointer grabs exercise ignored drag starts.
//! It models no shell, rendering, complete drag exchange or
//! clipboard-manager protocols. Fault controls suppress claims, transfers or
//! startup replies; they are fixture behavior rather than host-compositor policy.

use std::collections::HashSet;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_data_device::{self, WlDataDevice};
use wayland_server::protocol::wl_data_device_manager::{self, WlDataDeviceManager};
use wayland_server::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_server::protocol::wl_data_source::{self, WlDataSource};
use wayland_server::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_region::{self, WlRegion};
use wayland_server::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, ListeningSocket, New,
    Resource,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStall {
    FirstRoundTrip,
    SecondRoundTrip,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub seats: usize,
    pub startup_stall: Option<StartupStall>,
    pub ignored_drag_release_leave: bool,
    /// Advertise the primary selection global.
    pub primary: bool,
    /// Advertise `wl_shm`, `xdg_wm_base` and the toplevel drag manager.
    pub drag_window: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seats: 1,
            startup_stall: None,
            ignored_drag_release_leave: false,
            primary: false,
            drag_window: false,
        }
    }
}

#[path = "primary.rs"]
mod primary;

#[path = "drag_window.rs"]
mod drag_window;

pub use drag_window::IconSeen;

pub(crate) const SOCKET_NAME: &str = "wayland-clipboard-test";

// A `sun_path` holds 108 bytes including its NUL, so a longer pathname cannot
// be bound at all; this budget keeps headroom under that hard limit.
pub(crate) const SUN_PATH_BUDGET: usize = 100;
pub struct TestCompositor {
    socket: PathBuf,
    // A `/proc/<pid>/fd/<n>` socket pathname resolves only while this
    // descriptor is open, and peer processes resolve it for themselves.
    _scratch_directory: Option<std::fs::File>,
    control: Sender<Control>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) fn socket_path(directory: &Path, proc_alias: &Path) -> (PathBuf, bool) {
    let direct = directory.join(SOCKET_NAME);

    if direct.as_os_str().len() < SUN_PATH_BUDGET {
        (direct, false)
    } else {
        (proc_alias.join(SOCKET_NAME), true)
    }
}

fn proc_alias(directory: &std::fs::File) -> PathBuf {
    use std::os::fd::AsRawFd;

    PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        directory.as_raw_fd()
    ))
}

impl TestCompositor {
    pub fn start(directory: &Path, config: Config) -> Self {
        let owned = std::fs::File::open(directory).expect("the scratch directory opens");
        let (socket, aliased) = socket_path(directory, &proc_alias(&owned));
        let scratch_directory = aliased.then_some(owned);

        assert!(
            socket.as_os_str().len() < SUN_PATH_BUDGET,
            "the test socket path {} is too long for sun_path",
            socket.display()
        );

        let (control, orders) = channel();
        let (ready, started) = channel();
        let socket_path = socket.clone();

        let thread = std::thread::Builder::new()
            .name(String::from("enzui-clipboard-test-compositor"))
            .spawn(move || run(socket_path, config, orders, ready))
            .expect("the compositor thread starts");

        started
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor announces its socket")
            .expect("the compositor binds its socket");

        assert!(
            directory.join(SOCKET_NAME).exists(),
            "the bound socket must be an inode in the test's own scratch \
             directory {}, whatever pathname reached it",
            directory.display()
        );

        Self {
            socket,
            _scratch_directory: scratch_directory,
            control,
            thread: Some(thread),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn focus(&self, seat: usize, client: Option<usize>) {
        let (reply, answer) = channel();

        self.control
            .send(Control::Focus {
                seat,
                client,
                reply,
            })
            .expect("the compositor accepts focus orders");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor applies focus");
    }

    pub fn pointer_grab(&self, client: usize) {
        let (reply, answer) = channel();
        self.control
            .send(Control::PointerGrab { client, reply })
            .unwrap();
        assert!(
            answer.recv_timeout(Duration::from_secs(5)).unwrap(),
            "pointer and surface must exist"
        );
    }

    pub fn client_count(&self) -> usize {
        let (reply, answer) = channel();

        self.control
            .send(Control::Clients { reply })
            .expect("the compositor accepts queries");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor answers queries")
    }

    pub fn wait_for_clients(&self, count: usize, budget: Duration) {
        let deadline = Instant::now() + budget;

        while Instant::now() < deadline {
            if self.client_count() >= count {
                return;
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        panic!(
            "only {} of {count} clients connected within {budget:?}",
            self.client_count()
        );
    }

    pub fn wait_for_startup_stall(&self, expected: StartupStall, budget: Duration) {
        let deadline = Instant::now() + budget;

        loop {
            let (reply, answer) = channel();
            self.control
                .send(Control::StartupStatus { reply })
                .expect("the compositor accepts startup status queries");
            let stalled = answer
                .recv_timeout(Duration::from_secs(5))
                .expect("the compositor answers startup status queries");

            if stalled == Some(expected) {
                return;
            }

            assert!(
                Instant::now() < deadline,
                "startup did not stall at {expected:?} within {budget:?}; observed {stalled:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn resume_startup(&self) {
        let (reply, answer) = channel();
        self.control
            .send(Control::ResumeStartup { reply })
            .expect("the compositor accepts startup resume orders");
        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor resumes startup");
    }

    pub fn stall_transfers(&self, stalling: bool) {
        let (reply, answer) = channel();

        self.control
            .send(Control::Stall { stalling, reply })
            .expect("the compositor accepts stall orders");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor applies the stall setting");
    }

    pub fn discard_claims(&self, discarding: bool) {
        let (reply, answer) = channel();

        self.control
            .send(Control::Discard { discarding, reply })
            .expect("the compositor accepts claim orders");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor applies the claim setting");
    }

    pub fn selection_owner(&self, seat: usize) -> Option<usize> {
        let (reply, answer) = channel();

        self.control
            .send(Control::SelectionOwner { seat, reply })
            .expect("the compositor accepts queries");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor answers queries")
    }

    pub fn wait_for_selection_owner(&self, seat: usize, expected: Option<usize>, budget: Duration) {
        let deadline = Instant::now() + budget;

        loop {
            let owner = self.selection_owner(seat);

            if owner == expected {
                return;
            }

            assert!(
                Instant::now() < deadline,
                "seat {seat} is owned by {owner:?}, not {expected:?}, after {budget:?}"
            );

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn settle(&self, budget: Duration) {
        std::thread::sleep(budget);
        let _ = self.client_count();
    }

    pub fn wait_for_primary_owner(&self, seat: usize, expected: Option<usize>, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            let (reply, answer) = channel();
            self.control
                .send(Control::PrimaryOwner { seat, reply })
                .expect("the compositor accepts queries");
            let owner = answer.recv().expect("the compositor answers");
            if owner == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "primary selection owner stayed {owner:?}, expected {expected:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn selection_mimes(&self, seat: usize) -> Vec<String> {
        let (reply, answer) = channel();

        self.control
            .send(Control::SelectionMimes { seat, reply })
            .expect("the compositor accepts queries");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor answers queries")
    }

    pub fn drag_icon(&self) -> Option<IconSeen> {
        let (reply, answer) = channel();
        self.control
            .send(Control::DragIcon { reply })
            .expect("the compositor accepts queries");
        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor answers queries")
    }

    /// Each window attached to a toplevel drag: its title and offset.
    pub fn attached_windows(&self) -> Vec<(String, i32, i32)> {
        let (reply, answer) = channel();
        self.control
            .send(Control::Attached { reply })
            .expect("the compositor accepts queries");
        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor answers queries")
    }

    pub fn refused_selections(&self) -> usize {
        let (reply, answer) = channel();

        self.control
            .send(Control::Refusals { reply })
            .expect("the compositor accepts queries");

        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the compositor answers queries")
    }
}

impl TestCompositor {
    pub fn stop(&mut self) {
        let _ = self.control.send(Control::Stop);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for TestCompositor {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum Control {
    PointerGrab {
        client: usize,
        reply: Sender<bool>,
    },
    Focus {
        seat: usize,
        client: Option<usize>,
        reply: Sender<()>,
    },
    Clients {
        reply: Sender<usize>,
    },
    Stall {
        stalling: bool,
        reply: Sender<()>,
    },
    StartupStatus {
        reply: Sender<Option<StartupStall>>,
    },
    ResumeStartup {
        reply: Sender<()>,
    },
    Discard {
        discarding: bool,
        reply: Sender<()>,
    },
    SelectionOwner {
        seat: usize,
        reply: Sender<Option<usize>>,
    },
    SelectionMimes {
        seat: usize,
        reply: Sender<Vec<String>>,
    },
    PrimaryOwner {
        seat: usize,
        reply: Sender<Option<usize>>,
    },
    Refusals {
        reply: Sender<usize>,
    },
    DragIcon {
        reply: Sender<Option<IconSeen>>,
    },
    Attached {
        reply: Sender<Vec<(String, i32, i32)>>,
    },
    Stop,
}

#[derive(Debug, Clone, Copy)]
struct SeatIndex(usize);
#[derive(Debug, Default)]
struct SourceMimes(Mutex<Vec<String>>);
#[derive(Debug, Clone)]
struct OfferSource(WlDataSource);
struct ClientRecord {
    id: ClientId,
    surfaces: Vec<WlSurface>,
    keyboards: Vec<(usize, WlKeyboard)>,
    pointers: Vec<WlPointer>,
    devices: Vec<(usize, WlDataDevice)>,
    primary_devices: Vec<(
        usize,
        wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    )>,
    serials: HashSet<u32>,
}

struct SeatRecord {
    focus: Option<ClientId>,
    selection: Option<SelectionRecord>,
    primary: Option<primary::PrimaryRecord>,
}

struct SelectionRecord {
    source: WlDataSource,
    owner: ClientId,
    mimes: Vec<String>,
}

struct Server {
    dh: DisplayHandle,
    clients: Vec<ClientRecord>,
    seats: Vec<SeatRecord>,
    serial: u32,
    stalling: bool,
    discarding: bool,
    startup_stall: Option<StartupStall>,
    startup_stalled: Option<StartupStall>,
    ignored_drag_release_leave: bool,
    held: Vec<OwnedFd>,
    refusals: usize,
    gone: Arc<Mutex<Vec<ClientId>>>,
    drag_window: drag_window::DragWindowRecord,
}

struct Disconnects(Arc<Mutex<Vec<ClientId>>>);

impl ClientData for Disconnects {
    fn initialized(&self, _client: ClientId) {}

    fn disconnected(&self, client: ClientId, _reason: DisconnectReason) {
        if let Ok(mut gone) = self.0.lock() {
            gone.push(client);
        }
    }
}

impl Server {
    fn next_serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }

    fn stall_startup_at(&mut self, phase: StartupStall) {
        if self.startup_stall == Some(phase) {
            self.startup_stalled = Some(phase);
        }
    }

    fn record(&mut self, id: &ClientId) -> Option<&mut ClientRecord> {
        self.clients.iter_mut().find(|record| &record.id == id)
    }

    fn index_of(&self, id: &ClientId) -> Option<usize> {
        self.clients.iter().position(|record| &record.id == id)
    }

    fn set_focus(&mut self, seat: usize, client: Option<usize>) {
        if seat >= self.seats.len() {
            return;
        }

        let target = client
            .and_then(|index| self.clients.get(index))
            .map(|record| record.id.clone());

        if self.seats[seat].focus == target {
            return;
        }

        if let Some(previous) = self.seats[seat].focus.clone() {
            let serial = self.next_serial();

            if let Some(record) = self.record(&previous) {
                let surface = record.surfaces.first().cloned();

                for (index, keyboard) in record.keyboards.clone() {
                    if index == seat
                        && keyboard.is_alive()
                        && let Some(surface) = surface.as_ref()
                    {
                        keyboard.leave(serial, surface);
                    }
                }

                for (index, device) in record.devices.clone() {
                    if index == seat && device.is_alive() {
                        device.selection(None);
                    }
                }

                for (index, device) in record.primary_devices.clone() {
                    if index == seat && device.is_alive() {
                        device.selection(None);
                    }
                }
            }
        }

        self.seats[seat].focus.clone_from(&target);

        if let Some(next) = target {
            let serial = self.next_serial();

            if let Some(record) = self.record(&next) {
                record.serials.insert(serial);
                let surface = record.surfaces.first().cloned();

                for (index, keyboard) in record.keyboards.clone() {
                    if index == seat
                        && keyboard.is_alive()
                        && let Some(surface) = surface.as_ref()
                    {
                        keyboard.enter(serial, surface, Vec::new());
                    }
                }
            }

            self.send_selection(&next, seat);
            self.send_primary(&next, seat);
        }
    }

    fn send_selection(&mut self, client: &ClientId, seat: usize) {
        let Some(handle) =
            client_object(client, &self.clients).and_then(|object| self.dh.get_client(object).ok())
        else {
            return;
        };

        let devices: Vec<WlDataDevice> = self
            .clients
            .iter()
            .find(|record| &record.id == client)
            .map(|record| {
                record
                    .devices
                    .iter()
                    .filter(|(index, device)| *index == seat && device.is_alive())
                    .map(|(_, device)| device.clone())
                    .collect()
            })
            .unwrap_or_default();

        let selection = self.seats[seat].selection.as_ref().and_then(|selection| {
            selection
                .source
                .is_alive()
                .then(|| (selection.source.clone(), selection.mimes.clone()))
        });

        for device in devices {
            match &selection {
                Some((source, mimes)) => {
                    let Ok(offer) = handle.create_resource::<WlDataOffer, OfferSource, Server>(
                        &self.dh,
                        device.version(),
                        OfferSource(source.clone()),
                    ) else {
                        continue;
                    };

                    device.data_offer(&offer);

                    for mime in mimes {
                        offer.offer(mime.clone());
                    }

                    device.selection(Some(&offer));
                }
                None => device.selection(None),
            }
        }
    }

    fn clear_selection(&mut self, seat: usize) {
        self.seats[seat].selection = None;

        if let Some(focus) = self.seats[seat].focus.clone() {
            self.send_selection(&focus, seat);
        }
    }

    fn forget_client(&mut self, id: &ClientId) {
        for seat in 0..self.seats.len() {
            if self.seats[seat].focus.as_ref() == Some(id) {
                self.seats[seat].focus = None;
            }

            let owned = self.seats[seat]
                .selection
                .as_ref()
                .is_some_and(|selection| &selection.owner == id);

            if owned {
                self.clear_selection(seat);
            }

            let owns_primary = self.seats[seat]
                .primary
                .as_ref()
                .is_some_and(|primary| &primary.owner == id);

            if owns_primary {
                self.seats[seat].primary = None;
                if let Some(focus) = self.seats[seat].focus.clone() {
                    self.send_primary(&focus, seat);
                }
            }
        }

        // Retain records so connection indices stay stable after client disconnection.
        if let Some(record) = self.record(id) {
            record.surfaces.clear();
            record.keyboards.clear();
            record.devices.clear();
            record.primary_devices.clear();
        }
    }
}

fn client_object(
    id: &ClientId,
    clients: &[ClientRecord],
) -> Option<wayland_server::backend::ObjectId> {
    clients
        .iter()
        .find(|record| &record.id == id)
        .and_then(|record| {
            record
                .devices
                .first()
                .map(|(_, device)| device.id())
                .or_else(|| record.keyboards.first().map(|(_, keyboard)| keyboard.id()))
                .or_else(|| record.surfaces.first().map(Resource::id))
        })
}

fn run(
    socket_path: PathBuf,
    config: Config,
    orders: Receiver<Control>,
    ready: Sender<Result<(), String>>,
) {
    let mut display: Display<Server> = match Display::new() {
        Ok(display) => display,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };

    let mut dh = display.handle();

    dh.create_global::<Server, WlCompositor, ()>(4, ());
    dh.create_global::<Server, WlDataDeviceManager, ()>(3, ());
    if config.primary {
        dh.create_global::<
            Server,
            wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
            (),
        >(1, ());
    }

    if config.drag_window {
        dh.create_global::<Server, wayland_server::protocol::wl_shm::WlShm, ()>(1, ());
        dh.create_global::<
            Server,
            wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase,
            (),
        >(1, ());
        dh.create_global::<
            Server,
            wayland_protocols::xdg::toplevel_drag::v1::server::xdg_toplevel_drag_manager_v1::XdgToplevelDragManagerV1,
            (),
        >(1, ());
    }

    for index in 0..config.seats {
        dh.create_global::<Server, WlSeat, SeatIndex>(7, SeatIndex(index));
    }

    let socket = match ListeningSocket::bind_absolute(socket_path) {
        Ok(socket) => socket,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };

    let gone = Arc::new(Mutex::new(Vec::new()));

    let mut server = Server {
        dh: dh.clone(),
        clients: Vec::new(),
        seats: (0..config.seats)
            .map(|_| SeatRecord {
                focus: None,
                selection: None,
                primary: None,
            })
            .collect(),
        serial: 0,
        stalling: false,
        discarding: false,
        startup_stall: config.startup_stall,
        startup_stalled: None,
        ignored_drag_release_leave: config.ignored_drag_release_leave,
        held: Vec::new(),
        refusals: 0,
        gone: Arc::clone(&gone),
        drag_window: drag_window::DragWindowRecord::default(),
    };

    if ready.send(Ok(())).is_err() {
        return;
    }

    loop {
        while let Ok(Some(stream)) = socket.accept() {
            match dh.insert_client(stream, Arc::new(Disconnects(Arc::clone(&gone)))) {
                Ok(client) => server.clients.push(ClientRecord {
                    id: client.id(),
                    surfaces: Vec::new(),
                    keyboards: Vec::new(),
                    pointers: Vec::new(),
                    devices: Vec::new(),
                    primary_devices: Vec::new(),
                    serials: HashSet::new(),
                }),
                Err(_) => continue,
            }
        }

        if display.dispatch_clients(&mut server).is_err() {
            break;
        }

        let departed: Vec<ClientId> = server
            .gone
            .lock()
            .map(|mut gone| gone.drain(..).collect())
            .unwrap_or_default();

        for id in departed {
            server.forget_client(&id);
        }

        let mut stop = false;

        while let Ok(order) = orders.try_recv() {
            match order {
                Control::Focus {
                    seat,
                    client,
                    reply,
                } => {
                    server.set_focus(seat, client);
                    let _ = reply.send(());
                }
                Control::PointerGrab { client, reply } => {
                    let serial = server.next_serial();
                    let sent = server.clients.get(client).is_some_and(|record| {
                        let Some(surface) = record.surfaces.first() else {
                            return false;
                        };
                        for pointer in &record.pointers {
                            pointer.enter(serial, surface, 1.0, 1.0);
                            pointer.button(serial, 0, 272, wl_pointer::ButtonState::Pressed);
                        }
                        !record.pointers.is_empty()
                    });
                    let _ = reply.send(sent);
                }
                Control::Clients { reply } => {
                    let _ = reply.send(server.clients.len());
                }
                Control::Stall { stalling, reply } => {
                    server.stalling = stalling;

                    if !stalling {
                        server.held.clear();
                    }

                    let _ = reply.send(());
                }
                Control::StartupStatus { reply } => {
                    let _ = reply.send(server.startup_stalled);
                }
                Control::ResumeStartup { reply } => {
                    server.startup_stall = None;
                    server.startup_stalled = None;
                    let _ = reply.send(());
                }
                Control::Discard { discarding, reply } => {
                    server.discarding = discarding;
                    let _ = reply.send(());
                }
                Control::SelectionOwner { seat, reply } => {
                    let owner = server
                        .seats
                        .get(seat)
                        .and_then(|record| record.selection.as_ref())
                        .map(|selection| selection.owner.clone())
                        .and_then(|owner| server.index_of(&owner));

                    let _ = reply.send(owner);
                }
                Control::SelectionMimes { seat, reply } => {
                    let mimes = server
                        .seats
                        .get(seat)
                        .and_then(|record| record.selection.as_ref())
                        .map(|selection| selection.mimes.clone())
                        .unwrap_or_default();

                    let _ = reply.send(mimes);
                }
                Control::PrimaryOwner { seat, reply } => {
                    let _ = reply.send(server.primary_owner(seat));
                }
                Control::Refusals { reply } => {
                    let _ = reply.send(server.refusals);
                }
                Control::DragIcon { reply } => {
                    let _ = reply.send(server.drag_window.icon());
                }
                Control::Attached { reply } => {
                    let _ = reply.send(server.drag_window.attached.clone());
                }
                Control::Stop => stop = true,
            }
        }

        if stop {
            break;
        }

        if server.startup_stalled.is_none() {
            let _ = display.flush_clients();
        }

        // Polling also services control orders and new connections.
        std::thread::sleep(Duration::from_millis(5));
    }

    let _ = display.flush_clients();
}

impl GlobalDispatch<WlCompositor, ()> for Server {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlCompositor>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WlCompositor, ()> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let surface = data_init.init(id, ());

                if let Some(record) = state.record(&client.id()) {
                    record.surfaces.push(surface);
                }
            }
            wl_compositor::Request::CreateRegion { id } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for Server {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        state.drag_window.surface_request(resource, &request);
    }
}

impl Dispatch<WlRegion, ()> for Server {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlRegion,
        _request: wl_region::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlSeat, SeatIndex> for Server {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlSeat>,
        data: &SeatIndex,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let seat = data_init.init(resource, *data);

        seat.capabilities(Capability::Keyboard | Capability::Pointer);

        if seat.version() >= 2 {
            seat.name(format!("seat{}", data.0));
        }

        state.stall_startup_at(StartupStall::FirstRoundTrip);
    }
}

impl Dispatch<WlSeat, SeatIndex> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &WlSeat,
        request: wl_seat::Request,
        data: &SeatIndex,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetPointer { id } = request {
            let pointer = data_init.init(id, ());
            if let Some(record) = state.record(&client.id()) {
                record.pointers.push(pointer);
            }
            return;
        }
        if let wl_seat::Request::GetKeyboard { id } = request {
            state.stall_startup_at(StartupStall::SecondRoundTrip);
            let keyboard = data_init.init(id, *data);

            // No keymap descriptor is sent: this compositor issues focus and
            // serials, and the clipboard runtime interprets no keys.
            if keyboard.version() >= 4 {
                keyboard.repeat_info(0, 0);
            }

            let seat = data.0;
            let focused = state
                .seats
                .get(seat)
                .and_then(|record| record.focus.clone())
                .is_some_and(|focus| focus == client.id());

            if let Some(record) = state.record(&client.id()) {
                record.keyboards.push((seat, keyboard.clone()));
            }

            if focused {
                let serial = state.next_serial();

                if let Some(record) = state.record(&client.id()) {
                    record.serials.insert(serial);

                    if let Some(surface) = record.surfaces.first().cloned() {
                        keyboard.enter(serial, &surface, Vec::new());
                    }
                }
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlPointer,
        _: wl_pointer::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<WlKeyboard, SeatIndex> for Server {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WlKeyboard,
        _request: wl_keyboard::Request,
        _data: &SeatIndex,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlDataDeviceManager, ()> for Server {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WlDataDeviceManager>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                data_init.init(id, SourceMimes::default());
            }
            wl_data_device_manager::Request::GetDataDevice { id, seat } => {
                let index = seat.data::<SeatIndex>().copied().unwrap_or(SeatIndex(0));

                let device = data_init.init(id, index);

                if let Some(record) = state.record(&client.id()) {
                    record.devices.push((index.0, device));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, SourceMimes> for Server {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WlDataSource,
        request: wl_data_source::Request,
        data: &SourceMimes,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_source::Request::Offer { mime_type } => {
                if let Ok(mut mimes) = data.0.lock() {
                    mimes.push(mime_type);
                }
            }
            wl_data_source::Request::Destroy => {
                for seat in 0..state.seats.len() {
                    let current = state.seats[seat]
                        .selection
                        .as_ref()
                        .is_some_and(|selection| selection.source == *resource);

                    if current {
                        state.clear_selection(seat);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataDevice, SeatIndex> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &WlDataDevice,
        request: wl_data_device::Request,
        data: &SeatIndex,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_device::Request::StartDrag { origin, icon, .. } = &request {
            state.refusals += 1;
            state.drag_window.started_with_icon(icon.as_ref());
            if state.ignored_drag_release_leave {
                let released = state.next_serial();
                let left = state.next_serial();
                if let Some(record) = state.record(&client.id()) {
                    for pointer in &record.pointers {
                        pointer.button(released, 0, 272, wl_pointer::ButtonState::Released);
                        pointer.leave(left, origin);
                    }
                }
            }
        }
        if let wl_data_device::Request::SetSelection { source, serial } = request {
            let seat = data.0;

            if seat >= state.seats.len() {
                return;
            }

            let focused = state.seats[seat].focus.as_ref() == Some(&client.id());

            let issued = state
                .clients
                .iter()
                .find(|record| record.id == client.id())
                .is_some_and(|record| record.serials.contains(&serial));

            if state.discarding {
                state.refusals += 1;
                return;
            }

            if !focused || !issued {
                state.refusals += 1;
                return;
            }

            if let Some(previous) = state.seats[seat].selection.take() {
                let replaced = source.as_ref() != Some(&previous.source);

                if replaced && previous.source.is_alive() {
                    previous.source.cancelled();
                }
            }

            state.seats[seat].selection = source.map(|source| {
                let mimes = source
                    .data::<SourceMimes>()
                    .and_then(|data| data.0.lock().ok().map(|mimes| mimes.clone()))
                    .unwrap_or_default();

                SelectionRecord {
                    source,
                    owner: client.id(),
                    mimes,
                }
            });

            if let Some(focus) = state.seats[seat].focus.clone() {
                state.send_selection(&focus, seat);
            }
        }
    }
}

impl Dispatch<WlDataOffer, OfferSource> for Server {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &WlDataOffer,
        request: wl_data_offer::Request,
        data: &OfferSource,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_offer::Request::Receive { mime_type, fd } = request {
            if !data.0.is_alive() {
                return;
            }

            if state.stalling {
                // Retain the writer so the receiver sees neither data nor EOF.
                state.held.push(fd);
                return;
            }

            data.0.send(mime_type, fd.as_fd());
        }
    }
}
