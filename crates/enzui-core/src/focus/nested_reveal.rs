//! Reveal a target through nested scroll clips, from the inside out.

use iced_core::widget::{Id, operation};
use iced_core::{Rectangle, Size, Vector};
use operation::{Operation, Outcome};
use std::any::Any;

/// Which enclosing scroll views a nested reveal may move.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AnonymousScrollPolicy {
    /// Move all enclosing views, including views without a widget id.
    #[default]
    Include,
    /// Move only named views; anonymous views still clip the target.
    NamedOnly,
}

#[derive(Clone)]
enum Target {
    Named(Id),
    Focused,
}

#[derive(Clone)]
struct Frame {
    ordinal: usize,
    named: bool,
    bounds: Rectangle,
    translation: Vector,
}

struct Find {
    target: Target,
    policy: AnonymousScrollPolicy,
    processed: usize,
    ordinal: usize,
    pending: Option<Frame>,
    stack: Vec<Frame>,
    found: Option<(Rectangle, Vec<Frame>)>,
}

impl Find {
    fn new(target: Target, policy: AnonymousScrollPolicy, processed: usize) -> Self {
        Self {
            target,
            policy,
            processed,
            ordinal: 0,
            pending: None,
            stack: Vec::new(),
            found: None,
        }
    }

    fn record(&mut self, id: Option<&Id>, bounds: Rectangle, focused: bool) {
        if self.found.is_none()
            && match &self.target {
                Target::Named(target) => id == Some(target),
                Target::Focused => focused,
            }
        {
            self.found = Some((bounds, self.stack.clone()));
        }
    }
}

impl<T: Send + 'static> Operation<T> for Find {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        let pushed = self.pending.take();
        if let Some(frame) = pushed.as_ref() {
            self.stack.push(frame.clone());
        }
        operate(self);
        if pushed.is_some() {
            self.stack.pop();
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _: Size,
        translation: Vector,
        _: &mut dyn operation::Scrollable,
    ) {
        self.record(id, bounds, false);
        self.pending = Some(Frame {
            ordinal: self.ordinal,
            named: id.is_some(),
            bounds,
            translation,
        });
        self.ordinal += 1;
    }
    fn focusable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        self.record(id, bounds, state.is_focused());
    }
    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, _: &Rectangle) {
        self.record(id, bounds, false);
    }
    fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn operation::TextInput) {
        self.record(id, bounds, false);
    }
    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, _: &str) {
        self.record(id, bounds, false);
    }
    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Any) {
        self.record(id, bounds, false);
    }

    fn finish(&self) -> Outcome<T> {
        let Some((mut item, ancestors)) = self.found.clone() else {
            return Outcome::None;
        };
        let Some(index) = ancestors.len().checked_sub(self.processed + 1) else {
            return Outcome::None;
        };
        // Inner offsets may clamp or ignore a disabled axis. Read their actual state again,
        // then project only the portion visible through every intervening clip.
        for inner in ancestors[index + 1..].iter().rev() {
            item -= inner.translation;
            let Some(clipped) = item.intersection(&inner.bounds) else {
                return Outcome::None;
            };
            item = clipped;
        }
        let frame = &ancestors[index];
        let next = Find::new(self.target.clone(), self.policy, self.processed + 1);
        if self.policy == AnonymousScrollPolicy::NamedOnly && !frame.named {
            return Outcome::Chain(Box::new(next));
        }
        let offset = operation::scrollable::AbsoluteOffset {
            x: super::reveal_axis_offset(
                frame.translation.x,
                frame.bounds.width,
                item.x - frame.bounds.x,
                item.width,
            ),
            y: super::reveal_axis_offset(
                frame.translation.y,
                frame.bounds.height,
                item.y - frame.bounds.y,
                item.height,
            ),
        };
        if offset.x == frame.translation.x && offset.y == frame.translation.y {
            return Outcome::Chain(Box::new(next));
        }
        Outcome::Chain(Box::new(Apply {
            ordinal: frame.ordinal,
            current: 0,
            offset,
            next,
        }))
    }
}

// Runtime successors operate the same tree synchronously, so traversal ordinals
// can address anonymous state without retaining a mutable widget-state borrow.
struct Apply {
    ordinal: usize,
    current: usize,
    offset: operation::scrollable::AbsoluteOffset,
    next: Find,
}
impl<T: Send + 'static> Operation<T> for Apply {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }
    fn scrollable(
        &mut self,
        _: Option<&Id>,
        bounds: Rectangle,
        content: Size,
        _: Vector,
        state: &mut dyn operation::Scrollable,
    ) {
        if self.current == self.ordinal {
            state.scroll_to(
                self.offset.into(),
                operation::Animation::Instant,
                bounds,
                content,
            );
        }
        self.current += 1;
    }
    fn finish(&self) -> Outcome<T> {
        Outcome::Chain(Box::new(Find::new(
            self.next.target.clone(),
            self.next.policy,
            self.next.processed,
        )))
    }
}

/// Reveal a named target through every enclosing scroll viewport.
///
/// Anonymous ancestors are included. Missing and already-visible targets do not move.
/// Oversized targets retain the largest visible portion with minimum movement.
/// Scope the operation to its owning window with
/// [`crate::app_runtime::window_operation_task`].
#[must_use]
pub fn reveal_id_all<T: Send + 'static>(target: Id) -> Box<dyn Operation<T>> {
    reveal_id_all_with_policy(target, AnonymousScrollPolicy::Include)
}

/// Like [`reveal_id_all`], with an explicit anonymous-ancestor policy.
#[must_use]
pub fn reveal_id_all_with_policy<T: Send + 'static>(
    target: Id,
    policy: AnonymousScrollPolicy,
) -> Box<dyn Operation<T>> {
    Box::new(Find::new(Target::Named(target), policy, 0))
}

/// Reveal the first focused descendant through every enclosing scroll viewport.
#[must_use]
pub fn reveal_focused_all<T: Send + 'static>() -> Box<dyn Operation<T>> {
    reveal_focused_all_with_policy(AnonymousScrollPolicy::Include)
}

/// Like [`reveal_focused_all`], with an explicit anonymous-ancestor policy.
#[must_use]
pub fn reveal_focused_all_with_policy<T: Send + 'static>(
    policy: AnonymousScrollPolicy,
) -> Box<dyn Operation<T>> {
    Box::new(Find::new(Target::Focused, policy, 0))
}

// Focus operations visit focusable state only. Preserve every chained count/probe
// pass before revealing; `operation::then` alone skips operations finishing with None.
struct AfterFocus {
    operation: Box<dyn Operation<()>>,
}
impl<T: Send + 'static> Operation<T> for AfterFocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        self.operation
            .traverse(&mut |op| operate(&mut operation::black_box(op)));
    }
    fn focusable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        self.operation.focusable(id, bounds, state);
    }
    fn finish(&self) -> Outcome<T> {
        match self.operation.finish() {
            Outcome::Chain(next) => Outcome::Chain(Box::new(AfterFocus { operation: next })),
            Outcome::None | Outcome::Some(()) => Outcome::Chain(reveal_focused_all()),
        }
    }
}

pub(super) fn after_focus<T: Send + 'static>(
    operation: impl Operation<()> + 'static,
) -> Box<dyn Operation<T>> {
    Box::new(AfterFocus {
        operation: Box::new(operation),
    })
}

/// Focus an existing target, then reveal it through all enclosing viewports.
///
/// Missing targets preserve existing focus. Page-specific arrow navigation can
/// dispatch this through [`crate::app_runtime::window_operation_task`]; modal
/// hosts should additionally apply `operation::scope` to their presentation.
#[must_use]
pub fn focus_and_reveal_id<T: Send + 'static>(target: Id) -> Box<dyn Operation<T>> {
    struct FindTarget {
        target: Id,
        found: bool,
    }
    impl<T: Send + 'static> Operation<T> for FindTarget {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
            operate(self);
        }
        fn focusable(&mut self, id: Option<&Id>, _: Rectangle, _: &mut dyn operation::Focusable) {
            self.found |= id == Some(&self.target);
        }
        fn finish(&self) -> Outcome<T> {
            if self.found {
                Outcome::Chain(after_focus(operation::focusable::focus::<()>(
                    self.target.clone(),
                )))
            } else {
                Outcome::None
            }
        }
    }
    Box::new(FindTarget {
        target,
        found: false,
    })
}
