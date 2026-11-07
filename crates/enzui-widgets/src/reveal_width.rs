//! A child that slides open and closed along its width.
//!
//! [`reveal_width`] takes the child's natural width while `open` and none
//! while closed, easing between the two over the theme's `motion-normal`
//! duration, so the content after it moves with it: a tab whose label slides
//! out as it is chosen pushes its neighbours aside as it grows. The child
//! fades with the width and is clipped to it. Reduced motion snaps.
//!
//! [`reveal_width_unless`] also skips drawing the child while a check says so:
//! a tab strip whose selection slides draws a collapsing label only in the
//! pass clipped to its moving indicator, so the label leaves and arrives with
//! the indicator instead of standing on the bare track.

use iced::advanced::layout::{self, Layout, Measure as _};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, renderer};
use iced::{Element, Length, Rectangle, Renderer, Size, window};
use std::cell::Cell;

/// `child`, as wide as it is while `open` and no width while closed, sliding
/// between the two.
pub(crate) fn reveal_width<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
    open: bool,
) -> Element<'a, Message> {
    Element::new(RevealWidth {
        child: child.into(),
        open,
        hidden: None,
    })
}

/// [`reveal_width`], drawing nothing while `hidden` says so.
pub(crate) fn reveal_width_unless<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
    open: bool,
    hidden: impl Fn() -> bool + 'a,
) -> Element<'a, Message> {
    Element::new(RevealWidth {
        child: child.into(),
        open,
        hidden: Some(Box::new(hidden)),
    })
}

struct RevealWidth<'a, Message> {
    child: Element<'a, Message>,
    open: bool,
    hidden: Option<Box<dyn Fn() -> bool + 'a>>,
}

struct RevealState {
    swell: crate::motion::Swell,
    frame_at: Option<iced::time::Instant>,
    /// The share of the width shown at the last frame.
    shown: f32,
    /// The theme's reduced-motion policy, as the last draw saw it.
    reduced: Cell<bool>,
}

fn duration_ms() -> f32 {
    enzui_theme::MotionScale::default().normal_ms
}

impl<Message> Widget<Message, iced::Theme, Renderer> for RevealWidth<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RevealState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(RevealState {
            swell: crate::motion::Swell::snap(self.open),
            frame_at: None,
            shown: if self.open { 1.0 } else { 0.0 },
            reduced: Cell::new(false),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let shown = tree.state.downcast_ref::<RevealState>().shown;
        let child = self.child.as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, limits.max().height)),
        );
        let natural = child.size();
        layout::Node::with_children(
            Size::new((natural.width * shown).round(), natural.height),
            vec![child],
        )
        .apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the reveal lays its child out under it");
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        let state = tree.state.downcast_mut::<RevealState>();
        if state.reduced.get() {
            if state.swell.target != self.open || state.shown != f32::from(u8::from(self.open)) {
                state.swell = crate::motion::Swell::snap(self.open);
                state.shown = f32::from(u8::from(self.open));
                shell.invalidate_layout();
                shell.request_redraw();
            }
            return;
        }
        let RevealState {
            swell, frame_at, ..
        } = state;
        swell.track(frame_at, event, self.open, duration_ms(), shell);
        if let iced::Event::Window(window::Event::RedrawRequested(now)) = event {
            let shown = swell.value(*now, duration_ms());
            if (shown - state.shown).abs() > f32::EPSILON {
                state.shown = shown;
                shell.invalidate_layout();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<RevealState>();
        state
            .reduced
            .set(crate::theme_lookup::motion(theme).reduced_motion);
        let shown = state.shown;
        if shown <= 0.0 || self.hidden.as_ref().is_some_and(|hidden| hidden()) {
            return;
        }
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the reveal lays its child out under it");
        let draw = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        };
        if shown >= 1.0 {
            draw(renderer);
        } else {
            // Clipped to the width shown so far, and fading with it.
            let bounds = layout.bounds();
            renderer.with_layer_composite(bounds, shown, 1.0, 0.0, bounds.center(), draw);
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the reveal lays its child out under it");
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the reveal lays its child out under it");
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
            cursor,
            viewport,
            renderer,
        )
    }
}
