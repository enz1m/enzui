//! MPRIS wire behavior on a private session bus.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_assert_message,
    reason = "test failures report protocol observations"
)]

#[path = "../../enzui-core/tests/support/private_bus.rs"]
mod private_bus;

use enzui_services::media::mpris::zbus_transport::*;
use enzui_services::media::mpris::{NO_TRACK_ID, track_id};
use enzui_services::media::*;
use iced_futures::futures::StreamExt as _;
use private_bus::PrivateBus;
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, OwnedValue};

fn loaded() -> MediaState {
    let mut state = MediaState::new();
    let generation = state.replace_source(MediaSource::local("/private/track.wav"));
    assert!(state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::SourceReady {
            capabilities: Capabilities {
                seekable: true,
                has_audio: true,
                has_video: false
            },
            duration: Some(Duration::from_secs(100)),
        }
    }));
    state
}

fn apply(state: &mut MediaState, kind: MediaEventKind) {
    assert!(state.apply(&MediaEvent {
        generation: state.generation(),
        kind
    }));
}

async fn peer(bus: &PrivateBus) -> zbus::Connection {
    tokio::time::timeout(
        Duration::from_secs(3),
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .method_timeout(Duration::from_secs(3))
            .build(),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn wire(peer: &zbus::Connection) -> zbus::MessageStream {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(MPRIS_OBJECT_PATH)
        .unwrap()
        .build();
    zbus::MessageStream::for_match_rule(rule, peer, Some(64))
        .await
        .unwrap()
}

async fn publication(stream: &mut zbus::MessageStream) -> Vec<i64> {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut seeks = Vec::new();
        loop {
            let message = stream.next().await.unwrap().unwrap();
            let header = message.header();
            match header.member().map(zbus::names::MemberName::as_str) {
                Some("Seeked") => seeks.push(message.body().deserialize::<i64>().unwrap()),
                Some("PropertiesChanged") => {
                    let (interface, changed, invalidated): (
                        String,
                        HashMap<String, OwnedValue>,
                        Vec<String>,
                    ) = message.body().deserialize().unwrap();
                    assert_eq!(interface, MPRIS_PLAYER_INTERFACE);
                    for forbidden in ["Position", "CanControl"] {
                        assert!(
                            !changed.contains_key(forbidden),
                            "forbidden property signal: {changed:?}"
                        );
                        assert!(!invalidated.iter().any(|name| name == forbidden));
                    }
                    // The last property signal is a positive barrier for absent Seeked signals.
                    if changed.contains_key("CanGoPrevious") {
                        return seeks;
                    }
                }
                _ => {}
            }
        }
    })
    .await
    .expect("MPRIS publication deadline")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn mpris_protocol_seek_commands_obey_track_bounds() {
    let bus = PrivateBus::spawn();
    let transport = ZbusMprisConnection::connect_to_address("SeekBounds", &bus.address).unwrap();
    let adapter = MprisAdapter::new("SeekBounds");
    let mut state = loaded();
    apply(
        &mut state,
        MediaEventKind::PositionUpdated(Duration::from_secs(30)),
    );
    runtime().block_on(async {
        let peer = peer(&bus).await;
        let player: zbus::Proxy<'_> = zbus::proxy::Builder::new(&peer)
            .destination(adapter.bus_name())
            .unwrap()
            .path(MPRIS_OBJECT_PATH)
            .unwrap()
            .interface(MPRIS_PLAYER_INTERFACE)
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .unwrap();
        for (offset, expected) in [
            (71_000_000_i64, MediaCommand::NextTrack),
            (-31_000_000, MediaCommand::Seek(Duration::ZERO)),
            (70_000_000, MediaCommand::Seek(Duration::from_secs(100))),
            (i64::MAX, MediaCommand::NextTrack),
            (i64::MIN, MediaCommand::Seek(Duration::ZERO)),
        ] {
            player.call::<_, _, ()>("Seek", &(offset,)).await.unwrap();
            let call = transport
                .recv_call()
                .expect("wire seek reaches application queue");
            let request = adapter.handle(&call, &mut state).unwrap();
            assert_eq!(request.command, expected);
        }
        for (track, position) in [
            (NO_TRACK_ID.to_owned(), 1_i64),
            (track_id(state.generation()), -1),
            (track_id(state.generation()), 100_000_001),
        ] {
            player
                .call::<_, _, ()>(
                    "SetPosition",
                    &(ObjectPath::try_from(track).unwrap(), position),
                )
                .await
                .unwrap();
            let call = transport.recv_call().unwrap();
            assert!(adapter.handle(&call, &mut state).is_err());
            assert_eq!(state.snapshot().position, Duration::from_secs(30));
        }
    });
}

#[test]
fn mpris_protocol_seeked_follows_confirmed_discontinuities_and_property_contracts() {
    let bus = PrivateBus::spawn();
    let mut transport =
        ZbusMprisConnection::connect_to_address("SeekSignals", &bus.address).unwrap();
    let adapter = MprisAdapter::new("SeekSignals");
    let mut state = MediaState::new();
    runtime().block_on(async {
        let peer = peer(&bus).await;
        let player: zbus::Proxy<'_> = zbus::proxy::Builder::new(&peer)
            .destination(adapter.bus_name())
            .unwrap()
            .path(MPRIS_OBJECT_PATH)
            .unwrap()
            .interface(MPRIS_PLAYER_INTERFACE)
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .unwrap();
        let introspection = zbus::Proxy::new(
            &peer,
            adapter.bus_name(),
            MPRIS_OBJECT_PATH,
            "org.freedesktop.DBus.Introspectable",
        )
        .await
        .unwrap();
        let xml: String = introspection.call("Introspect", &()).await.unwrap();
        assert!(xml.contains("signal name=\"Seeked\""), "{xml}");
        assert!(
            xml.contains("property name=\"Position\" type=\"x\" access=\"read\""),
            "{xml}"
        );
        for (name, annotation) in [("Position", "false"), ("CanControl", "const")] {
            let (_, property) = xml
                .split_once(&format!("property name=\"{name}\""))
                .unwrap();
            let (item, _) = property.split_once("</property>").unwrap();
            assert!(item.contains(&format!("value=\"{annotation}\"")), "{item}");
        }
        assert!(player.get_property::<bool>("CanControl").await.unwrap());
        assert!(!player.get_property::<bool>("CanPlay").await.unwrap());
        assert!(
            player
                .get_property::<HashMap<String, OwnedValue>>("Metadata")
                .await
                .unwrap()
                .is_empty()
        );
        let mut stream = wire(&peer).await;
        adapter.publish_state(&state, &mut transport);
        assert!(publication(&mut stream).await.is_empty());
        state = loaded();
        apply(
            &mut state,
            MediaEventKind::PositionUpdated(Duration::from_secs(30)),
        );
        adapter.publish_state(&state, &mut transport);
        assert!(publication(&mut stream).await.is_empty());
        assert!(player.get_property::<bool>("CanPlay").await.unwrap());
        assert!(
            player
                .get_property::<HashMap<String, OwnedValue>>("Metadata")
                .await
                .unwrap()
                .contains_key("mpris:trackid")
        );
        adapter
            .handle_and_publish(
                &MprisCall::Seek {
                    offset: 10,
                    micros: false,
                },
                &mut state,
                &mut transport,
            )
            .unwrap();
        assert!(publication(&mut stream).await.is_empty());
        apply(
            &mut state,
            MediaEventKind::PositionDiscontinuity(Duration::from_secs(40)),
        );
        adapter
            .handle_and_publish(&MprisCall::Pause, &mut state, &mut transport)
            .unwrap();
        assert_eq!(publication(&mut stream).await, [40_000_000]);
        apply(
            &mut state,
            MediaEventKind::PositionDiscontinuity(Duration::from_secs(40)),
        );
        adapter.publish_state(&state, &mut transport);
        assert_eq!(publication(&mut stream).await, [40_000_000]);
        apply(
            &mut state,
            MediaEventKind::PositionUpdated(Duration::from_secs(41)),
        );
        adapter.publish_state(&state, &mut transport);
        assert!(publication(&mut stream).await.is_empty());
        assert_eq!(
            player.get_property::<i64>("Position").await.unwrap(),
            41_000_000
        );
        let stale = state.generation();
        state.clear_source();
        assert!(!state.apply(&MediaEvent {
            generation: stale,
            kind: MediaEventKind::PositionDiscontinuity(Duration::from_secs(70))
        }));
        adapter.publish_state(&state, &mut transport);
        assert!(publication(&mut stream).await.is_empty());
        assert!(player.get_property::<bool>("CanControl").await.unwrap());
        assert!(!player.get_property::<bool>("CanPlay").await.unwrap());
    });
    let shutdown = transport.begin_shutdown().unwrap();
    let before = transport.publish_count();
    let inert = transport.signal_stats().inert;
    adapter.publish_state(&state, &mut transport);
    assert_eq!(transport.publish_count(), before);
    assert_eq!(transport.signal_stats().inert, inert + 1);
    assert!(shutdown.wait_blocking().is_complete());
}
