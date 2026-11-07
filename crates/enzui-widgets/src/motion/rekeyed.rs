//! A child that fades in whenever what it shows changes identity: a wizard's
//! next step, a detail pane showing another item.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Renderer as _, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::hash::{Hash, Hasher};

/// Shows `child`, fading it in over the theme's `motion-normal` duration
/// whenever `key` differs from the key of the previous view. The first
/// appearance is simply there, and reduced motion shows every change at
/// once. A new key also gives the child a fresh widget state, as a keyed
/// list does, so one page never inherits another's scroll or focus.
pub fn fade_on_change<'a, Message>(
    key: impl Hash,
    child: impl Into<Element<'a, Message>>,
) -> FadeOnChange<'a, Message> {
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    FadeOnChange {
        key: hasher.finish(),
        child: child.into(),
    }
}

/// A child that fades in when its key changes; built by [`fade_on_change`].
pub struct FadeOnChange<'a, Message> {
    key: u64,
    child: Element<'a, Message>,
}

#[derive(Debug)]
struct State {
    key: u64,
    fade: crate::motion::Swell,
    frame_at: Option<iced::time::Instant>,
    /// The theme's duration, as the last draw read it.
    ms: std::cell::Cell<f32>,
}

/// How long a changed child takes to fade in, before the theme's scale.
fn fade_ms(theme: &iced::Theme) -> f32 {
    crate::theme_lookup::motion(theme).normal_ms
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FadeOnChange<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            key: self.key,
            fade: crate::motion::Swell::snap(true),
            frame_at: None,
            ms: std::cell::Cell::new(enzui_theme::MotionScale::default().normal_ms),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.key != self.key {
            state.key = self.key;
            state.fade = crate::motion::Swell::snap(false);
            tree.children.clear();
        }
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
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
        {
            let State {
                fade, frame_at, ms, ..
            } = tree.state.downcast_mut::<State>();
            fade.track(frame_at, event, true, ms.get(), shell);
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
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        state.ms.set(fade_ms(theme));
        let alpha = state.fade.paint(state.frame_at, state.ms.get(), theme);
        let draw = |renderer: &mut iced::Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        };
        if alpha >= 1.0 {
            draw(renderer);
        } else {
            renderer.with_layer_alpha(layout.bounds(), alpha, draw);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
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
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message: 'a> From<FadeOnChange<'a, Message>> for Element<'a, Message> {
    fn from(fade: FadeOnChange<'a, Message>) -> Self {
        Element::new(fade)
    }
}
