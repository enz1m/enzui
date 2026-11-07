//! Read-only selection across paragraph text.
//!
//! Positions use character offsets. A release in another paragraph, or at least one character from the press position, produces a drag rather than a link-activating click.

use std::fmt;

/// Maximum character travel that still counts as a click.
pub const DRAG_ACTIVATION_SLOP: usize = 0;

/// A paragraph index and character offset within a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct DocPosition {
    /// Index of the paragraph within the document.
    pub paragraph: usize,
    /// Character offset within that paragraph.
    pub offset: usize,
}

impl DocPosition {
    /// A position at `offset` characters into `paragraph`.
    pub fn new(paragraph: usize, offset: usize) -> Self {
        Self { paragraph, offset }
    }
}

impl fmt::Display for DocPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.paragraph, self.offset)
    }
}

/// Selection endpoints in gesture order; use [`Selection::range`] for document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Where the gesture began.
    pub anchor: DocPosition,
    /// Where the cursor is now.
    pub focus: DocPosition,
}

impl Selection {
    /// A caret (empty selection) at `at`.
    pub fn caret(at: DocPosition) -> Self {
        Self {
            anchor: at,
            focus: at,
        }
    }

    /// The selection's endpoints in document order, `(start, end)`.
    pub fn range(&self) -> (DocPosition, DocPosition) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// Whether the selection covers no characters.
    ///
    /// An empty selection is a caret, not a highlight: nothing is painted and
    /// [`SelectionModel::selected_text`] returns an empty string.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

/// What a completed press/release gesture meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureOutcome {
    /// The release is within activation slop of the press, so a link under the pointer may be activated.
    Click,
    /// A selection drag that must not activate a link under the pointer.
    Drag,
}

/// Read-only selection state over owned paragraph text.
#[derive(Debug, Clone, Default)]
pub struct SelectionModel {
    paragraphs: Vec<String>,
    selection: Option<Selection>,
    dragging: bool,
}

impl SelectionModel {
    /// A model over the given paragraph texts, with nothing selected.
    pub fn new(paragraphs: impl IntoIterator<Item = String>) -> Self {
        Self {
            paragraphs: paragraphs.into_iter().collect(),
            selection: None,
            dragging: false,
        }
    }

    /// The number of paragraphs the model spans.
    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    /// The current selection, if any.
    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// Whether a pointer drag is currently in progress.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    fn paragraph_len(&self, paragraph: usize) -> usize {
        self.paragraphs
            .get(paragraph)
            .map_or(0, |p| p.chars().count())
    }

    fn clamp(&self, pos: DocPosition) -> DocPosition {
        if self.paragraphs.is_empty() {
            return DocPosition::new(0, 0);
        }
        let paragraph = pos.paragraph.min(self.paragraphs.len() - 1);
        let offset = pos.offset.min(self.paragraph_len(paragraph));
        DocPosition::new(paragraph, offset)
    }

    /// Begin a selection at `at` (pointer pressed).
    pub fn press(&mut self, at: DocPosition) {
        let at = self.clamp(at);
        self.selection = Some(Selection::caret(at));
        self.dragging = true;
    }

    /// Extend the in-progress selection to `to` (pointer moved while down).
    ///
    /// A move with no press in progress is ignored rather than starting a
    /// selection: a hover across the pane must not paint a highlight.
    pub fn drag_to(&mut self, to: DocPosition) {
        if !self.dragging {
            return;
        }
        let to = self.clamp(to);
        if let Some(sel) = self.selection.as_mut() {
            sel.focus = to;
        }
    }

    /// Finishes the gesture and returns whether it was a click or drag.
    ///
    /// A release without an active press returns [`GestureOutcome::Click`] without changing selection.
    pub fn release(&mut self) -> GestureOutcome {
        if !self.dragging {
            return GestureOutcome::Click;
        }
        self.dragging = false;
        let Some(sel) = self.selection else {
            return GestureOutcome::Click;
        };
        let (start, end) = sel.range();
        let travelled = if start.paragraph != end.paragraph {
            // Crossing a paragraph boundary is unambiguously a drag.
            usize::MAX
        } else {
            end.offset - start.offset
        };
        if travelled > DRAG_ACTIVATION_SLOP {
            GestureOutcome::Drag
        } else {
            self.selection = Some(Selection::caret(start));
            GestureOutcome::Click
        }
    }

    /// Select the word under `at`, as a completed pointer gesture.
    ///
    /// Letters, numbers and `_` form a word; whitespace forms one contiguous
    /// run; punctuation selects its contiguous identical run. The position is
    /// clamped like every other model entry point, and an end-of-paragraph hit
    /// resolves to the final character instead of producing an empty range.
    pub fn select_word_at(&mut self, at: DocPosition) {
        let at = self.clamp(at);
        let Some(text) = self.paragraphs.get(at.paragraph) else {
            return;
        };
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            self.selection = Some(Selection::caret(at));
            self.dragging = false;
            return;
        }
        let index = at.offset.min(chars.len() - 1);
        let class = |ch: char| {
            if ch.is_alphanumeric() || ch == '_' {
                (0u8, '\0')
            } else if ch.is_whitespace() {
                (1u8, '\0')
            } else {
                (2u8, ch)
            }
        };
        let selected = class(chars[index]);
        let mut start = index;
        while start > 0 && class(chars[start - 1]) == selected {
            start -= 1;
        }
        let mut end = index + 1;
        while end < chars.len() && class(chars[end]) == selected {
            end += 1;
        }
        self.selection = Some(Selection {
            anchor: DocPosition::new(at.paragraph, start),
            focus: DocPosition::new(at.paragraph, end),
        });
        self.dragging = false;
    }

    /// Select the entire document (Ctrl+A).
    ///
    /// Selects from the start of the first paragraph to the end of the last.
    /// On an empty document this is a no-op, so Ctrl+A cannot produce a
    /// selection that [`selected_text`] would have to special-case.
    ///
    /// [`selected_text`]: SelectionModel::selected_text
    pub fn select_all(&mut self) {
        if self.paragraphs.is_empty() {
            return;
        }
        let last = self.paragraphs.len() - 1;
        self.selection = Some(Selection {
            anchor: DocPosition::new(0, 0),
            focus: DocPosition::new(last, self.paragraph_len(last)),
        });
        self.dragging = false;
    }

    /// Drop the selection (Escape, or a click landing outside the prose).
    pub fn clear(&mut self) {
        self.selection = None;
        self.dragging = false;
    }

    /// The text covered by the current selection.
    ///
    /// Each paragraph break the selection crosses is a `\n`, so copying across
    /// paragraphs yields prose that pastes as prose rather than as one run-on
    /// line, and selected empty paragraphs survive.
    /// Returns an empty string when there is no selection or the selection is a
    /// caret — an empty highlight must copy nothing rather than the whole
    /// document.
    pub fn selected_text(&self) -> String {
        let Some(sel) = self.selection else {
            return String::new();
        };
        if sel.is_empty() {
            return String::new();
        }
        let (start, end) = sel.range();
        let mut out = String::new();
        for p in start.paragraph..=end.paragraph {
            let Some(text) = self.paragraphs.get(p) else {
                continue;
            };
            // Slice by `char`, never by byte: `offset` is a char index and the
            // prose is routinely non-ASCII.
            let from = if p == start.paragraph {
                start.offset
            } else {
                0
            };
            let to = if p == end.paragraph {
                end.offset
            } else {
                text.chars().count()
            };
            // Every paragraph break the selection crosses is part of it, even
            // around an empty paragraph or a selection of the break alone.
            if p > start.paragraph {
                out.push('\n');
            }
            out.extend(text.chars().skip(from).take(to.saturating_sub(from)));
        }
        out
    }

    /// Whether a copy request should produce clipboard content.
    ///
    /// Distinct from `!selected_text().is_empty()` so a caller can ask the
    /// question without building the string.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| !s.is_empty())
    }

    /// Returns paragraph indices contributing at least one character to the selection.
    ///
    /// A terminal paragraph at offset zero is excluded, matching [`Self::selected_text`].
    pub fn selected_paragraphs(&self) -> Vec<usize> {
        let Some(sel) = self.selection else {
            return Vec::new();
        };
        if sel.is_empty() {
            return Vec::new();
        }
        let (start, end) = sel.range();
        let mut out = Vec::new();
        for p in start.paragraph..=end.paragraph {
            let Some(text) = self.paragraphs.get(p) else {
                continue;
            };
            let from = if p == start.paragraph {
                start.offset
            } else {
                0
            };
            let to = if p == end.paragraph {
                end.offset
            } else {
                text.chars().count()
            };
            if to.saturating_sub(from) > 0 {
                out.push(p);
            }
        }
        out
    }
}
