//! Restore a text input's focus and exact caret after its subtree is replaced.
//!
//! One pending claim belongs to the active widget-runtime scope. Restoration
//! requires a fresh mount, a matching stable target id, and a claim no older
//! than [`RETENTION_WINDOW`](crate::focus_retention::RETENTION_WINDOW) (400 ms). An already-focused target keeps its caret.
//! Mounted fields checkpoint after event, frame and operation delivery; losing
//! focus revokes their claim. The time limit permits short remounts, including
//! a quick return to the same id; it does not distinguish navigation intent.
//!
//! Targets must be unique within a runtime. Direct `UserInterface` harnesses
//! should retain a [`FocusScope`](crate::focus_retention::FocusScope) for the fixture; framework runtime scopes
//! isolate these claims automatically. Nested scopes restore their outer owner.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Id, Operation, Tree, operation, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::widget::text_input;
use iced::{Element, Event, Length, Rectangle, Size};

/// Maximum age of a retained focus claim: 400 milliseconds.
pub const RETENTION_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

/// A caret or selection stored in UTF-8 byte indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedCursor {
    /// A collapsed caret at this byte index.
    Index(usize),
    /// A selection spanning these byte indices.
    Selection {
        /// Anchor.
        start: usize,
        /// Focus end.
        end: usize,
    },
}

#[derive(Debug, Clone)]
struct Record {
    id: Id,
    cursor: RetainedCursor,
    at: std::time::Instant,
}

#[derive(Default)]
struct ClaimSlot(Option<Record>);

/// Clear the pending focus claim in the current widget-runtime scope.
/// Use [`FocusScope`] to isolate a directly driven harness and restore its outer
/// owner on exit; this function does not create a scope.
pub fn clear_claims() {
    enzui_core::widget_runtime::with_state::<ClaimSlot, _>(|slot| slot.0 = None);
}

/// A fresh widget-runtime owner that isolates focus claims until drop.
/// Dropping clears its claim and restores the enclosing runtime, whose claims
/// remain intact.
#[derive(Debug)]
pub struct FocusScope {
    // Drop clears this claim before the field restores the enclosing runtime owner.
    _runtime: enzui_core::widget_runtime::WidgetRuntimeScope,
}

impl FocusScope {
    /// Create and enter a fresh runtime scope.
    pub fn new() -> Self {
        // Install the new owner before clearing, preserving the enclosing claim.
        let runtime = enzui_core::widget_runtime::WidgetRuntimeScope::new();
        clear_claims();
        Self { _runtime: runtime }
    }
}

impl Default for FocusScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FocusScope {
    fn drop(&mut self) {
        clear_claims();
    }
}

fn write_claim(id: Id, cursor: RetainedCursor) {
    enzui_core::widget_runtime::with_state::<ClaimSlot, _>(|slot| {
        slot.0 = Some(Record {
            id,
            cursor,
            at: std::time::Instant::now(),
        });
    });
}

fn drop_claim_for(id: &Id) {
    enzui_core::widget_runtime::with_state::<ClaimSlot, _>(|slot| {
        if slot.0.as_ref().is_some_and(|record| &record.id == id) {
            slot.0 = None;
        }
    });
}

fn take_claim_for(id: &Id) -> Option<RetainedCursor> {
    enzui_core::widget_runtime::with_state::<ClaimSlot, _>(|slot| {
        let fresh = slot
            .0
            .as_ref()
            .is_some_and(|record| &record.id == id && record.at.elapsed() <= RETENTION_WINDOW);
        if fresh {
            slot.0.take().map(|record| record.cursor)
        } else {
            None
        }
    })
}

#[derive(Default)]
struct FocusProbe {
    focused: Option<Id>,
    text: Option<String>,
    pending: Option<(Option<Id>, String)>,
}

impl<T> Operation<T> for FocusProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn text_input(
        &mut self,
        id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn operation::text_input::TextInput,
    ) {
        self.pending = Some((id.cloned(), state.text().into_owned()));
    }

    fn focusable(
        &mut self,
        id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        if state.is_focused() {
            if let Some((input_id, text)) = self.pending.take()
                && input_id.as_ref() == id
                && let Some(input_id) = input_id
            {
                self.focused = Some(input_id);
                self.text = Some(text);
            }
        } else {
            self.pending = None;
        }
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
    // Input cursors count graphemes; restoring operations require byte indices.
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

pub(crate) fn line_position(index: usize) -> iced::advanced::text::Position {
    iced::advanced::text::Position { line: 0, index }
}

pub(crate) fn focused_retained_cursor(tree: &Tree, text: &str) -> Option<RetainedCursor> {
    focused_cursor_in(tree).map(|cursor| retained_cursor(cursor, text))
}

#[derive(Default)]
struct State {
    settled: bool,
}

/// Wrap one text input with focus and caret retention across fresh mounts.
pub struct FocusRetention<'a, Message> {
    child: Element<'a, Message>,
    target: Id,
}

impl<'a, Message> FocusRetention<'a, Message> {
    /// Wrap the text input carrying the stable target id.
    /// Use distinct target ids for unrelated fields in the same runtime.
    pub fn new(child: Element<'a, Message>, target: Id) -> Self {
        Self { child, target }
    }

    fn probe(&mut self, tree: &mut Tree, layout: Layout, renderer: &iced::Renderer) -> bool {
        let mut probe = FocusProbe::default();
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut probe,
        );
        probe.focused.as_ref() == Some(&self.target)
    }

    fn run(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        op: &mut dyn Operation<()>,
    ) {
        let mut boxed: Box<dyn Operation> = Box::new(operation::black_box(op));
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut *boxed,
        );
    }

    // Layout can probe focus through an operation before any event reaches a fresh mount.
    fn settle(&mut self, tree: &mut Tree, layout: Layout, renderer: &iced::Renderer) -> bool {
        if tree.state.downcast_ref::<State>().settled {
            return false;
        }
        tree.state.downcast_mut::<State>().settled = true;

        // Refocusing an already-restored input would move its exact caret to the end.
        if self.probe(tree, layout, renderer) {
            return false;
        }
        let Some(caret) = take_claim_for(&self.target) else {
            return false;
        };

        let mut focus = operation::focusable::focus::<()>(self.target.clone());
        self.run(tree, layout, renderer, &mut focus);
        match caret {
            RetainedCursor::Index(position) => {
                let mut op = operation::text_input::move_cursor_to::<()>(
                    self.target.clone(),
                    line_position(position),
                );
                self.run(tree, layout, renderer, &mut op);
            }
            RetainedCursor::Selection { start, end } => {
                let mut op = operation::text_input::select_range::<()>(
                    self.target.clone(),
                    line_position(start),
                    line_position(end),
                );
                self.run(tree, layout, renderer, &mut op);
            }
        }
        true
    }

    // Operations can move focus or caret without events, so every delivery path checkpoints.
    fn checkpoint(&mut self, tree: &mut Tree, layout: Layout, renderer: &iced::Renderer) {
        let mut probe = FocusProbe::default();
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut probe,
        );
        match (probe.focused, probe.text) {
            (Some(id), Some(text)) if id == self.target => {
                if let Some(cursor) = focused_cursor_in(&tree.children[0]) {
                    write_claim(self.target.clone(), retained_cursor(cursor, &text));
                }
            }
            _ => drop_claim_for(&self.target),
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FocusRetention<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
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
        self.settle(tree, layout, renderer);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
        self.checkpoint(tree, layout, renderer);
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
        if self.settle(tree, layout, renderer) {
            shell.request_redraw();
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
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
        self.checkpoint(tree, layout, renderer);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.state.downcast_ref::<State>().settled || crate::frame_routing::interest_opaque(tree)
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
        if self.settle(tree, layout, renderer) {
            shell.request_redraw();
        }
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
        self.checkpoint(tree, layout, renderer);
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
        translation: iced::Vector,
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

impl<'a, Message: 'a> From<FocusRetention<'a, Message>> for Element<'a, Message> {
    fn from(w: FocusRetention<'a, Message>) -> Self {
        Element::new(w)
    }
}
