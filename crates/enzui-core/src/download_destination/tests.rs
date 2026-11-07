use super::*;
use crate::portal::PortalResponse;
use crate::portal::backend::{BackendReply, MockBackend};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Spawn {
    id: DownloadId,
    destination: String,
}

fn spawnable_downloads(broker: &DestinationBroker, wanted: &[DownloadId]) -> Vec<Spawn> {
    wanted
        .iter()
        .filter_map(|id| {
            broker.destination(*id).map(|dest| Spawn {
                id: *id,
                destination: dest.uri().to_owned(),
            })
        })
        .collect()
}

fn live(_: iced_core::window::Id) -> bool {
    true
}

fn reply(response: PortalResponse, uris: &[&str]) -> BackendReply {
    BackendReply {
        response,
        uris: uris.iter().map(|s| (*s).to_owned()).collect(),
    }
}

const D1: DownloadId = DownloadId(1);

#[test]
fn a_save_that_returns_no_uri_is_refused() {
    let mut backend = MockBackend::new();
    let mut broker = DestinationBroker::new();

    broker.request(D1, &mut backend, PortalParent::None, live);
    backend.script(reply(PortalResponse::Success, &[]));
    let settled = broker.pump(&mut backend);

    assert_eq!(settled[0].1, DestinationOutcome::Failed);
    assert_eq!(spawnable_downloads(&broker, &[D1]), vec![]);
}
