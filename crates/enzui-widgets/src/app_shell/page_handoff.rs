use std::cell::{Cell, RefCell};
use std::rc::Rc;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Renderer as _, Shell, Widget, mouse, overlay, renderer};
use iced::time::{Duration, Instant};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(crate) const MAX_HELD: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub(crate) struct Signal {
    bounds: Cell<Option<Rectangle>>,
    anticipating: Cell<bool>,
    armed: Cell<bool>,
    holding: Cell<bool>,
    release: Cell<bool>,
    held_since: Cell<Option<Instant>>,
}

impl Signal {
    pub(crate) fn observe_input(
        &self,
        event: &Event,
        cursor: mouse::Cursor,
        published: bool,
    ) -> bool {
        let outside = match (cursor.position(), self.bounds.get()) {
            (Some(point), Some(bounds)) => !bounds.contains(point),
            _ => false,
        };
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorEntered) => {
                let was_anticipating = self.anticipating.replace(outside);
                return outside && !was_anticipating;
            }
            Event::Mouse(mouse::Event::CursorLeft) => self.anticipating.set(false),
            Event::Mouse(
                mouse::Event::ButtonPressed(mouse::Button::Left)
                | mouse::Event::ButtonReleased(mouse::Button::Left),
            ) if outside && published => self.armed.set(true),
            _ => {}
        }
        false
    }

    pub(crate) fn end_frame(&self, at: Instant, motion: bool) -> bool {
        self.armed.set(false);
        if !self.holding.get() {
            return false;
        }
        let since = self.held_since.get().unwrap_or(at);
        self.held_since.set(Some(since));
        if !motion || at.saturating_duration_since(since) >= MAX_HELD {
            self.release.set(true);
        }
        self.release.get()
    }

    fn settle(&self) {
        self.anticipating.set(false);
        self.holding.set(false);
        self.release.set(false);
        self.held_since.set(None);
    }
}

thread_local! {
    static PROBING: Cell<bool> = const { Cell::new(false) };
}

// Shell focus probes see a held page as empty instead of handing it in.
pub(crate) fn probing<R>(f: impl FnOnce() -> R) -> R {
    let previous = PROBING.replace(true);
    let result = f();
    PROBING.set(previous);
    result
}

// The built page must fill the boundary, whose size is reported before the page exists.
pub(crate) struct PageHandoff<'a, Message> {
    build: Box<dyn Fn() -> Element<'a, Message> + 'a>,
    child: Option<Element<'a, Message>>,
    signal: Rc<Signal>,
}

impl<'a, Message> PageHandoff<'a, Message> {
    pub(crate) fn new(build: impl Fn() -> Element<'a, Message> + 'a, signal: Rc<Signal>) -> Self {
        Self {
            build: Box::new(build),
            child: None,
            signal,
        }
    }

    fn page(&mut self) -> &mut Element<'a, Message> {
        self.child.get_or_insert_with(&self.build)
    }

    fn diff_page(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(self.page()));
    }

    fn end_hold(&mut self, tree: &mut Tree) -> Option<layout::Limits> {
        let state = tree.state.downcast_mut::<State>();
        state.held = false;
        let limits = state.limits;
        self.signal.settle();
        self.diff_page(tree);
        limits
    }

    fn layout_page(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.page()
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
        let state = tree.state.downcast_mut::<State>();
        state.limits = Some(*limits);
        if let Some(mut pending) = state.pending.take() {
            let layout = Layout::new(tree.size).move_to(pending.origin);
            self.page().as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                &pending.viewport,
                renderer,
                pending.operation.as_mut(),
            );
        }
    }

    fn release(&mut self, tree: &mut Tree, renderer: &iced::Renderer) {
        if let Some(limits) = self.end_hold(tree) {
            self.layout_page(tree, renderer, &limits);
        }
    }
}

#[derive(Default)]
struct State {
    retained: RefCell<Option<Retained>>,
    limits: Option<layout::Limits>,
    held: bool,
    pending: Option<Pending>,
}

struct Pending {
    operation: Box<dyn Operation>,
    origin: iced::Point,
    viewport: Rectangle,
}

struct Retained {
    recording: renderer::Recording,
    bounds: Rectangle,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for PageHandoff<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.held && !self.signal.release.get() {
            return;
        }
        if state.held {
            state.held = false;
            self.signal.settle();
        } else if self.signal.armed.get()
            && !tree.children.is_empty()
            && state.limits.is_some()
            && state.retained.get_mut().is_some()
        {
            state.held = true;
            self.signal.settle();
            self.signal.holding.set(true);
            return;
        }
        self.diff_page(tree);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.state.downcast_ref::<State>().held || crate::frame_routing::interest_opaque(tree)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let state = tree.state.downcast_ref::<State>();
        if state.held {
            if !self.signal.release.get() && state.limits == Some(*limits) {
                return;
            }
            let _ = self.end_hold(tree);
        }
        self.layout_page(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        if state.held {
            if PROBING.get() || operation.is_docked_clock_rebase() {
                return;
            }
            if state.pending.is_none()
                && let Some(operation) = operation.clone_for_deferred()
            {
                state.pending = Some(Pending {
                    operation,
                    origin: layout.position(),
                    viewport: *viewport,
                });
                return;
            }
            self.release(tree, renderer);
        }
        self.page().as_widget_mut().operate(
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
        self.signal.bounds.set(Some(layout.bounds()));
        if tree.state.downcast_ref::<State>().held {
            match event {
                Event::Window(_)
                | Event::Mouse(
                    mouse::Event::CursorMoved { .. }
                    | mouse::Event::CursorEntered
                    | mouse::Event::CursorLeft,
                ) => return,
                Event::Keyboard(_) | Event::InputMethod(_) => {
                    self.release(tree, renderer);
                    shell.invalidate_layout();
                }
                _ => {
                    self.release(tree, renderer);
                    shell.invalidate_layout();
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }
            }
        }
        let page = self.page();
        page.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(page, tree, 0);
    }

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
        self.signal.bounds.set(Some(layout.bounds()));
        if tree.state.downcast_ref::<State>().held {
            return;
        }
        crate::frame_routing::forward_frame_opaque(
            self.page(),
            tree,
            0,
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
        let bounds = layout.bounds();
        self.signal.bounds.set(Some(bounds));
        let state = tree.state.downcast_ref::<State>();
        if state.held {
            let replayed = state.retained.borrow().as_ref().is_some_and(|retained| {
                renderer.replay(
                    &retained.recording,
                    Vector::new(bounds.x - retained.bounds.x, bounds.y - retained.bounds.y),
                )
            });
            if !replayed {
                self.signal.release.set(true);
            }
            return;
        }
        let Some(page) = &self.child else {
            return;
        };
        let recording = self.signal.anticipating.get() && renderer.start_recording(bounds);
        page.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        *state.retained.borrow_mut() = if recording {
            renderer
                .finish_recording()
                .map(|recording| Retained { recording, bounds })
        } else {
            None
        };
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(page) = self
            .child
            .as_ref()
            .filter(|_| !tree.state.downcast_ref::<State>().held)
        else {
            return mouse::Interaction::None;
        };
        page.as_widget()
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
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
        if tree.state.downcast_ref::<State>().held {
            return Vec::new();
        }
        self.page().as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<PageHandoff<'a, Message>> for Element<'a, Message> {
    fn from(handoff: PageHandoff<'a, Message>) -> Self {
        Element::new(handoff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_must_publish_before_it_can_hold_the_page() {
        let signal = Signal::default();
        signal.bounds.set(Some(Rectangle::new(
            iced::Point::new(300.0, 0.0),
            Size::new(500.0, 500.0),
        )));
        let outside = mouse::Cursor::Available(iced::Point::new(100.0, 100.0));
        let inside = mouse::Cursor::Available(iced::Point::new(400.0, 100.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        signal.observe_input(&press, inside, true);
        signal.observe_input(&press, outside, false);
        assert!(!signal.armed.get());
        assert!(!signal.end_frame(Instant::now(), false));
        signal.observe_input(&release, outside, true);
        assert!(signal.armed.get());
        assert!(!signal.end_frame(Instant::now(), true));
        assert!(!signal.armed.get());

        signal.observe_input(&press, outside, true);
        assert!(signal.armed.get());
    }

    #[test]
    fn a_hold_ends_when_outside_motion_stops_or_after_the_bound() {
        let start = Instant::now();
        let signal = Signal::default();
        signal.holding.set(true);
        assert!(!signal.end_frame(start, true));
        assert!(!signal.end_frame(start + Duration::from_millis(100), true));
        assert!(signal.end_frame(start + Duration::from_millis(116), false));

        let perpetual = Signal::default();
        perpetual.holding.set(true);
        let mut at = start;
        while !perpetual.end_frame(at, true) {
            at += Duration::from_millis(16);
            assert!(at <= start + MAX_HELD + Duration::from_millis(16));
        }
        assert!(at >= start + MAX_HELD);
    }
}
