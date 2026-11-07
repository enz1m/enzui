//! Realized media-control geometry, semantics, input, and runtime-theme behavior.

use enzui_core::identity::derive_id;
use enzui_core::media::{Capabilities, Generation, PlaybackState, Snapshot};
use enzui_widgets::button::ButtonSize;
use enzui_widgets::media_controls::{MediaControlsMessage, TimeDisplay, enz_media_controls};
use iced::advanced::widget::Id;
use iced::{Element, Event, Point, Size, mouse};
use iced_test::Simulator;
use iced_test::selector::id as by_id;
use std::time::Duration;

const PREFIX: &str = "main-player";

fn snapshot() -> Snapshot {
    Snapshot {
        generation: Generation::FIRST,
        source: None,
        capabilities: Capabilities {
            seekable: true,
            has_audio: true,
            has_video: false,
        },
        playback: PlaybackState::Playing,
        position: Duration::from_secs(20),
        duration: Some(Duration::from_secs(100)),
        volume: 0.2,
        muted: false,
        rate: 1.0,
        error: None,
    }
}

fn stream_snapshot() -> Snapshot {
    Snapshot {
        capabilities: Capabilities {
            seekable: false,
            has_audio: false,
            has_video: false,
        },
        duration: None,
        ..snapshot()
    }
}

fn view() -> Element<'static, MediaControlsMessage> {
    enz_media_controls(PREFIX, snapshot()).into()
}

fn simulator() -> Simulator<'static, MediaControlsMessage> {
    Simulator::with_size(
        iced_test::core::Settings::default(),
        Size::new(900.0, 140.0),
        view(),
    )
}

fn seek_id() -> Id {
    derive_id(PREFIX, "seek")
}

fn drag_range(ui: &mut Simulator<'static, MediaControlsMessage>, id: Id) {
    let target = ui.find(by_id(id)).expect("range has its stable id");
    let bounds = target.bounds();
    let y = bounds.center_y();
    let start = Point::new(bounds.x + bounds.width * 0.25, y);
    let end = Point::new(bounds.x + bounds.width * 0.80, y);

    ui.point_at(start);
    let _ = ui.simulate([Event::Mouse(mouse::Event::ButtonPressed(
        mouse::Button::Left,
    ))]);
    ui.point_at(end);
    let _ = ui.simulate([Event::Mouse(mouse::Event::CursorMoved { position: end })]);
    let _ = ui.simulate([Event::Mouse(mouse::Event::ButtonReleased(
        mouse::Button::Left,
    ))]);
}

#[test]
fn media_controls_seek_play_and_announce_time() {
    // pointer_drag_on_seek_range_emits_seek
    {
        let mut ui = simulator();
        drag_range(&mut ui, seek_id());
        let messages: Vec<_> = ui.into_messages().collect();
        let seeks: Vec<_> = messages
            .iter()
            .filter_map(|message| match message {
                MediaControlsMessage::Seek(position) => Some(position.as_secs_f32()),
                _ => None,
            })
            .collect();

        assert!(
            seeks.len() >= 2,
            "press and drag must both drive the seek range, got {messages:?}"
        );
        assert!(
            seeks.last().is_some_and(|seconds| *seconds > 70.0),
            "dragging to 80% must emit a late seek, got {seeks:?}"
        );
    }

    // sizing_large_plain_stream_play_pause_is_square_and_activates
    {
        let element: Element<'static, MediaControlsMessage> =
            enz_media_controls(PREFIX, stream_snapshot())
                .transport_button_size(ButtonSize::Xl)
                .time_display(TimeDisplay::Hidden)
                .into();
        let mut ui = Simulator::with_size(
            iced_test::core::Settings::default(),
            Size::new(252.0, 100.0),
            element,
        );
        let play_id = derive_id(PREFIX, "play-pause");
        let target = ui
            .find(by_id(play_id.clone()))
            .expect("plain play/pause exists");
        // Play/pause leads: the Xl button height (52) plus 12.
        assert_eq!(
            (target.bounds().width, target.bounds().height),
            (64.0, 64.0)
        );
        ui.click(by_id(play_id))
            .expect("plain play/pause is clickable");
        assert_eq!(
            ui.drain().collect::<Vec<_>>(),
            [MediaControlsMessage::PlayPause]
        );
        let _ = ui.tap_key(iced::keyboard::key::Named::Enter);
        assert_eq!(
            ui.into_messages().collect::<Vec<_>>(),
            [MediaControlsMessage::PlayPause]
        );
    }

    // the_seek_slider_speaks_a_media_time
    {
        use enzui_core::collect::SemanticsCollector;
        use enzui_core::semantics::Role;
        use iced::advanced::widget::Operation;
        use iced_test::runtime::user_interface::{Cache, UserInterface};

        let long = Snapshot {
            position: Duration::from_secs(3847),
            duration: Some(Duration::from_secs(7200)),
            ..snapshot()
        };
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .expect("headless renderer");
        let mut ui = UserInterface::build(
            Element::from(enz_media_controls(PREFIX, long)),
            Size::new(900.0, 140.0),
            Cache::default(),
            &mut renderer,
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(&renderer, &mut collector as &mut dyn Operation);
        let seek = collector
            .into_collected()
            .into_iter()
            .find(|node| node.semantics.role == Role::Slider)
            .expect("a seek slider");
        assert_eq!(seek.semantics.value.as_deref(), Some("1:04:07"));
    }
}
