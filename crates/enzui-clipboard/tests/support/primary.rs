//! The fixture compositor's primary selection (`zwp_primary_selection_v1`),
//! advertised only when [`Config::primary`](super::compositor::Config) asks
//! for it. Like the ordinary selection it accepts a claim only from the
//! focused client with a serial it issued, and offers the selection only to
//! the focused client.

use std::os::fd::AsFd;
use std::sync::Mutex;

use wayland_protocols::wp::primary_selection::zv1::server::{
    zwp_primary_selection_device_manager_v1::{self, ZwpPrimarySelectionDeviceManagerV1},
    zwp_primary_selection_device_v1::{self, ZwpPrimarySelectionDeviceV1},
    zwp_primary_selection_offer_v1::{self, ZwpPrimarySelectionOfferV1},
    zwp_primary_selection_source_v1::{self, ZwpPrimarySelectionSourceV1},
};
use wayland_server::backend::ClientId;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::{SeatIndex, Server};

#[derive(Debug, Default)]
pub(crate) struct PrimaryMimes(Mutex<Vec<String>>);

#[derive(Debug, Clone)]
pub(crate) struct PrimaryOfferSource(ZwpPrimarySelectionSourceV1);

pub(crate) struct PrimaryRecord {
    source: ZwpPrimarySelectionSourceV1,
    pub(crate) owner: ClientId,
    mimes: Vec<String>,
}

impl Server {
    /// Sends the seat's primary selection, or its absence, to `client`.
    pub(crate) fn send_primary(&mut self, client: &ClientId, seat: usize) {
        let Some(handle) = self
            .dh
            .get_client(
                match self.clients.iter().find(|record| &record.id == client) {
                    Some(record) => match record.primary_devices.first() {
                        Some((_, device)) => device.id(),
                        None => return,
                    },
                    None => return,
                },
            )
            .ok()
        else {
            return;
        };
        let devices: Vec<ZwpPrimarySelectionDeviceV1> = self
            .clients
            .iter()
            .find(|record| &record.id == client)
            .map(|record| {
                record
                    .primary_devices
                    .iter()
                    .filter(|(index, device)| *index == seat && device.is_alive())
                    .map(|(_, device)| device.clone())
                    .collect()
            })
            .unwrap_or_default();
        let selection = self.seats[seat].primary.as_ref().and_then(|record| {
            record
                .source
                .is_alive()
                .then(|| (record.source.clone(), record.mimes.clone()))
        });
        for device in devices {
            match &selection {
                Some((source, mimes)) => {
                    let Ok(offer) = handle
                        .create_resource::<ZwpPrimarySelectionOfferV1, PrimaryOfferSource, Server>(
                            &self.dh,
                            device.version(),
                            PrimaryOfferSource(source.clone()),
                        )
                    else {
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

    pub(crate) fn primary_owner(&self, seat: usize) -> Option<usize> {
        let owner = self.seats.get(seat)?.primary.as_ref()?.owner.clone();
        self.index_of(&owner)
    }
}

impl GlobalDispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for Server {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpPrimarySelectionDeviceManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &ZwpPrimarySelectionDeviceManagerV1,
        request: zwp_primary_selection_device_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_primary_selection_device_manager_v1::Request::CreateSource { id } => {
                data_init.init(id, PrimaryMimes::default());
            }
            zwp_primary_selection_device_manager_v1::Request::GetDevice { id, seat } => {
                let index = seat.data::<SeatIndex>().copied().unwrap_or(SeatIndex(0));
                let device = data_init.init(id, index);
                if let Some(record) = state.record(&client.id()) {
                    record.primary_devices.push((index.0, device));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpPrimarySelectionSourceV1, PrimaryMimes> for Server {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwpPrimarySelectionSourceV1,
        request: zwp_primary_selection_source_v1::Request,
        data: &PrimaryMimes,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_primary_selection_source_v1::Request::Offer { mime_type } = request
            && let Ok(mut mimes) = data.0.lock()
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, SeatIndex> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &ZwpPrimarySelectionDeviceV1,
        request: zwp_primary_selection_device_v1::Request,
        data: &SeatIndex,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let zwp_primary_selection_device_v1::Request::SetSelection { source, serial } = request
        else {
            return;
        };
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
        if !focused || !issued {
            state.refusals += 1;
            return;
        }
        if let Some(previous) = state.seats[seat].primary.take()
            && source.as_ref() != Some(&previous.source)
            && previous.source.is_alive()
        {
            previous.source.cancelled();
        }
        state.seats[seat].primary = source.map(|source| {
            let mimes = source
                .data::<PrimaryMimes>()
                .and_then(|data| data.0.lock().ok().map(|mimes| mimes.clone()))
                .unwrap_or_default();
            PrimaryRecord {
                source,
                owner: client.id(),
                mimes,
            }
        });
        if let Some(focus) = state.seats[seat].focus.clone() {
            state.send_primary(&focus, seat);
        }
    }
}

impl Dispatch<ZwpPrimarySelectionOfferV1, PrimaryOfferSource> for Server {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwpPrimarySelectionOfferV1,
        request: zwp_primary_selection_offer_v1::Request,
        data: &PrimaryOfferSource,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Request::Receive { mime_type, fd } = request
            && data.0.is_alive()
        {
            data.0.send(mime_type, fd.as_fd());
        }
    }
}
