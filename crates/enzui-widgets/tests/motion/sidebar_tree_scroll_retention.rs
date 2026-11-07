//! Sidebar `tree_view` scroll retention across the collapse-and-reopen unmount.

use enzui_theme::EnzTheme;
use enzui_widgets::app_shell::{ShellLayout, capture_hold, enz_app_shell};
use enzui_widgets::nav_split::{NavSplitState, SidebarPreference};
use enzui_widgets::tree_view::{EnzTreeItem, enz_tree_view};
use iced::advanced::widget::operation::scrollable::Scrollable;
use iced::advanced::widget::{Id, Operation};
use iced::advanced::{mouse, renderer};
use iced::widget::scrollable::Viewport;
use iced::{Event, Rectangle, Size, Theme, Vector};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};
use std::cell::Cell;
use std::rc::Rc;

const DESKTOP: Size = Size {
    width: 1280.0,
    height: 800.0,
};
const FRAME_MS: f32 = 16.7;
/// Enough frames to walk any docked settle from either endpoint at 16.7 ms/pass.
const SETTLE_FRAMES: usize = 60;
/// One `enz_tree_view` row is 34 px; 50 rows → 1700 px of content, and the
/// sidebar viewport is well under that height, so the tree can scroll.
const TREE_ROWS: usize = 50;

#[derive(Debug, Clone)]
enum Msg {
    Toggle,
    TreeScrolled(f32),
}

/// A stable id for the tree's internal scrollable. Both the sidebar closure
/// and the probe below key on this exact value: the probe would otherwise
/// have no way to distinguish the tree's scrollable from any other scrollable
/// the app_shell may compose.
fn tree_scroll_id() -> Id {
    Id::new("sidebar-tree-scroll-retention-gate")
}

#[derive(Debug, Clone, Copy)]
struct Seams {
    stable_id: bool,
    seed_offset: bool,
}

impl Seams {
    const BOTH: Self = Self {
        stable_id: true,
        seed_offset: true,
    };
}

fn tree_items() -> Vec<EnzTreeItem> {
    (0..TREE_ROWS)
        .map(|i| EnzTreeItem::leaf(format!("row-{i}"), 0).key(format!("row-{i}")))
        .collect()
}

/// Build one frame of the shell. `offset` is the host-persisted scroll offset
/// the sidebar closure is currently seeding the tree with; `seams` picks which
/// retention seams the tree adopts. The closure captures `Rc<Cell<f32>>` so
/// `on_scroll` writes into the same value each subsequent build seeds from.
fn shell(state: NavSplitState, offset: Rc<Cell<f32>>, seams: Seams) -> iced::Element<'static, Msg> {
    let offset_for_view = offset;
    enz_app_shell(|| iced::widget::Space::new().into())
        .layout(ShellLayout::TitlebarSidebarContent)
        .state(state)
        .sidebar(move || {
            let mut tree = enz_tree_view(tree_items())
                .on_select(|_| Msg::Toggle) // unused; keeps the tree keyboard-live
                .on_scroll(|vp: Viewport| {
                    let y = vp.absolute_offset().y;
                    Msg::TreeScrolled(y)
                });
            if seams.stable_id {
                tree = tree.scroll_id(tree_scroll_id());
            }
            if seams.seed_offset {
                tree = tree.scroll_offset(offset_for_view.get());
            }
            tree.into()
        })
        .titlebar(|| iced::widget::Space::new().into())
        .on_toggle_sidebar(Msg::Toggle)
        .into()
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn theme() -> Theme {
    let mut enz = EnzTheme::dark();
    enz.motion.reduced_motion = false;
    enz.iced_theme()
}

fn draw(ui: &mut UserInterface<'_, Msg, Theme, Renderer>, r: &mut Renderer, theme: &Theme) {
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(theme);
    ui.draw(
        r,
        theme,
        &renderer::Style {
            text_color: base.text_color,
        },
        mouse::Cursor::Unavailable,
    );
}

fn probe_tree_translation(
    ui: &mut UserInterface<'_, Msg, Theme, Renderer>,
    r: &Renderer,
    target: &Id,
) -> Option<Vector> {
    struct Probe {
        target: Id,
        found: Option<Vector>,
    }
    impl<T> Operation<T> for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
            operate(self);
        }
        fn scrollable(
            &mut self,
            id: Option<&Id>,
            _bounds: Rectangle,
            _content_bounds: Size,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&self.target) {
                self.found = Some(translation);
            }
        }
    }
    let mut probe = Probe {
        target: target.clone(),
        found: None,
    };
    ui.operate(r, &mut probe as &mut dyn Operation);
    probe.found
}

fn any_scrollable_present(ui: &mut UserInterface<'_, Msg, Theme, Renderer>, r: &Renderer) -> bool {
    struct Any(bool);
    impl<T> Operation<T> for Any {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
            operate(self);
        }
        fn scrollable(
            &mut self,
            _id: Option<&Id>,
            _bounds: Rectangle,
            _content_bounds: Size,
            _translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            self.0 = true;
        }
    }
    let mut any = Any(false);
    ui.operate(r, &mut any as &mut dyn Operation);
    any.0
}

fn settle(
    ui: &mut UserInterface<'_, Msg, Theme, Renderer>,
    r: &mut Renderer,
    theme: &Theme,
    start: iced::time::Instant,
    offset_cell: &Rc<Cell<f32>>,
) -> usize {
    let mut drawn = 0usize;
    for step in 0..SETTLE_FRAMES {
        let at_ms = (step + 1) as f32 * FRAME_MS;
        let at = start + iced::time::Duration::from_micros((at_ms * 1000.0) as u64);
        let mut messages = iced::advanced::shell::Bus::new();
        let state = ui
            .update(
                &iced::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[Event::Window(iced::window::Event::RedrawRequested(at))],
                mouse::Cursor::Unavailable,
                r,
                &mut messages,
            )
            .0;
        // A moving frame may publish TreeScrolled; treat it the same way an
        // app would, so the seed stays authoritative even during the animation.
        for msg in messages {
            if let Msg::TreeScrolled(y) = msg {
                offset_cell.set(y);
            }
        }
        draw(ui, r, theme);
        drawn += 1;
        let wants_more = matches!(
            state,
            State::Updated {
                redraw_request: iced::window::RedrawRequest::NextFrame
                    | iced::window::RedrawRequest::At(_),
                ..
            }
        );
        if !wants_more {
            break;
        }
    }
    drawn
}

/// Rebuild the interface as the runtime does after a message-driven view()
/// call: retain the cache, replace the tree with a fresh element.
fn rebuild<'a>(
    ui: UserInterface<'a, Msg, Theme, Renderer>,
    size: Size,
    element: iced::Element<'a, Msg>,
    r: &mut Renderer,
) -> UserInterface<'a, Msg, Theme, Renderer> {
    let cache = ui.into_cache();
    UserInterface::build(element, size, cache, r)
}

/// The wheel-scroll input: the tree's viewport sits inside the sidebar band,
/// so hovering roughly a third across the desktop lands the cursor inside it
/// on any reasonable app_shell layout. Negative dy scrolls towards content end.
fn wheel_at(cursor_x: f32, cursor_y: f32, dy: f32) -> (mouse::Cursor, Event) {
    (
        mouse::Cursor::Available(iced::Point::new(cursor_x, cursor_y)),
        Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: dy },
        }),
    )
}

/// Drive the full expand → scroll → collapse → reopen → probe cycle. Returns
/// `(pre_collapse_offset, post_reopen_offset)` and the number of frames the
/// collapse and the reopen each needed to fully settle, plus whether the
/// unmounted-endpoint witness fired at collapse settle.
#[allow(dead_code, reason = "fixture fields are written but never read")]
struct Cycle {
    pre_offset: f32,
    /// The scroll offset the reopened tree sat at, probed through the shared
    /// `scroll_id`, or `None` if that id was not mounted this pass.
    post_offset: Option<f32>,
    /// Whether the collapse endpoint published `reserved_width == 0.0`, the
    /// witness that the docked sidebar actually unmounted rather than settling
    /// at a nonzero width the harness could have missed.
    unmounted_at_settle: bool,
    scrollable_after_reopen: bool,
}

fn run_cycle(seams: Seams) -> Cycle {
    capture_hold::reset();

    let mut r = renderer();
    let theme = theme();
    let size = DESKTOP;

    // Persisted host offset — the value `on_scroll` writes and the next build
    // seeds `scroll_offset` from.
    let offset = Rc::new(Cell::new(0.0_f32));

    // 1. Settle at "sidebar visible" so the tree is fully mounted.
    let mut ui = UserInterface::build(
        shell(NavSplitState::new(), Rc::clone(&offset), seams),
        size,
        Cache::default(),
        &mut r,
    );
    draw(&mut ui, &mut r, &theme);
    // The build stamped an initial edge; clear it before we start pumping.
    let _ = enzui_widgets::app_shell::build_probe::take_motion_edge();

    let (cursor, wheel) = wheel_at(120.0, size.height / 2.0, -400.0);
    let mut messages = iced::advanced::shell::Bus::new();
    let _ = ui
        .update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[wheel],
            cursor,
            &mut r,
            &mut messages,
        )
        .0;
    for msg in messages {
        if let Msg::TreeScrolled(y) = msg {
            offset.set(y);
        }
    }
    draw(&mut ui, &mut r, &theme);

    let probed_pre = probe_tree_translation(&mut ui, &r, &tree_scroll_id()).map_or(0.0, |v| v.y);
    let pre_offset = if seams.stable_id {
        probed_pre
    } else {
        offset.get()
    };

    // 3. Toggle → collapse. Rebuild with the hidden state, pump frames until
    //    the shell stops asking for more.
    let hidden = NavSplitState {
        preference: SidebarPreference::Hidden,
        overlay_open: false,
    };
    let mut ui = rebuild(ui, size, shell(hidden, Rc::clone(&offset), seams), &mut r);
    let start = enzui_widgets::app_shell::build_probe::take_motion_edge()
        .expect("a toggle away from the visible sidebar must stamp a transition edge");
    draw(&mut ui, &mut r, &theme);
    let _collapse_frames = settle(&mut ui, &mut r, &theme, start, &offset);

    // Positive control: at full collapse settle the docked sidebar has
    // published its unmounted-endpoint observation.
    let unmounted_at_settle =
        capture_hold::observation().is_some_and(|o| o.reserved_width == 0.0 && o.progress < 0.01);

    // 4. Toggle → reopen. Rebuild with the visible state, pump frames until
    //    settle. This is where the fresh internal scrollable is constructed.
    let mut ui = rebuild(
        ui,
        size,
        shell(NavSplitState::new(), Rc::clone(&offset), seams),
        &mut r,
    );
    let start = enzui_widgets::app_shell::build_probe::take_motion_edge()
        .expect("a toggle back to the visible sidebar must stamp a transition edge");
    draw(&mut ui, &mut r, &theme);
    let _reopen_frames = settle(&mut ui, &mut r, &theme, start, &offset);

    let scrollable_after_reopen = any_scrollable_present(&mut ui, &r);
    let post_offset = probe_tree_translation(&mut ui, &r, &tree_scroll_id()).map(|v| v.y);

    Cycle {
        pre_offset,
        post_offset,
        unmounted_at_settle,
        scrollable_after_reopen,
    }
}

/// With both seams adopted (`scroll_id` + `scroll_offset`), the reopened
/// tree's internal scrollable lands at the pre-collapse offset.
#[test]
fn adopting_scroll_id_and_scroll_offset_survives_the_collapse_reopen_cycle() {
    let cycle = run_cycle(Seams::BOTH);
    let post = cycle
        .post_offset
        .expect("the reopened tree must publish its scroll_id");
    assert!(
        (post - cycle.pre_offset).abs() <= 1.0,
        "reopened tree offset must equal the pre-collapse offset, \
         got pre={} post={}",
        cycle.pre_offset,
        post,
    );
}
