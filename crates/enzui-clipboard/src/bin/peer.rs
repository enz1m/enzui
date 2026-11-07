//! Line-driven Wayland clipboard peer for independent-process tests.
//!
//! Connects using `WAYLAND_DISPLAY` and accepts commands on stdin, returning
//! one response line on stdout. Payload bytes use hexadecimal encoding. Creates
//! a shell window when shell/shm globals exist, otherwise a bare surface whose
//! focus must be assigned by the test compositor.

use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use enzui_clipboard::lease::DisplayLease;
use enzui_clipboard::mime::MimeType;
use enzui_clipboard::{Kind, Payload, SeatSelector, TransferLimits, WaylandClipboard};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm::{self, Format, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

const WINDOW_SIZE: i32 = 64;
struct App {
    globals: Vec<(u32, String, u32)>,
    window: Option<Arc<Window>>,
}

impl App {
    fn interfaces(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .globals
            .iter()
            .map(|(_, interface, _)| interface.as_str())
            .collect();

        names.sort_unstable();
        names.dedup();
        names
    }

    fn global(&self, interface: &str) -> Option<(u32, u32)> {
        self.globals
            .iter()
            .find(|(_, name, _)| name == interface)
            .map(|(name, _, version)| (*name, *version))
    }
}

impl Dispatch<WlRegistry, ()> for App {
    fn event(
        state: &mut Self,
        _registry: &WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<WlCompositor, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WlSurface,
        _event: <WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        _event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for App {
    fn event(
        _state: &mut Self,
        proxy: &XdgWmBase,
        event: xdg_wm_base::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for App {
    fn event(
        state: &mut Self,
        proxy: &XdgSurface,
        event: xdg_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            proxy.ack_configure(serial);

            // Attach only to the current window after acknowledging its configure.
            let Some(window) = state.window.as_ref() else {
                return;
            };
            let (Ok(open), Some(buffer)) = (window.open.lock(), window.buffer.as_ref()) else {
                return;
            };

            if let Some(open) = open.as_ref()
                && open.xdg_surface.id() == proxy.id()
            {
                open.surface.attach(Some(buffer), 0, 0);
                open.surface.damage_buffer(0, 0, WINDOW_SIZE, WINDOW_SIZE);
                open.surface.commit();
            }
        }
    }
}

impl Dispatch<XdgToplevel, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &XdgToplevel,
        _event: xdg_toplevel::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

struct Open {
    surface: WlSurface,
    xdg_surface: XdgSurface,
    toplevel: XdgToplevel,
}

struct Window {
    compositor: WlCompositor,
    shell: Option<XdgWmBase>,
    buffer: Option<WlBuffer>,
    title: String,
    open: Mutex<Option<Open>>,
    bare: Mutex<Option<WlSurface>>,
}

impl Window {
    fn open(&self, qh: &QueueHandle<App>) -> Result<(), String> {
        let Some(shell) = self.shell.as_ref() else {
            return Err(String::from("no-shell"));
        };

        let mut open = self.open.lock().map_err(|_| String::from("poisoned"))?;

        if open.is_some() {
            return Err(String::from("already-open"));
        }

        let surface = self.compositor.create_surface(qh, ());
        let xdg_surface = shell.get_xdg_surface(&surface, qh, ());
        let toplevel = xdg_surface.get_toplevel(qh, ());

        toplevel.set_title(self.title.clone());
        toplevel.set_app_id(format!("dev.enzui.clipboard.peer.{}", self.title));
        surface.commit();

        *open = Some(Open {
            surface,
            xdg_surface,
            toplevel,
        });

        Ok(())
    }

    fn close(&self) -> Result<(), String> {
        let mut open = self.open.lock().map_err(|_| String::from("poisoned"))?;

        let Some(open) = open.take() else {
            return Err(String::from("not-open"));
        };

        open.toplevel.destroy();
        open.xdg_surface.destroy();
        open.surface.destroy();

        Ok(())
    }
}

fn build_window(registry: &WlRegistry, qh: &QueueHandle<App>, app: &App, title: &str) -> Window {
    let (compositor_name, compositor_version) = app
        .global("wl_compositor")
        .unwrap_or_else(|| fail("the compositor advertises no wl_compositor"));
    let compositor: WlCompositor =
        registry.bind(compositor_name, compositor_version.min(4), qh, ());

    let buffer = app.global("wl_shm").map(|(name, version)| {
        let shm: WlShm = registry.bind(name, version.min(1), qh, ());
        let stride = WINDOW_SIZE * 4;
        let size = stride * WINDOW_SIZE;
        let memory =
            rustix::fs::memfd_create("enzui-clipboard-peer", rustix::fs::MemfdFlags::CLOEXEC)
                .unwrap_or_else(|error| fail(&format!("memfd: {error}")));
        rustix::fs::ftruncate(&memory, size as u64)
            .unwrap_or_else(|error| fail(&format!("ftruncate: {error}")));

        let pool = shm.create_pool(memory.as_fd(), size, qh, ());
        pool.create_buffer(
            0,
            WINDOW_SIZE,
            WINDOW_SIZE,
            stride,
            Format::Argb8888,
            qh,
            (),
        )
    });

    let shell = app
        .global("xdg_wm_base")
        .filter(|_| buffer.is_some())
        .map(|(name, version)| registry.bind::<XdgWmBase, _, _>(name, version.min(3), qh, ()));

    let window = Window {
        compositor,
        shell,
        buffer,
        title: title.to_owned(),
        open: Mutex::new(None),
        bare: Mutex::new(None),
    };

    if window.shell.is_some() {
        window.open(qh).unwrap_or_else(|error| fail(&error));
    } else if let Ok(mut bare) = window.bare.lock() {
        let surface = window.compositor.create_surface(qh, ());
        surface.commit();
        *bare = Some(surface);
    }

    window
}

fn main() {
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(error) => fail(&format!("connect: {error}")),
    };

    let mut queue = connection.new_event_queue::<App>();
    let qh = queue.handle();
    let registry = connection.display().get_registry(&qh, ());

    let mut app = App {
        globals: Vec::new(),
        window: None,
    };

    if let Err(error) = queue.roundtrip(&mut app) {
        fail(&format!("registry: {error}"));
    }

    let title = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("peer"));
    let window = Arc::new(build_window(&registry, &qh, &app, &title));
    app.window = Some(Arc::clone(&window));
    let interfaces: Vec<String> = app.interfaces().into_iter().map(str::to_owned).collect();

    if let Err(error) = queue.roundtrip(&mut app) {
        fail(&format!("window: {error}"));
    }

    let clipboard = match start_clipboard(&connection) {
        Ok(clipboard) => clipboard,
        Err(error) => fail(&format!("clipboard: {error:?}")),
    };

    // The window's own queue keeps answering shell pings and configures while
    // the peer waits on its command stream.
    std::thread::Builder::new()
        .name(String::from("peer-window"))
        .spawn(move || while queue.blocking_dispatch(&mut app).is_ok() {})
        .unwrap_or_else(|error| fail(&format!("window thread: {error}")));

    reply(&format!("ready {}", std::process::id()));

    let deferred_session = enzui_clipboard::deferred::DeferredTextSession::default();
    let mut deferred = None;
    let stdin = std::io::stdin();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        if line == "quit" {
            reply("ok");
            break;
        }

        if let Some(answer) = run_deferred(&clipboard, &deferred_session, &mut deferred, line) {
            reply(&answer);
            continue;
        }
        reply(&run(
            &clipboard,
            &connection,
            &window,
            &qh,
            &interfaces,
            line,
        ));
    }
}

#[expect(
    unsafe_code,
    reason = "exercising the display-handle adoption path an iced application \
              uses requires the raw wl_display pointer of this process's own \
              live connection"
)]
fn start_clipboard(
    connection: &Connection,
) -> Result<WaylandClipboard, enzui_clipboard::ClipboardError> {
    let limits = limits_from_env();
    let seal = enzui_clipboard::bootstrap::AdoptionSeal::for_test();

    match std::env::var("ENZUI_CLIPBOARD_ADOPT").as_deref() {
        Ok("foreign-display") => {}
        // The peer's window thread reads this connection; the clipboard
        // waits for the events that thread routes to its queue.
        Ok("read-elsewhere") => {
            return WaylandClipboard::adopt_read_elsewhere(
                connection.clone(),
                limits,
                &seal,
                DisplayLease::new(connection.clone()),
            );
        }
        _ => return WaylandClipboard::adopt(connection, limits, &seal),
    }

    let display = std::ptr::NonNull::new(connection.backend().display_ptr().cast())
        .expect("a live connection has a display pointer");

    // SAFETY: the pointer belongs to `connection`, which this process owns and
    // keeps alive for the whole run; the lease holds a clone of that same
    // connection, so the display outlives every worker that dispatches on it.
    unsafe {
        WaylandClipboard::adopt_foreign_display(
            display,
            limits,
            &seal,
            DisplayLease::new(connection.clone()),
        )
    }
}

fn run_deferred(
    clipboard: &WaylandClipboard,
    session: &enzui_clipboard::deferred::DeferredTextSession,
    pending: &mut Option<enzui_clipboard::deferred::DeferredText>,
    line: &str,
) -> Option<String> {
    let mut words = line.split_whitespace();
    let result = match words.next()? {
        "defer" => {
            let (Some(millis), Some(max_bytes)) = (
                words.next().and_then(|s| s.parse::<u64>().ok()),
                words.next().and_then(|s| s.parse::<usize>().ok()),
            ) else {
                return Some("err bad-argument".into());
            };
            session
                .prepare(enzui_clipboard::deferred::DeferredTextLimits {
                    lifetime: Duration::from_millis(millis),
                    max_bytes,
                })
                .and_then(|(payload, fulfil)| {
                    clipboard.offer(payload)?;
                    *pending = Some(fulfil);
                    Ok(())
                })
        }
        "fulfil" => {
            let Some(text) = words.next().and_then(hex_to_string) else {
                return Some("err bad-argument".into());
            };
            pending
                .take()
                .ok_or(enzui_clipboard::ClipboardError::Stopped)
                .and_then(|fulfil| fulfil.fulfil(text))
        }
        "invalidate-deferred" => {
            session.invalidate_pending();
            Ok(())
        }
        "cancel-deferred" => {
            if let Some(fulfil) = pending.take() {
                fulfil.cancel();
            }
            Ok(())
        }
        _ => return None,
    };
    Some(answer(result))
}

fn run(
    clipboard: &WaylandClipboard,
    connection: &Connection,
    window: &Window,
    qh: &QueueHandle<App>,
    interfaces: &[String],
    line: &str,
) -> String {
    let mut words = line.split_whitespace();

    match words.next() {
        Some("owned") => format!(
            "ok {}",
            enzui_clipboard::keeper::owned_selection().is_some()
        ),
        Some("hand-off") => match enzui_clipboard::keeper::hand_off() {
            Ok(kept) => format!("ok {kept}"),
            Err(error) => format!("err {}", error.kind()),
        },
        Some("icon-drag") => {
            // A 2x2 icon with its hotspot on the bottom-right pixel, then this
            // window attached to the drag 12 px right and 6 px down of the grip.
            let pixel = [200, 100, 50, 128];
            let icon =
                enzui_clipboard::drag::DragIcon::from_rgba(&pixel.repeat(4), 2, 2, (1, 1), 1)
                    .unwrap_or_else(|error| fail(&format!("icon: {error:?}")));
            let native = clipboard.native_drag();
            let deadline = Instant::now() + Duration::from_secs(5);
            let source = loop {
                match native.start_with_icon(
                    Payload::text("tab"),
                    enzui_clipboard::drag::DragActions::MOVE,
                    SeatSelector::Focused,
                    Some(icon.clone()),
                ) {
                    Ok(source) => break source,
                    Err(enzui_clipboard::ClipboardError::NotFocused)
                        if Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return format!("err {error:?}"),
                }
            };
            let toplevel = window
                .open
                .lock()
                .ok()
                .and_then(|open| open.as_ref().map(|open| open.toplevel.clone()));
            let attached = match toplevel {
                Some(toplevel) => match native.attach_window(toplevel, (12, 6)) {
                    Ok(()) => String::from("attached"),
                    Err(error) => format!("refused:{error:?}").replace(' ', "_"),
                },
                None => String::from("no-window"),
            };
            loop {
                match source.try_result() {
                    Ok(Some(outcome)) => return format!("ok {attached} {outcome:?}"),
                    Err(error) => return format!("err {error:?}"),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(None) => return String::from("err source-pending"),
                }
            }
        }
        Some("file-drag") => {
            // Drags the named files out as `file://` URIs, as a file manager
            // does, once a press on this window allows a drag to start.
            let paths: Vec<std::path::PathBuf> = words.map(std::path::PathBuf::from).collect();
            let native = clipboard.native_drag();
            let deadline = Instant::now() + Duration::from_secs(15);
            let source = loop {
                match native.start(
                    Payload::uris(paths.iter()),
                    enzui_clipboard::drag::DragActions::COPY,
                    SeatSelector::Focused,
                ) {
                    Ok(source) => break source,
                    Err(enzui_clipboard::ClipboardError::NotFocused)
                        if Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return format!("err {error:?}"),
                }
            };
            loop {
                match source.try_result() {
                    Ok(Some(outcome)) => return format!("ok {outcome:?}"),
                    Err(error) => return format!("err {error:?}"),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(None) => return String::from("err source-pending"),
                }
            }
        }
        Some("ignored-drag") => {
            let native = clipboard.native_drag();
            let deadline = Instant::now() + Duration::from_secs(5);
            let source = loop {
                match native.start(
                    Payload::text("ignored"),
                    enzui_clipboard::drag::DragActions::COPY,
                    SeatSelector::Focused,
                ) {
                    Ok(source) => break source,
                    Err(enzui_clipboard::ClipboardError::NotFocused)
                        if Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return format!("err {error:?}"),
                }
            };
            loop {
                match source.try_result() {
                    Ok(Some(outcome)) => return format!("ok {outcome:?}"),
                    Err(error) => return format!("err {error:?}"),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(None) => return String::from("err source-pending"),
                }
            }
        }
        Some("offer") => match words.next() {
            Some("expired-sensitive") => {
                let Some(text) = words.next().and_then(hex_to_string) else {
                    return String::from("err bad-argument");
                };
                let payload = Payload::sensitive_text(
                    enzui_security::secret::SecretText::new(text),
                    Duration::from_millis(1),
                )
                .expect("fixture offer");
                std::thread::sleep(Duration::from_millis(20));
                answer(clipboard.offer(payload))
            }
            Some("sensitive") => {
                let (Some(millis), Some(text)) = (
                    words.next().and_then(|s| s.parse::<u64>().ok()),
                    words.next().and_then(hex_to_string),
                ) else {
                    return String::from("err bad-argument");
                };
                answer(
                    Payload::sensitive_text(
                        enzui_security::secret::SecretText::new(text),
                        Duration::from_millis(millis),
                    )
                    .and_then(|payload| clipboard.offer(payload)),
                )
            }
            Some("many-mimes") => {
                let mut payload = Payload::new();
                for index in 0..65 {
                    payload = payload.with(format!("application/x-{index}"), vec![42]);
                }
                payload = payload.with(format!("application/{}", "x".repeat(1025)), vec![43]);
                answer(clipboard.offer(payload))
            }
            Some("text") => {
                let Some(text) = words.next().and_then(hex_to_string) else {
                    return String::from("err bad-argument");
                };

                answer(clipboard.offer(Payload::text(text)))
            }
            Some("html") => {
                let (Some(html), Some(plain)) = (
                    words.next().and_then(hex_to_string),
                    words.next().and_then(hex_to_string),
                ) else {
                    return String::from("err bad-argument");
                };

                answer(clipboard.offer(Payload::html(html, plain)))
            }
            Some("mime") => {
                let (Some(mime), Some(bytes)) = (words.next(), words.next().and_then(from_hex))
                else {
                    return String::from("err bad-argument");
                };

                answer(clipboard.offer(Payload::new().with(mime, bytes)))
            }
            Some("uris") => {
                let Some(list) = words.next().and_then(hex_to_string) else {
                    return String::from("err bad-argument");
                };

                answer(clipboard.offer(Payload::uris(list.lines().map(std::path::PathBuf::from))))
            }
            Some("everything") => {
                // Bundle formats in one claim so they share the same input serial.
                let size: usize = words
                    .next()
                    .and_then(|size| size.parse().ok())
                    .unwrap_or(524_288);
                let image: Vec<u8> = (0..size).map(|index: usize| (index % 251) as u8).collect();

                let payload = Payload::html("<b>rich</b>", "rich")
                    .with(
                        "text/uri-list",
                        "file:///tmp/one.txt\r\nfile:///tmp/two.txt"
                            .as_bytes()
                            .to_vec(),
                    )
                    .with("image/png", image);

                answer(clipboard.offer(payload))
            }
            Some("bytes") => {
                let (Some(mime), Some(size)) = (
                    words.next(),
                    words.next().and_then(|size| size.parse().ok()),
                ) else {
                    return String::from("err bad-argument");
                };

                let bytes: Vec<u8> = (0..size).map(|index: usize| (index % 251) as u8).collect();

                answer(clipboard.offer(Payload::new().with(mime, bytes)))
            }
            _ => String::from("err bad-command"),
        },
        Some("paste") => {
            let outcome = match words.next() {
                Some("text") => clipboard.paste(Kind::Text),
                Some("html") => clipboard.paste(Kind::Html),
                Some("image") => clipboard.paste(Kind::Image),
                Some("uris") => clipboard.paste(Kind::Uris),
                Some("mime") => match words.next() {
                    Some(mime) => {
                        clipboard.paste_preferring(vec![MimeType::new(mime)], SeatSelector::Focused)
                    }
                    None => return String::from("err bad-argument"),
                },
                Some("any") => {
                    clipboard.paste_preferring(Kind::Text.preferences(), SeatSelector::Any)
                }
                Some("seat") => match words.next() {
                    Some(seat) => clipboard.paste_preferring(
                        Kind::Text.preferences(),
                        SeatSelector::Named(seat.to_owned()),
                    ),
                    None => return String::from("err bad-argument"),
                },
                _ => return String::from("err bad-command"),
            };

            match outcome {
                Ok(pasted) => format!("ok {} {}", pasted.mime(), to_hex(pasted.bytes())),
                Err(error) => format!("err {error:?}"),
            }
        }
        Some("offer-primary") => {
            let Some(text) = words.next().and_then(hex_to_string) else {
                return String::from("err bad-argument");
            };

            answer(clipboard.offer_primary(Payload::text(text)))
        }
        Some("offer-sensitive-primary") => {
            let Some(text) = words.next().and_then(hex_to_string) else {
                return String::from("err bad-argument");
            };
            answer(
                Payload::sensitive_text(
                    enzui_security::secret::SecretText::new(text),
                    Duration::from_secs(1),
                )
                .and_then(|payload| clipboard.offer_primary(payload)),
            )
        }
        Some("offer-sensitive-drag") => {
            let Some(text) = words.next().and_then(hex_to_string) else {
                return String::from("err bad-argument");
            };
            answer(
                Payload::sensitive_text(
                    enzui_security::secret::SecretText::new(text),
                    Duration::from_secs(1),
                )
                .and_then(|payload| {
                    clipboard
                        .native_drag()
                        .start(
                            payload,
                            enzui_clipboard::drag::DragActions::COPY,
                            SeatSelector::Focused,
                        )
                        .map(|_| ())
                }),
            )
        }
        Some("paste-primary") => match clipboard.paste_primary(Kind::Text) {
            Ok(pasted) => format!("ok {} {}", pasted.mime(), to_hex(pasted.bytes())),
            Err(error) => format!("err {error:?}"),
        },
        Some("offer-on-seat") => {
            let (Some(seat), Some(text)) = (words.next(), words.next().and_then(hex_to_string))
            else {
                return String::from("err bad-argument");
            };

            answer(clipboard.offer_on(Payload::text(text), SeatSelector::Named(seat.to_owned())))
        }
        Some("clear") => answer(clipboard.clear()),
        Some("seats") => match clipboard.seats() {
            Ok(seats) => {
                let rendered: Vec<String> = seats
                    .iter()
                    .map(|seat| {
                        let mimes: Vec<&str> = seat.offered.iter().map(MimeType::as_str).collect();

                        format!(
                            "{}:{}:{}:{}",
                            seat.name,
                            if seat.focused { "focused" } else { "unfocused" },
                            if seat.owns_selection {
                                "owner"
                            } else {
                                "guest"
                            },
                            mimes.join(",")
                        )
                    })
                    .collect();

                format!("ok {}", rendered.join("|"))
            }
            Err(error) => format!("err {error:?}"),
        },
        Some("wait-focus") => wait(words.next(), || {
            clipboard
                .seats()
                .is_ok_and(|seats| seats.iter().any(|seat| seat.focused))
        }),
        Some("wait-offer") => wait(words.next(), || {
            clipboard
                .seats()
                .is_ok_and(|seats| seats.iter().any(|seat| !seat.offered.is_empty()))
        }),
        Some("wait-no-offer") => wait(words.next(), || {
            clipboard
                .seats()
                .is_ok_and(|seats| seats.iter().all(|seat| seat.offered.is_empty()))
        }),
        Some("wait-lost-ownership") => wait(words.next(), || {
            clipboard
                .seats()
                .is_ok_and(|seats| seats.iter().all(|seat| !seat.owns_selection))
        }),
        Some("globals") => format!("ok {}", interfaces.join(",")),
        Some("close-window") => match window.close() {
            Ok(()) => {
                let _ = connection.flush();
                String::from("ok")
            }
            Err(error) => format!("err {error}"),
        },
        Some("open-window") => match window.open(qh) {
            Ok(()) => {
                let _ = connection.flush();
                String::from("ok")
            }
            Err(error) => format!("err {error}"),
        },
        Some("env") => format!(
            "ok display={} xauthority={} wayland={}",
            described(std::env::var("DISPLAY").ok()),
            described(std::env::var("XAUTHORITY").ok()),
            described(std::env::var("WAYLAND_DISPLAY").ok())
        ),
        _ => String::from("err bad-command"),
    }
}

fn wait(budget: Option<&str>, mut condition: impl FnMut() -> bool) -> String {
    let Some(budget) = budget.and_then(|budget| budget.parse().ok()) else {
        return String::from("err bad-argument");
    };

    let deadline = Instant::now() + Duration::from_millis(budget);

    while Instant::now() < deadline {
        if condition() {
            return String::from("ok");
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    String::from("err timeout")
}

fn described(value: Option<String>) -> String {
    value.unwrap_or_else(|| String::from("unset"))
}

fn answer(outcome: Result<(), enzui_clipboard::ClipboardError>) -> String {
    match outcome {
        Ok(()) => String::from("ok"),
        Err(error) => format!("err {error:?}"),
    }
}

fn limits_from_env() -> TransferLimits {
    let mut limits = TransferLimits::default();

    if let Some(millis) = env_number("ENZUI_CLIPBOARD_RECEIVE_TIMEOUT_MS") {
        limits.receive_timeout = Duration::from_millis(millis as u64);
    }

    if let Some(millis) = env_number("ENZUI_CLIPBOARD_SEND_TIMEOUT_MS") {
        limits.send_timeout = Duration::from_millis(millis as u64);
    }

    if let Some(bytes) = env_number("ENZUI_CLIPBOARD_MAX_BYTES") {
        limits.max_bytes = bytes;
    }

    if let Some(millis) = env_number("ENZUI_CLIPBOARD_CLAIM_TIMEOUT_MS") {
        limits.claim_timeout = Duration::from_millis(millis as u64);
    }

    if let Some(millis) = env_number("ENZUI_CLIPBOARD_STARTUP_TIMEOUT_MS") {
        limits.startup_timeout = Duration::from_millis(millis as u64);
    }

    limits
}

fn env_number(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.parse().ok()
}

fn reply(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

fn fail(reason: &str) -> ! {
    reply(&format!("fatal {reason}"));
    std::process::exit(2);
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }

    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn hex_to_string(text: &str) -> Option<String> {
    String::from_utf8(from_hex(text)?).ok()
}
