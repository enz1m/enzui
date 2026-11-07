//! Reuse retained subtree geometry when only an unrelated region requests layout.
//!
//! The key compares limit bits and renderer font/text size. A view diff or the
//! child’s own layout/widget invalidation clears it. Geometry changes outside
//! those inputs require a child invalidation or view rebuild. Events, paint,
//! operations and overlays continue to reach the child on cache hits.
//!
//! `ENZUI_DISABLE_LAYOUT_MEMO=1` bypasses reuse; the environment is sampled once
//! for the process on the first bypass check.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer, text};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
pub(crate) struct LayoutMemo<'a, Message> {
    child: Element<'a, Message>,
}

impl<'a, Message> LayoutMemo<'a, Message> {
    pub(crate) fn new(child: impl Into<Element<'a, Message>>) -> Self {
        Self {
            child: child.into(),
        }
    }
}

#[derive(Default)]
struct State {
    laid_out: Option<Key>,
}

#[derive(Clone, Copy, PartialEq)]
struct Key {
    limits: [u32; 4],
    font: iced::Font,
    text_size: u32,
}

impl Key {
    fn of(limits: &layout::Limits, renderer: &iced::Renderer) -> Self {
        use text::Renderer as _;
        Self {
            limits: [
                limits.min().width.to_bits(),
                limits.min().height.to_bits(),
                limits.max().width.to_bits(),
                limits.max().height.to_bits(),
            ],
            font: renderer.font(),
            text_size: renderer.text_size().0.to_bits(),
        }
    }
}

fn invalidates_layout(invalidation: iced::advanced::shell::Invalidation) -> bool {
    matches!(
        invalidation,
        iced::advanced::shell::Invalidation::Layout(_)
            | iced::advanced::shell::Invalidation::Widgets
    )
}
fn bypassed() -> bool {
    static BYPASS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BYPASS.get_or_init(|| {
        std::env::var_os("ENZUI_DISABLE_LAYOUT_MEMO").is_some_and(|value| value == "1")
    })
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for LayoutMemo<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // A rebuilt view can change geometry without changing limits or text defaults.
        tree.state.downcast_mut::<State>().laid_out = None;
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.first().is_none_or(Tree::has_frame_interest)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let key = Key::of(limits, renderer);
        let reuse = !bypassed() && tree.state.downcast_ref::<State>().laid_out == Some(key);
        if !reuse {
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
            tree.state.downcast_mut::<State>().laid_out = Some(key);
        }
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
        let child = &mut self.child;
        // Isolate child invalidation so an already-invalid sibling cannot hide it.
        let ((), own) = shell.scoped_invalidation(|shell| {
            child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        });
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
        if invalidates_layout(own) {
            tree.state.downcast_mut::<State>().laid_out = None;
        }
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
        let child = &mut self.child;
        let ((), own) = shell.scoped_invalidation(|shell| {
            crate::frame_routing::forward_frame_opaque(
                child, tree, 0, event, layout, cursor, renderer, shell, viewport,
            );
        });
        if invalidates_layout(own) {
            tree.state.downcast_mut::<State>().laid_out = None;
        }
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

impl<'a, Message: 'a> From<LayoutMemo<'a, Message>> for Element<'a, Message> {
    fn from(memo: LayoutMemo<'a, Message>) -> Self {
        Element::new(memo)
    }
}
