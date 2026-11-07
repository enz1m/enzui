//! Bounded undo and redo for an owned annotation document.
//!
//! Each committed annotation is one edit; previews and cancelled gestures are
//! absent. New edits clear redo, and exceeding the configured depth evicts the
//! oldest retained steps.

use std::collections::VecDeque;

use super::{Annotation, AnnotationId};

/// Default number of undoable steps retained per document.
pub const DEFAULT_HISTORY_DEPTH: usize = 128;

/// Reversible annotation edit retaining its original paint-order index.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationEdit {
    /// Addition of an annotation at a paint-order index.
    Added {
        /// Paint-order index of the addition.
        index: usize,
        /// Added annotation retained for redo.
        annotation: Box<Annotation>,
    },
    /// Removal of an annotation from a paint-order index.
    Removed {
        /// Original paint-order index of the removal.
        index: usize,
        /// Removed annotation retained for undo.
        annotation: Box<Annotation>,
    },
}

impl AnnotationEdit {
    /// The annotation this edit is about.
    #[must_use]
    pub fn annotation(&self) -> &Annotation {
        match self {
            Self::Added { annotation, .. } | Self::Removed { annotation, .. } => annotation,
        }
    }

    /// The document index this edit is about.
    #[must_use]
    pub fn index(&self) -> usize {
        match self {
            Self::Added { index, .. } | Self::Removed { index, .. } => *index,
        }
    }

    fn inverse(&self) -> Self {
        match self {
            Self::Added { index, annotation } => Self::Removed {
                index: *index,
                annotation: annotation.clone(),
            },
            Self::Removed { index, annotation } => Self::Added {
                index: *index,
                annotation: annotation.clone(),
            },
        }
    }
}

/// Result of one undo or redo, identifying the moved annotation and its final presence state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryStep {
    /// The annotation is present after undoing a removal or redoing an addition.
    Present(AnnotationId),
    /// The annotation is absent after undoing an addition or redoing a removal.
    Absent(AnnotationId),
}

impl HistoryStep {
    /// The annotation this step moved.
    #[must_use]
    pub fn id(&self) -> &AnnotationId {
        match self {
            Self::Present(id) | Self::Absent(id) => id,
        }
    }

    /// Whether the annotation is in the document after this step.
    #[must_use]
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }
}

/// Owns an annotation document and its bounded undo and redo history.
#[derive(Debug, Clone)]
pub struct AnnotationHistory {
    annotations: Vec<Annotation>,
    // Applied edits in chronological order, with the oldest at the front for eviction.
    undo: VecDeque<AnnotationEdit>,
    // Undone edits with the next redo last; its size cannot exceed the configured depth.
    redo: Vec<AnnotationEdit>,
    depth: usize,
}

impl Default for AnnotationHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl AnnotationHistory {
    /// An empty document with a [`DEFAULT_HISTORY_DEPTH`] history.
    #[must_use]
    pub fn new() -> Self {
        Self {
            annotations: Vec::new(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            depth: DEFAULT_HISTORY_DEPTH,
        }
    }

    /// Creates a document from existing annotations with no undoable edits.
    #[must_use]
    pub fn with_annotations(annotations: Vec<Annotation>) -> Self {
        Self {
            annotations,
            ..Self::new()
        }
    }

    /// Sets the retained undo depth.
    ///
    /// Zero disables retention without preventing edits from being applied.
    #[must_use]
    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self.evict();
        self
    }

    /// The document, in paint order.
    #[must_use]
    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    /// How many annotations the document holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.annotations.len()
    }

    /// Whether the document is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.annotations.is_empty()
    }

    /// Returns whether the document currently contains `id`.
    #[must_use]
    pub fn contains(&self, id: &AnnotationId) -> bool {
        self.annotations.iter().any(|a| a.id() == id)
    }

    /// Returns the retained-step limit.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Returns the number of currently undoable steps, never exceeding `depth`.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// How many steps can currently be redone.
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Whether there is anything to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Iterates retained undoable edits from oldest to newest.
    #[must_use = "consume the iterator to inspect undoable edits"]
    pub fn undoable(&self) -> impl DoubleEndedIterator<Item = &AnnotationEdit> {
        self.undo.iter()
    }

    /// Appends a committed annotation and records it as one undoable edit.
    pub fn commit(&mut self, annotation: Annotation) {
        let index = self.annotations.len();
        let edit = AnnotationEdit::Added {
            index,
            annotation: Box::new(annotation.clone()),
        };
        self.annotations.push(annotation);
        self.record(edit);
    }

    /// Removes `id` and records one undoable edit.
    ///
    /// Returns `false` and records nothing when `id` is absent.
    pub fn remove(&mut self, id: &AnnotationId) -> bool {
        let Some(index) = self.annotations.iter().position(|a| a.id() == id) else {
            return false;
        };
        let annotation = self.annotations.remove(index);
        self.record(AnnotationEdit::Removed {
            index,
            annotation: Box::new(annotation),
        });
        true
    }

    /// Reverses the newest retained edit, or returns `None` when none is available.
    pub fn undo(&mut self) -> Option<HistoryStep> {
        let edit = self.undo.pop_back()?;
        let step = self.apply(&edit.inverse());
        self.redo.push(edit);
        Some(step)
    }

    /// Reapplies the most recently undone edit, or returns `None` after redo is exhausted or invalidated.
    pub fn redo(&mut self) -> Option<HistoryStep> {
        let edit = self.redo.pop()?;
        let step = self.apply(&edit);
        // Push directly to undo so redoing one step does not clear the remaining redo stack.
        self.push_undo(edit);
        Some(step)
    }

    // An addition with a stale index appends instead of panicking.
    fn apply(&mut self, edit: &AnnotationEdit) -> HistoryStep {
        match edit {
            AnnotationEdit::Added { index, annotation } => {
                let at = (*index).min(self.annotations.len());
                self.annotations.insert(at, (**annotation).clone());
                HistoryStep::Present(annotation.id().clone())
            }
            AnnotationEdit::Removed { index, annotation } => {
                let id = annotation.id().clone();
                // Remove one matching annotation, preferring its recorded index, so duplicate ids cannot make one edit delete multiple shapes.
                let at = if self.annotations.get(*index).is_some_and(|a| a.id() == &id) {
                    Some(*index)
                } else {
                    self.annotations.iter().position(|a| a.id() == &id)
                };
                if let Some(at) = at {
                    self.annotations.remove(at);
                }
                HistoryStep::Absent(id)
            }
        }
    }

    // Every new edit clears redo here before entering the undo stack.
    fn record(&mut self, edit: AnnotationEdit) {
        self.redo.clear();
        self.push_undo(edit);
    }

    fn push_undo(&mut self, edit: AnnotationEdit) {
        self.undo.push_back(edit);
        self.evict();
    }

    fn evict(&mut self) {
        while self.undo.len() > self.depth {
            self.undo.pop_front();
        }
        // Bound redo as well because each retained redo entry can return to the undo stack.
        while self.redo.len() > self.depth {
            self.redo.remove(0);
        }
    }
}
