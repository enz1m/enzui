//! The docked-motion benchmark fixture: one sidebar transition, driven through the
//! real `UserInterface` loop over a generated document of a chosen size,
//! with or without an app message on every frame.
//!
//! It is deliberately separate from the gates that read it
//! (`motion_content_scaling`) so the same harness can be pointed at a new
//! question without touching an assertion, and it counts the same four
//! lifecycle phases `motion_frame_cost` counts (build / layout / operate
//! / draw) so the two files' numbers are comparable.
//!
//! # The chatter axis (the axis a timing-only fixture does not cover)
//!
//! `iced_winit` keeps one `UserInterface` alive while a widget animates and
//! rebuilds it from the `Cache` as soon as any message is produced. A
//! text editor with a blinking caret, a directory that is still streaming and
//! a document that autosaves all publish messages during a 250 ms sidebar
//! transition, so [`Chatter::MessagePerFrame`] — rebuild the interface on
//! every frame — is the loaded half of the real app, not a pessimisation.

#![allow(
    dead_code,
    reason = "shared by several tests, and each uses only part of it"
)]

use enzui_theme::{EnzTheme, Palette};
use enzui_widgets::app_shell::{ShellLayout, ShellTokens, enz_app_shell};
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::toolbar_icon::ToolbarIconTokens;
use iced::advanced::widget::{self, Operation, Widget};
use iced::advanced::{Layout, Shell, layout, mouse, overlay, renderer};
use iced::widget::{column, container, scrollable, text};
use iced::{Color, Element, Event, Length, Rectangle, Size, Theme, Vector};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum Msg {
    Content,
    Toggle,
}

pub const SIDEBAR_INK: Color = Color::from_rgb(0.0, 1.0, 1.0);
const TOGGLE_INK: Color = Color::from_rgb(1.0, 0.0, 1.0);

/// The device scale the toggle's ink is sampled at.
///
/// The shell's disclosure is a 16 px lucide glyph whose artwork is a
/// stroke: `stroke-width="2"` in a 24-unit viewBox, so 1.33 px wide at the
/// production `icon-sm` size. At one device pixel per logical pixel that
/// stroke never covers a whole pixel — the brightest pixel the shell paints
/// for pinned `(255, 0, 255)` ink is `(243, 3, 243)` — so an exact-colour
/// probe reports no toggle at all and every gate driven through this
/// harness panics on the press prerequisite instead of measuring its own
/// subject.
///
/// Sampling the same painted frame at 2x is a configuration the product runs
/// (every HiDPI desktop) and touches neither the tree, the layout, the glyph
/// size nor the sentinel colour: the stroke spans 2.67 device pixels, so its
/// core pixels carry the pinned ink exactly and [`painted_bounds`] keeps its
/// ±3 match. Device bounds are converted back to logical coordinates before
/// any caller sees them, so the press point and every measured extent stay in
/// the units the shell laid out.
const INK_SAMPLE_SCALE: f32 = 2.0;

/// The scale the opaque pane fills are sampled at.
///
/// A `container` background saturates whole pixels at any scale, so the cyan
/// sidebar needs no HiDPI sample — and this probe runs on every frame of a
/// cost benchmark, where a 4x pixel scan would be the most expensive thing in
/// the file.
const FILL_SAMPLE_SCALE: f32 = 1.0;

/// Whether the app publishes a message while the sidebar animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chatter {
    /// Nothing produces a message: the interface is retained across frames.
    Quiet,
    /// A message every frame, so the runtime rebuilds the interface from its
    /// cache — a caret blink, a streaming directory, an autosave tick.
    MessagePerFrame,
}

/// Per-frame work counters, shared with the generated content tree.
#[derive(Clone, Default)]
pub struct Work {
    /// Invocations of the content *view closure* — a whole-app rebuild.
    pub builds: Rc<Cell<u32>>,
    /// `Widget::diff` calls on counted content leaves.
    pub diffs: Rc<Cell<u32>>,
    /// `Widget::layout` calls on counted content leaves.
    pub layouts: Rc<Cell<u32>>,
    /// `Widget::operate` calls on counted content leaves.
    pub operates: Rc<Cell<u32>>,
    /// `Widget::draw` calls on counted content leaves.
    pub draws: Rc<Cell<u32>>,
    /// `Widget::update` calls on counted content leaves — the frame event
    /// reaching a document that asked for no frames.
    pub updates: Rc<Cell<u32>>,
    /// Whether the document's explicit frame-listener leaf is active.
    pub frame_listener_active: Rc<Cell<bool>>,
    /// Frame calls delivered to that explicit interested leaf.
    pub interested_frames: Rc<Cell<u32>>,
}

impl Work {
    pub fn clear(&self) {
        self.builds.set(0);
        self.diffs.set(0);
        self.layouts.set(0);
        self.operates.set(0);
        self.draws.set(0);
        self.updates.set(0);
        self.interested_frames.set(0);
    }
}

struct ContentFrameListener {
    work: Work,
}

impl Widget<Msg, Theme, Renderer> for ContentFrameListener {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(0.0), Length::Fixed(0.0))
    }

    fn layout(&mut self, tree: &mut widget::Tree, _renderer: &Renderer, _limits: &layout::Limits) {
        layout::Node::new(Size::ZERO).apply(tree);
    }

    fn frame_interest(&self, _tree: &widget::Tree) -> bool {
        self.work.frame_listener_active.get()
    }

    fn update_frame(
        &mut self,
        _tree: &mut widget::Tree,
        _event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _shell: &mut Shell<'_, Msg>,
        _viewport: &Rectangle,
    ) {
        self.work
            .interested_frames
            .set(self.work.interested_frames.get() + 1);
        self.work.frame_listener_active.set(false);
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}

/// A transparent wrapper that counts the lifecycle visits its child receives.
struct Counted<'a> {
    child: Element<'a, Msg>,
    work: Work,
}

impl<'a> Counted<'a> {
    fn new(child: impl Into<Element<'a, Msg>>, work: Work) -> Self {
        Self {
            child: child.into(),
            work,
        }
    }
}

impl Widget<Msg, Theme, Renderer> for Counted<'_> {
    fn diff(&mut self, tree: &mut widget::Tree) {
        self.work.diffs.set(self.work.diffs.get() + 1);
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn frame_interest(&self, tree: &widget::Tree) -> bool {
        tree.children
            .first()
            .is_none_or(widget::Tree::has_frame_interest)
    }

    fn layout(&mut self, tree: &mut widget::Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.work.layouts.set(self.work.layouts.get() + 1);
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.work.operates.set(self.work.operates.get() + 1);
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
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Msg>,
        viewport: &Rectangle,
    ) {
        self.work.updates.set(self.work.updates.get() + 1);
        if self.work.updates.get() == 1
            && matches!(
                event,
                Event::Window(iced::window::Event::RedrawRequested(_))
            )
            && std::env::var("DBG_BT").is_ok()
        {
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let frames: Vec<String> = bt
                .lines()
                .filter(|l| {
                    l.contains("enzui_widgets::")
                        || l.contains("iced_widget::")
                        || l.contains("iced_runtime::")
                })
                .filter(|l| !l.contains(" at "))
                .take(14)
                .map(|l| l.trim().chars().take(110).collect())
                .collect();
            eprintln!("DBG BT {frames:#?}");
        }
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.work.draws.set(self.work.draws.get() + 1);
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

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Msg, Theme, Renderer>> {
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

/// Zero-size runtime source that publishes a real app message on each frame.
/// It models a streaming/autosave subscription without manually forcing the
/// rebuild that the message is meant to cause.
struct FrameMessage {
    enabled: bool,
}

impl FrameMessage {
    fn publish_on_redraw(&self, event: &Event, shell: &mut Shell<'_, Msg>) {
        if self.enabled
            && matches!(
                event,
                Event::Window(iced::window::Event::RedrawRequested(_))
            )
        {
            shell.publish(Msg::Content);
        }
    }
}

impl Widget<Msg, Theme, Renderer> for FrameMessage {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(0.0), Length::Fixed(0.0))
    }

    fn layout(&mut self, tree: &mut widget::Tree, _renderer: &Renderer, _limits: &layout::Limits) {
        layout::Node::new(Size::ZERO).apply(tree);
    }

    fn frame_interest(&self, _tree: &widget::Tree) -> bool {
        self.enabled
    }

    fn update(
        &mut self,
        _tree: &mut widget::Tree,
        event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Msg>,
        _viewport: &Rectangle,
    ) {
        // The one-shot geometry route uses ordinary `update`, not
        // `update_frame`. A real redraw-driven source must publish on either
        // dispatch path; otherwise the chatty arm silently becomes quiet
        // exactly on geometry reconciliation frames.
        self.publish_on_redraw(event, shell);
    }

    fn update_frame(
        &mut self,
        _tree: &mut widget::Tree,
        event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Msg>,
        _viewport: &Rectangle,
    ) {
        self.publish_on_redraw(event, shell);
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}

/// A generated document: `paragraphs` wrapped, measured paragraphs — the
/// shape of an app's editor pane at a chosen size. The text is derived from
/// the index so every paragraph is a distinct shaping job (a repeated string
/// would let a text cache flatter the measurement).
pub fn document(paragraphs: usize, work: Work) -> Element<'static, Msg> {
    let mut items: Vec<Element<'static, Msg>> = Vec::with_capacity(paragraphs);
    for index in 0..paragraphs {
        let body = format!(
            "{index:04} The seam moves while this paragraph stays where it is; measuring it \
             again on every frame is the cost this fixture measures, and paragraph {index} is one of \
             many that pay it."
        );
        items.push(Element::new(Counted::new(
            text(body).size(14).width(Length::Fill),
            work.clone(),
        )));
    }
    column(items).spacing(6).into()
}

fn shell(
    state: NavSplitState,
    paragraphs: usize,
    work: Work,
    chatter: Chatter,
) -> Element<'static, Msg> {
    let build_work = work;
    let mut toggle_tokens = ToolbarIconTokens::from_palette(&Palette::dark());
    toggle_tokens.icon_color = TOGGLE_INK;
    toggle_tokens.hover_color = TOGGLE_INK;
    toggle_tokens.pressed_color = TOGGLE_INK;
    toggle_tokens.active_color = TOGGLE_INK;
    toggle_tokens.disabled_color = TOGGLE_INK;
    toggle_tokens.hover_background = Color::TRANSPARENT;
    toggle_tokens.pressed_background = Color::TRANSPARENT;
    toggle_tokens.active_background = Color::TRANSPARENT;
    let shell_tokens = ShellTokens {
        toggle_icon_tokens: Some(toggle_tokens),
        ..ShellTokens::default()
    };
    let shell: Element<'static, Msg> = enz_app_shell(move || {
        build_work.builds.set(build_work.builds.get() + 1);
        let document: Element<'static, Msg> = scrollable(document(paragraphs, build_work.clone()))
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        iced::widget::Stack::with_children(vec![
            document,
            Element::new(ContentFrameListener {
                work: build_work.clone(),
            }),
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    })
    .tokens(shell_tokens)
    .layout(ShellLayout::TitlebarSidebarContent)
    .state(state)
    .sidebar(|| {
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(SIDEBAR_INK.into()),
                ..container::Style::default()
            })
            .into()
    })
    .titlebar(|| iced::widget::Space::new().into())
    .on_toggle_sidebar(Msg::Toggle)
    .into();

    iced::widget::Stack::with_children(vec![
        shell,
        Element::new(FrameMessage {
            enabled: chatter == Chatter::MessagePerFrame,
        }),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

pub fn new_renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

pub fn theme(reduced_motion: bool) -> Theme {
    let mut enz = EnzTheme::dark();
    enz.motion.reduced_motion = reduced_motion;
    enz.iced_theme()
}

pub fn paint(
    ui: &mut UserInterface<'_, Msg, Theme, Renderer>,
    renderer: &mut Renderer,
    theme: &Theme,
) {
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(theme);
    ui.draw(
        renderer,
        theme,
        &renderer::Style {
            text_color: base.text_color,
        },
        mouse::Cursor::Unavailable,
    );
}

/// One measured frame of the driven interface.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// Milliseconds since the transition's own edge.
    pub at_ms: f32,
    /// Content view-closure invocations inside this frame.
    pub builds: u32,
    /// Counted content-leaf reconciliations inside this frame.
    pub diffs: u32,
    /// Counted content leaves laid out inside this frame.
    pub layouts: u32,
    /// Counted content leaves visited by a focus/semantics walk.
    pub operates: u32,
    /// Counted content leaves painted inside this frame.
    pub draws: u32,
    /// Counted static content leaves handed the frame event inside this frame.
    pub updates: u32,
    /// Calls delivered to the explicit interested content listener.
    pub interested_frames: u32,
    /// Shell layout invalidations requested inside this frame.
    pub invalidations: u32,
    /// Real toggle-input dispatch through first target-state paint, in ms.
    /// Populated on the first record only.
    pub input_to_first_frame_ms: f32,
    /// Wall time of the frame's `update` (plus the rebuild a message forces).
    pub update_ms: f32,
    /// Wall time of the frame's `draw`.
    pub draw_ms: f32,
    /// Did the frame ask for another one?
    pub wants_more: bool,
    /// Sidebar ink at the live laid-out width — the visible trajectory.
    pub progress: f32,
}

/// Drive one transition and return the per-frame record.
pub fn drive(
    size: Size,
    from: NavSplitState,
    to: NavSplitState,
    paragraphs: usize,
    frames: usize,
    chatter: Chatter,
) -> Vec<Frame> {
    drive_with(size, from, to, paragraphs, frames, chatter, Work::default())
}

/// [`drive`] with a caller-owned [`Work`], and the whole harness in one place.
pub fn drive_with(
    size: Size,
    from: NavSplitState,
    to: NavSplitState,
    paragraphs: usize,
    frames: usize,
    chatter: Chatter,
    work: Work,
) -> Vec<Frame> {
    drive_inner(size, from, to, paragraphs, frames, chatter, work, false)
}

/// The reduced-motion control: the same transition under a theme whose motion
/// policy is off. It must arrive on the first frame and request none.
pub fn drive_reduced_motion(
    size: Size,
    from: NavSplitState,
    to: NavSplitState,
    paragraphs: usize,
    frames: usize,
) -> Vec<Frame> {
    drive_inner(
        size,
        from,
        to,
        paragraphs,
        frames,
        Chatter::Quiet,
        Work::default(),
        true,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "each argument is one knob of the driven scenario"
)]
fn drive_inner(
    size: Size,
    from: NavSplitState,
    to: NavSplitState,
    paragraphs: usize,
    frames: usize,
    chatter: Chatter,
    work: Work,
    reduced_motion: bool,
) -> Vec<Frame> {
    let mut renderer = new_renderer();
    let theme = theme(reduced_motion);

    // Settle at the starting state, then drive the shell's real disclosure
    // control. The emitted Toggle message is the authority for constructing
    // the target state; this keeps the benchmark on the same input -> update
    // -> view -> first-paint path as the running application.
    let mut ui = UserInterface::build(
        shell(from, paragraphs, work.clone(), chatter),
        size,
        Cache::default(),
        &mut renderer,
    );
    paint(&mut ui, &mut renderer, &theme);
    let toggle = toggle_bounds(&mut renderer, &theme, size).expect("painted shell toggle");
    let point = iced::Point::new(toggle.center_x(), toggle.center_y());
    let mut input_messages = iced::advanced::shell::Bus::new();
    let input_clock = std::time::Instant::now();
    let _ = ui.update(
        &iced::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[
            Event::Mouse(mouse::Event::CursorMoved { position: point }),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ],
        mouse::Cursor::Available(point),
        &mut renderer,
        &mut input_messages,
    );
    assert!(
        input_messages
            .iter()
            .any(|message| matches!(message, Msg::Toggle)),
        "the real sidebar control must publish Toggle"
    );
    let cache = ui.into_cache();

    let _ = enzui_widgets::app_shell::build_probe::take_motion_edge();
    let mut ui = UserInterface::build(
        shell(to, paragraphs, work.clone(), chatter),
        size,
        cache,
        &mut renderer,
    );
    // Anchor on the transition's own edge, never on wall time — a
    // harness that reads its own clock after building a 900-paragraph tree
    // measures the test schedule, not the shell.
    let start = enzui_widgets::app_shell::build_probe::take_motion_edge();
    let start = match start {
        Some(start) => start,
        None if reduced_motion => iced::time::Instant::now(),
        None => panic!(
            "the target state must have stamped a transition edge; without it this harness \
             would anchor on wall time and measure the test schedule"
        ),
    };
    paint(&mut ui, &mut renderer, &theme);
    let input_to_first_frame_ms = input_clock.elapsed().as_secs_f32() * 1000.0;

    let mut records = Vec::with_capacity(frames);
    let mut held = Some(ui);
    for index in 0..frames {
        let at_ms = index as f32 * 16.7;
        let at = start + iced::time::Duration::from_micros((at_ms * 1000.0) as u64);
        work.clear();
        let mut ui = held
            .take()
            .expect("the interface is retained across frames");
        let mut messages = iced::advanced::shell::Bus::new();
        let invalidations_before = enzui_widgets::app_shell::build_probe::layout_invalidations();
        let clock = std::time::Instant::now();
        let state = ui
            .update(
                &iced::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[Event::Window(iced::window::Event::RedrawRequested(at))],
                mouse::Cursor::Unavailable,
                &mut renderer,
                &mut messages,
            )
            .0;
        // What `iced_winit` does when the application produced a message: the
        // view is rebuilt and the interface reconstructed from the cache. The
        // clock spans it, because the user feels it inside the same frame.
        let has_content_message = messages
            .iter()
            .any(|message| matches!(message, Msg::Content));

        assert_eq!(
            has_content_message,
            chatter == Chatter::MessagePerFrame,
            "the chatty arm must rebuild only because its real frame source published (frame {index})"
        );
        let mut ui = if has_content_message {
            let cache = ui.into_cache();
            UserInterface::build(
                shell(to, paragraphs, work.clone(), chatter),
                size,
                cache,
                &mut renderer,
            )
        } else {
            ui
        };
        let update_ms = clock.elapsed().as_secs_f32() * 1000.0;

        let clock = std::time::Instant::now();
        paint(&mut ui, &mut renderer, &theme);
        let draw_ms = clock.elapsed().as_secs_f32() * 1000.0;
        let wants_more = matches!(
            state,
            State::Updated {
                redraw_request: iced::window::RedrawRequest::NextFrame
                    | iced::window::RedrawRequest::At(_),
                ..
            }
        );
        records.push(Frame {
            at_ms,
            builds: work.builds.get(),
            diffs: work.diffs.get(),
            layouts: work.layouts.get(),
            operates: work.operates.get(),
            draws: work.draws.get(),
            updates: work.updates.get(),
            interested_frames: work.interested_frames.get(),
            invalidations: (enzui_widgets::app_shell::build_probe::layout_invalidations()
                - invalidations_before) as u32,
            input_to_first_frame_ms: if index == 0 {
                input_to_first_frame_ms
            } else {
                0.0
            },
            update_ms,
            draw_ms,
            wants_more,
            progress: sidebar_extent(&mut renderer, &theme, size),
        });
        held = Some(ui);
    }
    records
}

/// Logical bounds of the pixels the last painted frame filled with `color`,
/// sampled at `scale` device pixels per logical pixel.
fn painted_bounds(
    renderer: &mut Renderer,
    theme: &Theme,
    size: Size,
    color: Color,
    scale: f32,
) -> Option<Rectangle> {
    painted_bounds_with_tolerance(renderer, theme, size, color, scale, 3, 3)
}

/// Like [`painted_bounds`], but with separate tolerances for channels that
/// carry the ink (`ink_tol`) and channels that are nominally zero in the
/// target colour (`zero_tol`). SVG icon glyphs composite against the surface
/// background, so their body pixels rarely reach the nominal ink — a
/// tolerance calibrated for flat fills misses the entire glyph.
fn painted_bounds_with_tolerance(
    renderer: &mut Renderer,
    theme: &Theme,
    size: Size,
    color: Color,
    scale: f32,
    ink_tol: i16,
    zero_tol: i16,
) -> Option<Rectangle> {
    use iced::advanced::renderer::Headless as _;

    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(theme);
    let size = Size::new(size.width * scale, size.height * scale);
    let pixels = renderer.screenshot(
        Size::new(size.width as u32, size.height as u32),
        scale,
        base.background_color,
    );
    let want = [
        (color.r * 255.0).round() as i16,
        (color.g * 255.0).round() as i16,
        (color.b * 255.0).round() as i16,
    ];
    let tols = [
        if want[0] > 127 { ink_tol } else { zero_tol },
        if want[1] > 127 { ink_tol } else { zero_tol },
        if want[2] > 127 { ink_tol } else { zero_tol },
    ];
    let (mut min_x, mut min_y) = (size.width as u32, size.height as u32);
    let (mut max_x, mut max_y) = (0, 0);
    let mut seen = false;

    for y in 0..size.height as u32 {
        for x in 0..size.width as u32 {
            let offset = ((y * size.width as u32 + x) * 4) as usize;
            if (0..3).all(|channel| {
                (i16::from(pixels[offset + channel]) - want[channel]).abs() <= tols[channel]
            }) {
                seen = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    seen.then(|| {
        Rectangle::new(
            iced::Point::new(min_x as f32 / scale, min_y as f32 / scale),
            Size::new(
                (max_x - min_x + 1) as f32 / scale,
                (max_y - min_y + 1) as f32 / scale,
            ),
        )
    })
}

fn toggle_bounds(renderer: &mut Renderer, theme: &Theme, size: Size) -> Option<Rectangle> {
    // The toggle is a composited SVG icon glyph, not a flat fill: anti-aliased
    // edges blend against the dark pane background. Keep enough tolerance to
    // find the blended body while still excluding background pixels.
    painted_bounds_with_tolerance(renderer, theme, size, TOGGLE_INK, INK_SAMPLE_SCALE, 60, 35)
}

/// Logical bounds of the shell toggle's pinned magenta glyph in the last
/// painted frame.
pub fn toggle_ink(renderer: &mut Renderer, theme: &Theme, size: Size) -> Option<Rectangle> {
    toggle_bounds(renderer, theme, size)
}

/// What the harness can see of the shell's own disclosure control, so a gate
/// can calibrate the prerequisite the gates reading this fixture depend on.
/// Nothing here asserts: the caller owns the verdict.
#[derive(Debug, Clone)]
pub struct ToggleCalibration {
    /// Logical bounds of the toggle's pinned magenta ink.
    pub ink: Option<Rectangle>,
    /// Logical bounds of the sidebar's pinned cyan ink — a second pinned
    /// colour in the same frame, so a probe that matched anything painted
    /// would be caught.
    pub sidebar_ink: Option<Rectangle>,
    /// Bounds for a colour nothing in this fixture paints: the probe's
    /// negative control.
    pub unpainted_ink: Option<Rectangle>,
    /// The point pressed, taken from the painted ink.
    pub pressed_at: Option<iced::Point>,
    /// Did that press publish the shell's own `Toggle` message?
    pub published_toggle: bool,
}

/// A colour no fixture token pins and no theme surface resolves.
const UNPAINTED_INK: Color = Color::from_rgb(0.0, 1.0, 0.0);

/// Paint one settled shell, probe it, and press whatever ink was found —
/// exactly the prerequisite [`drive_inner`] runs before it measures anything.
pub fn calibrate_toggle(size: Size) -> ToggleCalibration {
    let mut renderer = new_renderer();
    let theme = theme(false);
    let mut ui = UserInterface::build(
        shell(NavSplitState::new(), 6, Work::default(), Chatter::Quiet),
        size,
        Cache::default(),
        &mut renderer,
    );
    paint(&mut ui, &mut renderer, &theme);
    let ink = toggle_bounds(&mut renderer, &theme, size);
    let sidebar_ink = painted_bounds(&mut renderer, &theme, size, SIDEBAR_INK, FILL_SAMPLE_SCALE);
    let unpainted_ink =
        painted_bounds(&mut renderer, &theme, size, UNPAINTED_INK, INK_SAMPLE_SCALE);
    let pressed_at = ink.map(|bounds| iced::Point::new(bounds.center_x(), bounds.center_y()));
    let mut published_toggle = false;
    if let Some(point) = pressed_at {
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[
                Event::Mouse(mouse::Event::CursorMoved { position: point }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            mouse::Cursor::Available(point),
            &mut renderer,
            &mut messages,
        );
        published_toggle = messages
            .iter()
            .any(|message| matches!(message, Msg::Toggle));
    }
    ToggleCalibration {
        ink,
        sidebar_ink,
        unpainted_ink,
        pressed_at,
        published_toggle,
    }
}

/// Logical bounds of a caller-pinned flat fill in the last painted frame.
///
/// The sentinel colour is the caller's, so a gate can mark any slot it wants
/// to follow (a titlebar marker, a page band) without the fixture knowing what
/// the mark means. Flat container fills saturate whole pixels, so the cheap
/// 1x sample is enough — see [`FILL_SAMPLE_SCALE`].
pub fn fill_bounds(
    renderer: &mut Renderer,
    theme: &Theme,
    size: Size,
    color: Color,
) -> Option<Rectangle> {
    painted_bounds(renderer, theme, size, color, FILL_SAMPLE_SCALE)
}

/// The width of cyan sidebar ink at the live responsive reservation.
///
/// This deliberately does not run a widget operation: the runtime correctly grants
/// every operation one unpruned geometry probe, so an operation used as a
/// per-frame measurement would itself turn this routing benchmark into a
/// 900-descendant walk.
/// Logical bounds of the pixels the last painted frame filled with `color`.
pub fn ink_bounds(
    renderer: &mut Renderer,
    theme: &Theme,
    size: Size,
    color: Color,
) -> Option<Rectangle> {
    painted_bounds(renderer, theme, size, color, FILL_SAMPLE_SCALE)
}

pub fn sidebar_extent(renderer: &mut Renderer, theme: &Theme, size: Size) -> f32 {
    painted_bounds(renderer, theme, size, SIDEBAR_INK, FILL_SAMPLE_SCALE)
        .map_or(0.0, |bounds| bounds.width)
}
