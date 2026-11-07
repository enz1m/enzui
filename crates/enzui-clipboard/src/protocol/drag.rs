use super::drag_icon;
use super::*;
use crate::drag::{
    DragAction, DragActions, DragCommand, DragEvent, DragEvents, DragId, DragOutcome, EventQueue,
    MAX_MIME_BYTES, MAX_MIMES, ReceivedDrop, SourceDrag,
};
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, AtomicU64};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::protocol::wl_touch::WlTouch;
use wayland_protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_v1::XdgToplevelDragV1;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const MAX_INCOMING: usize = 16;
const START_CONFIRMATION_BUDGET: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct SeatDrag {
    pub(super) pointer_surface: Option<WlSurface>,
    pub(super) pointer_grab: Option<(u32, u32, WlSurface)>,
    pub(super) touch: Option<WlTouch>,
    pub(super) touches: Vec<(i32, u32, WlSurface)>,
    hover: Option<DragId>,
    outgoing: Option<Outgoing>,
}

struct Outgoing {
    source: WlDataSource,
    icon: Option<drag_icon::IconSurface>,
    toplevel_drag: Option<XdgToplevelDragV1>,
    actions: DragActions,
    action: Option<DragAction>,
    dropped: bool,
    unconfirmed_until: Option<Instant>,
    pointer_origin: Option<(WlSurface, u32)>,
    cancelled: Arc<AtomicBool>,
    reply: Sender<Result<DragOutcome, ClipboardError>>,
}

impl Outgoing {
    /// Ends the source, then what it showed: a toplevel drag may only be
    /// destroyed once its drag is over, which destroying the source ensures.
    fn release(self) -> Sender<Result<DragOutcome, ClipboardError>> {
        self.source.destroy();
        if let Some(toplevel_drag) = self.toplevel_drag {
            toplevel_drag.destroy();
        }
        if let Some(icon) = self.icon {
            icon.destroy();
        }
        self.reply
    }
}

impl SeatDrag {
    pub(super) fn confirm_pointer_start(&mut self, surface: &WlSurface) {
        if self.outgoing.as_ref().is_some_and(|outgoing| {
            outgoing
                .pointer_origin
                .as_ref()
                .is_some_and(|(origin, _)| origin == surface)
        }) {
            self.confirm_start();
        }
    }

    pub(super) fn pointer_released(&self, button: u32) {
        if let Some(outgoing) = &self.outgoing
            && !outgoing.dropped
            && outgoing
                .pointer_origin
                .as_ref()
                .is_some_and(|(_, started)| *started == button)
        {
            outgoing.cancelled.store(true, Ordering::Release);
        }
    }

    fn confirm_start(&mut self) {
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.unconfirmed_until = None;
        }
    }
}

#[derive(Default)]
pub(super) struct NativeState {
    events: Weak<EventQueue>,
    incoming: Vec<Incoming>,
}

#[derive(PartialEq, Eq)]
enum Phase {
    Hover,
    Dropped,
    Reading,
}

struct Incoming {
    id: DragId,
    global: u32,
    offer: WlDataOffer,
    serial: u32,
    mimes: Vec<MimeType>,
    mime: Option<MimeType>,
    allowed: DragActions,
    action: Option<DragAction>,
    phase: Phase,
    accepted: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}

pub(super) fn valid_mime(mime: &MimeType) -> bool {
    !mime.as_str().is_empty()
        && mime.as_str().len() <= MAX_MIME_BYTES
        && !mime.as_str().chars().any(char::is_control)
}

fn refused(reason: &str) -> ClipboardError {
    ClipboardError::Unavailable {
        reason: reason.to_owned(),
    }
}

fn action(value: wayland_client::WEnum<DndAction>) -> Option<DragAction> {
    match value {
        wayland_client::WEnum::Value(DndAction::Copy) => Some(DragAction::Copy),
        wayland_client::WEnum::Value(DndAction::Move) => Some(DragAction::Move),
        _ => None,
    }
}

fn source_actions(offer: &WlDataOffer) -> DragActions {
    if offer.version() < 3 {
        DragActions::COPY
    } else {
        DragActions(
            offer
                .data::<Arc<OfferData>>()
                .map_or(0, |data| data.source_actions.load(Ordering::Acquire))
                & 3,
        )
    }
}

impl State {
    pub(crate) fn drag_command(&mut self, command: DragCommand, qh: &QueueHandle<Self>) {
        if let Some(error) = self.fatal() {
            command.refuse(error);
            return;
        }
        match command {
            DragCommand::Listen(reply) => {
                let result = if self.drag.events.upgrade().is_some() {
                    Err(ClipboardError::Busy)
                } else {
                    let events = Arc::new(EventQueue::default());
                    self.drag.events = Arc::downgrade(&events);
                    Ok(DragEvents(events))
                };
                let _ = reply.send(result);
            }
            DragCommand::Start {
                payload,
                actions,
                seat,
                icon,
                reply,
            } => {
                let _ = reply.send(self.start_drag(payload, actions, &seat, icon.as_ref(), qh));
            }
            DragCommand::AttachWindow {
                toplevel,
                offset,
                reply,
            } => {
                let _ = reply.send(self.attach_window(&toplevel, offset));
            }
            DragCommand::Accept {
                id,
                preferences,
                actions,
                preferred,
                reply,
            } => {
                let result = self.accept_drag(id, &preferences, actions, preferred);
                let _ = reply.send(result);
            }
            DragCommand::Reject { id, reply } => {
                let result = self.reject_drag(id);
                let _ = reply.send(result);
            }
            DragCommand::Receive { id, reply } => {
                if let Err(error) = self.receive_drag(id, reply.clone()) {
                    let _ = reply.send(Err(error));
                }
            }
        }
    }

    fn start_drag(
        &mut self,
        payload: Payload,
        actions: DragActions,
        selector: &SeatSelector,
        icon: Option<&crate::drag::DragIcon>,
        qh: &QueueHandle<Self>,
    ) -> Result<SourceDrag, ClipboardError> {
        if !payload.is_ordinary() {
            return Err(refused(
                "sensitive text or deferred text requires the ordinary clipboard selection",
            ));
        }
        let mimes = payload.mime_types();
        if self.manager.version() < 3 {
            return Err(refused(
                "native drag sources require wl_data_device_manager version 3",
            ));
        }
        if actions == DragActions::NONE
            || mimes.is_empty()
            || mimes.len() > MAX_MIMES
            || mimes.iter().any(|mime| {
                !valid_mime(mime)
                    || payload
                        .bytes_for(mime)
                        .is_none_or(|bytes| bytes.len() > self.limits.max_bytes)
            })
        {
            return Err(refused(
                "native drag payload or actions exceed the supported limits",
            ));
        }
        let index = self
            .seats
            .iter()
            .position(|seat| {
                let selected = match selector {
                    SeatSelector::Named(name) => name == &seat.name,
                    _ => true,
                };
                selected && (seat.drag.pointer_grab.is_some() || !seat.drag.touches.is_empty())
            })
            .ok_or(ClipboardError::NotFocused)?;
        let seat = &mut self.seats[index];
        if seat.drag.outgoing.is_some() {
            return Err(ClipboardError::Busy);
        }
        let (serial, origin) = if let Some((serial, _, surface)) = &seat.drag.pointer_grab {
            (*serial, surface.clone())
        } else if let Some((_, serial, surface)) = seat.drag.touches.first() {
            (*serial, surface.clone())
        } else {
            return Err(ClipboardError::NotFocused);
        };
        let pointer_origin = seat
            .drag
            .pointer_grab
            .as_ref()
            .map(|(_, button, surface)| (surface.clone(), *button));
        let device = seat.device.as_ref().ok_or(ClipboardError::NoSeat)?;
        let source = self.manager.create_data_source(
            qh,
            SourceData::new(payload, self.limits, Arc::clone(&self.transfers), true),
        );
        for mime in mimes {
            source.offer(mime.into_string());
        }
        source.set_actions(DndAction::from_bits_truncate(actions.0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply, receiver) = std::sync::mpsc::channel();
        let toplevel_drag = self.drag_globals.toplevel_drag(&source, qh);
        let icon =
            icon.and_then(|icon| drag_icon::IconSurface::create(&self.drag_globals, icon, qh));
        device.start_drag(
            Some(&source),
            &origin,
            icon.as_ref().map(|icon| &icon.surface),
            serial,
        );
        if let Some(icon) = &icon {
            icon.commit();
        }
        seat.drag.pointer_grab = None;
        seat.drag.touches.clear();
        seat.drag.outgoing = Some(Outgoing {
            source,
            icon,
            toplevel_drag,
            actions,
            action: None,
            dropped: false,
            unconfirmed_until: Some(Instant::now() + START_CONFIRMATION_BUDGET),
            pointer_origin,
            cancelled: Arc::clone(&cancelled),
            reply,
        });
        self.flush()?;
        Ok(SourceDrag {
            receiver,
            cancelled,
        })
    }

    fn attach_window(
        &mut self,
        toplevel: &crate::drag::XdgToplevel,
        (x, y): (i32, i32),
    ) -> Result<(), ClipboardError> {
        let outgoing = self
            .seats
            .iter()
            .find_map(|seat| seat.drag.outgoing.as_ref())
            .ok_or_else(|| refused("no drag of this application is under way"))?;
        let toplevel_drag = outgoing.toplevel_drag.as_ref().ok_or_else(|| {
            refused("the compositor cannot move windows with a drag (xdg_toplevel_drag_v1)")
        })?;
        if !toplevel.is_alive() {
            return Err(refused("the window to attach is gone"));
        }
        toplevel_drag.attach(toplevel, x, y);
        self.flush()
    }

    fn accept_drag(
        &mut self,
        id: DragId,
        preferences: &[MimeType],
        actions: DragActions,
        preferred: DragAction,
    ) -> Result<MimeType, ClipboardError> {
        let incoming = self
            .drag
            .incoming
            .iter_mut()
            .find(|incoming| incoming.id == id && incoming.phase == Phase::Hover)
            .ok_or_else(|| refused("native drag offer is no longer hovering"))?;
        let allowed = DragActions(actions.0 & source_actions(&incoming.offer).0);
        if allowed == DragActions::NONE || !allowed.allows(preferred) {
            incoming.offer.accept(incoming.serial, None);
            incoming.mime = None;
            incoming.action = None;
            return Err(refused("native drag offers no requested operation"));
        }
        let mime = negotiate(preferences, &incoming.mimes).ok_or_else(|| {
            ClipboardError::MimeUnavailable {
                offered: incoming.mimes.clone(),
            }
        });
        let mime = match mime {
            Ok(mime) => mime,
            Err(error) => {
                incoming.offer.accept(incoming.serial, None);
                incoming.mime = None;
                incoming.action = None;
                return Err(error);
            }
        };
        if incoming.offer.version() >= 3 {
            incoming.offer.set_actions(
                DndAction::from_bits_truncate(allowed.0),
                DndAction::from_bits_truncate(preferred.bit()),
            );
        } else {
            incoming.action = Some(DragAction::Copy);
        }
        incoming
            .offer
            .accept(incoming.serial, Some(mime.as_str().to_owned()));
        incoming.allowed = allowed;
        incoming.mime = Some(mime.clone());
        self.flush()?;
        Ok(mime)
    }

    fn reject_drag(&mut self, id: DragId) -> Result<(), ClipboardError> {
        let index = self
            .drag
            .incoming
            .iter()
            .position(|incoming| incoming.id == id)
            .ok_or_else(|| refused("native drag offer has expired"))?;
        self.remove_incoming(index, false);
        self.flush()
    }

    fn receive_drag(
        &mut self,
        id: DragId,
        reply: Sender<Result<ReceivedDrop, ClipboardError>>,
    ) -> Result<(), ClipboardError> {
        let incoming = self
            .drag
            .incoming
            .iter_mut()
            .find(|incoming| incoming.id == id && incoming.phase == Phase::Dropped)
            .ok_or_else(|| refused("native drag has not dropped or has already been consumed"))?;
        let mime = incoming
            .mime
            .clone()
            .ok_or_else(|| refused("native drag MIME was not accepted"))?;
        let action = incoming
            .action
            .filter(|action| incoming.allowed.allows(*action))
            .ok_or_else(|| refused("native drag action was not accepted"))?;
        let limits = self.limits;
        let conn = self.conn.clone();
        let (read_end, permit) =
            prepare_paste_transfer(&self.transfers, limits, transfer_pipe, |write_end| {
                incoming
                    .offer
                    .receive(mime.as_str().to_owned(), write_end.as_fd());
                drop(write_end);
                conn.flush().map_err(|_| ClipboardError::Disconnected)
            })?;
        incoming.phase = Phase::Reading;
        let accepted = Arc::clone(&incoming.accepted);
        let cancelled = Arc::clone(&incoming.cancelled);
        let failed = Arc::clone(&cancelled);
        if let Err(error) = std::thread::Builder::new()
            .name("enzui-drag-receive".to_owned())
            .spawn(move || {
                let outcome = read_bounded(read_end, limits).map(|bytes| ReceivedDrop {
                    data: Pasted::new(mime, bytes),
                    action,
                    accepted,
                    cancelled: Arc::clone(&cancelled),
                });
                if outcome.is_err() {
                    cancelled.store(true, Ordering::Release);
                }
                drop(permit);
                let _ = reply.send(outcome);
            })
        {
            failed.store(true, Ordering::Release);
            return Err(ClipboardError::TransferFailed {
                reason: error.to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn drag_enter(
        &mut self,
        global: u32,
        serial: u32,
        surface: WlSurface,
        position: (f64, f64),
        offer: Option<WlDataOffer>,
    ) {
        self.drag_leave(global);
        let Some(offer) = offer else {
            return;
        };
        let mimes = offer
            .data::<Arc<OfferData>>()
            .map(|data| data.snapshot())
            .unwrap_or_default();
        if mimes.is_empty()
            || mimes.len() > MAX_MIMES
            || mimes.iter().any(|mime| !valid_mime(mime))
            || self.drag.events.upgrade().is_none()
            || self.drag.incoming.len() >= MAX_INCOMING
            || !position.0.is_finite()
            || !position.1.is_finite()
        {
            offer.accept(serial, None);
            offer.destroy();
            return;
        }
        let Some(seat) = self.seats.iter_mut().find(|seat| seat.global == global) else {
            offer.destroy();
            return;
        };
        let Ok(value) = NEXT_ID.try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        }) else {
            offer.destroy();
            return;
        };
        let id = DragId(value);
        let actions = source_actions(&offer);
        let event = DragEvent::Entered {
            id,
            surface: surface.id().protocol_id(),
            seat: seat.name.clone(),
            position,
            mimes: mimes.clone(),
            actions,
        };
        seat.drag.hover = Some(id);
        self.drag.incoming.push(Incoming {
            id,
            global,
            offer,
            serial,
            mimes,
            mime: None,
            allowed: DragActions::NONE,
            action: None,
            phase: Phase::Hover,
            accepted: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        self.emit_drag(event);
    }

    pub(super) fn drag_motion(&mut self, global: u32, position: (f64, f64)) {
        if !position.0.is_finite() || !position.1.is_finite() {
            self.drag_leave(global);
            return;
        }
        if let Some(id) = self
            .seats
            .iter()
            .find(|seat| seat.global == global)
            .and_then(|seat| seat.drag.hover)
        {
            self.emit_drag(DragEvent::Motion { id, position });
        }
    }

    pub(super) fn drag_leave(&mut self, global: u32) {
        let id = self
            .seats
            .iter_mut()
            .find(|seat| seat.global == global)
            .and_then(|seat| seat.drag.hover.take());
        if let Some(id) = id
            && let Some(index) = self
                .drag
                .incoming
                .iter()
                .position(|incoming| incoming.id == id && incoming.phase == Phase::Hover)
        {
            self.remove_incoming(index, false);
        }
    }

    pub(super) fn drag_drop(&mut self, global: u32) {
        let id = self
            .seats
            .iter_mut()
            .find(|seat| seat.global == global)
            .and_then(|seat| seat.drag.hover.take());
        let Some(index) = id.and_then(|id| {
            self.drag
                .incoming
                .iter()
                .position(|incoming| incoming.id == id)
        }) else {
            return;
        };
        let incoming = &mut self.drag.incoming[index];
        let selected = incoming
            .action
            .filter(|action| incoming.allowed.allows(*action));
        if incoming.mime.is_none() || selected.is_none() {
            self.remove_incoming(index, false);
            return;
        }
        let Some(action) = selected else {
            return;
        };
        incoming.phase = Phase::Dropped;
        let id = incoming.id;
        self.emit_drag(DragEvent::Dropped { id, action });
    }

    pub(super) fn drag_offer_action(
        &mut self,
        proxy: &WlDataOffer,
        value: wayland_client::WEnum<DndAction>,
    ) {
        if let Some(incoming) = self
            .drag
            .incoming
            .iter_mut()
            .find(|incoming| incoming.offer.id() == proxy.id())
        {
            incoming.action = action(value).filter(|action| incoming.allowed.allows(*action));
            let event = DragEvent::Action {
                id: incoming.id,
                action: incoming.action,
            };
            self.emit_drag(event);
        }
    }

    pub(super) fn drag_source_event(
        &mut self,
        proxy: &WlDataSource,
        event: &wl_data_source::Event,
    ) {
        let Some(seat) = self.seats.iter_mut().find(|seat| {
            seat.drag
                .outgoing
                .as_ref()
                .is_some_and(|outgoing| outgoing.source.id() == proxy.id())
        }) else {
            return;
        };
        seat.drag.confirm_start();
        let terminal = match event {
            wl_data_source::Event::Action { dnd_action } => {
                if let Some(outgoing) = seat.drag.outgoing.as_mut() {
                    outgoing.action =
                        action(*dnd_action).filter(|action| outgoing.actions.allows(*action));
                }
                None
            }
            wl_data_source::Event::DndDropPerformed => {
                if let Some(outgoing) = seat.drag.outgoing.as_mut() {
                    outgoing.dropped = true;
                }
                None
            }
            wl_data_source::Event::DndFinished => Some(
                seat.drag
                    .outgoing
                    .as_ref()
                    .and_then(|outgoing| outgoing.action.filter(|_| outgoing.dropped))
                    .map_or(DragOutcome::Cancelled, DragOutcome::Finished),
            ),
            wl_data_source::Event::Cancelled => Some(DragOutcome::Cancelled),
            _ => None,
        };
        if let Some(result) = terminal
            && let Some(outgoing) = seat.drag.outgoing.take()
        {
            let _ = outgoing.release().send(Ok(result));
        }
    }

    fn emit_drag(&mut self, event: DragEvent) {
        if !self
            .drag
            .events
            .upgrade()
            .is_some_and(|events| events.push(event))
        {
            while !self.drag.incoming.is_empty() {
                self.remove_incoming(0, false);
            }
        }
    }

    fn remove_incoming(&mut self, index: usize, finish: bool) {
        let incoming = self.drag.incoming.remove(index);
        if finish && incoming.offer.version() >= 3 {
            incoming.offer.finish();
        } else if incoming.phase == Phase::Hover {
            incoming.offer.accept(incoming.serial, None);
        }
        incoming.cancelled.store(!finish, Ordering::Release);
        incoming.offer.destroy();
        for seat in &mut self.seats {
            if seat.drag.hover == Some(incoming.id) {
                seat.drag.hover = None;
            }
        }
        if !finish && let Some(events) = self.drag.events.upgrade() {
            events.push(DragEvent::Left { id: incoming.id });
        }
    }

    pub(crate) fn expire_drags(&mut self) {
        let listening = self.drag.events.upgrade().is_some();
        let mut index = 0;
        while index < self.drag.incoming.len() {
            let incoming = &self.drag.incoming[index];
            let finish =
                incoming.phase == Phase::Reading && incoming.accepted.load(Ordering::Acquire);
            if finish || !listening || incoming.cancelled.load(Ordering::Acquire) {
                self.remove_incoming(index, finish);
            } else {
                index += 1;
            }
        }
        for seat in &mut self.seats {
            if seat.drag.outgoing.as_ref().is_some_and(|outgoing| {
                outgoing.cancelled.load(Ordering::Acquire)
                    || outgoing
                        .unconfirmed_until
                        .is_some_and(|deadline| Instant::now() >= deadline)
            }) && let Some(outgoing) = seat.drag.outgoing.take()
            {
                let _ = outgoing.release().send(Ok(DragOutcome::Cancelled));
            }
        }
    }

    pub(crate) fn drag_deadline(&self) -> Option<Instant> {
        (!self.drag.incoming.is_empty()
            || self.seats.iter().any(|seat| seat.drag.outgoing.is_some()))
        .then(|| Instant::now() + Duration::from_millis(16))
    }

    pub(super) fn remove_seat_drags(&mut self, global: u32) {
        while let Some(index) = self
            .drag
            .incoming
            .iter()
            .position(|incoming| incoming.global == global)
        {
            self.remove_incoming(index, false);
        }
        if let Some(seat) = self.seat_mut(global) {
            if let Some(outgoing) = seat.drag.outgoing.take() {
                let _ = outgoing.release().send(Ok(DragOutcome::Cancelled));
            }
            if let Some(touch) = seat.drag.touch.take()
                && touch.version() >= 3
            {
                touch.release();
            }
        }
    }

    pub(super) fn shutdown_drags(&mut self) {
        if let Some(events) = self.drag.events.upgrade() {
            events.push(DragEvent::Reset);
            events.close();
        }
        let globals: Vec<_> = self.seats.iter().map(|seat| seat.global).collect();
        for global in globals {
            self.remove_seat_drags(global);
        }
    }
}
