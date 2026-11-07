//! A custom widget that animates through replay stays correct when it
//! settles before its neighbours do.
//!
//! The widget is written the plain way: it publishes a track while it moves
//! and stops when it arrives. The runtime keeps replaying while the slower
//! neighbour moves, and a replay paints a node without a track where it was
//! recorded, mid-travel. The runtime draws that frame instead, so the widget
//! needs no settling logic of its own. It paints through
//! `Renderer::with_motion`, which snaps its offset as replay does, so drawn
//! and replayed frames put its edges on the same pixels.

use enzui_frame_check::FrameCheck;
use enzui_widgets::motion::{MotionEffect, SlideDirection, motion, next_scene_node};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, NodeId, Track};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Renderer as _, Shell};
use iced::time::{Duration, Instant};
use iced::widget::{column, container};
use iced::{Color, Element, Length, Rectangle, Size, Vector, mouse};
use std::cell::Cell;

const TRAVEL: Duration = Duration::from_millis(150);
const DISTANCE: f32 = 120.0;

struct Mover;

struct Moving {
    node: NodeId,
    start: Option<Instant>,
    offset: Cell<Vector>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Mover {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<Moving>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(Moving {
            node: next_scene_node(),
            start: None,
            offset: Cell::new(Vector::new(-DISTANCE, 0.0)),
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(200.0), Length::Fixed(40.0))
    }

    fn layout(&mut self, tree: &mut Tree, _: &iced::Renderer, limits: &layout::Limits) {
        layout::empty(limits, Length::Fixed(200.0), Length::Fixed(40.0)).apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        _: Layout,
        _: mouse::Cursor,
        _: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _: &Rectangle,
    ) {
        let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let state = tree.state.downcast_mut::<Moving>();
        let start = *state.start.get_or_insert(*now);
        let t =
            (now.saturating_duration_since(start).as_secs_f32() / TRAVEL.as_secs_f32()).min(1.0);
        let offset = Vector::new(-DISTANCE * (1.0 - t), 0.0);
        state.offset.set(offset);
        if t < 1.0 {
            shell.request_redraw();
            shell.publish_track(Track {
                node: state.node,
                translation: offset,
                opacity: 1.0,
                scale: 1.0,
                scale_origin: iced::Point::ORIGIN,
                rotation: 0.0,
                clip: None,
            });
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _: &iced::Theme,
        _: &renderer::Style,
        layout: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<Moving>();
        let offset = state.offset.get();
        renderer.with_motion(state.node, offset, |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    ..renderer::Quad::default()
                },
                Color::from_rgb8(230, 120, 40),
            );
        });
    }
}

fn view(shown: bool) -> Element<'static, ()> {
    let slow = motion(
        container(iced::widget::text("slow neighbour")).padding(10),
        shown,
        MotionEffect::Slide(SlideDirection::Left),
    )
    .spring(Some(enzui_theme::ExpressiveMotion::SlowSpatial.spring()));
    let mover: Element<'static, ()> = if shown {
        Element::new(Mover)
    } else {
        iced::widget::Space::new().height(40).into()
    };
    container(column![mover, container(slow).width(300).height(60)].spacing(10))
        .padding(150)
        .into()
}

#[test]
fn a_widget_that_stops_publishing_when_it_arrives_is_drawn_where_it_arrived() {
    let Some(mut check) = FrameCheck::new(
        Size::new(640.0, 400.0),
        enzui_theme::EnzTheme::dark().iced_theme(),
        || view(false),
    ) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    check.frames(5);
    check.view(|| view(true));
    check.frames(150);
    check.assert_replays_match();
    assert!(check.replays() > 10, "only {} replays", check.replays());
}
