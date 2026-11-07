//! Keeping the clipboard after the application exits.
//!
//! On Wayland the clipboard is a promise: the copying application keeps the
//! data and sends it to whoever pastes. When it exits, the copy is gone,
//! unless a clipboard manager took a copy (GNOME's compositor and KDE's
//! Klipper do; wlroots compositors such as sway have none by default).
//!
//! [`hand_off`] gives what this process still owns to a keeper: the
//! application's own executable started again with [`KEEPER_ENV`] set. A
//! constructor that runs before `main` turns that process into the keeper,
//! so no application code runs in it. The keeper takes the selection through
//! `ext_data_control_v1` and serves it until another client copies, the
//! compositor goes away or [`MAX_KEEP`] passes; where the compositor offers
//! no data control it exits at once and the copy is lost as before.
//!
//! The keeper never replaces a newer copy. It takes the selection only when
//! the clipboard is empty (the application's copy went away with it) or
//! holds exactly the copy it was handed (the application has not exited
//! yet); anything else on the clipboard was copied since, and the keeper
//! exits without taking it.

use std::io::{self, Read, Write};
use std::sync::Mutex;
use std::time::Duration;

use crate::mime::MimeType;
use crate::payload::Payload;

/// Set in the keeper's environment; its value is ignored.
pub const KEEPER_ENV: &str = "ENZUI_CLIPBOARD_KEEPER";

/// The most bytes handed to a keeper; a larger copy is not kept.
pub const MAX_KEPT_BYTES: usize = 64 * 1024 * 1024;

/// How long a keeper serves at most.
pub const MAX_KEEP: Duration = Duration::from_secs(24 * 60 * 60);

static OWNED: Mutex<Option<Payload>> = Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_SELECTION: Mutex<()> = Mutex::new(());

/// Records that this process now owns the selection with `payload`.
pub(crate) fn remember(payload: &Payload) {
    if let Ok(mut owned) = OWNED.lock() {
        *owned = (payload.expires_at().is_none()).then(|| payload.clone());
    }
}

/// Records that this process no longer owns the selection.
pub(crate) fn forget() {
    if let Ok(mut owned) = OWNED.lock() {
        *owned = None;
    }
}

/// The selection this process owns, as far as the compositor has said.
#[must_use]
pub fn owned_selection() -> Option<Payload> {
    OWNED.lock().ok()?.clone()
}

/// Hands the selection this process owns to a keeper process, so it can be
/// pasted after this process exits. Call it as the application exits;
/// `app_setup::run` does. `Ok(false)` when there is nothing to keep, it is
/// too large, or this is not a Wayland session.
///
/// # Errors
/// The keeper could not be started or sent the payload.
pub fn hand_off() -> io::Result<bool> {
    // The constructor must be linked into every binary that hands off.
    std::hint::black_box(&KEEPER_ENTRY);
    let Some(payload) = owned_selection() else {
        return Ok(false);
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("WAYLAND_SOCKET").is_none()
    {
        return Ok(false);
    }
    let Some(encoded) = encode(&payload) else {
        return Ok(false);
    };
    let mut keeper = std::process::Command::new(std::env::current_exe()?);
    keeper
        .env(KEEPER_ENV, "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Its own process group: closing the terminal or the app's session
    // leader does not take the copy with it.
    std::os::unix::process::CommandExt::process_group(&mut keeper, 0);
    let mut child = keeper.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&encoded)?;
    }
    Ok(true)
}

/// The payload as the keeper reads it: per type, the name's length (u32),
/// the name, the data's length (u64) and the data, little-endian. `None`
/// when it is empty or larger than [`MAX_KEPT_BYTES`].
fn encode(payload: &Payload) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for mime in payload.mime_types() {
        let bytes = payload.bytes_for(&mime)?;
        let name = mime.as_str().as_bytes();
        out.extend_from_slice(&u32::try_from(name.len()).ok()?.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(&bytes);
        if out.len() > MAX_KEPT_BYTES {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

fn decode(mut input: &[u8]) -> Option<Payload> {
    let mut payload = Payload::new();
    let take = |count: usize, input: &mut &[u8]| -> Option<Vec<u8>> {
        let (head, rest) = (input.get(..count)?, input.get(count..)?);
        *input = rest;
        Some(head.to_vec())
    };
    let mut entries = 0;
    while !input.is_empty() {
        let name_len = u32::from_le_bytes(take(4, &mut input)?.try_into().ok()?);
        let name = String::from_utf8(take(usize::try_from(name_len).ok()?, &mut input)?).ok()?;
        let data_len = u64::from_le_bytes(take(8, &mut input)?.try_into().ok()?);
        let data = take(usize::try_from(data_len).ok()?, &mut input)?;
        payload = payload.with(MimeType::new(name), data);
        entries += 1;
    }
    (entries > 0).then_some(payload)
}

// SAFETY: the function only reads the environment and, in a keeper, serves
// the clipboard and exits. It touches no state that `main` sets up.
#[allow(unsafe_code, reason = "a constructor must be placed in .init_array")]
#[unsafe(link_section = ".init_array")]
#[used]
static KEEPER_ENTRY: extern "C" fn() = keeper_entry;

extern "C" fn keeper_entry() {
    if std::env::var_os(KEEPER_ENV).is_none() {
        return;
    }
    let mut input = Vec::new();
    let read = io::stdin()
        .take(MAX_KEPT_BYTES as u64 + 1)
        .read_to_end(&mut input);
    if let (Ok(_), Some(payload)) = (read, decode(&input)) {
        serve::run(&payload);
    }
    std::process::exit(0);
}

mod serve {
    //! The keeper's `ext_data_control_v1` client.

    use std::collections::HashMap;
    use std::os::fd::{AsFd, OwnedFd};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use wayland_client::EventQueue;
    use wayland_client::protocol::wl_callback::{self, WlCallback};

    use wayland_client::backend::ObjectId;

    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_seat::WlSeat;
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
    use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
        self, ExtDataControlDeviceV1,
    };
    use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
    use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{
        self, ExtDataControlOfferV1,
    };
    use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{
        self, ExtDataControlSourceV1,
    };

    use super::{MAX_KEEP, Payload};
    use crate::mime::MimeType;
    use crate::protocol::TransferPermit;
    use crate::transfer::{TransferLimits, read_bounded, transfer_pipe, write_bounded};

    struct Keeper {
        payload: Payload,
        done: bool,
        /// Sends in flight, under the same cap as an ordinary copy's.
        transfers: Arc<AtomicUsize>,
        /// The startup sync came back.
        synced: bool,
        /// The types each offer the compositor announced carries.
        offers: HashMap<ObjectId, Vec<String>>,
        /// The selection as the device last reported it: `None` before the
        /// first report, `Some(None)` for an empty clipboard.
        selection: Option<Option<ExtDataControlOfferV1>>,
    }

    /// Takes the selection with `payload` and serves it until it is replaced.
    pub(super) fn run(payload: &Payload) {
        let Ok(connection) = Connection::connect_to_env() else {
            return;
        };
        let Ok((globals, mut queue)) = registry_queue_init::<Keeper>(&connection) else {
            return;
        };
        let qh = queue.handle();
        let (Ok(manager), Ok(seat)) = (
            globals.bind::<ExtDataControlManagerV1, _, _>(&qh, 1..=1, ()),
            globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()),
        ) else {
            return;
        };
        let device = manager.get_data_device(&seat, &qh, ());
        let mut keeper = Keeper {
            payload: payload.clone(),
            done: false,
            transfers: Arc::new(AtomicUsize::new(0)),
            synced: false,
            offers: HashMap::new(),
            selection: None,
        };
        // A new device reports the current selection before the sync returns;
        // a compositor that never answers ends the keeper.
        let _sync = connection.display().sync(&qh, ());
        let startup = Instant::now() + TransferLimits::default().startup_timeout;
        while !keeper.synced {
            if !dispatch_until(&connection, &mut queue, &mut keeper, startup)
                || Instant::now() >= startup
            {
                return;
            }
        }
        let may_take = match keeper.selection.take() {
            Some(None) => true,
            Some(Some(offer)) => keeper.holds_the_copy(&connection, &offer),
            None => false,
        };
        if !may_take {
            return;
        }
        let source = manager.create_data_source(&qh, ());
        for mime in payload.mime_types() {
            source.offer(mime.into_string());
        }
        device.set_selection(Some(&source));
        let deadline = Instant::now() + MAX_KEEP;
        while !keeper.done && Instant::now() < deadline {
            if !dispatch_until(&connection, &mut queue, &mut keeper, deadline) {
                return;
            }
        }
    }

    /// Dispatches what arrives before `deadline`, waiting on the socket no
    /// longer than that: an idle clipboard still lets the keeper's lifetime
    /// run out. Returns `false` when the connection failed.
    fn dispatch_until<State>(
        connection: &Connection,
        queue: &mut EventQueue<State>,
        state: &mut State,
        deadline: Instant,
    ) -> bool {
        if queue.dispatch_pending(state).is_err() || connection.flush().is_err() {
            return false;
        }
        // Events already queued are dispatched on the next turn.
        let Some(guard) = queue.prepare_read() else {
            return true;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = Timespec {
            tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(left.subsec_nanos()),
        };
        let socket = guard.connection_fd();
        let mut fds = [PollFd::new(&socket, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => true,
            Ok(_) if fds[0].revents().intersects(PollFlags::IN) => {
                guard.read().is_ok() && queue.dispatch_pending(state).is_ok()
            }
            Ok(_) | Err(_) => false,
        }
    }

    impl Keeper {
        /// Whether `offer` is the copy this keeper was handed: the same types,
        /// each with the same bytes.
        fn holds_the_copy(&self, connection: &Connection, offer: &ExtDataControlOfferV1) -> bool {
            let mut offered = self.offers.get(&offer.id()).cloned().unwrap_or_default();
            let mut kept: Vec<String> = self
                .payload
                .mime_types()
                .into_iter()
                .map(MimeType::into_string)
                .collect();
            offered.sort();
            kept.sort();
            if offered != kept {
                return false;
            }
            kept.iter().all(|mime| {
                let Some(expected) = self.payload.bytes_for(&MimeType::new(mime.as_str())) else {
                    return false;
                };
                let Ok((read, write)) = transfer_pipe() else {
                    return false;
                };
                offer.receive(mime.clone(), write.as_fd());
                if connection.flush().is_err() {
                    return false;
                }
                drop(write);
                let limits = TransferLimits {
                    max_bytes: expected.len() + 1,
                    ..TransferLimits::default()
                };
                read_bounded(read, limits).is_ok_and(|bytes| bytes[..] == expected[..])
            })
        }

        /// Serves one paste, admitted like an ordinary copy's: past the
        /// transfer cap the descriptor is closed at once, so a flood of
        /// requests cannot pile up threads and descriptors.
        fn send(&self, mime: &str, fd: OwnedFd) {
            let Some(bytes) = self.payload.bytes_for(&MimeType::new(mime)) else {
                return;
            };
            let limits = TransferLimits::default();
            let Some(permit) = TransferPermit::claim(&self.transfers, limits) else {
                return;
            };
            let _ = std::thread::Builder::new()
                .name(String::from("enzui-clipboard-keep"))
                .spawn(move || {
                    let _ = write_bounded(fd, &bytes, limits);
                    drop(permit);
                });
        }
    }

    impl Dispatch<ExtDataControlSourceV1, ()> for Keeper {
        fn event(
            state: &mut Self,
            _: &ExtDataControlSourceV1,
            event: ext_data_control_source_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                    state.send(&mime_type, fd);
                }
                // Someone copied something else: the copy is no longer ours.
                ext_data_control_source_v1::Event::Cancelled => state.done = true,
                _ => {}
            }
        }
    }

    impl Dispatch<ExtDataControlDeviceV1, ()> for Keeper {
        fn event(
            state: &mut Self,
            _: &ExtDataControlDeviceV1,
            event: ext_data_control_device_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_data_control_device_v1::Event::Finished => state.done = true,
                ext_data_control_device_v1::Event::DataOffer { id } => {
                    state.offers.insert(id.id(), Vec::new());
                }
                ext_data_control_device_v1::Event::Selection { id } => {
                    state.selection = Some(id);
                }
                _ => {}
            }
        }

        event_created_child!(Keeper, ExtDataControlDeviceV1, [
            ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
        ]);
    }

    impl Dispatch<ExtDataControlOfferV1, ()> for Keeper {
        fn event(
            state: &mut Self,
            offer: &ExtDataControlOfferV1,
            event: ext_data_control_offer_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let ext_data_control_offer_v1::Event::Offer { mime_type } = event
                && let Some(types) = state.offers.get_mut(&offer.id())
            {
                types.push(mime_type);
            }
        }
    }

    impl Dispatch<WlCallback, ()> for Keeper {
        fn event(
            state: &mut Self,
            _: &WlCallback,
            event: wl_callback::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_callback::Event::Done { .. } = event {
                state.synced = true;
            }
        }
    }

    impl Dispatch<ExtDataControlManagerV1, ()> for Keeper {
        fn event(
            _: &mut Self,
            _: &ExtDataControlManagerV1,
            _: <ExtDataControlManagerV1 as Proxy>::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlSeat, ()> for Keeper {
        fn event(
            _: &mut Self,
            _: &WlSeat,
            _: <WlSeat as Proxy>::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for Keeper {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Read as _;
        use std::sync::atomic::Ordering;
        use std::time::Duration;

        #[test]
        fn an_idle_compositor_does_not_outlast_the_deadline() {
            let (ours, _silent) = std::os::unix::net::UnixStream::pair().unwrap();
            let connection = Connection::from_socket(ours).unwrap();
            let mut queue = connection.new_event_queue::<()>();
            let started = Instant::now();
            let alive = dispatch_until(
                &connection,
                &mut queue,
                &mut (),
                started + Duration::from_millis(100),
            );
            assert!(alive, "a quiet connection is not a failed one");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{:?}",
                started.elapsed()
            );
        }

        #[test]
        fn sends_past_the_transfer_cap_are_closed_at_once() {
            let payload = Payload::text("kept");
            let mime = payload.mime_types()[0].clone().into_string();
            let keeper = Keeper {
                payload,
                done: false,
                transfers: Arc::new(AtomicUsize::new(0)),
                synced: true,
                offers: HashMap::new(),
                selection: None,
            };
            let read_all = |read: OwnedFd| {
                let mut bytes = Vec::new();
                std::fs::File::from(read).read_to_end(&mut bytes).unwrap();
                bytes
            };

            let (read, write) = rustix::pipe::pipe().unwrap();
            keeper.send(&mime, write);
            assert_eq!(read_all(read), b"kept", "under the cap the paste is served");

            let cap = TransferLimits::default().max_concurrent_transfers;
            keeper.transfers.store(cap, Ordering::SeqCst);
            let (read, write) = rustix::pipe::pipe().unwrap();
            keeper.send(&mime, write);
            assert!(read_all(read).is_empty(), "refused: closed without data");
            assert_eq!(keeper.transfers.load(Ordering::SeqCst), cap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_survives_the_trip_to_the_keeper() {
        let payload = Payload::new()
            .with("text/plain;charset=utf-8", b"kept".to_vec())
            .with("text/html", b"<b>kept</b>".to_vec());
        let decoded = decode(&encode(&payload).unwrap()).unwrap();
        assert_eq!(decoded.mime_types(), payload.mime_types());
        for mime in payload.mime_types() {
            assert_eq!(decoded.bytes_for(&mime), payload.bytes_for(&mime));
        }
        assert!(
            decode(&encode(&payload).unwrap()[..9]).is_none(),
            "a cut stream is refused"
        );
        assert!(encode(&Payload::new()).is_none(), "nothing to keep");
    }

    #[test]
    fn a_copy_too_large_to_keep_is_not_handed_off() {
        let huge = Payload::new().with("application/octet-stream", vec![0; MAX_KEPT_BYTES + 1]);
        assert!(encode(&huge).is_none());
    }

    #[test]
    fn ownership_follows_offers_and_cancellation() {
        let _selection = TEST_SELECTION.lock().unwrap();
        forget();
        assert!(owned_selection().is_none());
        remember(&Payload::text("mine"));
        assert!(owned_selection().is_some());
        forget();
        assert!(owned_selection().is_none());
    }
}
