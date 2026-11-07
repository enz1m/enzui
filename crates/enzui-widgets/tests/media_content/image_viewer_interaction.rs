//! Behavioural interaction tests for [`EnzImageViewer`].
//!
//! These drive the real widget through `iced_test::Simulator` and assert on
//! *published transforms*. That matters more here than for most widgets: the
//! whole point of the widget is that the transform is controlled, so "the widget
//! published the transform the application should adopt" is the only
//! observable that means anything. A test that reached into widget state would
//! bypass the public controlled-transform contract this target protects.
//!
//! The viewer is driven with a real decoded RGBA handle so the renderer can
//! measure it; a viewer whose image cannot be measured has an empty geometry
//! and correctly refuses to move, which is itself asserted below.

use enzui_core::zoom_pan::{ViewTransform, ZoomRange};
use enzui_widgets::image_viewer::enz_image_viewer;
use iced::widget::image;
use iced::{ContentFit, Element, Length, Point, Vector, mouse};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Moved(ViewTransform),
}

const FRAME: f32 = 200.0;

/// A 400x400 opaque image, so `Contain` into a 200x200 frame fits at exactly
/// 200x200 and every scale above 1 overflows on both axes.
fn handle() -> image::Handle {
    image::Handle::from_rgba(400, 400, vec![255; 400 * 400 * 4])
}

fn viewer(transform: ViewTransform) -> Element<'static, Msg> {
    enz_image_viewer(handle(), transform)
        .on_transform(Msg::Moved)
        .fit(ContentFit::Contain)
        .zoom_range(ZoomRange::new(0.5, 8.0).unwrap())
        .width(Length::Fixed(FRAME))
        .height(Length::Fixed(FRAME))
        .into()
}

fn messages(ui: Simulator<Msg>) -> Vec<Msg> {
    ui.into_messages().collect()
}

fn transforms(ui: Simulator<Msg>) -> Vec<ViewTransform> {
    messages(ui).into_iter().map(|Msg::Moved(t)| t).collect()
}

fn zoomed(scale: f32) -> ViewTransform {
    ViewTransform::new(scale, Vector::new(0.0, 0.0)).unwrap()
}

fn press() -> impl Iterator<Item = iced::Event> {
    std::iter::once(iced::Event::Mouse(mouse::Event::ButtonPressed(
        mouse::Button::Left,
    )))
}

fn release() -> impl Iterator<Item = iced::Event> {
    std::iter::once(iced::Event::Mouse(mouse::Event::ButtonReleased(
        mouse::Button::Left,
    )))
}

/// Move the pointer and emit the corresponding `CursorMoved`.
///
/// `Simulator::point_at` only updates the cursor the *next* event is delivered
/// with; the widget's pan handler listens for `CursorMoved`, so a drag has to
/// send both. Getting this wrong silently produces a test that never drags.
fn move_to(ui: &mut Simulator<Msg>, x: f32, y: f32) {
    ui.point_at(Point::new(x, y));
    let _ = ui.simulate(std::iter::once(iced::Event::Mouse(
        mouse::Event::CursorMoved {
            position: Point::new(x, y),
        },
    )));
}

// ---------------------------------------------------------------------------
// Guards
//
// Several tests below assert that nothing is published. Those assertions are
// worthless unless the same inputs demonstrably do publish under conditions
// where they should — otherwise a typo'd coordinate or an unhandled event type
// passes them all. These two tests are that guard.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Wheel
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Drag
// ---------------------------------------------------------------------------

#[test]
fn a_release_ends_the_drag() {
    let mut ui = Simulator::new(viewer(zoomed(2.0)));
    ui.point_at(Point::new(100.0, 100.0));
    let _ = ui.simulate(press());
    move_to(&mut ui, 80.0, 100.0);
    let _ = ui.simulate(release());

    let before = transforms(Simulator::new(viewer(zoomed(2.0))));
    assert!(before.is_empty());

    // A fresh viewer, dragged and released, then moved again: the post-release
    // move must not pan.
    let mut ui2 = Simulator::new(viewer(zoomed(2.0)));
    ui2.point_at(Point::new(100.0, 100.0));
    let _ = ui2.simulate(press());
    let _ = ui2.simulate(release());
    move_to(&mut ui2, 10.0, 100.0);

    assert!(
        messages(ui2).is_empty(),
        "a move after release must not pan"
    );

    let _ = ui;
}

// ---------------------------------------------------------------------------
// Grab cancellation on pointer / window loss
//
// `state.grab` was cleared in exactly one place — `ButtonReleased(Left)`. So a
// grab that never sees an explicit release (the pointer leaves the window, or
// the window loses focus to alt-tab / a modal / the compositor) stayed live,
// and the next `CursorMoved` resumed panning from the stale press origin —
// teleporting the image by however far the pointer travelled unseen. These
// assert the *observable* consequence (the far move must not pan), not that a
// private field was cleared: a test on `grab.is_none()` would pass even if a
// later event resurrected the pan.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Inert viewer
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Source and geometry edge cases
// ---------------------------------------------------------------------------

#[test]
fn a_touchpad_pinch_zooms_continuously_about_the_pointer() {
    let pinch = |scale: f32| {
        iced::Event::Gesture(iced::gesture::Event::Pinch {
            scale,
            phase: iced::gesture::Phase::Moved,
        })
    };
    let mut ui = Simulator::new(viewer(zoomed(1.0)));
    ui.point_at(Point::new(100.0, 100.0));
    let _ = ui.simulate(std::iter::once(pinch(0.5)));
    let published = transforms(ui);
    assert_eq!(published.len(), 1, "{published:?}");
    assert!(
        (published[0].scale() - 1.5).abs() < 1e-4,
        "1.5x, not a whole step: {published:?}"
    );

    let mut outside = Simulator::new(viewer(zoomed(1.0)));
    outside.point_at(Point::new(FRAME + 50.0, 100.0));
    let _ = outside.simulate(std::iter::once(pinch(0.5)));
    assert!(
        transforms(outside).is_empty(),
        "a pinch beside the viewer does nothing"
    );
}
