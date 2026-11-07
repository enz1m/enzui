//! Rich-text document, controller and native editing surface: a
//! formatting toolbar over an editable document with bold, italic,
//! underline, strike-through, code, colours, links, lists and paragraph
//! alignment, read from and written to HTML.
//!
//! Start with [`RichEditor`]: one value in the application state, shown with
//! [`RichEditor::view`] and updated with [`RichEditor::update`]. The parts
//! below it are for hosts that need more control.
//!
//! [`RichEditController`] owns text, per-byte formatting, paragraph attributes,
//! selection and bounded undo history. Apply native edit commands through
//! [`RichEditAction`]; native cursor and selection actions use
//! [`sync_selection_from_content`] and [`sync_content_cursor_from_controller`].
//! [`enz_rich_edit_builder`] composes the formatting toolbar and native input.
//! [`EnzRichEdit::show_toolbar`] separates the editor from a shared
//! [`enz_rich_edit_toolbar_builder`]. [`EnzRichEdit::editor_variant`] selects
//! transparent document chrome; [`EnzRichEdit::editor_tokens`] pins inks and padding.
//! [`EnzRichEdit::font`] selects a base family while inline bold, italic and code
//! keep their formatting. [`EnzRichEdit::text_size_px`] sets absolute logical
//! pixels; unpinned sizes follow desktop text scaling. Hosts persist these base
//! settings separately from the document's [`to_html`] / [`from_html`] output.
//! [`RichEditAction::SetFont`] and [`RichEditAction::SetTextSize`] apply selected
//! or pending-caret overrides. Authored runs persist inline; clearing overrides
//! restores base inheritance.
//! For immutable display models, use [`crate::rich_text`].

#![allow(
    clippy::string_slice,
    reason = "caret and paragraph offsets come from the document, on char boundaries"
)]

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::enz_text::TextTone;
use crate::rich_text::{RichSpan, RichText, SpanStyle};

mod document;
mod editor;
mod html;
mod paint;
mod toolbar;
mod widget;

pub(crate) use paint::{RichEditHighlighter, RichEditPaintSettings, format_for_rich_edit};

pub use document::*;
pub use editor::RichEditor;
pub use html::{FromHtmlError, from_html, rich_text_to_html, to_html};
pub use toolbar::{
    DEFAULT_TOOLBAR_ID_PREFIX, EnzRichEditToolbar, RichEditSwatch, RichEditToolbarDensity,
    enz_rich_edit_toolbar, enz_rich_edit_toolbar_builder, link_field_id,
};
pub use widget::{
    DEFAULT_EDITOR_HEIGHT, EnzRichEdit, RichEditAction, content_cursor_actions, enz_rich_edit,
    enz_rich_edit_builder, sync_content_cursor_from_controller, sync_selection_from_content,
};

/// A toggleable inline style. Kept minimal and renderer-neutral; maps onto
/// [`SpanStyle`] for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InlineStyle {
    /// Bold weight.
    Bold,
    /// Italic slant.
    Italic,
    /// Underline decoration.
    Underline,
    /// Strikethrough decoration.
    Strikethrough,
    /// Monospace inline code.
    Code,
}

impl InlineStyle {
    /// All styles in canonical (toolbar) order.
    pub const ALL: [InlineStyle; 5] = [
        InlineStyle::Bold,
        InlineStyle::Italic,
        InlineStyle::Underline,
        InlineStyle::Strikethrough,
        InlineStyle::Code,
    ];
}

/// Per-byte inline formatting, authored typography and semantic or authored inks.
///
/// Authored ink and pixel size carry floats, so equality is structural.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EditFormat {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    code: bool,
    tone: Option<TextTone>,
    color: Option<iced::Color>,
    font: Option<iced::font::Family>,
    text_size: Option<f32>,
    highlight: Option<iced::Color>,
    // Indices refer to the document's link table.
    link: Option<usize>,
}

impl EditFormat {
    fn has(self, style: InlineStyle) -> bool {
        match style {
            InlineStyle::Bold => self.bold,
            InlineStyle::Italic => self.italic,
            InlineStyle::Underline => self.underline,
            InlineStyle::Strikethrough => self.strikethrough,
            InlineStyle::Code => self.code,
        }
    }

    fn set(&mut self, style: InlineStyle, on: bool) {
        match style {
            InlineStyle::Bold => self.bold = on,
            InlineStyle::Italic => self.italic = on,
            InlineStyle::Underline => self.underline = on,
            InlineStyle::Strikethrough => self.strikethrough = on,
            InlineStyle::Code => self.code = on,
        }
    }

    // The style HTML writes: a link's underline and accent are how it is
    // painted, and the anchor itself says it is a link.
    pub(crate) fn to_written_style(self) -> SpanStyle {
        Self { link: None, ..self }.to_span_style()
    }

    fn to_span_style(self) -> SpanStyle {
        // A link reads as one: underlined, in the accent unless the author
        // inked it.
        let link = self.link.is_some();
        SpanStyle {
            bold: self.bold,
            italic: self.italic,
            code: self.code,
            underline: self.underline || link,
            strikethrough: self.strikethrough,
            tone: if link && self.tone.is_none() {
                Some(TextTone::Accent)
            } else {
                self.tone
            },
            color: self.color,
            highlight: self.highlight,
            font: self.font,
            text_size: self.text_size,
        }
    }

    /// Authored family, or `None` for base inheritance.
    pub fn font(self) -> Option<iced::font::Family> {
        self.font
    }

    /// Authored logical pixel size, or `None` for base inheritance.
    pub fn text_size(self) -> Option<f32> {
        self.text_size
    }

    /// The marker colour behind this format, if any.
    pub fn highlight(self) -> Option<iced::Color> {
        self.highlight
    }

    /// The authored ink carried by this format, if any.
    pub fn color(self) -> Option<iced::Color> {
        self.color
    }
}

// Whole-document snapshots keep text, formatting and selection in one undo transaction.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    text: String,
    formats: Vec<EditFormat>,
    // Restore block formatting with the text whose paragraphs it describes.
    paragraphs: Vec<ParagraphFormat>,
    selection: Range<usize>,
}

impl Snapshot {
    fn capture(doc: &RichEditDocument, selection: &Range<usize>) -> Self {
        Self {
            text: doc.text.clone(),
            formats: doc.formats.clone(),
            paragraphs: doc.paragraphs.clone(),
            selection: selection.clone(),
        }
    }

    /// Restores this snapshot and returns the state it replaced, moved out
    /// rather than copied.
    fn swap_into(self, doc: &mut RichEditDocument, selection: &mut Range<usize>) -> Snapshot {
        Snapshot {
            text: std::mem::replace(&mut doc.text, self.text),
            formats: std::mem::replace(&mut doc.formats, self.formats),
            paragraphs: std::mem::replace(&mut doc.paragraphs, self.paragraphs),
            selection: std::mem::replace(selection, self.selection),
        }
    }
}

/// What kind of edit just committed — used only to decide whether the *next*
/// edit may coalesce into the same undo unit.
///
/// Coalescing merges consecutive non-whitespace insertions typed contiguously
/// into one undo unit (per-keystroke undo is universally hated), and BREAKS on
/// any of:
///
///   * whitespace insertion (space / tab / newline — the natural word/line
///     boundary),
///   * any operation that is not an insertion (delete / backspace / style /
///     tone / clear-formatting / replace-over-selection),
///   * an external [`RichEditController::set_selection`] (a caret move breaks
///     the run — what a user thinks of as "a word" ends here),
///   * an insertion whose caret is not the exact continuation of the previous
///     insertion (the last run's end).
///
/// Deliberately no timeout: the pure model has no clock, and adding an
/// injected one is a separate design surface. The structural set above is what
/// most editors also rely on; a timeout would be a refinement, not a
/// replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum EditKind {
    /// The last committed action was an insert of non-whitespace at caret
    /// position `run_end`; a subsequent non-whitespace insert whose start is
    /// exactly `run_end` may coalesce into the same undo unit.
    ContinuingInsert { run_end: usize },
    /// Any other committed action — the next edit starts a fresh undo unit.
    /// This is the default so `RichEditController::default()` starts with no
    /// coalescing run in progress.
    #[default]
    Break,
}

/// A stateful editing controller: wraps a [`RichEditDocument`] and a selection,
/// tracks pending typing attributes — formatting toggled with an empty
/// selection that applies to the next inserted text (acceptance: "applying
/// formatting with no selection defines pending typing attributes") — and owns
/// the undo/redo history.
///
/// # Undo/redo model
///
/// Every mutation (`insert`, `backspace`, `delete_forward`, `set_style`,
/// `toggle_style`, `set_tone`, `clear_formatting`) commits atomically through
/// the internal `commit` path, which snapshots the mutation state and pushes it
/// onto the undo stack. `undo` restores the last snapshot and moves the previous
/// state onto the redo stack; `redo` is the mirror. Any non-undo/non-redo
/// mutation clears the redo stack (the standard "a new edit invalidates redo"
/// rule).
///
/// Consecutive inserts of non-whitespace at a contiguous caret coalesce into a
/// single undo unit — see `EditKind` for the exact break rules. This is why
/// `undo` returns to the *start* of the run rather than one keystroke back.
///
/// # History cap
///
/// The undo stack is bounded to [`Self::UNDO_LIMIT`] entries; the oldest is
/// dropped on overflow (an unbounded history is a memory leak). Redo is bounded
/// by the same value — it can never exceed the undo stack that produced it.
///
/// # What is restored
///
/// Text and selection and per-byte formats. A style-only change is its
/// own undo unit: undoing it leaves the text intact and restores the previous
/// formats. Restoring text without the caret was ruled out by design (a common
/// bug in editors that snapshot only the buffer).
#[derive(Debug, Clone, Default)]
pub struct RichEditController {
    doc: RichEditDocument,
    /// Current selection / caret (caret = empty range).
    selection: Range<usize>,
    /// Pending typing format, `Some` only while a caret-time toggle is armed.
    pending: Option<EditFormat>,
    /// Undo stack: compatibility mutation snapshots, newest at the back.
    undo_stack: std::collections::VecDeque<Snapshot>,
    /// Redo stack: post-undo snapshots, newest at the back.
    redo_stack: Vec<Snapshot>,
    /// Classification of the last committed action, driving coalescing.
    last_kind: EditKind,
    pub(crate) format_revision: u64,
    /// The paint projection of this revision, made on first use.
    paint: std::sync::OnceLock<paint::RichEditPaintSettings>,
    /// The toolbar popover open over the editor, if any.
    menu: Option<ToolbarMenu>,
    /// The URL being typed into the link popover.
    link_draft: String,
}

/// A toolbar popover the controller keeps open across frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarMenu {
    /// Text and highlight colours.
    Color,
    /// The link editor.
    Link,
}

impl RichEditController {
    /// Maximum number of undo (and redo) entries retained. Older entries are
    /// silently dropped on overflow; the operation itself is committed and
    /// cannot be recovered. Chosen for correctness-first rather than tuning:
    /// a real session rarely benefits from more, and each entry is O(document)
    /// in memory.
    pub const UNDO_LIMIT: usize = 200;

    /// New controller over `doc`, caret at the end.
    pub fn new(doc: RichEditDocument) -> Self {
        let end = doc.len();
        Self {
            doc,
            selection: end..end,
            pending: None,
            undo_stack: std::collections::VecDeque::new(),
            redo_stack: Vec::new(),
            last_kind: EditKind::Break,
            format_revision: 0,
            paint: std::sync::OnceLock::new(),
            menu: None,
            link_draft: String::new(),
        }
    }

    // ---- Undo/redo internals ----

    /// Push a compatibility mutation snapshot onto the undo stack, applying the history
    /// cap and clearing the redo stack (any new edit invalidates redo).
    ///
    /// This is the *only* place snapshots enter the history — every mutating
    /// public method routes through `commit_*`, so the invariant "every edit
    /// is either undoable or an explicit coalesce into the previous unit" is a
    /// property of this function's callers.
    fn push_snapshot(&mut self, snapshot: Snapshot) {
        self.redo_stack.clear();
        if self.undo_stack.len() >= Self::UNDO_LIMIT {
            // Drop the *oldest* entry: preserving the most recent history is
            // what a user expects.
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(snapshot);
    }

    /// Take a compatibility mutation snapshot and run `edit`. `edit` sets `self.last_kind`
    /// to the classification of the action it committed (see [`EditKind`]).
    ///
    /// This is the coalescing choke point: if the previous committed action
    /// was a `ContinuingInsert` whose `run_end` matches the current caret and
    /// the caller advertises that this action can extend the run, the
    /// compatibility snapshot is *dropped* (the earlier snapshot remains authoritative,
    /// so `undo` returns to the run's start). Otherwise a fresh snapshot is
    /// pushed.
    fn commit(&mut self, coalesce_with_prior_insert: bool, edit: impl FnOnce(&mut Self)) {
        let should_coalesce = coalesce_with_prior_insert
            && matches!(self.last_kind, EditKind::ContinuingInsert { .. });
        // A coalescing keystroke keeps the run's earlier snapshot, so it
        // copies nothing.
        let snapshot = (!should_coalesce).then(|| Snapshot::capture(&self.doc, &self.selection));
        edit(self);
        self.format_revision = self.format_revision.wrapping_add(1);
        self.paint = std::sync::OnceLock::new();
        if let Some(snapshot) = snapshot {
            self.push_snapshot(snapshot);
        }
    }

    /// Run `edit` as the rest of the unit the previous commit opened: no
    /// snapshot of its own, so one undo reverses both (a list Enter's break
    /// and the marker it adds).
    fn extend_last_commit(&mut self, edit: impl FnOnce(&mut Self)) {
        edit(self);
        self.format_revision = self.format_revision.wrapping_add(1);
        self.paint = std::sync::OnceLock::new();
    }

    /// The paint projection of the current formatting, kept until the next
    /// change, so a view rebuilt every frame does not rescan the document.
    pub(crate) fn paint(&self) -> paint::RichEditPaintSettings {
        self.paint
            .get_or_init(|| paint::RichEditPaintSettings::from_controller(self))
            .clone()
    }

    /// Undo the most recent unit. Returns whether anything was undone.
    ///
    /// Restores text, per-byte formats, and selection. Any pending typing
    /// attribute is cleared (its arm-point is gone). The next edit starts a
    /// fresh undo unit.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop_back() else {
            return false;
        };
        let current = prev.swap_into(&mut self.doc, &mut self.selection);
        self.redo_stack.push(current);
        self.pending = None;
        self.last_kind = EditKind::Break;
        self.format_revision = self.format_revision.wrapping_add(1);
        self.paint = std::sync::OnceLock::new();
        true
    }

    /// Redo the most recently undone unit. Returns whether anything was redone.
    ///
    /// A new edit between an `undo` and a `redo` clears the redo stack, so
    /// this only succeeds when the caller has issued only `undo`/`redo` since
    /// the last edit.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        let current = next.swap_into(&mut self.doc, &mut self.selection);
        self.undo_stack.push_back(current);
        self.pending = None;
        self.last_kind = EditKind::Break;
        self.format_revision = self.format_revision.wrapping_add(1);
        self.paint = std::sync::OnceLock::new();
        true
    }

    /// How many undo entries are currently on the stack. Public so tests can
    /// assert coalescing ("a run of typing produced one undo unit") — the
    /// only structural property visible from outside.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    /// How many redo entries are currently on the stack. See `undo_depth`.
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }

    /// The underlying document.
    pub fn document(&self) -> &RichEditDocument {
        &self.doc
    }

    /// The current selection (caret is an empty range).
    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    /// Move / set the selection. Clears any pending typing attribute if the
    /// selection becomes non-empty or the caret moves. An external caret move
    /// also breaks the current coalescing run, so the *next* typed character
    /// starts a fresh undo unit — what a user thinks of as "the end of a word".
    pub fn set_selection(&mut self, range: Range<usize>) {
        let range = self.doc.normalize(range);
        if range != self.selection {
            self.pending = None;
            self.last_kind = EditKind::Break;
        }
        self.selection = range;
    }

    /// The styles a toolbar should show as "on": the pending typing attribute
    /// if armed, else the styles active across the selection.
    pub fn active_styles(&self) -> Vec<InlineStyle> {
        if let Some(p) = self.pending {
            return InlineStyle::ALL.into_iter().filter(|&s| p.has(s)).collect();
        }
        self.doc.active_styles(self.selection.clone())
    }

    /// Toggle `style`. With a selection, toggles it across the selection (an
    /// undoable style-only unit — the text is unchanged but the formats are).
    /// With a caret (empty selection), arms/updates the pending typing
    /// attribute so the next insert carries it; arming pending is *not* an
    /// undoable unit — nothing has been committed to the document yet.
    pub fn toggle_style(&mut self, style: InlineStyle) {
        if self.selection.is_empty() {
            let mut base = self.pending.unwrap_or_else(|| {
                // Seed from the char to the left (what typing would inherit).
                self.selection
                    .start
                    .checked_sub(1)
                    .and_then(|i| self.doc.formats.get(i))
                    .copied()
                    .unwrap_or_default()
            });
            let now = !base.has(style);
            base.set(style, now);
            self.pending = Some(base);
            // A format chosen at the caret starts a new typing unit: undo
            // takes back the newly formatted text on its own.
            self.last_kind = EditKind::Break;
        } else {
            self.commit(false, |c| {
                c.doc.toggle_style(style, c.selection.clone());
                c.last_kind = EditKind::Break;
            });
        }
    }

    /// Apply or remove `style` across the selection. A style-only undo unit
    /// (no text change). No-op with an empty selection (matches
    /// [`RichEditDocument::set_style`]) and *not* undoable in that case.
    pub fn set_style(&mut self, style: InlineStyle, on: bool) {
        if self.selection.is_empty() {
            return;
        }
        self.commit(false, |c| {
            c.doc.set_style(style, on, c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// Set (or clear with `None`) the semantic tone across the selection. A
    /// style-only undo unit. No-op on an empty selection.
    pub fn set_tone(&mut self, tone: Option<TextTone>) {
        if self.selection.is_empty() {
            return;
        }
        self.commit(false, |c| {
            c.doc.set_tone(tone, c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// Set (or clear with `None`) the authored ink across the selection.
    ///
    /// With a caret rather than a selection this arms the pending typing
    /// attribute, so text typed next carries the ink. Arming commits nothing
    /// and is not an undo unit; applying to a real selection is a style-only
    /// undo unit.
    pub fn set_color(&mut self, color: Option<iced::Color>) {
        self.menu = None;
        if self.selection.is_empty() {
            let mut base = self.pending.unwrap_or_else(|| {
                self.selection
                    .start
                    .checked_sub(1)
                    .and_then(|i| self.doc.formats.get(i))
                    .copied()
                    .unwrap_or_default()
            });
            base.color = color;
            self.pending = Some(base);
            // A format chosen at the caret starts a new typing unit: undo
            // takes back the newly formatted text on its own.
            self.last_kind = EditKind::Break;
            return;
        }
        self.commit(false, |c| {
            c.doc.set_color(color, c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// Set the selected family, or inherit the editor family with `None`.
    ///
    /// At a caret this arms typing attributes without creating an undo entry.
    pub fn set_font(&mut self, font: Option<iced::font::Family>) {
        self.set_typography(|format| format.font = font);
    }

    /// Set selected text size in logical pixels, or inherit with `None`.
    ///
    /// Nonfinite or nonpositive sizes are ignored. At a caret this arms typing
    /// attributes without creating an undo entry.
    pub fn set_text_size(&mut self, size: Option<f32>) {
        if size.is_some_and(|size| !size.is_finite() || size <= 0.0) {
            return;
        }
        self.set_typography(|format| format.text_size = size);
    }

    fn set_typography(&mut self, update: impl Fn(&mut EditFormat)) {
        if self.selection.is_empty() {
            let mut base = self.pending.unwrap_or_else(|| {
                self.selection
                    .start
                    .checked_sub(1)
                    .and_then(|index| self.doc.formats.get(index))
                    .copied()
                    .unwrap_or_default()
            });
            update(&mut base);
            self.pending = Some(base);
            // A format chosen at the caret starts a new typing unit: undo
            // takes back the newly formatted text on its own.
            self.last_kind = EditKind::Break;
        } else {
            self.commit(false, |controller| {
                let range = controller.doc.normalize(controller.selection.clone());
                for format in &mut controller.doc.formats[range] {
                    update(format);
                }
                controller.last_kind = EditKind::Break;
            });
        }
    }

    /// Family shared by the selection or pending typing attributes.
    /// Mixed families and inherited defaults return `None`.
    pub fn active_font(&self) -> Option<iced::font::Family> {
        self.active_typography().and_then(|format| format.font)
    }

    /// Logical pixel size shared by the selection or pending typing attributes.
    /// Mixed sizes and inherited defaults return `None`.
    pub fn active_text_size(&self) -> Option<f32> {
        self.active_typography().and_then(|format| format.text_size)
    }

    fn active_typography(&self) -> Option<EditFormat> {
        if let Some(pending) = self.pending {
            return Some(pending);
        }
        let range = self.doc.normalize(self.selection.clone());
        if range.is_empty() {
            return range
                .start
                .checked_sub(1)
                .and_then(|index| self.doc.formats.get(index))
                .copied();
        }
        let first = *self.doc.formats.get(range.start)?;
        let mut common = first;
        for format in &self.doc.formats[range] {
            if format.font != first.font {
                common.font = None;
            }
            if format.text_size != first.text_size {
                common.text_size = None;
            }
        }
        Some(common)
    }

    /// The ink a colour control should show as chosen: the pending typing
    /// attribute when armed, else the ink shared by the whole selection.
    ///
    /// A selection spanning two inks reports `None`, the same answer as an
    /// uninked selection: neither has an ink the next edit would keep.
    pub fn active_color(&self) -> Option<iced::Color> {
        if let Some(pending) = self.pending {
            return pending.color;
        }
        self.doc.active_color(self.selection.clone())
    }

    /// Align every paragraph the selection touches. A block-only undo unit:
    /// the text and the per-byte formats are unchanged.
    ///
    /// Applies with a caret too — a caret sits in exactly one paragraph, which
    /// is how alignment is normally applied — so there is no pending
    /// counterpart to arm, unlike an inline style toggled with no selection.
    pub fn set_paragraph_align(&mut self, align: ParagraphAlign) {
        self.commit(false, |c| {
            c.doc.set_paragraph_align(align, c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// The alignment an alignment control should show as chosen: the one every
    /// paragraph the selection touches shares, or `None` when they disagree.
    pub fn active_paragraph_align(&self) -> Option<ParagraphAlign> {
        self.doc.active_paragraph_align(self.selection.clone())
    }

    /// The toolbar popover currently open, if any.
    pub fn menu(&self) -> Option<ToolbarMenu> {
        self.menu
    }

    /// Open `menu`, or close it when it is already open. Opening the link
    /// editor seeds it with the link under the selection.
    pub fn toggle_menu(&mut self, menu: ToolbarMenu) {
        if self.menu == Some(menu) {
            self.menu = None;
            return;
        }
        if menu == ToolbarMenu::Link {
            self.link_draft = self.active_link().unwrap_or_default().to_owned();
        }
        self.menu = Some(menu);
    }

    /// Close any open toolbar popover.
    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    /// The URL typed into the link popover so far.
    pub fn link_draft(&self) -> &str {
        &self.link_draft
    }

    /// Replace the URL typed into the link popover.
    pub fn set_link_draft(&mut self, draft: String) {
        self.link_draft = draft;
    }

    /// Set (or clear with `None`) the marker colour across the selection, or
    /// arm it for the next typed text at a caret. Closes the colour popover.
    pub fn set_highlight(&mut self, color: Option<iced::Color>) {
        self.menu = None;
        if self.selection.is_empty() {
            let mut base = self.pending.unwrap_or_else(|| {
                self.selection
                    .start
                    .checked_sub(1)
                    .and_then(|i| self.doc.formats.get(i))
                    .copied()
                    .unwrap_or_default()
            });
            base.highlight = color;
            self.pending = Some(base);
            // A format chosen at the caret starts a new typing unit: undo
            // takes back the newly formatted text on its own.
            self.last_kind = EditKind::Break;
            return;
        }
        self.commit(false, |c| {
            c.doc.set_highlight(color, c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// The marker colour a highlight control should show as chosen.
    pub fn active_highlight(&self) -> Option<iced::Color> {
        if let Some(pending) = self.pending {
            return pending.highlight;
        }
        self.doc.active_highlight(self.selection.clone())
    }

    /// The link target under the selection's start, if any.
    pub fn active_link(&self) -> Option<&str> {
        let range = self.doc.link_range_at(self.selection.start)?;
        self.doc.link_at(range.start)
    }

    /// Link the selection to the drafted URL and close the link popover.
    ///
    /// A caret inside a link retargets that link; a caret elsewhere inserts
    /// the URL itself as linked text. A scheme-less address gets `https://`.
    /// An empty draft removes the link instead.
    pub fn apply_link_draft(&mut self) {
        self.menu = None;
        let draft = self.link_draft.trim().to_owned();
        if draft.is_empty() {
            self.remove_link();
            return;
        }
        let url = if draft.contains("://") || draft.starts_with("mailto:") {
            draft
        } else if draft.contains('@') && !draft.contains('/') {
            format!("mailto:{draft}")
        } else {
            format!("https://{draft}")
        };
        let target = if self.selection.is_empty() {
            self.doc.link_range_at(self.selection.start)
        } else {
            Some(self.selection.clone())
        };
        self.commit(false, |c| {
            match target {
                Some(range) => c.doc.set_link(Some(&url), range),
                None => {
                    let at = c.selection.start;
                    let end = c.doc.insert_with(at, &url, EditFormat::default());
                    c.doc.set_link(Some(&url), at..end);
                    c.selection = end..end;
                }
            }
            c.last_kind = EditKind::Break;
        });
    }

    /// Remove the link under the selection (or the caret) and close the link
    /// popover.
    pub fn remove_link(&mut self) {
        self.menu = None;
        let target = if self.selection.is_empty() {
            self.doc.link_range_at(self.selection.start)
        } else {
            Some(self.selection.clone())
        };
        let Some(range) = target else {
            return;
        };
        self.commit(false, |c| {
            c.doc.set_link(None, range);
            c.last_kind = EditKind::Break;
        });
    }

    /// The list kind every paragraph the selection touches shares, if any.
    pub fn active_list(&self) -> Option<ListKind> {
        self.doc.active_list(self.selection.clone())
    }

    /// Make the paragraphs the selection touches a `kind` list, or plain
    /// paragraphs again when they already are one. One undo unit.
    pub fn toggle_list(&mut self, kind: ListKind) {
        self.commit(false, |c| {
            c.selection = c.doc.toggle_list(kind, c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// Break the paragraph at the caret, continuing a list.
    ///
    /// In a list item the new paragraph starts with the next marker; Enter on
    /// an empty item ends the list there instead, as word processors do.
    pub fn enter(&mut self) {
        let index = self.doc.paragraph_index_at(self.selection.start);
        let Some((kind, marker_len)) = self.doc.list_item(index) else {
            self.insert("\n");
            return;
        };
        let body = self.doc.paragraph_ranges()[index].clone();
        if self.selection.is_empty() && body.len() == marker_len {
            self.commit(false, |c| {
                c.doc.delete(body.clone());
                c.doc.paragraphs[index].list = None;
                c.selection = body.start..body.start;
                c.last_kind = EditKind::Break;
            });
            return;
        }
        let number = ListKind::number_in(&self.doc.text()[body]).map_or(1, |n| n + 1);
        // A break never coalesces, so this opens the unit the marker joins.
        self.insert("\n");
        self.extend_last_commit(|c| {
            let at = c.selection.start;
            let end = c
                .doc
                .insert_with(at, &kind.marker(number), EditFormat::default());
            c.selection = end..end;
            c.last_kind = EditKind::Break;
        });
    }

    /// Remove all inline styles and tone across the selection. A style-only
    /// undo unit. No-op on an empty selection.
    pub fn clear_formatting(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.commit(false, |c| {
            c.doc.clear_formatting(c.selection.clone());
            c.last_kind = EditKind::Break;
        });
    }

    /// Type `text` at the caret / over the selection. Uses the pending typing
    /// attribute when armed, otherwise the normal left-inheriting insert. The
    /// selection collapses to a caret just past the inserted text and the
    /// pending attribute is consumed.
    ///
    /// Undoable. Coalesces with the previous insert when the caret is exactly
    /// where the previous run ended, no selection was replaced, and neither the
    /// previous nor the current input contained whitespace — the standard
    /// word-run rule.
    pub fn insert(&mut self, text: &str) {
        // Empty inputs neither mutate nor start a new undo unit; skip.
        if text.is_empty() {
            return;
        }
        let sel = self.doc.normalize(self.selection.clone());
        let selection_was_empty = sel.is_empty();
        // A run continues only when: (a) no selection was replaced, (b) the
        // caret is exactly at the previous run's end (which is the same as
        // "the previous run's end matches the current selection start"), and
        // (c) the incoming text contains no whitespace (space / tab / newline
        // at the natural word/line boundary). We look at (b) *before* the edit
        // because after it, the caret has moved.
        let contains_break_char = text.chars().any(char::is_whitespace);
        let may_coalesce = selection_was_empty
            && !contains_break_char
            && match self.last_kind {
                EditKind::ContinuingInsert { run_end } => run_end == sel.start,
                EditKind::Break => false,
            };

        // Capture the format at the selection start *before* deleting, so that
        // typing over a selection keeps the style of what it replaced — the
        // same rule `RichEditDocument::replace` follows. Without this, a
        // selection at offset 0 (a bold first word, say) would delete its
        // formats and then `insert` at `at == 0`, which has no left neighbour
        // and falls back to the default, unstyled format. A plain caret
        // insert (empty selection) still inherits from the left as before.
        let replaced_format = if sel.is_empty() {
            None
        } else {
            self.doc.formats.get(sel.start).copied()
        };

        self.commit(may_coalesce, |c| {
            let start = c.doc.delete(c.selection.clone());
            // Precedence: an armed pending typing attribute always wins over
            // the captured replaced format.
            let end = if let Some(format) = c.pending.take() {
                c.doc.insert_with(start, text, format)
            } else if let Some(format) = replaced_format {
                c.doc.insert_with(start, text, format)
            } else {
                c.doc.insert(start, text)
            };
            c.selection = end..end;
            // A run continues only if this insert was a pure typing action:
            // no selection replaced, no whitespace, no pending format burn.
            c.last_kind = if selection_was_empty && !contains_break_char {
                EditKind::ContinuingInsert { run_end: end }
            } else {
                EditKind::Break
            };
        });
    }

    /// Delete the selection, or the grapheme before the caret (backspace) when
    /// the selection is empty. Undoable; always breaks the current coalescing
    /// run.
    pub fn backspace(&mut self) {
        // No-op backspace at buffer start with no selection: do not commit
        // (nothing changed) and do not break the run — a spurious event that
        // hit an empty buffer must not throw away word-in-progress coalescing.
        if self.selection.is_empty() && self.selection.start == 0 {
            return;
        }
        self.commit(false, |c| {
            if c.selection.is_empty() {
                let caret = c.selection.start;
                // Delete a whole grapheme cluster, not a single code point: a
                // user pressing Backspace expects one *visible* character to
                // vanish. A regional-indicator flag, a ZWJ family emoji, or a
                // letter plus a combining mark are each one grapheme spanning
                // several code points; removing only the last code point
                // leaves visible garbage (a half-flag, a re-attached combining
                // accent). `grapheme_indices` with `is_extended = true` gives
                // extended grapheme clusters, whose last boundary at or before
                // `caret` is the start of the cluster ending at the caret.
                let prev = c.doc.text[..caret]
                    .grapheme_indices(true)
                    .next_back()
                    .map_or(0, |(offset, _)| offset);
                let at = c.doc.delete(prev..caret);
                c.selection = at..at;
            } else {
                let at = c.doc.delete(c.selection.clone());
                c.selection = at..at;
            }
            c.pending = None;
            c.last_kind = EditKind::Break;
        });
    }

    /// Deletes the selection, or the extended grapheme after the caret.
    ///
    /// Returns whether text was removed. A caret at the end of the document is
    /// a no-op and leaves both history stacks unchanged.
    pub fn delete_forward(&mut self) -> bool {
        let range = if self.selection.is_empty() {
            let caret = self.selection.start;
            let Some(grapheme) = self.doc.text[caret..].graphemes(true).next() else {
                return false;
            };
            caret..caret + grapheme.len()
        } else {
            self.selection.clone()
        };

        self.commit(false, |c| {
            let at = c.doc.delete(range);
            c.selection = at..at;
            c.pending = None;
            c.last_kind = EditKind::Break;
        });
        true
    }
}
