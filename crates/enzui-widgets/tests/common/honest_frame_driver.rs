//! The honest-frame drivers: harnesses that never manufacture a frame the
//! widget did not ask for, and never hand a widget a relayout it did not
//! request.
//!
//! The drivers behave exactly as the one in
//! `tests/overlay_interactions/expander_opens_on_toggle.rs`, so every widget
//! with a show/hide slide is gated without manufactured frames. They are
//! generic over the message type and over a `view(flag)` closure, and
//! [`LiveLoop`] additionally records `has_layout_changed` so a
//! test can assert that a settled animation invalidates nothing (the
//! spin control).
//!
//! # Why the shape matters
//!
//! The usual widget harness in this crate (a) pumps a `RedrawRequested`
//! unconditionally every frame, manufacturing the very frames the widget is
//! supposed to request, and (b) calls `UserInterface::build` every frame, which
//! re-lays-out the whole tree and so manufactures the layout invalidation too.
//! A widget that requests no frames and invalidates no layout passes such a
//! harness while being completely frozen on screen.
//!
//! Both drivers here are built the other way round:
//!
//! * [`Driver`] separates the message-driven view rebuild
//!   ([`Driver::rebuild_pass`], which runs `diff` and carries no
//!   `RedrawRequested`) from the delivery of a single explicit event
//!   ([`Driver::deliver`]), and reports whether the resulting UI state asked
//!   for another frame. Nothing here pumps a frame that was not asked for.
//! * [`LiveLoop`] keeps one `UserInterface` alive across frames exactly as
//!   `iced_winit` does, rebuilding only when a message is applied. Against a
//!   live UI, a widget that asks for frames but never calls
//!   `shell.invalidate_layout()` redraws the same stale cached geometry
//!   forever — the failure mode that actually freezes the panel.
//!
//! `tests/common/` has no `main.rs`, so cargo does not compile it as a test
//! binary of its own. Directory targets that use this harness pull it in with a
//! `#[path = "../common/honest_frame_driver.rs"]` module declaration.

use iced::advanced::renderer;
use iced::advanced::widget::Operation;
use iced::mouse;
use iced::window;
use iced::{Element, Point, Size};
use std::time::{Duration, Instant};

use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

/// A view builder: takes the animated widget's input flag (expanded / shown)
/// and returns the tree to lay out.
pub type ViewFn<Message> = fn(bool) -> Element<'static, Message, iced::Theme, Renderer>;

/// A headless renderer honouring `ICED_TEST_BACKEND`.
pub fn new_renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

/// Cursor parked far away: no hover work can be mistaken for the animation's
/// work, and no stray hover animation can request a frame on the widget's
/// behalf.
pub fn far_cursor() -> mouse::Cursor {
    mouse::Cursor::Available(Point::new(5000.0, 5000.0))
}

/// A positional event that carries no frame instant — what a real app
/// delivers next after a message-driven rebuild.
pub fn cursor_moved() -> iced::Event {
    iced::Event::Mouse(mouse::Event::CursorMoved {
        position: Point::new(1.0, 1.0),
    })
}

pub fn theme_style() -> (iced::Theme, renderer::Style) {
    let theme = iced::Theme::Dark;
    let base = <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
    (
        theme,
        renderer::Style {
            text_color: base.text_color,
        },
    )
}

/// Whether a UI state carries a request for another frame.
pub fn asked_for_frame(state: &State) -> bool {
    matches!(
        state,
        State::Updated {
            redraw_request: window::RedrawRequest::NextFrame | window::RedrawRequest::At(_),
            ..
        }
    )
}

/// Default `motion-normal` is 180 ms; sleep well past it to settle.
pub fn settle() {
    std::thread::sleep(Duration::from_millis(400));
}

// ---------------------------------------------------------------------------
// Driver A — rebuild-per-pass, used to observe the *request* itself.
// ---------------------------------------------------------------------------

/// Separates the two things a real `iced` app keeps separate and the usual
/// suppression harnesses fuse together: the message-driven view rebuild
/// ([`Self::rebuild_pass`], which runs `diff` and carries no
/// `RedrawRequested`) and the delivery of a single explicit event
/// ([`Self::deliver`]). Nothing here pumps a frame that was not asked for.
pub struct Driver<Message> {
    cache: Option<Cache>,
    renderer: Renderer,
    bounds: Size,
    view: ViewFn<Message>,
    flag: bool,
}

impl<Message> Driver<Message> {
    pub fn new(view: ViewFn<Message>, flag: bool, bounds: Size) -> Self {
        Self {
            cache: Some(Cache::default()),
            renderer: new_renderer(),
            bounds,
            view,
            flag,
        }
    }

    fn build(&mut self) -> UserInterface<'static, Message, iced::Theme, Renderer> {
        UserInterface::build(
            (self.view)(self.flag),
            self.bounds,
            self.cache.take().expect("cache present"),
            &mut self.renderer,
        )
    }

    fn draw(&mut self, ui: &mut UserInterface<'static, Message, iced::Theme, Renderer>) {
        let (theme, style) = theme_style();
        ui.draw(&mut self.renderer, &theme, &style, far_cursor());
    }

    /// The message-driven view rebuild: build (runs `diff`) + draw, no
    /// event of any kind. This is the pass that starts the animation.
    pub fn rebuild_pass(&mut self) {
        let mut ui = self.build();
        self.draw(&mut ui);
        self.cache = Some(ui.into_cache());
    }

    /// Rebuild + draw + deliver exactly one event. Returns `true` when the
    /// resulting UI state asks for another frame.
    pub fn deliver(&mut self, event: iced::Event) -> bool {
        let mut ui = self.build();
        self.draw(&mut ui);
        let mut messages = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            far_cursor(),
            &mut self.renderer,
            &mut messages,
        );
        self.cache = Some(ui.into_cache());
        asked_for_frame(&state)
    }

    /// Deliver one `RedrawRequested`.
    pub fn frame(&mut self) -> bool {
        self.deliver(iced::Event::Window(window::Event::RedrawRequested(
            Instant::now(),
        )))
    }

    pub fn set_flag(&mut self, f: bool) {
        self.flag = f;
    }

    /// Build + draw + run `operation` over the laid-out tree.
    pub fn operate(&mut self, operation: &mut dyn Operation) {
        let mut ui = self.build();
        self.draw(&mut ui);
        ui.operate(&self.renderer, operation);
        self.cache = Some(ui.into_cache());
    }
}

// ---------------------------------------------------------------------------
// Driver B — one live UserInterface across frames, as iced_winit does.
// ---------------------------------------------------------------------------

/// Holds one `UserInterface` alive across frames, like `iced_winit`'s
/// `user_interfaces` map, and rebuilds it only when a message is applied.
///
/// This distinction is the whole point of the layout half of the gate.
/// Rebuilding per frame (what [`Driver`] and the older suppression harnesses
/// do) calls `UserInterface::build`, which re-runs `layout` over the entire
/// tree — so it silently supplies the relayout the widget is supposed to
/// request via `shell.invalidate_layout()`. Against a live UI, a widget that
/// asks for frames but never invalidates redraws the same stale cached
/// geometry forever.
pub struct LiveLoop<Message> {
    ui: Option<UserInterface<'static, Message, iced::Theme, Renderer>>,
    renderer: Renderer,
    bounds: Size,
    view: ViewFn<Message>,
    /// Whether the last [`Self::redraw_frame`] relaid the tree out — i.e.
    /// whether some widget called `shell.invalidate_layout()` (or the UI went
    /// `Outdated`). The settled-animation negative control reads this.
    last_layout_changed: bool,
}

impl<Message> LiveLoop<Message> {
    pub fn new(view: ViewFn<Message>, flag: bool, bounds: Size) -> Self {
        let mut renderer = new_renderer();
        let ui = UserInterface::build(view(flag), bounds, Cache::default(), &mut renderer);
        let mut me = Self {
            ui: Some(ui),
            renderer,
            bounds,
            view,
            last_layout_changed: false,
        };
        me.draw();
        me
    }

    fn draw(&mut self) {
        let (theme, style) = theme_style();
        let ui = self.ui.as_mut().expect("live ui");
        ui.draw(&mut self.renderer, &theme, &style, far_cursor());
    }

    /// `iced_winit`'s `build_user_interfaces`: take the cache, rebuild the view
    /// (which runs `diff`, starting the transition), draw. No event.
    pub fn rebuild(&mut self, flag: bool) {
        let cache = self.ui.take().expect("live ui").into_cache();
        let ui = UserInterface::build((self.view)(flag), self.bounds, cache, &mut self.renderer);
        self.ui = Some(ui);
        self.draw();
    }

    /// `iced_winit`'s `RedrawRequested` branch: `update` on the live ui — no
    /// rebuild, so no free relayout — then draw. Returns whether another frame
    /// was requested; [`Self::layout_changed`] reports whether this frame
    /// actually re-measured.
    pub fn redraw_frame(&mut self) -> bool {
        self.redraw_frame_counting().0
    }

    /// [`Self::redraw_frame`], also counting the messages the frame published
    /// to the application: each one is an `update` and a view rebuild.
    pub fn redraw_frame_counting(&mut self) -> (bool, usize) {
        self.redraw_frame_at(Instant::now())
    }

    /// [`Self::redraw_frame_counting`] for a frame presented at `at`.
    pub fn redraw_frame_at(&mut self, at: Instant) -> (bool, usize) {
        let mut messages = iced::advanced::shell::Bus::new();
        let ui = self.ui.as_mut().expect("live ui");
        let (state, _) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[iced::Event::Window(window::Event::RedrawRequested(at))],
            far_cursor(),
            &mut self.renderer,
            &mut messages,
        );
        self.last_layout_changed = state.has_layout_changed();
        self.draw();
        (asked_for_frame(&state), messages.messages().len())
    }

    /// Whether the most recent [`Self::redraw_frame`] invalidated the layout.
    pub fn layout_changed(&self) -> bool {
        self.last_layout_changed
    }

    /// An RGBA8 screenshot of the live interface at `scale`, drawn from the
    /// same cached geometry the user is looking at.
    ///
    /// Deliberately a *pixel* readback rather than another `operate` pass: an
    /// `Operation` reports the laid-out rectangle, which is exactly the thing a
    /// frozen-layout bug leaves stale-but-self-consistent. Painted pixels are
    /// the independent witness.
    pub fn screenshot(&mut self, scale: f32) -> (Vec<u8>, u32, u32) {
        use iced::advanced::graphics::core::theme::Base;
        use iced::advanced::renderer::Headless;

        let (theme, style) = theme_style();
        {
            let ui = self.ui.as_mut().expect("live ui");
            ui.draw(&mut self.renderer, &theme, &style, far_cursor());
        }
        let w = (self.bounds.width * scale) as u32;
        let h = (self.bounds.height * scale) as u32;
        let buf = self.renderer.screenshot(
            Size::new(w, h),
            scale,
            <iced::Theme as Base>::base(&theme).background_color,
        );
        (buf, w, h)
    }

    /// Run `operation` over the live (never rebuilt) interface, so the geometry
    /// read is the cached one the user would actually be looking at.
    pub fn operate(&mut self, operation: &mut dyn Operation) {
        self.ui
            .as_mut()
            .expect("live ui")
            .operate(&self.renderer, operation);
    }
}
