//! Follow the first descendant scroll region until its observed offset leaves the tail.
//!
//! [`crate::chat_transcript`] and [`crate::chat_list`] enable following by default.
//! Changing the descendant scrollable ID starts a new tail-follow session.
//! The initial pin waits for nonempty content and is applied during widget
//! operations or event updates. Returning within [`RELEASE_DISTANCE`] rejoins.
//! Programmatic offset changes are judged by the same distance rule as gestures.
//! The pin snaps both scroll axes to their relative end; disable following
//! when the host needs to control those offsets independently.

use enzui_core::chat_stream::Follow;
use iced::advanced::widget::operation::scrollable::{RelativeOffset, Scrollable};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
/// Inclusive distance from the vertical end that still permits following, in logical pixels.
pub const RELEASE_DISTANCE: f32 = 24.0;
/// Compute nonnegative remaining vertical distance from offset and geometry in pixels.
/// The maximum offset is clamped to zero; non-finite translation or maximum returns zero.
#[must_use]
pub fn distance_from_end(translation_y: f32, viewport_h: f32, content_h: f32) -> f32 {
    let max_offset = (content_h - viewport_h).max(0.0);
    if !translation_y.is_finite() || !max_offset.is_finite() {
        return 0.0;
    }
    (max_offset - translation_y).max(0.0)
}
/// Compare distance against the inclusive [`RELEASE_DISTANCE`] band.
/// Negative distances qualify; NaN does not.
#[must_use]
pub fn is_at_end(distance: f32) -> bool {
    distance <= RELEASE_DISTANCE
}
#[derive(Debug)]
struct TailState {
    follow: Follow,
    last_content_h: f32,
    pinned_once: bool,
    scroll_id: Option<iced::advanced::widget::Id>,
}

impl Default for TailState {
    fn default() -> Self {
        Self {
            follow: Follow::following(),
            last_content_h: 0.0,
            pinned_once: false,
            scroll_id: None,
        }
    }
}
#[derive(Debug, Clone)]
struct Geom {
    scroll_id: Option<iced::advanced::widget::Id>,
    viewport_h: f32,
    content_h: f32,
    translation_y: f32,
}
fn read_geometry<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) -> Option<Geom>
where
    Renderer: iced::advanced::Renderer,
{
    struct ReadGeom {
        found: Option<Geom>,
    }
    impl Operation for ReadGeom {
        fn scrollable(
            &mut self,
            id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            content_bounds: Size,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if self.found.is_none() {
                self.found = Some(Geom {
                    scroll_id: id.cloned(),
                    viewport_h: bounds.height,
                    content_h: content_bounds.height,
                    translation_y: translation.y,
                });
            }
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if self.found.is_none() {
                operate(self);
            }
        }
    }
    let mut op = ReadGeom { found: None };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut op);
    op.found
}
fn snap_to_end<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) where
    Renderer: iced::advanced::Renderer,
{
    struct SnapEnd {
        done: bool,
    }
    impl Operation for SnapEnd {
        fn scrollable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            content_bounds: Size,
            _translation: Vector,
            state: &mut dyn Scrollable,
        ) {
            if !self.done {
                state.snap_to(
                    RelativeOffset::END.into(),
                    iced::advanced::widget::operation::Animation::Instant,
                    bounds,
                    content_bounds,
                );
                self.done = true;
            }
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if !self.done {
                operate(self);
            }
        }
    }
    let mut op = SnapEnd { done: false };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut op);
}
/// Follow the first descendant scrollable, initially pinning nonempty content to its end.
/// Offsets beyond [`RELEASE_DISTANCE`] release following; returning inside the band
/// rejoins. Rebuilds retain this state, and pins snap both scroll axes to their relative end.
pub fn enz_tail_follow<'a, Message>(child: impl Into<Element<'a, Message>>) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(TailFollow {
        child: child.into(),
    })
}
struct TailFollow<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

impl<Message, Theme, Renderer> TailFollow<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn prepare_initial_pin(&mut self, tree: &mut Tree, layout: Layout, renderer: &Renderer) {
        let Some(geom) = read_geometry(&mut self.child, &mut tree.children[0], layout, renderer)
        else {
            return;
        };
        let state = tree.state.downcast_ref::<TailState>();
        if !state.pinned_once || state.scroll_id != geom.scroll_id {
            self.apply_pin(tree, layout, renderer);
        }
    }

    fn apply_pin(&mut self, tree: &mut Tree, layout: Layout, renderer: &Renderer) {
        let Some(geom) = read_geometry(&mut self.child, &mut tree.children[0], layout, renderer)
        else {
            return;
        };
        let state = tree.state.downcast_mut::<TailState>();
        if state.scroll_id != geom.scroll_id {
            *state = TailState {
                scroll_id: geom.scroll_id,
                ..TailState::default()
            };
        }
        // An empty pass must not consume the initial pin before content arrives.
        if geom.content_h <= 0.0 {
            return;
        }

        let was_following = state.follow.is_following();
        let grew = geom.content_h > state.last_content_h + f32::EPSILON;
        let distance = distance_from_end(geom.translation_y, geom.viewport_h, geom.content_h);

        if state.pinned_once {
            state.follow = if is_at_end(distance) {
                state.follow.rejoin()
            } else {
                state.follow.release()
            };
            if grew && !state.follow.is_following() {
                state.follow = state.follow.on_new_message();
            }
        }

        state.last_content_h = geom.content_h;

        let first_pin = !state.pinned_once;
        if first_pin {
            state.pinned_once = true;
        }
        let following = state.follow.is_following();
        // Relative end tracks growth; repeated snaps would cancel smooth wheel movement.
        if first_pin || (!was_following && following) {
            snap_to_end(&mut self.child, &mut tree.children[0], layout, renderer);
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for TailFollow<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<TailState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(TailState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.prepare_initial_pin(tree, layout, renderer);
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        self.apply_pin(tree, layout, renderer);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        // Operations must observe the post-pin geometry.
        self.apply_pin(tree, layout, renderer);
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.prepare_initial_pin(tree, layout, renderer);
        // Judge the offset after the child applies the wheel or drag event.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
        self.apply_pin(tree, layout, renderer);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
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
