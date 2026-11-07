//! The primary selection (`zwp_primary_selection_v1`): the text a middle
//! click pastes, offered whenever text is selected.
//!
//! It mirrors the ordinary selection: the same seats, focus and serial rules,
//! payload sources and bounded transfers, on a separate device per seat. A
//! compositor without the protocol leaves every seat without a device, and
//! requests report [`ClipboardError::NoPrimarySelection`].

use std::sync::Arc;
use std::sync::mpsc::Sender;

use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
    zwp_primary_selection_device_v1::{self, ZwpPrimarySelectionDeviceV1},
    zwp_primary_selection_offer_v1::{self, ZwpPrimarySelectionOfferV1},
    zwp_primary_selection_source_v1::{self, ZwpPrimarySelectionSourceV1},
};

use super::{
    OfferData, SeatSelector, SourceData, State, prepare_paste_transfer, spawn_paste_reader,
};
use crate::error::ClipboardError;
use crate::mime::{MimeType, negotiate};
use crate::payload::{Pasted, Payload};
use crate::transfer::transfer_pipe;

/// One seat's primary selection state.
#[derive(Default)]
pub(super) struct SeatPrimary {
    device: Option<ZwpPrimarySelectionDeviceV1>,
    selection: Option<ZwpPrimarySelectionOfferV1>,
    mimes: Vec<MimeType>,
    source: Option<ZwpPrimarySelectionSourceV1>,
}

impl SeatPrimary {
    /// Creates the seat's device when the compositor offers the protocol.
    pub(super) fn new(
        manager: Option<&ZwpPrimarySelectionDeviceManagerV1>,
        seat: &wayland_client::protocol::wl_seat::WlSeat,
        global: u32,
        qh: &QueueHandle<State>,
    ) -> Self {
        Self {
            device: manager.map(|manager| manager.get_device(seat, qh, global)),
            ..Self::default()
        }
    }

    /// Destroys every object this seat holds.
    pub(super) fn release(&mut self) {
        if let Some(source) = self.source.take() {
            source.destroy();
        }
        if let Some(offer) = self.selection.take() {
            offer.destroy();
        }
        if let Some(device) = self.device.take() {
            device.destroy();
        }
    }

    pub(super) fn owns(&self) -> bool {
        self.source.is_some()
    }
}

impl State {
    /// Offers `payload` as the primary selection on the seats `selector`
    /// resolves to, with the same focus and serial rules as [`State::offer`].
    ///
    /// Success means the request was flushed; the primary selection has no
    /// claim confirmation.
    ///
    /// # Errors
    ///
    /// Returns the stored fatal error, `NoPrimarySelection`, seat/focus errors,
    /// `MimeUnavailable` for an empty payload, or `Disconnected`.
    pub fn offer_primary(
        &mut self,
        payload: Payload,
        selector: &SeatSelector,
        qh: &QueueHandle<State>,
    ) -> Result<(), ClipboardError> {
        if payload.expires_at().is_some() {
            return Err(ClipboardError::Unavailable {
                reason: "sensitive text or deferred text requires the ordinary clipboard selection"
                    .into(),
            });
        }
        if let Some(error) = self.fatal() {
            return Err(error);
        }
        let Some(manager) = self.primary_manager.clone() else {
            return Err(ClipboardError::NoPrimarySelection);
        };
        if payload.is_empty() {
            return Err(ClipboardError::MimeUnavailable {
                offered: Vec::new(),
            });
        }
        let targets = self.writable_seats(selector)?;
        let limits = self.limits;
        for index in targets {
            let seat = &mut self.seats[index];
            let (Some(serial), Some(device)) =
                (seat.selection_serial(), seat.primary.device.clone())
            else {
                continue;
            };
            let source = manager.create_source(
                qh,
                SourceData::new(payload.clone(), limits, Arc::clone(&self.transfers), false),
            );
            for mime in payload.mime_types() {
                source.offer(mime.into_string());
            }
            device.set_selection(Some(&source), serial);
            if let Some(previous) = seat.primary.source.replace(source) {
                previous.destroy();
            }
        }
        self.flush()
    }

    /// Reads the primary selection using `preferences`, like [`State::paste`].
    ///
    /// # Errors
    ///
    /// Returns the stored fatal error, `NoPrimarySelection`, seat/focus or
    /// negotiation errors, transfer admission failures, or `Disconnected`.
    pub fn paste_primary(
        &mut self,
        preferences: &[MimeType],
        selector: &SeatSelector,
        reply: Sender<Result<Pasted, ClipboardError>>,
    ) -> Result<(), ClipboardError> {
        if let Some(error) = self.fatal() {
            return Err(error);
        }
        if self.primary_manager.is_none() {
            return Err(ClipboardError::NoPrimarySelection);
        }
        let index = self.readable_seat(selector)?;
        let seat = &self.seats[index];
        let Some(offer) = seat.primary.selection.clone() else {
            return Err(ClipboardError::NoSelection);
        };
        let Some(mime) = negotiate(preferences, &seat.primary.mimes) else {
            return Err(ClipboardError::MimeUnavailable {
                offered: seat.primary.mimes.clone(),
            });
        };
        let limits = self.limits;
        let (read_end, permit) =
            prepare_paste_transfer(&self.transfers, limits, transfer_pipe, |write_end| {
                use std::os::fd::AsFd;
                offer.receive(mime.as_str().to_owned(), write_end.as_fd());
                drop(write_end);
                self.flush()
            })?;
        spawn_paste_reader(read_end, mime, limits, reply, permit)
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpPrimarySelectionDeviceManagerV1,
        _event: <ZwpPrimarySelectionDeviceManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, u32> for State {
    event_created_child!(State, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE
            => (ZwpPrimarySelectionOfferV1, Arc::new(OfferData::default()))
    ]);

    fn event(
        state: &mut Self,
        _proxy: &ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_device_v1::Event::Selection { id } = event {
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
            // The protocol requires the previous offer to be destroyed when a
            // new selection arrives.
            if let Some(previous) = seat.primary.selection.take() {
                previous.destroy();
            }
            seat.primary.selection = id;
            seat.primary.mimes = mimes;
        }
    }
}

impl Dispatch<ZwpPrimarySelectionOfferV1, Arc<OfferData>> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        data: &Arc<OfferData>,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            data.push(MimeType::new(mime_type));
        }
    }
}

impl Dispatch<ZwpPrimarySelectionSourceV1, SourceData> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        data: &SourceData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                let Some(transfer) = data.transfer(&MimeType::new(mime_type)) else {
                    // Refuse delivery without injecting error text into a paste.
                    drop(fd);
                    return;
                };
                let _ = std::thread::Builder::new()
                    .name(String::from("enzui-primary-send"))
                    .spawn(move || {
                        let _ = transfer.write(fd);
                    });
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                // Another client (or a newer source of ours) owns the
                // selection now; only the current source clears ownership.
                for seat in &mut state.seats {
                    if seat.primary.source.as_ref().map(Proxy::id) == Some(proxy.id()) {
                        seat.primary.source = None;
                    }
                }
                proxy.destroy();
            }
            _ => {}
        }
    }
}
