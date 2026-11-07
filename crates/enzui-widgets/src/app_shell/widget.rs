//! Retained shell state, motion, focus checkpoints, and event routing.

use super::*;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{self, operation, tree};
use iced::widget::text_input;

#[derive(Debug, Clone)]
struct RetainedTextInput {
    id: widget::Id,
    cursor: RetainedCursor,
}

#[derive(Debug, Clone, Copy)]
enum RetainedCursor {
    Index(usize),
    Selection { start: usize, end: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarMotionFrame {
    pub(crate) docked_driver: iced::advanced::renderer::NodeId,
    pub(crate) toggle_driver: iced::advanced::renderer::NodeId,
    pub(crate) inline_progress: f32,
    // Docked panel, content, divider, and chrome read the same progress during local layout.
    pub(crate) inline_live_progress: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) inline_mounted: bool,
    pub(crate) overlay_progress: f32,
    pub(crate) overlay_live_progress: std::rc::Rc<std::cell::Cell<f32>>,
    // Remaining travel is sampled by the render lane even while the UI thread is blocked.
    pub(crate) overlay_travel: std::rc::Rc<std::cell::RefCell<Option<crate::motion::TrackCurve>>>,
    pub(crate) overlay_mounted: bool,
    // The docked travel's remaining curve, which the render lane samples to
    // keep the seam moving while the UI thread is late with a layout.
    pub(crate) inline_travel: std::rc::Rc<std::cell::RefCell<Option<crate::motion::TrackCurve>>>,
    // The docked panel's recording node while it travels; see
    // `RevealClock::retained_panel`.
    pub(crate) inline_retained_panel:
        std::rc::Rc<std::cell::Cell<Option<iced::advanced::renderer::NodeId>>>,
    pub(crate) toggle_mounted: bool,
    // Arrival needs a clearance node on the zero-progress frame before the control mounts.
    pub(crate) toggle_arriving: bool,
    // A dismissed toggle still travels out with its seam until its motion settles.
    pub(crate) toggle_departing: bool,
    pub(crate) toggle_clearance: std::rc::Rc<std::cell::Cell<f32>>,
}

impl SidebarMotionFrame {
    /// Whether the toggle is travelling in or out with its seam, drawn but inert.
    pub(crate) fn toggle_riding(&self) -> bool {
        self.toggle_arriving || self.toggle_departing
    }

    pub(crate) fn inline_clock(&self) -> crate::motion::RevealClock {
        crate::motion::RevealClock {
            spatial: None,
            layout_driver: Some(self.docked_driver),
            progress: std::rc::Rc::clone(&self.inline_live_progress),
            travel: None,
            pin: None,
            responsive_reserve: true,
            retained_panel: Some(std::rc::Rc::clone(&self.inline_retained_panel)),
        }
    }

    /// [`inline_clock`](Self::inline_clock) with the docked travel's curve, for
    /// a rider that publishes its travel to the render lane.
    pub(crate) fn inline_lane_clock(&self) -> crate::motion::RevealClock {
        crate::motion::RevealClock {
            travel: (crate::app_shell::capture_hold::state()
                != crate::app_shell::capture_hold::State::Held)
                .then(|| std::rc::Rc::clone(&self.inline_travel)),
            ..self.inline_clock()
        }
    }

    pub(crate) fn seam_layout_fraction(&self) -> impl Fn() -> f32 + 'static {
        let progress = std::rc::Rc::clone(&self.inline_live_progress);
        move || progress.get().clamp(0.0, 1.0)
    }

    pub(crate) fn overlay_clock(&self) -> crate::motion::RevealClock {
        crate::motion::RevealClock {
            spatial: None,
            layout_driver: None,
            progress: std::rc::Rc::clone(&self.overlay_live_progress),
            travel: (crate::app_shell::capture_hold::state()
                != crate::app_shell::capture_hold::State::Held)
                .then(|| std::rc::Rc::clone(&self.overlay_travel)),
            pin: None,
            responsive_reserve: false,
            retained_panel: None,
        }
    }
}

#[derive(Debug)]
struct AppShellState {
    docked_driver: iced::advanced::renderer::NodeId,
    toggle_driver: iced::advanced::renderer::NodeId,
    retained_text_input: Option<RetainedTextInput>,
    presented_from: std::cell::Cell<[Option<f32>; 2]>,
    // Cache element construction within this widget instance, independently of live layout progress.
    last_built_width: Option<f32>,
    last_built_inline_mounted: Option<bool>,
    last_built_overlay_mounted: Option<bool>,
    last_nav_state: Option<NavSplitState>,
    last_sidebar_available: Option<bool>,
    last_built_toggle: Option<(bool, bool)>,
    inline_motion: Option<crate::motion::PanelMotion>,
    overlay_motion: Option<crate::motion::PanelMotion>,
    toggle_motion: Option<crate::motion::PanelMotion>,
    inline_live_progress: std::rc::Rc<std::cell::Cell<f32>>,
    toggle_live_progress: std::rc::Rc<std::cell::Cell<f32>>,
    overlay_live_progress: std::rc::Rc<std::cell::Cell<f32>>,
    // Tree ownership preserves the render lane’s motion generation across view rebuilds.
    overlay_travel: std::rc::Rc<std::cell::RefCell<Option<crate::motion::TrackCurve>>>,
    inline_travel: std::rc::Rc<std::cell::RefCell<Option<crate::motion::TrackCurve>>>,
    inline_retained_panel: std::rc::Rc<std::cell::Cell<Option<iced::advanced::renderer::NodeId>>>,
    clock: std::cell::Cell<iced::time::Instant>,
    pending_frame_anchor: Option<iced::time::Instant>,
    // Capture holds need this edge after the take-once probe has consumed its own copy.
    motion_edge: std::cell::Cell<Option<iced::time::Instant>>,
    // Ordinary layout clears this key; duplicate island passes may reuse unchanged geometry.
    last_island: std::cell::Cell<Option<([f32; 3], Size)>>,
}

impl Default for AppShellState {
    fn default() -> Self {
        Self {
            docked_driver: crate::motion::next_scene_node(),
            toggle_driver: crate::motion::next_scene_node(),
            retained_text_input: None,
            presented_from: std::cell::Cell::new([None, None]),
            last_built_width: None,
            last_built_inline_mounted: None,
            last_built_overlay_mounted: None,
            last_nav_state: None,
            last_sidebar_available: None,
            last_built_toggle: None,
            inline_motion: None,
            overlay_motion: None,
            toggle_motion: None,
            inline_live_progress: std::rc::Rc::new(std::cell::Cell::new(0.0)),
            toggle_live_progress: std::rc::Rc::new(std::cell::Cell::new(0.0)),
            overlay_live_progress: std::rc::Rc::new(std::cell::Cell::new(0.0)),
            overlay_travel: std::rc::Rc::new(std::cell::RefCell::new(None)),
            inline_travel: std::rc::Rc::new(std::cell::RefCell::new(None)),
            inline_retained_panel: std::rc::Rc::new(std::cell::Cell::new(None)),
            clock: std::cell::Cell::new(iced::time::Instant::now()),
            pending_frame_anchor: None,
            motion_edge: std::cell::Cell::new(None),
            last_island: std::cell::Cell::new(None),
        }
    }
}

/// The most a docked travel advances between two frames.
const MAX_TRAVEL_STEP: iced::time::Duration = iced::time::Duration::from_micros(33_333);

impl iced::advanced::widget::operation::DockedClock for AppShellState {
    fn owns_animations(&self, animations: &[iced::advanced::renderer::TrackAnimation]) -> bool {
        !animations.is_empty()
            && animations.len() <= 8
            && animations.iter().all(|animation| {
                [
                    (self.docked_driver, self.inline_motion.as_ref()),
                    (self.toggle_driver, self.toggle_motion.as_ref()),
                ]
                .into_iter()
                .any(|(node, motion)| {
                    node == animation.node
                        && motion.is_some_and(|motion| motion.generation() == animation.generation)
                })
            })
    }

    fn can_shift(&self, delay: iced::time::Duration) -> bool {
        crate::app_shell::capture_hold::clock_is_unshifted()
            && self.clock.get().checked_add(delay).is_some()
            && self
                .motion_edge
                .get()
                .is_none_or(|at| at.checked_add(delay).is_some())
            && [
                &self.inline_motion,
                &self.overlay_motion,
                &self.toggle_motion,
            ]
            .into_iter()
            .flatten()
            .all(|motion| motion.can_shift_clock(delay))
    }

    fn shift(&mut self, delay: iced::time::Duration) -> bool {
        if !self.can_shift(delay) {
            return false;
        }
        let Some(clock) = self.clock.get().checked_add(delay) else {
            return false;
        };
        for motion in [
            &mut self.inline_motion,
            &mut self.overlay_motion,
            &mut self.toggle_motion,
        ]
        .into_iter()
        .flatten()
        {
            if !motion.shift_clock(delay) {
                return false;
            }
        }
        self.clock.set(clock);
        self.motion_edge
            .set(self.motion_edge.get().and_then(|at| at.checked_add(delay)));
        true
    }
}

impl AppShellState {
    fn accept_presented(&self, sample: &iced::advanced::renderer::presentation::Sample) {
        let values = [
            sample.driver(self.docked_driver.0, 0),
            sample.driver(self.toggle_driver.0, 0),
        ]
        .map(|value| value.filter(|value| value.is_finite() && (0.0..=1.0).contains(value)));
        if let Some(value) = values[0] {
            self.inline_live_progress.set(value);
        }
        if let Some(value) = values[1] {
            self.toggle_live_progress.set(value);
        }
        self.presented_from.set(values);
    }

    fn motion_frame(
        &mut self,
        geometry: ShellGeometry,
        layout: ShellLayout,
        nav_state: NavSplitState,
        sidebar_available: bool,
        toggle_target: bool,
    ) -> SidebarMotionFrame {
        let presented = self.presented_from.take();
        // FloatingSidebar overlays its base even at wide widths and reserves no content geometry.
        let inline_target = geometry.sidebar == SidebarPresentation::Inline
            && layout != ShellLayout::FloatingSidebar;
        let overlay_open = nav_state.effective_overlay_open(geometry.sidebar);
        let overlay_target = if layout == ShellLayout::FloatingSidebar {
            geometry.sidebar == SidebarPresentation::Inline || overlay_open
        } else {
            geometry.sidebar == SidebarPresentation::Overlay && overlay_open
        };

        // Geometry-only target changes settle; navigation and route availability changes animate.
        let nav_changed = self
            .last_nav_state
            .replace(nav_state)
            .is_some_and(|previous| previous != nav_state);
        let availability_changed = self
            .last_sidebar_available
            .replace(sidebar_available)
            .is_some_and(|previous| previous != sidebar_available);
        let nav_changed = nav_changed || availability_changed;

        let edge = self
            .inline_motion
            .as_ref()
            .is_some_and(|motion| motion.target() != inline_target)
            || self
                .overlay_motion
                .as_ref()
                .is_some_and(|motion| motion.target() != overlay_target)
            || self
                .toggle_motion
                .as_ref()
                .is_some_and(|motion| motion.target() != toggle_target);
        if edge {
            let unpresented = nav_changed
                && presented.iter().all(Option::is_none)
                && (!self.motion_needs_frame() || self.pending_frame_anchor.is_some())
                && crate::app_shell::capture_hold::clock_is_unshifted();
            // Retargeting must not precede the last presented frame or use a different capture clock.
            let stamped = self.pending_frame_anchor.unwrap_or_else(|| {
                crate::app_shell::capture_hold::effective(iced::time::Instant::now())
                    .max(self.clock.get())
            });
            if unpresented {
                self.pending_frame_anchor = Some(stamped);
            }
            self.clock.set(stamped);
            self.motion_edge.set(Some(stamped));
            crate::app_shell::build_probe::note_motion_edge(self.clock.get());
        }
        let now = self.clock.get();

        let inline = self
            .inline_motion
            .get_or_insert_with(|| crate::motion::PanelMotion::at_target(inline_target));
        if inline.target() != inline_target && !nav_changed {
            inline.settle_at(inline_target);
        } else if let Some(from) = presented[0] {
            let _ = inline.set_target_from_presented(inline_target, from, now);
        } else {
            inline.set_target(inline_target, now);
        }
        let overlay = self
            .overlay_motion
            .get_or_insert_with(|| crate::motion::PanelMotion::at_target(overlay_target));
        if overlay.target() != overlay_target && !nav_changed {
            overlay.settle_at(overlay_target);
        } else {
            overlay.set_target(overlay_target, now);
        }
        let toggle = self
            .toggle_motion
            .get_or_insert_with(|| crate::motion::PanelMotion::at_target(toggle_target));
        if toggle.target() != toggle_target && !nav_changed {
            toggle.settle_at(toggle_target);
        } else if let Some(from) = presented[1] {
            let _ = toggle.set_target_from_presented(toggle_target, from, now);
        } else {
            toggle.set_target(toggle_target, now);
        }
        let toggle_progress = toggle.progress(now);
        let toggle_animating = toggle.is_animating(now);
        self.toggle_live_progress.set(toggle_progress);
        // Withhold controls during clearance travel so no live hit box outgrows its reservation.
        let toggle_mounted = toggle_target && !toggle_animating;
        if crate::app_shell::capture_hold::state() == crate::app_shell::capture_hold::State::Armed
            && let Some(edge) = self.motion_edge.get()
        {
            let travel = if overlay.is_animating(now) {
                overlay.applied_travel()
            } else {
                inline.applied_travel()
            };
            crate::app_shell::capture_hold::resolve_pin(edge, travel, now);
        }
        let inline_progress = inline.progress(now);
        self.inline_live_progress.set(inline_progress);
        self.inline_travel
            .replace(inline.track_curve_at(crate::motion::TRACK_SAMPLES, now));
        let inline_mounted =
            inline_target || inline.is_animating(now) || inline_progress > f32::EPSILON;
        let overlay_progress = overlay.progress(now);
        self.overlay_live_progress.set(overlay_progress);
        // Publish the curve and sample from the same motion generation.
        self.overlay_travel
            .replace(overlay.track_curve_at(crate::motion::TRACK_SAMPLES, now));
        let overlay_mounted =
            overlay_target || overlay.is_animating(now) || overlay_progress > f32::EPSILON;

        SidebarMotionFrame {
            docked_driver: self.docked_driver,
            toggle_driver: self.toggle_driver,
            toggle_mounted,
            toggle_arriving: toggle_target && !toggle_mounted,
            toggle_departing: !toggle_target && toggle_animating,
            toggle_clearance: std::rc::Rc::clone(&self.toggle_live_progress),
            inline_progress,
            inline_live_progress: std::rc::Rc::clone(&self.inline_live_progress),
            inline_mounted,
            overlay_progress,
            overlay_live_progress: std::rc::Rc::clone(&self.overlay_live_progress),
            overlay_travel: std::rc::Rc::clone(&self.overlay_travel),
            inline_travel: std::rc::Rc::clone(&self.inline_travel),
            inline_retained_panel: std::rc::Rc::clone(&self.inline_retained_panel),
            overlay_mounted,
        }
    }

    fn note_motion_layout(&mut self) {
        let now = self.clock.get();
        if let Some(motion) = self.inline_motion.as_mut() {
            motion.note_layout(now);
        }
        if let Some(motion) = self.overlay_motion.as_mut() {
            motion.note_layout(now);
        }
        if let Some(motion) = self.toggle_motion.as_mut() {
            motion.note_layout(now);
        }
    }

    fn motion_needs_frame(&self) -> bool {
        let now = self.clock.get();
        self.inline_motion
            .as_ref()
            .is_some_and(|motion| motion.is_animating(now))
            || self
                .overlay_motion
                .as_ref()
                .is_some_and(|motion| motion.is_animating(now))
            || self
                .toggle_motion
                .as_ref()
                .is_some_and(|motion| motion.is_animating(now))
    }

    /// Whether the overlay drawer is travelling open. An opening drawer
    /// exposes ink its recording may not hold, so it publishes no track and
    /// each of its frames is drawn.
    fn overlay_opening(&self) -> bool {
        let now = self.clock.get();
        self.overlay_motion
            .as_ref()
            .is_some_and(|motion| motion.target() && motion.is_animating(now))
    }

    fn motion_needs_local_layout(&self) -> bool {
        let now = self.clock.get();
        self.inline_motion
            .as_ref()
            .is_some_and(|motion| motion.is_animating(now))
            || self
                .toggle_motion
                .as_ref()
                .is_some_and(|motion| motion.is_animating(now))
    }

    fn motion_needs_structural_layout(&self) -> bool {
        let now = self.clock.get();
        self.inline_motion
            .as_ref()
            .is_some_and(|motion| !motion.is_animating(now) && motion.needs_layout(now))
            || self
                .overlay_motion
                .as_ref()
                .is_some_and(|motion| !motion.is_animating(now) && motion.needs_layout(now))
            || self
                .toggle_motion
                .as_ref()
                .is_some_and(|motion| !motion.is_animating(now) && motion.needs_layout(now))
    }

    fn refresh_live_progress(&self) {
        let now = self.clock.get();
        if let Some(motion) = self.inline_motion.as_ref() {
            self.inline_live_progress.set(motion.progress(now));
            self.inline_travel
                .replace(motion.track_curve_at(crate::motion::TRACK_SAMPLES, now));
        }
        if let Some(motion) = self.overlay_motion.as_ref() {
            self.overlay_live_progress.set(motion.progress(now));
            self.overlay_travel
                .replace(motion.track_curve_at(crate::motion::TRACK_SAMPLES, now));
        }
        if let Some(motion) = self.toggle_motion.as_ref() {
            self.toggle_live_progress.set(motion.progress(now));
        }
    }

    fn docked_drivers(
        &self,
    ) -> (
        Vec<iced::advanced::renderer::TrackAnimation>,
        Vec<iced::advanced::renderer::Track>,
    ) {
        use iced::advanced::renderer::{Track, TrackAnimation};
        let now = self.clock.get();
        let animation = |node, motion: &crate::motion::PanelMotion| {
            motion
                .track_curve_at(crate::motion::TRACK_SAMPLES, now)
                .map(|curve| TrackAnimation {
                    node,
                    start: curve.start,
                    duration: curve.duration,
                    generation: curve.generation,
                    keyframes: curve
                        .points
                        .into_iter()
                        .map(|(at, progress)| {
                            (
                                at,
                                Track {
                                    translation: Vector::new(progress, 0.0),
                                    ..Track::identity(node)
                                },
                            )
                        })
                        .collect(),
                })
        };
        let mut curves = Vec::new();
        let mut stationary = Vec::new();
        for (node, motion) in [
            (self.docked_driver, self.inline_motion.as_ref()),
            (self.toggle_driver, self.toggle_motion.as_ref()),
        ] {
            let Some(motion) = motion else {
                continue;
            };
            if let Some(curve) = animation(node, motion) {
                curves.push(curve);
            } else {
                stationary.push(Track {
                    translation: Vector::new(motion.progress(now), 0.0),
                    ..Track::identity(node)
                });
            }
        }
        (curves, stationary)
    }

    fn docked_capture(
        &self,
        bounds: Rectangle,
    ) -> Option<iced::advanced::renderer::docked::Capture> {
        let (mut curves, stationary) = self.docked_drivers();
        if curves.is_empty() {
            return None;
        }
        let primary = curves.remove(0);
        Some(iced::advanced::renderer::docked::Capture {
            layout: Err(iced::advanced::renderer::presentation::Error::DockedOwnershipRequired),
            bounds,
            animation: primary,
            additional: curves,
            stationary,
        })
    }

    fn observe_motion_theme(&self, theme: &iced::Theme) {
        if let Some(motion) = self.inline_motion.as_ref() {
            motion.observe_theme(theme);
        }
        if let Some(motion) = self.overlay_motion.as_ref() {
            motion.observe_theme(theme);
        }
        if let Some(motion) = self.toggle_motion.as_ref() {
            motion.observe_theme(theme);
        }
    }
}

#[derive(Default)]
// Preserve text focus without overriding a deliberate focus move to another control.
struct FocusProbe {
    any_focused: bool,
    focused_input: Option<(widget::Id, String)>,
    pending_input: Option<(Option<widget::Id>, String)>,
}

impl<T> Operation<T> for FocusProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn text_input(
        &mut self,
        id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn operation::text_input::TextInput,
    ) {
        self.pending_input = Some((id.cloned(), state.text().into_owned()));
    }

    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        if state.is_focused() {
            self.any_focused = true;

            if let Some((input_id, text)) = self.pending_input.take()
                && input_id.as_ref() == id
            {
                let Some(id) = input_id else {
                    debug_assert!(
                        false,
                        "EnzAppShell cannot retain a focused text input across a subtree rebuild \
                         without a stable Id; assign `.id(Id::new(\"…\"))` (or use an enzUI \
                         field's stable label-derived id) so focus and caret loss is loud rather \
                         than silent"
                    );
                    return;
                };
                self.focused_input = Some((id, text));
            }
        } else {
            self.pending_input = None;
        }
    }
}

struct TextInputPresence<'a> {
    target: &'a widget::Id,
    found: bool,
}

impl<T> Operation<T> for TextInputPresence<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn text_input(
        &mut self,
        id: Option<&widget::Id>,
        _bounds: Rectangle,
        _state: &mut dyn operation::text_input::TextInput,
    ) {
        self.found |= id == Some(self.target);
    }
}

fn focused_cursor_in(tree: &Tree) -> Option<text_input::Cursor> {
    type InputState =
        text_input::State<<iced::Renderer as iced::advanced::text::Renderer>::Paragraph>;

    if tree.tag == tree::Tag::of::<InputState>() {
        let state = tree.state.downcast_ref::<InputState>();
        if state.is_focused() {
            return Some(state.cursor());
        }
    }

    tree.children.iter().find_map(focused_cursor_in)
}

fn retained_cursor(cursor: text_input::Cursor, text: &str) -> RetainedCursor {
    // Cursor state counts graphemes; restoration operations take byte offsets.
    let byte = |grapheme: usize| {
        unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true)
            .nth(grapheme)
            .map_or(text.len(), |(byte, _)| byte)
    };

    match cursor.state(&text_input::Value::new(text)) {
        text_input::cursor::State::Index(position) => RetainedCursor::Index(byte(position)),
        text_input::cursor::State::Selection { start, end } => RetainedCursor::Selection {
            start: byte(start),
            end: byte(end),
        },
    }
}

/// Takes the first tree tagged with each of `tags` out of `tree`, in one
/// walk: a rebuild runs this over the whole shell, so it visits each node
/// once, and never descends into a tree it took.
fn take_retained_trees<const N: usize>(tree: &mut Tree, tags: [tree::Tag; N]) -> [Option<Tree>; N] {
    fn walk<const N: usize>(tree: &mut Tree, tags: &[tree::Tag; N], found: &mut [Option<Tree>; N]) {
        if let Some(i) = tags.iter().position(|tag| *tag == tree.tag)
            && found[i].is_none()
        {
            found[i] = Some(std::mem::replace(tree, Tree::empty()));
            return;
        }
        // Nested shells own their retained roles; transfer must stop at their boundary.
        if tree.tag == tree::Tag::of::<AppShellState>() {
            return;
        }
        for child in &mut tree.children {
            if found.iter().all(Option::is_some) {
                return;
            }
            walk(child, tags, found);
        }
    }
    let mut found = std::array::from_fn(|_| None);
    walk(tree, &tags, &mut found);
    found
}

fn operate_child<Message>(
    content: &mut Element<'_, Message>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &iced::Renderer,
    operation: &mut dyn Operation,
) {
    content
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, operation);
}

fn probe_child<Message>(
    content: &mut Element<'_, Message>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &iced::Renderer,
) -> FocusProbe {
    crate::app_shell::build_probe::note_focus_probe();
    let mut probe = FocusProbe::default();
    operate_child(content, tree, layout, renderer, &mut probe);
    probe
}

// Only redraw events can skip the focus checkpoint; input and IME events must retain it.
fn event_cannot_mutate_focus_state<Message>(event: &Event) -> bool {
    let _ = std::marker::PhantomData::<Message>;
    matches!(
        event,
        Event::Window(iced::window::Event::RedrawRequested(_))
    )
}

fn checkpoint_child<Message>(
    content: &mut Element<'_, Message>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &iced::Renderer,
) -> Option<RetainedTextInput> {
    let probe = probe_child(content, tree, layout, renderer);
    let (id, text) = probe.focused_input?;
    let cursor = focused_cursor_in(tree)?;
    Some(RetainedTextInput {
        id,
        cursor: retained_cursor(cursor, &text),
    })
}

impl<'a, Message: Clone + 'a> From<EnzAppShell<'a, Message>> for Element<'a, Message> {
    fn from(shell: EnzAppShell<'a, Message>) -> Self {
        shell.into_element()
    }
}

pub(crate) struct AppShellWidget<'a, Message> {
    pub(crate) shell: EnzAppShell<'a, Message>,
    pub(crate) content: Element<'a, Message>,
    pub(crate) built_this_cycle: bool,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Layout work selected for a shell animation frame.
///
/// Disabling isolation routes local motion through root invalidation; settled
/// structural endpoints also require a root pass.
pub enum MotionLayoutRoute {
    /// Recompute the retained shell subtree.
    RetainedIsland,
    /// Request a root diff and layout pass.
    RootInvalidate,
    /// Perform no layout work.
    Idle,
}

#[doc(hidden)]
#[must_use]
/// Choose local motion layout, root invalidation, or idle work.
///
/// Local motion takes precedence over endpoint settlement. Disabled isolation
/// turns local work into root invalidation.
pub fn motion_layout_route(
    isolation_disabled: bool,
    needs_local_layout: bool,
    needs_structural_layout: bool,
) -> MotionLayoutRoute {
    if needs_local_layout {
        if isolation_disabled {
            MotionLayoutRoute::RootInvalidate
        } else {
            MotionLayoutRoute::RetainedIsland
        }
    } else if needs_structural_layout {
        MotionLayoutRoute::RootInvalidate
    } else {
        MotionLayoutRoute::Idle
    }
}

impl<Message> AppShellWidget<'_, Message> {
    fn docked_presentation_enabled() -> bool {
        false
    }

    fn presented_child(
        tree: &Tree,
        renderer: &iced::Renderer,
        outer: Layout,
    ) -> Option<layout::Node> {
        use iced::advanced::Renderer as _;
        let sample = renderer.presented_layout()?;
        Self::child_layout(tree, outer)
            .project_presented(&tree.children[0], sample)
            .ok()
    }

    fn present_child(
        tree: &mut Tree,
        renderer: &iced::Renderer,
        outer: Layout,
    ) -> Option<layout::Node> {
        let projected = Self::presented_child(tree, renderer, outer)?;
        let content = &mut tree.children[0];
        projected.clone().apply(content);
        content.translation = projected.position() - outer.position();
        Some(projected)
    }

    fn child_layout(tree: &Tree, outer: Layout) -> Layout {
        outer.child(&tree.children, 0)
    }

    fn bound_docked_capture(
        tree: &Tree,
        child: Layout,
        bounds: Rectangle,
    ) -> Option<iced::advanced::renderer::docked::Capture> {
        // Docked capture has unbounded admission cost; shipping motion uses retained local layout.
        if !Self::docked_presentation_enabled() {
            return None;
        }

        let capture = tree
            .state
            .downcast_ref::<AppShellState>()
            .docked_capture(bounds)?;
        let program = child.docked_presentation(&tree.children[0], capture.animation.node, bounds);
        Some(capture.bind_layout(program))
    }

    fn note_frame(&self, tree: &mut Tree, event: &Event, shell: &mut Shell<'_, Message>) -> bool {
        if let Event::Window(iced::window::Event::RedrawRequested(at)) = event {
            let state = tree.state.downcast_mut::<AppShellState>();
            let at = crate::app_shell::capture_hold::effective(*at);
            if let Some(anchor) = state.pending_frame_anchor.take()
                && let Some(delay) = at.checked_duration_since(anchor)
            {
                let _ = iced::advanced::widget::operation::DockedClock::shift(state, delay);
            } else if state.motion_needs_local_layout()
                && let Some(gap) = at.checked_duration_since(state.clock.get())
                && let Some(late) = gap.checked_sub(MAX_TRAVEL_STEP)
                && !late.is_zero()
            {
                // A docked travel advances at most a step per frame: the frame
                // after a stall (a page being built) continues the slide from
                // where it held instead of jumping ahead, since the render lane
                // holds a late layout rather than moving it rigidly
                // (enzUI-ICED #3660).
                let _ = iced::advanced::widget::operation::DockedClock::shift(state, late);
            }
            state.presented_from.set([None, None]);
            state.clock.set(at);
            state.refresh_live_progress();
            crate::app_shell::frame_cost::note_frame(at);
        }
        let state = tree.state.downcast_ref::<AppShellState>();
        if state.motion_needs_local_layout() {
            // A docked travel or the riding toggle lays the shell out again
            // each frame, so each frame is drawn: replaying the last scene
            // under the riders' tracks would show the old layout moved
            // rigidly, its trailing edge off the window's (enzUI-ICED #3660).
            shell.request_redraw();
        } else if state.overlay_opening() {
            // Without a track of its own, a track frame would let the scrim's
            // fade hand the opening to the render lane, which replays the
            // drawer where its first frame left it.
            shell.request_redraw();
        } else if state.motion_needs_frame() {
            // A closing overlay drawer only moves: its reveal's tracks carry it.
            shell.request_track_frame();
        }
        // Thread-local overrides keep negative-control tests independent of process environment races.
        let disabled = crate::app_shell::build_probe::isolation_override()
            .unwrap_or_else(crate::app_shell::frame_cost::isolation_disabled);
        match motion_layout_route(
            disabled,
            state.motion_needs_local_layout()
                && !shell.presentation_owns_layout(state.docked_driver),
            state.motion_needs_structural_layout(),
        ) {
            MotionLayoutRoute::RetainedIsland => true,
            MotionLayoutRoute::RootInvalidate => {
                crate::app_shell::build_probe::note_layout_invalidation();
                shell.invalidate_layout_with(iced::advanced::graphics::core::shell::Diff::Perform);
                false
            }
            MotionLayoutRoute::Idle => false,
        }
    }

    fn refresh_responsive_layout(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
    ) {
        let bounds = layout.bounds();
        let limits = layout::Limits::new(Size::ZERO, bounds.size());
        let state = tree.state.downcast_ref::<AppShellState>();
        let progress = state.inline_live_progress.get();
        let inputs = (
            [
                progress,
                state.toggle_live_progress.get(),
                state.overlay_live_progress.get(),
            ],
            bounds.size(),
        );
        if state.last_island.replace(Some(inputs)) == Some(inputs) {
            return;
        }
        let traced = crate::app_shell::frame_cost::begin();
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &limits);
        tree.state
            .downcast_mut::<AppShellState>()
            .note_motion_layout();
        crate::app_shell::build_probe::note_responsive_island_layout();
        crate::app_shell::frame_cost::end_responsive(traced, progress, bounds.width);
        if !tree
            .state
            .downcast_ref::<AppShellState>()
            .inline_motion
            .as_ref()
            .is_some_and(crate::motion::PanelMotion::target)
            && progress <= f32::EPSILON
        {
            crate::app_shell::capture_hold::note_unmounted_docked(progress);
        }
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for AppShellWidget<'a, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AppShellState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AppShellState::default())
    }

    // Layout builds at the assigned width before reconciliation; diff must not inspect the initial Space seed.
    fn diff(&mut self, _tree: &mut Tree) {
        crate::app_shell::build_probe::note_root_diff();
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        crate::app_shell::build_probe::note_top_level_layout();
        tree.state
            .downcast_ref::<AppShellState>()
            .last_island
            .set(None);
        let limits = limits.width(Length::Fill).height(Length::Fill);
        let traced = crate::app_shell::frame_cost::begin();
        let available = limits.max().width;
        let geometry = self.shell.geometry_for(available);
        let state = tree.state.downcast_mut::<AppShellState>();
        let retained = state.retained_text_input.clone();
        let motion = state.motion_frame(
            geometry,
            self.shell.layout,
            self.shell.state,
            self.shell.sidebar_available,
            self.shell.would_render_toggle(geometry),
        );
        let docked_driver = motion.docked_driver;
        let inline_progress = motion.inline_progress;
        let inline_mounted = motion.inline_mounted;
        let overlay_mounted = motion.overlay_mounted;
        let motion_toggle = (motion.toggle_mounted, motion.toggle_riding());

        // Live progress changes geometry without changing element construction or mount topology.
        let can_skip = self.built_this_cycle
            && state.last_built_width == Some(available)
            && state.last_built_inline_mounted == Some(motion.inline_mounted)
            && state.last_built_overlay_mounted == Some(motion.overlay_mounted)
            && state.last_built_toggle == Some(motion_toggle);

        if can_skip {
            crate::app_shell::build_probe::note_layout_skip();
            let node = self.content.as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &limits.loose(),
            );
            tree.state
                .downcast_mut::<AppShellState>()
                .note_motion_layout();
            let size = limits.resolve(Length::Fill, Length::Fill, node.size());
            crate::app_shell::frame_cost::end(traced, false, inline_progress, available);
            if !inline_mounted && !overlay_mounted {
                crate::app_shell::capture_hold::note_unmounted_docked(inline_progress);
            }
            crate::app_shell::build_probe::note_docked_layout(
                &tree.children[0],
                docked_driver,
                inline_mounted,
                inline_progress,
            );
            return layout::Node::with_children(size, vec![node]).apply(tree);
        }

        let [
            retained_content,
            retained_titlebar_content,
            retained_titlebar,
        ] = tree
            .children
            .first_mut()
            .map_or([None, None, None], |tree| {
                take_retained_trees(
                    tree,
                    [
                        tree::Tag::of::<super::seam::Retained>(),
                        tree::Tag::of::<super::seam::RetainedFloatingTitlebarContent>(),
                        tree::Tag::of::<super::seam::RetainedFloatingTitlebar>(),
                    ],
                )
            });
        let custody = super::seam::ContentTree::new(retained_content);
        let titlebar_content_custody = super::seam::ContentTree::new(retained_titlebar_content);
        let titlebar_custody = super::seam::ContentTree::new(retained_titlebar);
        self.content = self.shell.build(
            available,
            motion,
            &custody,
            &titlebar_custody,
            &titlebar_content_custody,
        );
        crate::app_shell::build_probe::note_content_diff();
        tree.diff_children(std::slice::from_mut(&mut self.content));
        debug_assert!(
            custody.is_consumed(),
            "shell rebuild omitted its retained content boundary"
        );
        let node =
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits.loose());
        let child_layout = Layout::of(&node);

        self.built_this_cycle = true;
        {
            let state = tree.state.downcast_mut::<AppShellState>();
            state.last_built_width = Some(available);
            state.last_built_inline_mounted = Some(inline_mounted);
            state.last_built_overlay_mounted = Some(overlay_mounted);
            state.last_built_toggle = Some(motion_toggle);
        }

        // Restore only a surviving input when no replacement control already owns focus.
        if let Some(retained) = retained {
            let probe = probe_child(
                &mut self.content,
                &mut tree.children[0],
                child_layout,
                renderer,
            );
            if !probe.any_focused {
                let mut presence = TextInputPresence {
                    target: &retained.id,
                    found: false,
                };
                operate_child(
                    &mut self.content,
                    &mut tree.children[0],
                    child_layout,
                    renderer,
                    &mut presence,
                );

                if presence.found {
                    let mut focus = operation::focusable::focus::<()>(retained.id.clone());
                    operate_child(
                        &mut self.content,
                        &mut tree.children[0],
                        child_layout,
                        renderer,
                        &mut focus,
                    );

                    match retained.cursor {
                        RetainedCursor::Index(position) => {
                            let mut cursor = operation::text_input::move_cursor_to::<()>(
                                retained.id.clone(),
                                crate::focus_retention::line_position(position),
                            );
                            operate_child(
                                &mut self.content,
                                &mut tree.children[0],
                                child_layout,
                                renderer,
                                &mut cursor,
                            );
                        }
                        RetainedCursor::Selection { start, end } => {
                            let mut cursor = operation::text_input::select_range::<()>(
                                retained.id.clone(),
                                crate::focus_retention::line_position(start),
                                crate::focus_retention::line_position(end),
                            );
                            operate_child(
                                &mut self.content,
                                &mut tree.children[0],
                                child_layout,
                                renderer,
                                &mut cursor,
                            );
                        }
                    }
                }
            }

            tree.state
                .downcast_mut::<AppShellState>()
                .retained_text_input = checkpoint_child(
                &mut self.content,
                &mut tree.children[0],
                child_layout,
                renderer,
            );
        }
        tree.state
            .downcast_mut::<AppShellState>()
            .note_motion_layout();

        crate::diagnostics_overlay::emit_tree_audit_once(
            &mut self.content,
            &mut tree.children[0],
            child_layout,
            renderer,
        );
        let size = limits.resolve(Length::Fill, Length::Fill, node.size());
        crate::app_shell::frame_cost::end(traced, true, inline_progress, available);
        if !inline_mounted && !overlay_mounted {
            crate::app_shell::capture_hold::note_unmounted_docked(inline_progress);
        }
        crate::app_shell::build_probe::note_docked_layout(
            &tree.children[0],
            docked_driver,
            inline_mounted,
            inline_progress,
        );
        layout::Node::with_children(size, vec![node]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if operation.is_docked_clock_rebase() {
            operation.docked_clock(tree.state.downcast_mut::<AppShellState>());
            return;
        }
        use iced::advanced::Renderer as _;
        let projected = Self::present_child(tree, renderer, layout);
        if renderer.presented_layout().is_some() && projected.is_none() {
            tree.children[0].cancel_interactions();
            tree.state
                .downcast_ref::<AppShellState>()
                .presented_from
                .set([None, None]);
            return;
        }
        let state = tree.state.downcast_ref::<AppShellState>();
        if projected.is_some() {
            if let Some(sample) = renderer.presented_layout() {
                state.accept_presented(sample);
            }
        } else {
            state.presented_from.set([None, None]);
        }
        let child_layout = Self::child_layout(tree, layout);
        if operation.is_native_reconciliation()
            && let Some(projected) = &projected
            && let Some(root) = projected.presentation_node()
        {
            operation.projection_applied(root);
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
        );
        if operation.is_native_reconciliation() {
            return;
        }
        tree.state
            .downcast_mut::<AppShellState>()
            .retained_text_input = checkpoint_child(
            &mut self.content,
            &mut tree.children[0],
            child_layout,
            renderer,
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
        use iced::advanced::Renderer as _;
        let projected = if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            None
        } else {
            Self::present_child(tree, renderer, layout)
        };
        if renderer.presented_layout().is_some()
            && projected.is_none()
            && !matches!(
                event,
                Event::Window(iced::window::Event::RedrawRequested(_))
            )
        {
            tree.children[0].cancel_interactions();
            tree.state
                .downcast_ref::<AppShellState>()
                .presented_from
                .set([None, None]);
            shell.invalidate_layout_with(iced::advanced::graphics::core::shell::Diff::Perform);
            shell.capture_event();
            return;
        }
        if projected.is_some() {
            if let Some(sample) = renderer.presented_layout() {
                tree.state
                    .downcast_ref::<AppShellState>()
                    .accept_presented(sample);
            }
        } else {
            tree.state
                .downcast_ref::<AppShellState>()
                .presented_from
                .set([None, None]);
            let local_layout = self.note_frame(tree, event, shell);
            if local_layout {
                self.refresh_responsive_layout(tree, layout, renderer);
            }
        }
        let child_layout = Self::child_layout(tree, layout);
        if projected.is_none()
            && let Some(capture) = Self::bound_docked_capture(tree, child_layout, layout.bounds())
        {
            let _ = capture.publish(shell);
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if !event_cannot_mutate_focus_state::<Message>(event) {
            tree.state
                .downcast_mut::<AppShellState>()
                .retained_text_input = checkpoint_child(
                &mut self.content,
                &mut tree.children[0],
                child_layout,
                renderer,
            );
        }
    }

    fn frame_interest(&self, _tree: &Tree) -> bool {
        true
    }

    // Frame routing prunes uninterested descendants and cannot mutate the retained focus checkpoint.
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let local_layout = self.note_frame(tree, event, shell);
        if local_layout {
            self.refresh_responsive_layout(tree, layout, renderer);
        }
        let child_layout = Self::child_layout(tree, layout);
        if let Some(capture) = Self::bound_docked_capture(tree, child_layout, layout.bounds()) {
            let _ = capture.publish(shell);
        }

        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        use iced::advanced::Renderer as _;
        let projected = Self::presented_child(tree, renderer, layout);
        if renderer.presented_layout().is_some() && projected.is_none() {
            return mouse::Interaction::None;
        }
        let child_layout = Self::child_layout(tree, layout);
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        tree.state
            .downcast_ref::<AppShellState>()
            .observe_motion_theme(theme);
        let child_layout = Self::child_layout(tree, layout);
        if let Some(capture) = Self::bound_docked_capture(tree, child_layout, layout.bounds()) {
            use iced::advanced::Renderer as _;
            let _ = renderer.request_docked_presentation(capture);
        }
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child_layout,
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
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        use iced::advanced::Renderer as _;
        let presented_child = Self::present_child(tree, renderer, layout);
        if renderer.presented_layout().is_some() && presented_child.is_none() {
            return Vec::new();
        }
        let child_layout = Self::child_layout(tree, layout);
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
