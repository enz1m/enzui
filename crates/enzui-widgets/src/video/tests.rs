//! Video capability gating, composed transport, loading semantics, and frame conversion.

use super::*;
use crate::media_controls::MediaControlsMessage;
use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::media::{Capabilities, Generation, PlaybackState, Snapshot};
use enzui_core::semantics::Role;
use iced::advanced::widget::Operation;
use std::time::Duration;

fn full_snapshot() -> Snapshot {
    Snapshot {
        generation: Generation::FIRST,
        source: None,
        capabilities: Capabilities {
            seekable: true,
            has_audio: true,
            has_video: true,
        },
        playback: PlaybackState::Playing,
        position: Duration::from_secs(30),
        duration: Some(Duration::from_secs(120)),
        volume: 0.8,
        muted: false,
        rate: 1.0,
        error: None,
    }
}

fn collected(view: iced::Element<'_, MediaControlsMessage>) -> Vec<CollectedNode> {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        view,
        iced::Size::new(800.0, 600.0),
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );

    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    collector.into_collected()
}

#[test]
fn surface_semantics_matches_the_region_the_widget_publishes() {
    let mut snap = full_snapshot();
    snap.capabilities.has_video = false;

    let surface = VideoSurface::NoVideoTrack;
    let helper = surface_semantics(surface, None);

    let view: iced::Element<'_, MediaControlsMessage> = enz_video_awaiting("p", snap).into();
    let nodes = collected(view);
    let region = nodes
        .iter()
        .find(|n| {
            n.semantics.role == Role::Status
                && n.semantics.label.as_deref() == Some("No video track")
        })
        .expect("a named Status region must be published for the absent video track");

    assert_eq!(
        helper.label.as_deref(),
        Some("No video track"),
        "the helper must carry the surface name"
    );

    assert_eq!(
        helper.role, region.semantics.role,
        "helper role must equal the published region role"
    );
    assert_eq!(
        helper.label, region.semantics.label,
        "helper label must equal the published region label"
    );
    assert_eq!(
        helper.live, region.semantics.live,
        "helper live-politeness must equal the published region"
    );

    let framed = surface_semantics(VideoSurface::Frame, Some("Sintel"));
    assert_eq!(
        framed.label.as_deref(),
        Some("Video: \u{2068}Sintel\u{2069}")
    );
    assert_ne!(framed.label, helper.label);
}

/// A paused (unchanged) borrowed frame keeps one handle, so the renderer
/// keeps its upload; a new frame gets a new one.
#[test]
fn an_unchanged_borrowed_frame_keeps_its_handle() {
    use enzui_core::media::{FrameSeq, Rgba8Frame};
    let frame =
        |seq| Rgba8Frame::new(Generation::FIRST, FrameSeq(seq), 2, 1, vec![9; 8]).expect("frame");
    let paused = frame(1);
    let first = shown_handle(FrameRef::Borrowed(&paused));
    let again = shown_handle(FrameRef::Borrowed(&paused));
    assert_eq!(first.id(), again.id(), "the same frame reuses its handle");
    let next = frame(2);
    assert_ne!(shown_handle(FrameRef::Borrowed(&next)).id(), first.id());
    let owned = shown_handle(FrameRef::Owned(frame(3)));
    let iced::advanced::image::Handle::Rgba { pixels, .. } = owned else {
        panic!("an RGBA handle")
    };
    assert_eq!(&pixels[..], &[9; 8][..]);
}
