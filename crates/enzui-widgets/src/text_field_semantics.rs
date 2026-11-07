//! Text-input semantics with live focus, routed value commits and lazy submission.
//!
//! Wrap only the input row so its bounds exclude labels and helper text. The
//! probe ID must match a focusable child; the published ID can be different or
//! absent. Child semantics remain reachable through operation traversal.
//!
//! [`TextFieldSemantics::on_input`] enables routed value requests when the node
//! has a published ID and its semantics are neither disabled nor read-only.
//! Requests latch during operations and commit on the next update or presented
//! frame, before child input. Rebuilding disabled or read-only discards pending
//! edits. The caller still owns the displayed value.

use crate::value_action::{ValueIntent, ValueRequest};
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::activation_from_key;
use enzui_core::semantics::Semantics;
use iced::advanced::widget::operation::{Focusable, Outcome};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

#[derive(Debug, Clone, Default)]
struct SemanticsState {
    pending_value: Option<ValueIntent>,
    composing: bool,
}

/// Single-child semantics wrapper using the child's bounds and observed focus.
/// The probe ID must match a focusable child; unmatched probes report unfocused.
#[must_use]
pub struct TextFieldSemantics<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    composition_indicator: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    semantics: Semantics,
    probe_id: Id,
    published_id: Option<Id>,
    on_input: Option<Box<dyn Fn(String) -> Vec<Message> + 'a>>,
    editable: bool,
    on_submit: Option<Box<dyn Fn() -> Message + 'a>>,
}

impl<'a, Message, Theme, Renderer> TextFieldSemantics<'a, Message, Theme, Renderer> {
    /// Wrap an input row with separate focus-probe and published identities.
    /// `None` publishes anonymous semantics while retaining live focus observation.
    /// No value or submit callbacks are installed by default.
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        probe_id: Id,
        published_id: Option<Id>,
        semantics: Semantics,
    ) -> Self {
        let editable = !semantics.disabled && !semantics.read_only;
        Self {
            child: child.into(),
            composition_indicator: None,
            semantics,
            probe_id,
            published_id,
            on_input: None,
            editable,
            on_submit: None,
        }
    }

    pub(crate) fn composition_indicator(
        mut self,
        indicator: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    ) -> Self {
        self.composition_indicator = indicator;
        self
    }

    /// Enable routed text commits through this callback.
    /// Use the same callback as the child input. Requests require a published ID
    /// and semantics that are neither disabled nor read-only. Payloads are forwarded
    /// unchanged; numeric increment and decrement intents are discarded. Without this
    /// callback, a SetValue action advertised by supplied semantics has no handler here.
    pub fn on_input(mut self, on_input: impl Fn(String) -> Message + 'a) -> Self {
        self.on_input = Some(Box::new(move |text| vec![on_input(text)]));
        self
    }

    /// [`Self::on_input`] for a value that takes several messages to apply,
    /// published in order (select everything, then paste over it).
    pub(crate) fn on_input_messages(
        mut self,
        on_input: impl Fn(String) -> Vec<Message> + 'a,
    ) -> Self {
        self.on_input = Some(Box::new(on_input));
        self
    }

    /// Install the routed commit callback when `offer` is true.
    /// False preserves the existing callback. An enclosing numeric control should
    /// own its value requests without a competing inner text handler.
    pub fn on_input_when(self, offer: bool, on_input: impl Fn(String) -> Message + 'a) -> Self {
        if offer { self.on_input(on_input) } else { self }
    }

    /// Run a submit factory on non-repeating Enter in a focused, editable child.
    /// Shift is accepted; Control, Alt and Logo suppress submission. The event is
    /// captured before child update. The factory never runs during construction.
    /// Build the child without its own eager submit message to give this wrapper
    /// sole ownership of submission.
    pub fn on_submit_with(mut self, on_submit: impl Fn() -> Message + 'a) -> Self {
        self.on_submit = Some(Box::new(on_submit));
        self
    }

    /// Set or clear the optional submit factory with the same event-time behavior
    /// as [`Self::on_submit_with`].
    pub fn on_submit_with_maybe(
        mut self,
        on_submit: Option<Box<dyn Fn() -> Message + 'a>>,
    ) -> Self {
        self.on_submit = on_submit;
        self
    }
}

struct FocusProbe<'a> {
    target: &'a Id,
    is_focused: Option<bool>,
}

impl<T> Operation<T> for FocusProbe<'_> {
    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if self.is_focused.is_none() && id.is_some_and(|id| id == self.target) {
            self.is_focused = Some(state.is_focused());
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        if self.is_focused.is_none() {
            operate(self);
        }
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for TextFieldSemantics<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SemanticsState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SemanticsState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
        if let Some(indicator) = &self.composition_indicator {
            indicator.set(tree.state.downcast_ref::<SemanticsState>().composing);
        }
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut probe = FocusProbe {
            target: &self.probe_id,
            is_focused: None,
        };
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            &mut probe,
        );
        let focused = probe.is_focused.unwrap_or(false);
        let mut node = SemanticsNode::new(self.semantics.clone().focused(focused));
        operation.custom(self.published_id.as_ref(), layout.bounds(), &mut node);
        if self.editable
            && self.on_input.is_some()
            && let Some(id) = self.published_id.as_ref()
        {
            let mut request = ValueRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if let Some(intent) = request.take() {
                tree.state.downcast_mut::<SemanticsState>().pending_value = Some(intent);
            }
        }
        operation.container(self.published_id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        {
            // Take before checking editability so disabled rebuilds discard rather than defer edits.
            let pending = tree
                .state
                .downcast_mut::<SemanticsState>()
                .pending_value
                .take();
            if let Some(ValueIntent::Set(text)) = pending
                && self.editable
                && let Some(on_input) = self.on_input.as_ref()
            {
                for message in on_input(text) {
                    shell.publish(message);
                }
                shell.request_redraw();
            }
        }
        let enter_activation = matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
                ..
            })
        ) && matches!(event, Event::Keyboard(kb) if activation_from_key(kb));
        if let Some(on_submit) = self.on_submit.as_ref()
            && self.editable
            && enter_activation
        {
            let mut probe = FocusProbe {
                target: &self.probe_id,
                is_focused: None,
            };
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                &layout.bounds(),
                renderer,
                &mut probe,
            );
            if probe.is_focused.unwrap_or(false) {
                shell.publish(on_submit());
                shell.capture_event();
                shell.request_redraw();
                return;
            }
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
        if let Some(indicator) = &self.composition_indicator {
            let mut probe = FocusProbe {
                target: &self.probe_id,
                is_focused: None,
            };
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                &mut probe,
            );
            let focused = probe.is_focused.unwrap_or(false);
            let state = tree.state.downcast_mut::<SemanticsState>();
            let before = state.composing;
            if !focused {
                state.composing = false;
            } else if let Event::InputMethod(event) = event {
                use iced::advanced::input_method::Event;
                match event {
                    Event::Preedit(content, _) => state.composing = !content.is_empty(),
                    Event::Opened | Event::Closed | Event::Commit(_) => state.composing = false,
                }
            }
            indicator.set(state.composing);
            if before != state.composing {
                shell.request_redraw();
            }
        }
    }

    // Operations can arm a value without refreshing ancestor event-interest caches.
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.editable && self.on_input.is_some()
            || tree
                .state
                .downcast_ref::<SemanticsState>()
                .pending_value
                .is_some()
            || crate::frame_routing::interest_opaque(tree)
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
        let pending = tree
            .state
            .downcast_mut::<SemanticsState>()
            .pending_value
            .take();
        if let Some(ValueIntent::Set(text)) = pending
            && self.editable
            && let Some(on_input) = self.on_input.as_ref()
        {
            for message in on_input(text) {
                shell.publish(message);
            }
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

impl<'a, Message, Theme, Renderer> From<TextFieldSemantics<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: TextFieldSemantics<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
