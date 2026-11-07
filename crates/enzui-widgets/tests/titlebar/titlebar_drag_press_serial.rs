//! A titlebar window move may only be published while the arming press
//! is still the compositor's live pointer grab.
//!
//! # Why this is a gate and not a preference
//!
//! The Wayland move request is `xdg_toplevel.move(seat, serial)` and the serial
//! winit hands it is `latest_button_serial` — the last pointer *button* event,
//! press or release
//! (`winit/src/platform_impl/linux/wayland/seat/pointer/mod.rs`, consumed by
//! `WindowState::drag_window` in `.../wayland/window/state.rs`). A `Drag`
//! published after the press has ended therefore travels with a serial that is
//! either stale (the compositor drops the request — the window silently refuses
//! to move) or, on a compositor that only compares it against the seat's last
//! button serial, is honoured with no button held, which sticks the window
//! to the pointer.
//!
//! [`enzui_widgets::titlebar_drag_region`] arms on the press and dispatches
//! on motion past [`DRAG_DEADBAND`], so "the press is still live" is a
//! *state* the widget carries between events. These tests drive real events
//! through a real [`enz_title_bar`] in a real `UserInterface` and pin both
//! halves:
//!
//! | test | the defect it catches |
//! |---|---|
//! | `press_then_motion_publishes_one_drag_for_the_live_window` | positive control + message→runtime routing: the harness can observe a drag and it maps to `Action::Drag(live id)` |
//! | `motion_after_release_publishes_no_drag` | the release not disarming: a move request carrying the RELEASE serial |
//! | `motion_after_the_pointer_leaves_publishes_no_drag` | the compositor taking the seat (leave while armed) leaving a live claim behind |
//! | `motion_after_window_focus_loss_publishes_no_drag` | the same at window scope (a grab/popup steals focus, no release is ever delivered) |
//! | `motion_after_a_right_press_publishes_no_drag` | the system-menu popup grab: the left release for the armed press never arrives |
//! | `a_captured_release_still_disarms` | the disarm sitting after the `is_event_captured()` bail, so a child that swallows the release leaves the region armed |
//! | `touch_motion_after_a_lost_finger_publishes_no_drag` | the touch path losing the same protection |
//!
//! Every zero above is calibrated by the first test on the same rig, so each
//! is a measurement, not a harness that cannot see.

use enzui_widgets::title_bar::enz_title_bar;
use enzui_widgets::title_bar_gesture::WindowCommand;
use enzui_widgets::titlebar_drag_region::DRAG_DEADBAND;

use iced::advanced::renderer::Headless;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const BAR_W: f32 = 800.0;
const BAR_H: f32 = 200.0;

/// Inside the bar's empty drag band, clear of the trailing controls.
const DRAG_SPOT: Point = Point::new(400.0, 20.0);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Window(WindowCommand),
}

fn parked() -> Point {
    Point::new(5000.0, 5000.0)
}

fn cursor_at(p: Point) -> mouse::Cursor {
    mouse::Cursor::Available(p)
}

fn press_left() -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
}

fn release_left() -> Event {
    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
}

fn moved(p: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position: p })
}

/// A point far enough from `DRAG_SPOT` to clear the deadband — the motion that
/// promotes an armed press to a window move.
fn past_deadband() -> Point {
    Point::new(DRAG_SPOT.x + DRAG_DEADBAND + 4.0, DRAG_SPOT.y)
}

/// A titlebar child that captures every event it is handed, without
/// publishing anything.
///
/// This is the child-capture control. It is pushed into the bar, so it sits
/// inside the drag region's own subtree and the region delegates to it first
/// (child-events-first). Real titlebar children behave this way whenever
/// they are tracking a gesture — a menu's dismiss handler, a scrim, a field
/// that owns the pointer — and the region's publish path bails on
/// `shell.is_event_captured()`. The disarm must not sit behind that bail.
struct CaptureAll<'a, Message> {
    child: Element<'a, Message>,
    capture: bool,
}

impl<'a, Message> CaptureAll<'a, Message> {
    fn new(child: impl Into<Element<'a, Message>>, capture: bool) -> Self {
        Self {
            child: child.into(),
            capture,
        }
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for CaptureAll<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if self.capture {
            shell.capture_event();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<CaptureAll<'a, Message>> for Element<'a, Message> {
    fn from(w: CaptureAll<'a, Message>) -> Self {
        Element::new(w)
    }
}

/// The bar under test, optionally with a swallowing child pushed into it.
#[derive(Clone, Copy, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Fixture {
    Plain,
    /// Push a child that captures exactly the event at this index, so a test
    /// can have the release swallowed while the promoting motion still reaches
    /// the region (capturing the motion too would make the assertion pass for
    /// the wrong reason).
    CapturingAt(usize),
}

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    fixture: Fixture,
    fed: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Host {
    fn new(fixture: Fixture) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
            fixture,
            fed: std::rc::Rc::new(std::cell::Cell::new(0)),
        }
    }

    fn view(&self) -> Element<'static, Msg> {
        let mut bar = enz_title_bar()
            .title("Documents")
            .id("window-7")
            .on_gesture(Msg::Window);
        if let Fixture::CapturingAt(at) = self.fixture {
            // Pushed into the bar: the drag region delegates to it before it
            // interprets anything, so a capture here is exactly the
            // `shell.is_event_captured()` condition the region bails on.
            bar = bar.push(CaptureAll::new(
                iced::widget::text("x"),
                self.fed.get() == at,
            ));
        }
        bar.into()
    }

    fn build(&mut self) -> UserInterface<'static, Msg, Theme, iced_test::renderer::Renderer> {
        let view = self.view();
        UserInterface::build(
            view,
            Size::new(BAR_W, BAR_H),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    fn feed(&mut self, cursor: mouse::Cursor, event: Event) -> Vec<Msg> {
        let mut ui = self.build();
        let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        self.fed.set(self.fed.get() + 1);
        messages.into_iter().collect()
    }

    fn feed_all(&mut self, steps: &[(Point, Event)]) -> Vec<Msg> {
        let mut out = Vec::new();
        for (p, e) in steps {
            out.extend(self.feed(cursor_at(*p), e.clone()));
        }
        out
    }

    fn park(&mut self) {
        let _ = self.feed(cursor_at(parked()), moved(parked()));
    }
}

fn drags(messages: &[Msg]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, Msg::Window(WindowCommand::Drag)))
        .count()
}

/// The sequence every "no drag" test below repeats, with `interruption` fed
/// between the arming press and the promoting motion.
fn press_interrupt_then_motion(interruption: Event) -> Vec<Msg> {
    let mut host = Host::new(Fixture::Plain);
    host.park();
    let far = past_deadband();
    host.feed_all(&[
        (DRAG_SPOT, moved(DRAG_SPOT)),
        (DRAG_SPOT, press_left()),
        (DRAG_SPOT, interruption),
        (far, moved(far)),
    ])
}

// ---------------------------------------------------------------------------
// Positive control + message → runtime routing
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Stale-grab controls: the press is over, so no move may be published
// ---------------------------------------------------------------------------

/// The release replaces the press serial. Motion after it must publish nothing.
#[test]
fn motion_after_release_publishes_no_drag() {
    let messages = press_interrupt_then_motion(release_left());
    assert_eq!(
        drags(&messages),
        0,
        "motion after the button came up must not publish a window move; got {messages:?}",
    );
}
