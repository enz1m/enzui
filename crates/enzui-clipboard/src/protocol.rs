//! Per-seat `wl_data_device` state and shared transfer admission.
//!
//! Selection writes require this application's keyboard focus and an input
//! serial observed on its connection. Keyboard and pointer events supply serials.
//! Claim confirmation compares advertised MIME multisets; it cannot independently
//! distinguish a competing source that advertises identical types.

use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};

use enzui_core::monotonic_ms::{Policy, checked_instant_deadline};

use crate::error::ClipboardError;
use crate::mime::{MimeType, negotiate};
use crate::payload::{OfferedBytes, Pasted, Payload};
use crate::transfer::{
    TransferLimits, read_bounded, transfer_pipe, write_bounded, write_sensitive_bounded,
};

mod drag;
mod drag_icon;
mod primary;

/// The seat a request applies to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SeatSelector {
    /// Write to all focused seats with serials; read from the first focused seat
    /// with a selection, falling back to the first focused seat. The default selector.
    #[default]
    Focused,

    /// Select one named seat; writes still require focus and an input serial.
    Named(String),

    /// Read from the first seat with a selection, regardless of focus.
    /// Writes still target all focused seats with serials; this grants no privileged access.
    Any,
}

/// What one seat currently looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatStatus {
    /// The compositor's name, or `seat-<global>` until a name event arrives.
    pub name: String,

    /// Whether this application holds keyboard focus on the seat.
    pub focused: bool,

    /// Whether a local selection source exists, including an unconfirmed claim.
    pub owns_selection: bool,

    /// Whether a local primary-selection source exists.
    pub owns_primary: bool,

    /// Last received selection types; focus loss alone does not clear them.
    pub offered: Vec<MimeType>,
}

/// Per-offer accumulator for the `wl_data_offer::offer` events that arrive
/// before the offer is attached to a selection.
#[derive(Debug, Default)]
pub struct OfferData {
    mimes: Mutex<Vec<MimeType>>,
    source_actions: std::sync::atomic::AtomicU32,
}

impl OfferData {
    fn push(&self, mime: MimeType) {
        if let Ok(mut mimes) = self.mimes.lock() {
            mimes.push(mime);
        }
    }

    fn snapshot(&self) -> Vec<MimeType> {
        self.mimes
            .lock()
            .map(|mimes| mimes.clone())
            .unwrap_or_default()
    }
}

/// The payload a `wl_data_source` promises, plus the transfer budget for
/// serving it.
#[derive(Debug)]
pub struct SourceData {
    payload: Mutex<Option<Payload>>,
    active: Arc<AtomicBool>,
    expires: Option<Instant>,
    limits: TransferLimits,
    transfers: Arc<AtomicUsize>,
    native_drag: bool,
}

impl SourceData {
    fn new(
        payload: Payload,
        limits: TransferLimits,
        transfers: Arc<AtomicUsize>,
        native_drag: bool,
    ) -> Self {
        Self {
            expires: payload.expires_at(),
            payload: Mutex::new(Some(payload)),
            active: Arc::new(AtomicBool::new(true)),
            limits,
            transfers,
            native_drag,
        }
    }

    fn bytes_for(&self, mime: &MimeType) -> Option<OfferedBytes> {
        if !self.active.load(Ordering::Acquire)
            || self.expires.is_some_and(|at| at <= Instant::now())
        {
            return None;
        }
        self.payload.lock().ok()?.as_ref()?.offered_bytes(mime)
    }

    fn revoke(&self) {
        self.active.store(false, Ordering::Release);
        let payload = self
            .payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(payload) = payload {
            payload.revoke_deferred();
        }
    }

    fn transfer(&self, mime: &MimeType) -> Option<SourceTransfer> {
        Some(SourceTransfer {
            bytes: self.bytes_for(mime)?,
            active: Arc::clone(&self.active),
            expires: self.expires,
            limits: self.limits,
            _permit: TransferPermit::claim(&self.transfers, self.limits)?,
        })
    }
}

struct SourceTransfer {
    bytes: OfferedBytes,
    active: Arc<AtomicBool>,
    expires: Option<Instant>,
    limits: TransferLimits,
    _permit: TransferPermit,
}

impl SourceTransfer {
    fn write(self, fd: std::os::fd::OwnedFd) -> Result<(), ClipboardError> {
        let bytes = match &self.bytes {
            OfferedBytes::Ordinary(bytes) => bytes.as_ref(),
            OfferedBytes::Sensitive(text) => text.expose_secret().as_bytes(),
            OfferedBytes::Deferred(source) => {
                let (bytes, deadline) = source.wait(&self.active, self.limits.send_timeout)?;
                return write_sensitive_bounded(fd, &bytes, self.limits, &self.active, deadline);
            }
        };
        match self.expires {
            Some(deadline) => {
                write_sensitive_bounded(fd, bytes, self.limits, &self.active, deadline)
            }
            None => write_bounded(fd, bytes, self.limits),
        }
    }
}

impl Drop for SourceData {
    fn drop(&mut self) {
        self.revoke();
    }
}

fn destroy_source(source: WlDataSource) {
    if let Some(data) = source.data::<SourceData>() {
        data.revoke();
    }
    source.destroy();
}

struct ClaimGroup {
    payload: Payload,
    reply: Mutex<Option<Sender<Result<(), ClipboardError>>>>,
    remaining: AtomicUsize,
}

impl ClaimGroup {
    fn new(reply: Sender<Result<(), ClipboardError>>, seats: usize, payload: Payload) -> Arc<Self> {
        Arc::new(Self {
            payload,
            reply: Mutex::new(Some(reply)),
            remaining: AtomicUsize::new(seats),
        })
    }

    fn confirm(&self) {
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1
            && let Ok(mut reply) = self.reply.lock()
            && let Some(reply) = reply.take()
        {
            let _ = reply.send(self.payload.confirm_deferred());
        }
    }

    fn refuse(&self, error: ClipboardError) {
        self.payload.revoke_deferred();
        if let Ok(mut reply) = self.reply.lock()
            && let Some(reply) = reply.take()
        {
            let _ = reply.send(Err(error));
        }
    }
}

struct PendingClaim {
    group: Arc<ClaimGroup>,
    source: ObjectId,
    expected: Vec<MimeType>,
    deadline: Instant,
    budget: Duration,
}

struct Seat {
    global: u32,
    proxy: WlSeat,
    name: String,
    keyboard: Option<WlKeyboard>,
    device: Option<WlDataDevice>,
    focused_surface: Option<ObjectId>,
    serial: Option<u32>,
    selection: Option<WlDataOffer>,
    selection_mimes: Vec<MimeType>,
    source: Option<WlDataSource>,
    pointer: Option<WlPointer>,
    claim: Option<PendingClaim>,
    drag: drag::SeatDrag,
    primary: primary::SeatPrimary,
}

impl Seat {
    fn status(&self) -> SeatStatus {
        SeatStatus {
            name: self.name.clone(),
            focused: self.focused_surface.is_some(),
            owns_selection: self.source.is_some(),
            owns_primary: self.primary.owns(),
            offered: self.selection_mimes.clone(),
        }
    }

    fn selection_serial(&self) -> Option<u32> {
        self.focused_surface.as_ref()?;
        self.serial
    }
}

/// Protocol state for one clipboard worker and its shared transfer slots.
pub struct State {
    conn: Connection,
    manager: WlDataDeviceManager,
    primary_manager: Option<
        wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
    >,
    seats: Vec<Seat>,
    limits: TransferLimits,
    transfers: Arc<AtomicUsize>,
    fatal: Option<ClipboardError>,
    drag: drag::NativeState,
    drag_globals: drag_icon::DragGlobals,
}

impl State {
    /// Creates the state and binds the clipboard globals.
    ///
    /// Binds the ordinary data-device manager and currently advertised seats.
    /// Empty seat lists are accepted; later requests report `NoSeat`.
    ///
    /// # Errors
    ///
    /// Returns `NoDataDeviceManager` when a compatible manager cannot be bound.
    pub fn new(
        conn: Connection,
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<State>,
        limits: TransferLimits,
    ) -> Result<Self, ClipboardError> {
        let manager: WlDataDeviceManager = globals
            .bind(qh, 1..=4, ())
            .map_err(|_| ClipboardError::NoDataDeviceManager)?;

        // Optional: without it the primary selection is unavailable while the
        // ordinary clipboard keeps working.
        let primary_manager = globals.bind(qh, 1..=1, ()).ok();

        let mut state = Self {
            conn,
            manager,
            primary_manager,
            seats: Vec::new(),
            limits,
            transfers: Arc::new(AtomicUsize::new(0)),
            fatal: None,
            drag: drag::NativeState::default(),
            drag_globals: drag_icon::DragGlobals::bind(globals, qh),
        };

        for global in globals.contents().clone_list() {
            if global.interface == WlSeat::interface().name {
                state.add_seat(globals.registry(), global.name, global.version, qh);
            }
        }

        Ok(state)
    }

    /// Return the first recorded fatal error, if any.
    pub fn fatal(&self) -> Option<ClipboardError> {
        self.fatal.clone()
    }

    /// Record the first fatal error without immediately resolving pending claims.
    pub fn fail(&mut self, error: ClipboardError) {
        self.fatal.get_or_insert(error);
    }

    /// One status entry per seat, in the order the compositor advertised them.
    pub fn statuses(&self) -> Vec<SeatStatus> {
        self.seats.iter().map(Seat::status).collect()
    }

    /// Claims the selection on every seat the selector resolves to.
    ///
    /// Requires focus and serials, including with `Named` or `Any`. Success here
    /// means the requests were flushed; `reply` receives the later aggregate
    /// MIME confirmation or refusal. Replaces pending claims on targeted seats.
    ///
    /// # Errors
    ///
    /// Returns the stored fatal error, seat/focus errors, `MimeUnavailable` for
    /// empty payloads, or `Disconnected` if flushing fails.
    pub fn offer(
        &mut self,
        payload: Payload,
        selector: &SeatSelector,
        qh: &QueueHandle<State>,
        reply: Sender<Result<(), ClipboardError>>,
    ) -> Result<(), ClipboardError> {
        if let Some(error) = self.fatal() {
            return Err(error);
        }

        if payload.expires_at().is_some_and(|at| at <= Instant::now()) {
            return Err(ClipboardError::Stopped);
        }

        if payload.is_empty() {
            return Err(ClipboardError::MimeUnavailable {
                offered: Vec::new(),
            });
        }

        if let Some(source) = payload.deferred_source() {
            if source.revoked() {
                return Err(ClipboardError::Stopped);
            }
            if source.limits.max_bytes > self.limits.max_bytes {
                return Err(ClipboardError::TransferTooLarge {
                    limit: self.limits.max_bytes,
                });
            }
        }

        let targets = self.writable_seats(selector)?;
        let limits = self.limits;
        let transfers = Arc::clone(&self.transfers);
        let manager = self.manager.clone();
        let group = ClaimGroup::new(reply, targets.len(), payload.clone());
        let expected = payload.mime_types();
        // Overflow expires immediately; a claim never becomes unbounded.
        let deadline = Self::claim_deadline(Instant::now(), limits);

        for index in targets {
            let seat = &mut self.seats[index];
            let Some(serial) = seat.selection_serial() else {
                continue;
            };
            let Some(device) = seat.device.clone() else {
                continue;
            };

            let source = manager.create_data_source(
                qh,
                SourceData::new(payload.clone(), limits, Arc::clone(&transfers), false),
            );

            for mime in payload.mime_types() {
                source.offer(mime.into_string());
            }

            device.set_selection(Some(&source), serial);

            // Delay the reply until a selection event matches the advertised types.
            seat.claim = Some(PendingClaim {
                group: Arc::clone(&group),
                source: source.id(),
                expected: expected.clone(),
                deadline,
                budget: limits.claim_timeout,
            });

            // Cancellation must identify the old source before clearing the current one.
            if let Some(previous) = seat.source.replace(source)
                && previous
                    .data::<SourceData>()
                    .is_some_and(|data| data.expires.is_some())
            {
                destroy_source(previous);
            }
            crate::keeper::remember(&payload);
        }

        self.flush()
    }

    #[inline]
    fn claim_deadline(now: Instant, limits: TransferLimits) -> Instant {
        checked_instant_deadline(now, limits.claim_timeout, Policy::Expire).unwrap_or(now)
    }

    /// Refuse elapsed claims and destroy their sources when this method is called.
    ///
    /// Deadline overflow expires immediately; expiry is not a separate timer thread.
    pub fn expire_claims(&mut self) {
        let now = Instant::now();

        for seat in &mut self.seats {
            let expired = seat
                .claim
                .as_ref()
                .is_some_and(|claim| claim.deadline <= now);

            if expired && let Some(claim) = seat.claim.take() {
                claim.group.refuse(ClipboardError::ClaimNotHonoured {
                    after: claim.budget,
                });

                if let Some(source) = seat.source.take() {
                    destroy_source(source);
                }
            }
        }
    }

    /// When the worker must next answer an unconfirmed claim.
    pub fn next_claim_deadline(&self) -> Option<Instant> {
        self.seats
            .iter()
            .filter_map(|seat| seat.claim.as_ref().map(|claim| claim.deadline))
            .min()
    }

    pub(crate) fn expire_timed_sources(&mut self) {
        let now = Instant::now();
        for seat in &mut self.seats {
            let expired = seat
                .source
                .as_ref()
                .and_then(|source| source.data::<SourceData>())
                .is_some_and(|data| {
                    data.expires.is_some_and(|at| at <= now)
                        || data
                            .payload
                            .lock()
                            .ok()
                            .and_then(|payload| {
                                payload
                                    .as_ref()
                                    .and_then(Payload::deferred_source)
                                    .map(|source| source.revoked())
                            })
                            .unwrap_or(false)
                });
            if expired && let Some(source) = seat.source.take() {
                if seat
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.source == source.id())
                    && let Some(claim) = seat.claim.take()
                {
                    claim.group.refuse(ClipboardError::Stopped);
                }
                // Destroying this source lets the compositor revoke that identity only.
                // A global set_selection(None) could race a newer clipboard owner.
                destroy_source(source);
            }
        }
    }

    pub(crate) fn next_source_deadline(&self) -> Option<Instant> {
        self.seats
            .iter()
            .filter_map(|seat| {
                seat.source
                    .as_ref()
                    .and_then(|source| source.data::<SourceData>())
                    .and_then(|data| data.expires)
            })
            .min()
    }

    /// Request selection removal on resolved focused seats and destroy local sources.
    ///
    /// Success reports flushing, not compositor confirmation. Pending claims are
    /// retained until a later selection event, cancellation, expiry or state drop.
    ///
    /// # Errors
    ///
    /// Returns the stored fatal error, seat/focus errors or `Disconnected` on flush failure.
    pub fn clear(&mut self, selector: &SeatSelector) -> Result<(), ClipboardError> {
        if let Some(error) = self.fatal() {
            return Err(error);
        }

        let targets = self.writable_seats(selector)?;

        for index in targets {
            let seat = &mut self.seats[index];
            let (Some(serial), Some(device)) = (seat.selection_serial(), seat.device.clone())
            else {
                continue;
            };

            device.set_selection(None, serial);

            if let Some(source) = seat.source.take() {
                destroy_source(source);
            }
        }
        if self.seats.iter().all(|seat| seat.source.is_none()) {
            crate::keeper::forget();
        }

        self.flush()
    }

    /// Starts a paste: negotiates a type, asks the owner for it and hands the
    /// descriptor to a transfer thread.
    ///
    /// Claims a shared transfer slot before requesting bytes; the reader sends
    /// the eventual result through `reply` and releases its slot on completion.
    ///
    /// # Errors
    ///
    /// Returns the stored fatal error, seat/focus/selection or negotiation errors,
    /// `TransferFailed` for admission/pipe/thread failure, or `Disconnected` on flush failure.
    pub fn paste(
        &mut self,
        preferences: &[MimeType],
        selector: &SeatSelector,
        reply: std::sync::mpsc::Sender<Result<Pasted, ClipboardError>>,
    ) -> Result<(), ClipboardError> {
        if let Some(error) = self.fatal() {
            return Err(error);
        }

        let index = self.readable_seat(selector)?;
        let seat = &self.seats[index];

        let Some(offer) = seat.selection.clone() else {
            return Err(ClipboardError::NoSelection);
        };

        let Some(mime) = negotiate(preferences, &seat.selection_mimes) else {
            return Err(ClipboardError::MimeUnavailable {
                offered: seat.selection_mimes.clone(),
            });
        };

        let limits = self.limits;
        let transfers = Arc::clone(&self.transfers);

        // Reserve capacity before `receive`; the owner may start writing as
        // soon as the request is flushed.
        let (read_end, permit) =
            prepare_paste_transfer(&transfers, limits, transfer_pipe, |write_end| {
                offer.receive(mime.as_str().to_owned(), write_end.as_fd());
                drop(write_end);
                self.flush()
            })?;

        spawn_paste_reader(read_end, mime, limits, reply, permit)
    }

    /// Destroy sources and offers, release supported input/data-device objects,
    /// and attempt to flush those requests.
    ///
    /// Does not wait for compositor acknowledgement or clear pending claims;
    /// their reply channels close when the state is subsequently dropped.
    pub fn shutdown(&mut self) {
        self.shutdown_drags();
        for seat in &mut self.seats {
            if let Some(source) = seat.source.take() {
                destroy_source(source);
            }

            if let Some(offer) = seat.selection.take() {
                offer.destroy();
            }

            seat.primary.release();

            if let Some(keyboard) = seat.keyboard.take()
                && keyboard.version() >= 3
            {
                keyboard.release();
            }

            if let Some(pointer) = seat.pointer.take()
                && pointer.version() >= 3
            {
                pointer.release();
            }

            if let Some(device) = seat.device.take()
                && device.version() >= 2
            {
                device.release();
            }
        }

        let _ = self.conn.flush();
    }

    fn flush(&self) -> Result<(), ClipboardError> {
        self.conn.flush().map_err(|_| ClipboardError::Disconnected)
    }

    fn writable_seats(&self, selector: &SeatSelector) -> Result<Vec<usize>, ClipboardError> {
        if self.seats.is_empty() {
            return Err(ClipboardError::NoSeat);
        }

        let focused: Vec<usize> = match selector {
            SeatSelector::Named(name) => {
                let index = self
                    .seats
                    .iter()
                    .position(|seat| &seat.name == name)
                    .ok_or_else(|| ClipboardError::UnknownSeat {
                        requested: name.clone(),
                    })?;

                self.seats[index]
                    .selection_serial()
                    .map(|_| vec![index])
                    .unwrap_or_default()
            }
            SeatSelector::Focused | SeatSelector::Any => self
                .seats
                .iter()
                .enumerate()
                .filter(|(_, seat)| seat.selection_serial().is_some())
                .map(|(index, _)| index)
                .collect(),
        };

        if focused.is_empty() {
            return Err(ClipboardError::NotFocused);
        }

        Ok(focused)
    }

    fn readable_seat(&self, selector: &SeatSelector) -> Result<usize, ClipboardError> {
        if self.seats.is_empty() {
            return Err(ClipboardError::NoSeat);
        }

        match selector {
            SeatSelector::Named(name) => self
                .seats
                .iter()
                .position(|seat| &seat.name == name)
                .ok_or_else(|| ClipboardError::UnknownSeat {
                    requested: name.clone(),
                }),
            SeatSelector::Focused => self
                .seats
                .iter()
                .position(|seat| seat.focused_surface.is_some() && seat.selection.is_some())
                .or_else(|| {
                    self.seats
                        .iter()
                        .position(|seat| seat.focused_surface.is_some())
                })
                .ok_or(ClipboardError::NotFocused),
            SeatSelector::Any => self
                .seats
                .iter()
                .position(|seat| seat.selection.is_some())
                .ok_or(ClipboardError::NoSelection),
        }
    }

    fn add_seat(
        &mut self,
        registry: &WlRegistry,
        global: u32,
        version: u32,
        qh: &QueueHandle<State>,
    ) {
        if self.seats.iter().any(|seat| seat.global == global) {
            return;
        }

        let version = version.min(7);
        let proxy: WlSeat = registry.bind(global, version, qh, global);
        let device = self.manager.get_data_device(&proxy, qh, global);
        let primary = primary::SeatPrimary::new(self.primary_manager.as_ref(), &proxy, global, qh);

        self.seats.push(Seat {
            global,
            proxy,
            name: format!("seat-{global}"),
            keyboard: None,
            device: Some(device),
            focused_surface: None,
            serial: None,
            selection: None,
            selection_mimes: Vec::new(),
            source: None,
            pointer: None,
            claim: None,
            drag: drag::SeatDrag::default(),
            primary,
        });
    }

    fn remove_seat(&mut self, global: u32) {
        self.remove_seat_drags(global);
        let Some(index) = self.seats.iter().position(|seat| seat.global == global) else {
            return;
        };

        let mut seat = self.seats.remove(index);

        if let Some(claim) = seat.claim.take() {
            claim.group.refuse(ClipboardError::ClaimNotHonoured {
                after: claim.budget,
            });
        }

        if let Some(source) = seat.source.take() {
            destroy_source(source);
        }

        if let Some(offer) = seat.selection.take() {
            offer.destroy();
        }

        seat.primary.release();

        if let Some(device) = seat.device.take()
            && device.version() >= 2
        {
            device.release();
        }
    }

    fn seat_mut(&mut self, global: u32) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|seat| seat.global == global)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.shutdown();
        if self.manager.version() >= 4 {
            self.manager.release();
            let _ = self.flush();
        }
    }
}

#[derive(Debug)]
pub(crate) struct TransferPermit {
    transfers: Arc<AtomicUsize>,
}

impl TransferPermit {
    pub(crate) fn claim(transfers: &Arc<AtomicUsize>, limits: TransferLimits) -> Option<Self> {
        transfers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                (in_flight < limits.max_concurrent_transfers).then_some(in_flight + 1)
            })
            .ok()
            .map(|_| Self {
                transfers: Arc::clone(transfers),
            })
    }
}

impl Drop for TransferPermit {
    fn drop(&mut self) {
        let previous = self.transfers.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "transfer permit counter underflow");
    }
}

fn prepare_paste_transfer<ReadEnd, WriteEnd>(
    transfers: &Arc<AtomicUsize>,
    limits: TransferLimits,
    pipe: impl FnOnce() -> Result<(ReadEnd, WriteEnd), ClipboardError>,
    receive_and_flush: impl FnOnce(WriteEnd) -> Result<(), ClipboardError>,
) -> Result<(ReadEnd, TransferPermit), ClipboardError> {
    let Some(permit) = TransferPermit::claim(transfers, limits) else {
        return Err(ClipboardError::TransferFailed {
            reason: format!(
                "{} clipboard transfers already in flight",
                limits.max_concurrent_transfers
            ),
        });
    };
    let (read_end, write_end) = pipe()?;
    receive_and_flush(write_end)?;
    Ok((read_end, permit))
}

fn paste_reader_task(
    read_end: std::os::fd::OwnedFd,
    mime: MimeType,
    limits: TransferLimits,
    reply: Sender<Result<Pasted, ClipboardError>>,
    permit: TransferPermit,
) -> impl FnOnce() + Send + 'static {
    move || {
        let outcome = read_bounded(read_end, limits).map(|bytes| Pasted::new(mime, bytes));
        drop(permit);
        let _ = reply.send(outcome);
    }
}

fn spawn_paste_reader(
    read_end: std::os::fd::OwnedFd,
    mime: MimeType,
    limits: TransferLimits,
    reply: Sender<Result<Pasted, ClipboardError>>,
    permit: TransferPermit,
) -> Result<(), ClipboardError> {
    spawn_paste_reader_with(
        paste_reader_task(read_end, mime, limits, reply, permit),
        |reader| {
            std::thread::Builder::new()
                .name(String::from("enzui-clipboard-receive"))
                .spawn(reader)
                .map(drop)
        },
    )
}

fn spawn_paste_reader_with<Reader>(
    reader: Reader,
    spawn: impl FnOnce(Reader) -> std::io::Result<()>,
) -> Result<(), ClipboardError>
where
    Reader: FnOnce() + Send + 'static,
{
    spawn(reader).map_err(|error| ClipboardError::TransferFailed {
        reason: error.to_string(),
    })
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlSeat::interface().name => {
                state.add_seat(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => state.remove_seat(name),
            _ => {}
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlDataDeviceManager,
        _event: <WlDataDeviceManager as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, u32> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlSeat,
        event: wl_seat::Event,
        global: &u32,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let keyboard = matches!(capabilities, wayland_client::WEnum::Value(caps) if caps.contains(Capability::Keyboard));

                let Some(seat) = state.seat_mut(*global) else {
                    return;
                };

                let pointer = matches!(
                    capabilities,
                    wayland_client::WEnum::Value(caps) if caps.contains(Capability::Pointer)
                );
                let touch = matches!(capabilities, wayland_client::WEnum::Value(caps) if caps.contains(Capability::Touch));
                match (touch, seat.drag.touch.is_some()) {
                    (true, false) => seat.drag.touch = Some(seat.proxy.get_touch(qh, *global)),
                    (false, true) => {
                        if let Some(touch) = seat.drag.touch.take()
                            && touch.version() >= 3
                        {
                            touch.release();
                        }
                        seat.drag.touches.clear();
                    }
                    _ => {}
                }

                // A copy driven by a mouse click carries a pointer serial, and
                // the compositor accepts only a serial it issued for real
                // input, so both device kinds have to be watched.
                match (pointer, seat.pointer.is_some()) {
                    (true, false) => seat.pointer = Some(seat.proxy.get_pointer(qh, *global)),
                    (false, true) => {
                        seat.drag.pointer_surface = None;
                        seat.drag.pointer_grab = None;
                        if let Some(pointer) = seat.pointer.take()
                            && pointer.version() >= 3
                        {
                            pointer.release();
                        }
                    }
                    _ => {}
                }

                match (keyboard, seat.keyboard.is_some()) {
                    // The keyboard exists only to learn focus and serials; the
                    // application's own keyboard object is a different one and
                    // is never touched.
                    (true, false) => seat.keyboard = Some(seat.proxy.get_keyboard(qh, *global)),
                    (false, true) => {
                        if let Some(keyboard) = seat.keyboard.take()
                            && keyboard.version() >= 3
                        {
                            keyboard.release();
                        }

                        seat.focused_surface = None;
                        seat.serial = None;
                    }
                    _ => {}
                }
            }
            wl_seat::Event::Name { name } => {
                if let Some(seat) = state.seat_mut(*global) {
                    seat.name = name;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, u32> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: wl_keyboard::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seat_mut(*global) else {
            return;
        };

        match event {
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                seat.focused_surface = Some(surface.id());
                seat.serial = Some(serial);
            }
            wl_keyboard::Event::Leave { serial, .. } => {
                seat.focused_surface = None;
                seat.serial = Some(serial);
            }
            wl_keyboard::Event::Key { serial, .. }
            | wl_keyboard::Event::Modifiers { serial, .. } => {
                seat.serial = Some(serial);
            }
            // The keymap descriptor is closed by dropping the event; this
            // runtime interprets no keys, it only needs the serials.
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, u32> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlPointer,
        event: wl_pointer::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seat_mut(*global) else {
            return;
        };

        match event {
            wl_pointer::Event::Enter {
                serial, surface, ..
            } => {
                seat.serial = Some(serial);
                seat.drag.pointer_surface = Some(surface);
            }
            wl_pointer::Event::Leave { serial, surface } => {
                seat.drag.confirm_pointer_start(&surface);
                seat.serial = Some(serial);
                seat.drag.pointer_surface = None;
                seat.drag.pointer_grab = None;
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state: button_state,
                ..
            } => {
                seat.serial = Some(serial);
                match button_state {
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        if seat.drag.pointer_grab.is_none()
                            && let Some(surface) = &seat.drag.pointer_surface
                        {
                            seat.drag.pointer_grab = Some((serial, button, surface.clone()));
                        }
                    }
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Released) => {
                        seat.drag.pointer_released(button);
                        if seat
                            .drag
                            .pointer_grab
                            .as_ref()
                            .is_some_and(|(_, grabbed, _)| *grabbed == button)
                        {
                            seat.drag.pointer_grab = None;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlTouch, u32> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlTouch,
        event: wl_touch::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seat_mut(*global) else {
            return;
        };
        match event {
            wl_touch::Event::Down {
                serial,
                surface,
                id,
                ..
            } => {
                let track_touch = seat.drag.touches.len() < 16
                    && !seat.drag.touches.iter().any(|(touch, _, _)| *touch == id);
                if track_touch {
                    seat.drag.touches.push((id, serial, surface));
                }
            }
            wl_touch::Event::Up { id, .. } => {
                seat.drag.touches.retain(|(touch, _, _)| *touch != id);
            }
            wl_touch::Event::Cancel => seat.drag.touches.clear(),
            _ => {}
        }
    }
}

fn same_types(left: &[MimeType], right: &[MimeType]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    let mut left: Vec<String> = left.iter().map(MimeType::normalized).collect();
    let mut right: Vec<String> = right.iter().map(MimeType::normalized).collect();
    left.sort();
    right.sort();

    left == right
}

impl Dispatch<WlDataDevice, u32> for State {
    event_created_child!(State, WlDataDevice, [
        0 => (WlDataOffer, Arc::new(OfferData::default()))
    ]);

    fn event(
        state: &mut Self,
        _proxy: &WlDataDevice,
        event: wl_data_device::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                let mimes = id
                    .as_ref()
                    .and_then(|offer| offer.data::<Arc<OfferData>>())
                    .map(|data| data.snapshot())
                    .unwrap_or_default();

                let Some(seat) = state.seat_mut(*global) else {
                    if let Some(offer) = id {
                        offer.destroy();
                    }
                    return;
                };

                // The protocol requires the previous offer to be destroyed when
                // a new selection arrives.
                if let Some(previous) = seat.selection.take() {
                    previous.destroy();
                }

                seat.selection = id;
                seat.selection_mimes = mimes;

                // MIME matching cannot distinguish competing sources with identical types.
                if let Some(claim) = seat.claim.take() {
                    if same_types(&claim.expected, &seat.selection_mimes) {
                        claim.group.confirm();
                    } else {
                        claim.group.refuse(ClipboardError::ClaimNotHonoured {
                            after: claim.budget,
                        });

                        if let Some(source) = seat.source.take() {
                            destroy_source(source);
                        }
                    }
                }
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id,
            } => state.drag_enter(*global, serial, surface, (x, y), id),
            wl_data_device::Event::Motion { x, y, .. } => state.drag_motion(*global, (x, y)),
            wl_data_device::Event::Leave => state.drag_leave(*global),
            wl_data_device::Event::Drop => state.drag_drop(*global),
            _ => {}
        }
    }
}

impl Dispatch<WlDataOffer, Arc<OfferData>> for State {
    fn event(
        state: &mut Self,
        proxy: &WlDataOffer,
        event: wl_data_offer::Event,
        data: &Arc<OfferData>,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => data.push(MimeType::new(mime_type)),
            wl_data_offer::Event::SourceActions {
                source_actions: wayland_client::WEnum::Value(actions),
            } => {
                data.source_actions.store(actions.bits(), Ordering::Release);
            }
            wl_data_offer::Event::Action { dnd_action } => {
                state.drag_offer_action(proxy, dnd_action);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, SourceData> for State {
    fn event(
        state: &mut Self,
        proxy: &WlDataSource,
        event: wl_data_source::Event,
        data: &SourceData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if data.native_drag {
            state.drag_source_event(proxy, &event);
            if !matches!(event, wl_data_source::Event::Send { .. }) {
                return;
            }
        }
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let mime = MimeType::new(mime_type);

                let Some(transfer) = data.transfer(&mime) else {
                    // Closing the descriptor refuses delivery without injecting error text into a paste.
                    drop(fd);
                    return;
                };

                let _ = std::thread::Builder::new()
                    .name(String::from("enzui-clipboard-send"))
                    .spawn(move || {
                        let _ = transfer.write(fd);
                    });
            }
            wl_data_source::Event::Cancelled => {
                data.revoke();
                // Ownership moved to another client, or to a newer source of
                // ours. Only the source that is still current may clear the
                // seat's ownership.
                for seat in &mut state.seats {
                    if seat.source.as_ref().map(Proxy::id) == Some(proxy.id()) {
                        seat.source = None;
                    }

                    let claimed = seat
                        .claim
                        .as_ref()
                        .is_some_and(|claim| claim.source == proxy.id());

                    if claimed && let Some(claim) = seat.claim.take() {
                        claim.group.refuse(ClipboardError::ClaimNotHonoured {
                            after: claim.budget,
                        });
                    }
                }
                if state.seats.iter().all(|seat| seat.source.is_none()) {
                    crate::keeper::forget();
                }

                proxy.destroy();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn sensitive_sources_redact_wipe_and_stop_blocked_transfers() {
        let _selection = crate::keeper::TEST_SELECTION.lock().unwrap();
        use enzui_security::secret::{SecretText, observer};
        const TEXT: &str = "clipboard-fixture-not-a-real-password";
        let mime = MimeType::new("text/plain;charset=utf-8");
        let limits = TransferLimits::default();
        observer::reset();
        let text = SecretText::new(TEXT.to_owned());
        let payload =
            Payload::sensitive_text(text, Duration::from_secs(2)).expect("sensitive offer");
        assert!(!format!("{payload:?}").contains(TEXT));
        assert!(
            payload.bytes_for(&mime).is_none(),
            "protected bytes must not escape into Arc<[u8]>"
        );
        crate::keeper::remember(&payload);
        assert!(crate::keeper::owned_selection().is_none());
        let data = SourceData::new(payload, limits, Arc::new(AtomicUsize::new(0)), false);
        let bytes = data
            .bytes_for(&mime)
            .expect("protected text representation");
        assert_eq!(bytes.immediate(), Some(TEXT.as_bytes()));
        assert_eq!(
            observer::snapshot().secret_text_allocations,
            1,
            "aliases share one protected allocation"
        );
        assert_eq!(observer::snapshot().wiped_bytes, 0);
        data.revoke();
        assert!(data.bytes_for(&mime).is_none());
        assert_eq!(
            observer::snapshot().wiped_bytes,
            0,
            "an in-flight lease remains owned"
        );
        drop(bytes);
        let wiped = observer::snapshot();
        assert!(wiped.wiped_bytes >= TEXT.len());
        assert_eq!(wiped.wiped_bytes, wiped.zeroed_bytes);

        let data = SourceData::new(
            Payload::sensitive_text(
                SecretText::new("s".repeat(64 * 1024)),
                Duration::from_secs(2),
            )
            .expect("offer"),
            limits,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        let (read, write) = transfer_pipe().expect("pipe");
        crate::transfer::set_non_blocking(write.as_fd()).expect("nonblocking pipe");
        while rustix::io::write(write.as_fd(), &[0; 4096]).is_ok() {}
        let transfer = data.transfer(&mime).expect("source transfer");
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            observer::reset();
            let result = transfer.write(write);
            tx.send((result, observer::snapshot()))
                .expect("test receiver");
        });
        assert!(
            matches!(
                rx.recv_timeout(Duration::from_millis(20)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "a full pipe must keep the transfer pending"
        );
        drop(data);
        let completed = rx.recv_timeout(Duration::from_secs(1));
        drop(read);
        writer.join().expect("writer stopped");
        let (result, wiped) = completed.expect("revocation interrupts a blocked send");
        assert!(matches!(result, Err(ClipboardError::Stopped)));
        assert!(wiped.wiped_bytes >= 64 * 1024);
        assert_eq!(wiped.wiped_bytes, wiped.zeroed_bytes);
        for lifetime in [Duration::ZERO, Duration::from_secs(121), Duration::MAX] {
            assert!(Payload::sensitive_text(SecretText::new(TEXT.to_owned()), lifetime).is_err());
        }
        assert!(matches!(
            Payload::sensitive_text(
                SecretText::new("s".repeat(64 * 1024 + 1)),
                Duration::from_secs(1)
            ),
            Err(ClipboardError::TransferTooLarge { .. })
        ));
    }

    fn limits_with_claim_timeout(claim_timeout: Duration) -> TransferLimits {
        TransferLimits {
            claim_timeout,
            ..TransferLimits::default()
        }
    }

    #[test]
    fn claim_deadline_with_duration_max_fails_closed_to_now_without_panic() {
        let started = Instant::now();
        let now = Instant::now();

        let deadline = State::claim_deadline(now, limits_with_claim_timeout(Duration::MAX));

        assert_eq!(
            deadline, now,
            "Duration::MAX must fail closed to Some(now), not panic and not silently become no-limit",
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the fail-closed claim deadline must return immediately, not wait",
        );
    }
}

#[cfg(test)]
#[path = "protocol/deferred_tests.rs"]
mod deferred_tests;
