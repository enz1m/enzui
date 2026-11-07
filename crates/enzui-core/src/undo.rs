//! A generic undo/redo stack: edits that know how to apply and revert
//! themselves, merging of small steps, groups, and a clean (saved) state.
//!
//! An [`Edit`] changes some document `T`. [`UndoStack::push`] applies it and
//! records it; [`UndoStack::undo`] and [`UndoStack::redo`] walk the history.
//! Typing a word should be one step, not one per letter: an edit that
//! [`Edit::merge`]s into the previous one extends it until
//! [`UndoStack::seal`] (call it after a pause, a cursor jump or a save).
//! [`UndoStack::begin_group`] / [`UndoStack::end_group`] make several edits
//! one step, such as "Replace all". [`UndoStack::set_clean`] marks the saved
//! state; [`UndoStack::is_clean`] says whether the document matches it, for
//! the title's modified dot and the close guard.
//!
//! ```
//! use enzui_core::undo::{Edit, UndoStack};
//!
//! #[derive(Debug)]
//! struct Insert { at: usize, text: String }
//!
//! impl Edit<String> for Insert {
//!     fn apply(&self, doc: &mut String) { doc.insert_str(self.at, &self.text); }
//!     fn revert(&self, doc: &mut String) {
//!         doc.replace_range(self.at..self.at + self.text.len(), "");
//!     }
//!     fn merge(&mut self, next: &Self) -> bool {
//!         let joins = next.at == self.at + self.text.len();
//!         if joins { self.text.push_str(&next.text); }
//!         joins
//!     }
//!     fn label(&self) -> String { "Typing".into() }
//! }
//!
//! let mut doc = String::new();
//! let mut history = UndoStack::new();
//! history.push(Insert { at: 0, text: "a".into() }, &mut doc);
//! history.push(Insert { at: 1, text: "b".into() }, &mut doc);
//! assert_eq!(doc, "ab");
//! history.undo(&mut doc);
//! assert_eq!(doc, "", "the two letters were one step");
//! ```

/// A change to a document of type `T` that can be applied and reverted.
pub trait Edit<T> {
    /// Makes the change.
    fn apply(&self, target: &mut T);

    /// Undoes the change, exactly as [`Self::apply`] made it.
    fn revert(&self, target: &mut T);

    /// Absorbs `next`, which was just applied after this edit, so both undo
    /// as one step; `false` keeps them separate.
    fn merge(&mut self, _next: &Self) -> bool {
        false
    }

    /// The name of the step for "Undo …" and "Redo …" menu items.
    fn label(&self) -> String;
}

/// One undoable step: a single edit or a group.
#[derive(Debug)]
enum Step<E> {
    One(E),
    Group { label: String, edits: Vec<E> },
}

impl<E> Step<E> {
    fn apply<T>(&self, target: &mut T)
    where
        E: Edit<T>,
    {
        match self {
            Self::One(edit) => edit.apply(target),
            Self::Group { edits, .. } => edits.iter().for_each(|edit| edit.apply(target)),
        }
    }

    fn revert<T>(&self, target: &mut T)
    where
        E: Edit<T>,
    {
        match self {
            Self::One(edit) => edit.revert(target),
            Self::Group { edits, .. } => edits.iter().rev().for_each(|edit| edit.revert(target)),
        }
    }

    fn label<T>(&self) -> String
    where
        E: Edit<T>,
    {
        match self {
            Self::One(edit) => edit.label(),
            Self::Group { label, .. } => label.clone(),
        }
    }
}

/// The history of a document of type `T`, made of edits `E`. See the
/// [module](self) documentation.
#[derive(Debug)]
pub struct UndoStack<T, E> {
    steps: std::collections::VecDeque<Step<E>>,
    /// How many steps are applied: `steps[..index]` are done, the rest redo.
    index: usize,
    /// The `index` the document was saved at; `None` once that state left
    /// the history.
    clean: Option<usize>,
    /// The top step may take merges.
    open: bool,
    /// Edits of the group being built.
    group: Option<(String, Vec<E>)>,
    limit: usize,
    document: std::marker::PhantomData<fn(&mut T)>,
}

impl<T, E: Edit<T>> Default for UndoStack<T, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, E: Edit<T>> UndoStack<T, E> {
    /// An empty, clean history keeping up to 1000 steps.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(1000)
    }

    /// An empty, clean history keeping up to `limit` steps (at least one);
    /// older steps are dropped.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            steps: std::collections::VecDeque::new(),
            index: 0,
            clean: Some(0),
            open: false,
            group: None,
            limit: limit.max(1),
            document: std::marker::PhantomData,
        }
    }

    /// Applies `edit` to `target` and records it, dropping anything that
    /// could be redone. Inside a group it joins the group; otherwise it may
    /// [`Edit::merge`] into the previous step unless that was sealed.
    pub fn push(&mut self, edit: E, target: &mut T) {
        edit.apply(target);
        if let Some((_, edits)) = &mut self.group {
            edits.push(edit);
            return;
        }
        self.steps.truncate(self.index);
        if self.clean.is_some_and(|clean| clean > self.index) {
            self.clean = None;
        }
        // Undo, redo and `set_clean` seal the top step, so a merge never
        // reaches into the saved state.
        if self.open
            && let Some(Step::One(top)) = self.steps.back_mut()
            && top.merge(&edit)
        {
            return;
        }
        self.record(Step::One(edit));
    }

    fn record(&mut self, step: Step<E>) {
        self.steps.push_back(step);
        self.index = self.steps.len();
        self.open = true;
        if self.steps.len() > self.limit {
            self.steps.pop_front();
            self.index -= 1;
            self.clean = match self.clean {
                Some(0) | None => None,
                Some(clean) => Some(clean - 1),
            };
        }
    }

    /// Stops the top step taking merges: the next edit starts a new step.
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// Starts collecting edits into one step named `label`. Groups do not
    /// nest: a second call before [`Self::end_group`] is ignored.
    pub fn begin_group(&mut self, label: impl Into<String>) {
        if self.group.is_none() {
            self.group = Some((label.into(), Vec::new()));
        }
    }

    /// Records the group as one step; an empty group records nothing.
    pub fn end_group(&mut self) {
        let Some((label, edits)) = self.group.take() else {
            return;
        };
        if edits.is_empty() {
            return;
        }
        self.steps.truncate(self.index);
        if self.clean.is_some_and(|clean| clean > self.index) {
            self.clean = None;
        }
        self.record(Step::Group { label, edits });
        self.open = false;
    }

    /// Reverts the last step; `false` when there is none.
    pub fn undo(&mut self, target: &mut T) -> bool {
        self.end_group();
        if self.index == 0 {
            return false;
        }
        self.index -= 1;
        self.steps[self.index].revert(target);
        self.open = false;
        true
    }

    /// Applies the next undone step again; `false` when there is none.
    pub fn redo(&mut self, target: &mut T) -> bool {
        if self.group.is_some() || self.index == self.steps.len() {
            return false;
        }
        self.steps[self.index].apply(target);
        self.index += 1;
        self.open = false;
        true
    }

    /// Whether [`Self::undo`] would do something.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.index > 0
            || self
                .group
                .as_ref()
                .is_some_and(|(_, edits)| !edits.is_empty())
    }

    /// Whether [`Self::redo`] would do something.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.group.is_none() && self.index < self.steps.len()
    }

    /// The name of the step [`Self::undo`] reverts.
    #[must_use]
    pub fn undo_label(&self) -> Option<String> {
        self.index
            .checked_sub(1)
            .map(|index| self.steps[index].label::<T>())
    }

    /// The name of the step [`Self::redo`] applies.
    #[must_use]
    pub fn redo_label(&self) -> Option<String> {
        self.steps.get(self.index).map(Step::label::<T>)
    }

    /// Marks the current state as saved, and seals the top step so later
    /// typing does not merge into the saved state.
    pub fn set_clean(&mut self) {
        self.end_group();
        self.clean = Some(self.index);
        self.open = false;
    }

    /// Whether the document is as it was when last marked clean.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.group.is_none() && self.clean == Some(self.index)
    }

    /// Forgets the history; the current state becomes clean.
    pub fn clear(&mut self) {
        *self = Self::with_limit(self.limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Changes a number: an addition (consecutive additions of the same sign
    /// merge) or setting it, which does not commute.
    #[derive(Debug)]
    enum Op {
        Add(i32),
        Set { from: i32, to: i32 },
    }

    #[allow(non_snake_case, reason = "reads like the variant it builds")]
    fn Add(amount: i32) -> Op {
        Op::Add(amount)
    }

    impl Edit<i32> for Op {
        fn apply(&self, target: &mut i32) {
            match self {
                Op::Add(amount) => *target += amount,
                Op::Set { to, .. } => *target = *to,
            }
        }
        fn revert(&self, target: &mut i32) {
            match self {
                Op::Add(amount) => *target -= amount,
                Op::Set { from, .. } => *target = *from,
            }
        }
        fn merge(&mut self, next: &Self) -> bool {
            match (self, next) {
                (Op::Add(amount), Op::Add(more)) if (*amount > 0) == (*more > 0) => {
                    *amount += more;
                    true
                }
                _ => false,
            }
        }
        fn label(&self) -> String {
            match self {
                Op::Add(amount) => format!("Add {amount}"),
                Op::Set { to, .. } => format!("Set {to}"),
            }
        }
    }

    #[test]
    fn history_preserves_groups_clean_points_redo_and_capacity_1() {
        // steps_undo_and_redo_in_order
        {
            let mut value = 0;
            let mut history = UndoStack::new();
            history.push(Add(5), &mut value);
            history.seal();
            history.push(Add(-2), &mut value);
            assert_eq!(value, 3);
            assert_eq!(history.undo_label().as_deref(), Some("Add -2"));
            assert!(history.undo(&mut value));
            assert_eq!(value, 5);
            assert_eq!(history.redo_label().as_deref(), Some("Add -2"));
            assert!(history.undo(&mut value));
            assert!(!history.undo(&mut value), "nothing left");
            assert_eq!(value, 0);
            assert!(history.redo(&mut value) && history.redo(&mut value));
            assert!(!history.redo(&mut value));
            assert_eq!(value, 3);
        }

        // small_steps_merge_until_sealed
        {
            let mut value = 0;
            let mut history = UndoStack::new();
            for _ in 0..3 {
                history.push(Add(1), &mut value);
            }
            history.push(Add(-1), &mut value);
            assert_eq!(history.undo_label().as_deref(), Some("Add -1"));
            history.undo(&mut value);
            assert_eq!(history.undo_label().as_deref(), Some("Add 3"));
            history.push(Add(1), &mut value);
            assert_eq!(
                history.undo_label().as_deref(),
                Some("Add 1"),
                "no merging into a step after an undo"
            );
            history.seal();
            history.push(Add(1), &mut value);
            history.undo(&mut value);
            assert_eq!(value, 4, "a sealed step does not take merges");
        }

        // a_new_edit_drops_the_redo_branch
        {
            let mut value = 0;
            let mut history = UndoStack::new();
            history.push(Add(1), &mut value);
            history.seal();
            history.push(Add(10), &mut value);
            history.undo(&mut value);
            history.push(Add(-5), &mut value);
            assert!(!history.can_redo());
            assert_eq!(value, -4);
            history.undo(&mut value);
            assert_eq!(value, 1);
            history.undo(&mut value);
            assert_eq!(value, 0, "the dropped step is never reverted");
        }

        // a_group_is_one_step
        {
            let mut value = 0;
            let mut history = UndoStack::new();
            history.begin_group("Renumber");
            history.push(Op::Set { from: 0, to: 1 }, &mut value);
            history.push(Op::Set { from: 1, to: 2 }, &mut value);
            history.push(Add(5), &mut value);
            history.end_group();
            assert_eq!(value, 7);
            assert_eq!(history.undo_label().as_deref(), Some("Renumber"));
            history.undo(&mut value);
            assert_eq!(value, 0, "the whole group reverted, last edit first");
            history.redo(&mut value);
            assert_eq!(value, 7);
        }
    }

    #[test]
    fn history_preserves_groups_clean_points_redo_and_capacity_2() {
        // the_clean_state_follows_undo_and_redo
        {
            let mut value = 0;
            let mut history = UndoStack::new();
            assert!(history.is_clean());
            history.push(Add(1), &mut value);
            assert!(!history.is_clean());
            history.set_clean();
            history.push(Add(1), &mut value);
            assert!(!history.is_clean());
            assert_eq!(
                history.undo_label().as_deref(),
                Some("Add 1"),
                "no merge into the saved state"
            );
            history.undo(&mut value);
            assert!(history.is_clean(), "back at the saved state");
            history.undo(&mut value);
            history.push(Add(5), &mut value);
            assert!(!history.is_clean());
            history.undo(&mut value);
            history.redo(&mut value);
            assert!(!history.is_clean(), "the saved state left the history");
        }

        // the_oldest_steps_fall_off_the_limit
        {
            let mut value = 0;
            let mut history = UndoStack::with_limit(2);
            history.set_clean();
            for step in [1, -1, 1] {
                history.push(Add(step), &mut value);
            }
            assert!(history.undo(&mut value) && history.undo(&mut value));
            assert!(!history.undo(&mut value), "only two steps are kept");
            assert_eq!(value, 1);
            assert!(!history.is_clean(), "the saved state fell off");
        }
    }
}
