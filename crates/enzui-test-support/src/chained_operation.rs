//! Follow `Outcome::Chain` exactly as the real iced runtime does.
//!
//! Many widget operations — `focus_next`, `focus_previous`, scoped initial
//! focus — are chained: the first pass counts or locates targets; the
//! `Outcome::Chain` pass is what actually mutates widget state. A test
//! harness that calls `ui.operate` once and stops measures only the counting
//! pass and reports focus behaviour that never happened.
//!
//! The real runtime (`iced/test/src/emulator.rs`) loops over each
//! successor returned by `finish`. [`operate_chained`] replicates that loop
//! with a bounded chain-depth guard. [`operate_chained_counted`] additionally
//! records deterministic traversal work per pass, so a regression can
//! distinguish a chain being replayed from one expensive tree pass.

use iced_core::widget::operation::{
    Focusable, Outcome, Scrollable, SelectableTextLayout, TextInput,
};
use iced_core::widget::{Id, Operation};
use iced_core::{Rectangle, Size, Vector};
use std::any::Any;

/// Maximum number of operation passes the chain-depth guard will perform.
///
/// Real focus operations normally chain at most twice (count → act). Reaching
/// this configured limit does not prove a cycle: a cycle, an unbounded
/// producer, or an unexpectedly deep finite composition may be responsible.
pub const MAX_CHAIN_DEPTH: usize = 128;

/// Deterministic widget-operation work observed during one pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PassWork {
    /// Calls to [`Operation::traverse`]. This is the useful tree-walk measure:
    /// broad/deep trees increase it even when they expose no typed state.
    pub traversals: usize,
    /// Calls to [`Operation::container`].
    pub containers: usize,
    /// Calls to [`Operation::scrollable`].
    pub scrollables: usize,
    /// Calls to [`Operation::focusable`].
    pub focusables: usize,
    /// Calls to [`Operation::text_input`].
    pub text_inputs: usize,
    /// Calls to [`Operation::text`].
    pub texts: usize,
    /// Calls to [`Operation::selectable_text`].
    pub selectable_texts: usize,
    /// Calls to [`Operation::custom`].
    pub custom: usize,
}

impl PassWork {
    /// All typed callbacks, excluding traversal decisions.
    #[must_use]
    pub const fn typed_callbacks(self) -> usize {
        self.containers
            + self.scrollables
            + self.focusables
            + self.text_inputs
            + self.texts
            + self.selectable_texts
            + self.custom
    }
}

/// Per-pass evidence produced by [`operate_chained_counted`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationReport {
    /// The work of each pass, in order.
    pub passes: Vec<PassWork>,
}

impl OperationReport {
    /// How many passes ran.
    #[must_use]
    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    /// Traversals summed over every pass.
    #[must_use]
    pub fn total_traversals(&self) -> usize {
        self.passes.iter().map(|pass| pass.traversals).sum()
    }

    /// Typed callbacks summed over every pass.
    #[must_use]
    pub fn total_typed_callbacks(&self) -> usize {
        self.passes.iter().map(|pass| pass.typed_callbacks()).sum()
    }
}

/// A transparent operation adapter that counts the work a real widget tree
/// asks of the wrapped operation. Successors are deliberately not wrapped in
/// `finish`: the outer runtime loop wraps each successor once, immediately
/// before its own pass. This makes accidental rewrapping visible as extra
/// passes instead of hiding it inside the counter.
struct Counted<'a, T> {
    inner: &'a mut dyn Operation<T>,
    work: &'a mut PassWork,
}

impl<T> Operation<T> for Counted<'_, T> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        self.work.traversals += 1;
        let work = &mut *self.work;
        self.inner.traverse(&mut |inner| {
            let mut counted = Counted { inner, work };
            operate(&mut counted);
        });
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, viewport: &Rectangle) {
        self.work.containers += 1;
        self.inner.container(id, bounds, viewport);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Size,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        self.work.scrollables += 1;
        self.inner
            .scrollable(id, bounds, content_bounds, translation, state);
    }

    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        self.work.focusables += 1;
        self.inner.focusable(id, bounds, state);
    }

    fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
        self.work.text_inputs += 1;
        self.inner.text_input(id, bounds, state);
    }

    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, text: &str) {
        self.work.texts += 1;
        self.inner.text(id, bounds, text);
    }

    fn selectable_text(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        text: &str,
        layout: &dyn SelectableTextLayout,
    ) {
        self.work.selectable_texts += 1;
        self.inner.selectable_text(id, bounds, text, layout);
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Any) {
        self.work.custom += 1;
        self.inner.custom(id, bounds, state);
    }
}

/// Run a widget operation and follow every [`Outcome::Chain`] exactly as the
/// real runtime does.
///
/// Returns the number of passes performed (always ≥ 1). Use
/// [`operate_chained_counted`] when a regression needs deterministic per-pass
/// traversal evidence.
///
/// # Panics
///
/// Panics before operating a successor that would exceed
/// [`MAX_CHAIN_DEPTH`] passes.
pub fn operate_chained<T>(
    operate: impl FnMut(&mut dyn Operation<T>),
    operation: &mut dyn Operation<T>,
) -> usize {
    operate_chained_counted(operate, operation).pass_count()
}

/// Run a widget operation to completion and report deterministic work for
/// every pass.
///
/// The callback receives a transparent counting adapter. It should pass that
/// adapter directly to `UserInterface::operate`; retaining it is impossible by
/// construction. The original operation is finished exactly once per pass.
///
/// # Panics
///
/// Panics before operating a successor that would exceed
/// [`MAX_CHAIN_DEPTH`] passes. The diagnostic distinguishes completed passes
/// from the next pass that would have run.
pub fn operate_chained_counted<T>(
    mut operate: impl FnMut(&mut dyn Operation<T>),
    operation: &mut dyn Operation<T>,
) -> OperationReport {
    fn run_pass<T>(
        operate: &mut impl FnMut(&mut dyn Operation<T>),
        operation: &mut dyn Operation<T>,
    ) -> PassWork {
        let mut work = PassWork::default();
        operate(&mut Counted {
            inner: operation,
            work: &mut work,
        });
        work
    }

    let mut report = OperationReport {
        passes: vec![run_pass(&mut operate, operation)],
    };
    let mut outcome = operation.finish();

    loop {
        match outcome {
            Outcome::Chain(mut next) => {
                let passes = report.pass_count();
                let attempted_pass = passes + 1;
                assert!(
                    passes < MAX_CHAIN_DEPTH,
                    "operate_chained: operated {passes} passes and produced another successor; \
                     operating pass {attempted_pass} would exceed the configured chain depth \
                     limit of {MAX_CHAIN_DEPTH}. A cycle or unbounded producer is possible"
                );
                report.passes.push(run_pass(&mut operate, &mut *next));
                outcome = next.finish();
            }
            Outcome::None | Outcome::Some(_) => return report,
        }
    }
}
