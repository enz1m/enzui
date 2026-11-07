//! Formatting toolbar and native rich-text input driven by one controller.
//!
//! Apply edit commands to [`RichEditController`], rebuild the borrowed native
//! content from its document, and restore the cursor with
//! [`sync_content_cursor_from_controller`]. Forward `Native` cursor/selection
//! actions to the content and call [`sync_selection_from_content`]. The
//! controller owns undo history and formatting; the native content supplies
//! input and shaped geometry.
//!
//! [`EnzRichEdit`] configures editor height and base typography independently
//! of inline formatting. Hosts persist base font and size alongside the
//! document; HTML conversion preserves document formatting, not builder settings.

#![allow(
    clippy::string_slice,
    reason = "caret offsets come from the document, on char boundaries"
)]

use super::document::{ListKind, ParagraphAlign};
use super::toolbar::{RichEditSwatch, RichEditToolbarDensity, enz_rich_edit_toolbar_builder};
use super::{InlineStyle, RichEditController, ToolbarMenu};
use crate::text_area::{AreaHeight, TextAreaTokens, TextAreaVariant, enz_text_area};
use enzui_theme::{EnzTheme, FontFamily, FontRole};
use iced::Element;
use iced::widget::column;
use iced::widget::text_editor::{Action, Content, Cursor, Edit, Motion};

/// An action to apply to a [`RichEditController`].
///
/// These are produced by [`enz_rich_edit`] and the caller forwards them
/// to their controller + content in their update function. This is the
/// message type that bridges the native `text_editor::Action` to the
/// single-owner controller.
#[derive(Debug, Clone)]
pub enum RichEditAction {
    /// Insert a single character at the caret / over the selection.
    Insert(char),
    /// Paste text at the caret / over the selection.
    Paste(String),
    /// Insert a newline.
    Enter,
    /// Delete the previous grapheme (Backspace).
    Backspace,
    /// Delete the selection or the next extended grapheme (Delete key).
    Delete,
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Toggle an [`InlineStyle`] (from the toolbar).
    ToggleStyle(InlineStyle),
    /// Align every paragraph the selection touches (from the toolbar).
    ///
    /// A block action, not an inline one: it carries no pending counterpart
    /// because a caret already sits in exactly one paragraph.
    SetParagraphAlign(ParagraphAlign),
    /// Open a toolbar popover, or close it when open.
    ToggleMenu(ToolbarMenu),
    /// Close any open toolbar popover.
    CloseMenu,
    /// Set (or clear) the text colour; closes the colour popover.
    SetColor(Option<iced::Color>),
    /// Set (or clear) the highlight colour; closes the colour popover.
    SetHighlight(Option<iced::Color>),
    /// Set the selected or pending family; `None` restores inheritance.
    SetFont(Option<iced::font::Family>),
    /// Set selected or pending logical pixel size; `None` restores inheritance.
    SetTextSize(Option<f32>),
    /// Replace the URL typed into the link popover.
    EditLink(String),
    /// Link the selection to the typed URL.
    ApplyLink,
    /// Remove the link under the selection.
    RemoveLink,
    /// Make the touched paragraphs a list of this kind, or plain again.
    ToggleList(ListKind),
    /// Remove inline formatting from the selection.
    ClearFormatting,
    /// A non-edit action (cursor movement, click, drag, scroll, select)
    /// that should be forwarded to `Content::perform` so the native
    /// editor tracks cursor state. The caller must then read the cursor
    /// position back from the content and sync it to the controller.
    Native(Action),
}

impl RichEditAction {
    /// Whether the host should hand keyboard focus back to the editor after
    /// applying this action.
    ///
    /// A toolbar click takes focus from the editor, so without this the next
    /// keystroke would go to the button (a space re-pressing it) instead of
    /// the text. The link popover is the exception: it needs its URL field.
    pub fn returns_to_editor(&self) -> bool {
        match self {
            RichEditAction::ToggleStyle(_)
            | RichEditAction::SetParagraphAlign(_)
            | RichEditAction::SetColor(_)
            | RichEditAction::SetHighlight(_)
            | RichEditAction::SetFont(_)
            | RichEditAction::SetTextSize(_)
            | RichEditAction::ApplyLink
            | RichEditAction::RemoveLink
            | RichEditAction::ToggleList(_)
            | RichEditAction::ClearFormatting
            | RichEditAction::CloseMenu
            | RichEditAction::ToggleMenu(ToolbarMenu::Color) => true,
            RichEditAction::ToggleMenu(ToolbarMenu::Link)
            | RichEditAction::EditLink(_)
            | RichEditAction::Insert(_)
            | RichEditAction::Paste(_)
            | RichEditAction::Enter
            | RichEditAction::Backspace
            | RichEditAction::Delete
            | RichEditAction::Undo
            | RichEditAction::Redo
            | RichEditAction::Native(_) => false,
        }
    }

    /// Apply this action to a [`RichEditController`].
    ///
    /// Returns `true` if the action changed the document's text or formatting,
    /// so the caller knows to bring the `Content` up to date.
    pub fn apply(self, controller: &mut RichEditController) -> bool {
        match self {
            RichEditAction::Insert(ch) => {
                let mut buf = [0u8; 4];
                controller.insert(ch.encode_utf8(&mut buf));
                true
            }
            RichEditAction::Paste(text) => {
                controller.insert(&text);
                true
            }
            RichEditAction::Enter => {
                controller.enter();
                true
            }
            RichEditAction::Backspace => {
                controller.backspace();
                true
            }
            RichEditAction::Delete => controller.delete_forward(),
            RichEditAction::Undo => {
                controller.undo();
                true
            }
            RichEditAction::Redo => {
                controller.redo();
                true
            }
            RichEditAction::ToggleStyle(style) => {
                controller.toggle_style(style);
                true
            }
            RichEditAction::SetParagraphAlign(align) => {
                controller.set_paragraph_align(align);
                true
            }
            RichEditAction::ToggleMenu(menu) => {
                controller.toggle_menu(menu);
                false
            }
            RichEditAction::CloseMenu => {
                controller.close_menu();
                false
            }
            RichEditAction::SetColor(color) => {
                controller.set_color(color);
                true
            }
            RichEditAction::SetHighlight(color) => {
                controller.set_highlight(color);
                true
            }
            RichEditAction::SetFont(font) => {
                controller.set_font(font);
                true
            }
            RichEditAction::SetTextSize(size) => {
                controller.set_text_size(size);
                true
            }
            RichEditAction::EditLink(draft) => {
                controller.set_link_draft(draft);
                false
            }
            RichEditAction::ApplyLink => {
                controller.apply_link_draft();
                true
            }
            RichEditAction::RemoveLink => {
                controller.remove_link();
                true
            }
            RichEditAction::ToggleList(kind) => {
                controller.toggle_list(kind);
                true
            }
            RichEditAction::ClearFormatting => {
                controller.clear_formatting();
                true
            }
            RichEditAction::Native(_) => {
                // Native actions (cursor moves, clicks) are handled by
                // the caller forwarding to Content::perform. The caller
                // then syncs the cursor position to the controller.
                false
            }
        }
    }

    /// Convert a `text_editor::Action` into a `RichEditAction`.
    ///
    /// Edit actions are translated to controller commands; non-edit
    /// actions are wrapped as `Native` for the caller to forward.
    pub fn from_editor_action(action: Action) -> Self {
        match action {
            Action::Edit(edit) => match edit {
                Edit::Insert(ch) => RichEditAction::Insert(ch),
                Edit::Paste(text) => RichEditAction::Paste((*text).clone()),
                Edit::Enter => RichEditAction::Enter,
                Edit::Backspace => RichEditAction::Backspace,
                Edit::Delete => RichEditAction::Delete,
                Edit::Undo => RichEditAction::Undo,
                Edit::Redo => RichEditAction::Redo,
                // Indent/Unindent: treat as tab/shift-tab insertion for now.
                Edit::Indent => RichEditAction::Insert('\t'),
                Edit::Unindent => RichEditAction::Native(Action::Edit(Edit::Unindent)),
                // Range batches are a plain-text editor operation. Rich edit
                // has no producer for them; preserve the native action rather
                // than losing it or pretending its formatted document changed.
                Edit::Replace(batch) => RichEditAction::Native(Action::Edit(Edit::Replace(batch))),
                // The controller has no word or line boundaries yet, and a
                // native deletion would leave it out of step with the content.
                // Delete one grapheme, as these bindings did before iced
                // distinguished them.
                Edit::BackspaceWord | Edit::BackspaceLine => RichEditAction::Backspace,
                Edit::DeleteWord | Edit::DeleteLine => RichEditAction::Delete,
            },
            other => RichEditAction::Native(other),
        }
    }
}

/// Sync the controller's selection from the native `Content`'s cursor.
///
/// The caller should invoke this after forwarding a `Native` action to
/// `Content::perform`, so the controller's selection tracks the native
/// cursor.
///
/// Converts the line/column `Cursor` into a byte offset in the
/// controller's document text. The caller should call this after
/// forwarding a `Native` action to `Content::perform`.
pub fn sync_selection_from_content<R: iced::advanced::text::Renderer>(
    controller: &mut RichEditController,
    content: &Content<R>,
) {
    let cursor = content.cursor();
    let text = controller.document().text();

    let start = position_to_byte_offset(text, cursor.position);
    let end = cursor
        .selection
        .map_or(start, |sel| position_to_byte_offset(text, sel));

    // Ensure ascending order.
    let (a, b) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    controller.set_selection(a..b);
}

/// Restore the native editor cursor and selection from the controller.
///
/// Call this after rebuilding [`Content`] from the controller document. It
/// changes only native cursor metadata; document text and history remain owned
/// by [`RichEditController`].
pub fn sync_content_cursor_from_controller<R: iced::advanced::text::Renderer>(
    controller: &RichEditController,
    content: &mut Content<R>,
) {
    let text = controller.document().text();
    let selection = controller.selection();
    let start = byte_offset_to_position(text, selection.start);
    let end = byte_offset_to_position(text, selection.end);
    content.move_to(Cursor {
        position: end,
        selection: (start != end).then_some(start),
    });
}

/// Convert a line/column `Position` to a byte offset in `text`.
///
/// Reached via the public [`sync_selection_from_content`], so it needs no
/// `allow(dead_code)` annotation.
///
/// # Column semantics
///
/// Native `Position::column` is a byte index within the line, not a
/// char count. iced pipes `Position::column` straight through into
/// cosmic-text's `Cursor::index`, which cosmic-text documents as
/// "first-byte-index of glyph at cursor" — see
/// `iced/graphics/src/text/editor.rs` (`cursor()` and `move_to`).
///
/// The previous implementation treated `column` as a char index (via
/// `char_indices().nth(pos.column)`), which agreed with native only when
/// every code-point in the line was one byte long or the caret happened
/// to land past the last char (falling back to `line.len()`). On text
/// with combining marks or multi-byte characters it produced the wrong
/// byte offset by exactly `(byte_count - char_count)` of the prefix, and
/// the same error surfaced in the round-trip through
/// [`sync_selection_from_content`].
fn position_to_byte_offset(text: &str, pos: iced::widget::text_editor::Position) -> usize {
    // Lines end where the document's (and the native editor's) breaks do:
    // LF, CR, CRLF or LFCR.
    let Some(body) = super::document::line_body_ranges(text)
        .into_iter()
        .nth(pos.line)
    else {
        // Past the last line: clamp to end.
        return text.len();
    };
    // The column is a byte index (cosmic-text's convention); clamp it and snap
    // down to a char boundary.
    let line = &text[body.clone()];
    let mut col = pos.index.min(line.len());
    while col > 0 && !line.is_char_boundary(col) {
        col -= 1;
    }
    body.start + col
}

/// Convert a byte offset in `text` to a line/column `Position`.
///
/// The inverse of [`position_to_byte_offset`]; used by
/// [`content_cursor_actions`] to translate the controller's byte-range
/// selection back into a native cursor location. An offset inside a line
/// break is the end of the line before it.
///
/// Returns a `Position` whose `column` is a **byte index within the
/// line** (see [`position_to_byte_offset`] for the rationale).
fn byte_offset_to_position(text: &str, byte_offset: usize) -> iced::widget::text_editor::Position {
    let offset = byte_offset.min(text.len());
    let bodies = super::document::line_body_ranges(text);
    let line = bodies
        .iter()
        .position(|body| offset <= body.end)
        .unwrap_or(bodies.len() - 1);
    // Column is a BYTE index within the line, not a char count. This is
    // the unit cosmic-text (and hence iced's Position) uses natively —
    // see the doc comment on `position_to_byte_offset`.
    let index = offset.clamp(bodies[line].start, bodies[line].end) - bodies[line].start;
    iced::widget::text_editor::Position { line, index }
}

/// Native `text_editor` [`Action`]s that move a *freshly rebuilt* [`Content`]
/// caret to the controller's current selection cursor .
///
/// After a mutating [`RichEditAction`] the caller rebuilds the native
/// [`Content`] from the controller's text
/// (`Content::with_text(controller.document().text())`). A rebuilt `Content`
/// places its caret at the end of the buffer, so the visible cursor jumps
/// away from where the edit happened — most glaringly after `Undo`/`Redo` or a
/// toolbar style toggle, where the controller's selection is restored but the
/// native caret is not. iced's [`Action`] has no "move to line/column", only
/// relative [`Motion`]s, so this returns the deterministic motion sequence
/// (`DocumentStart`, then `Down`×line, then `Right`×column) that lands the
/// caret on the controller's selection end. Feed each to
/// `Content::perform` after the rebuild.
///
/// This is the inverse of [`sync_selection_from_content`]: that pulls the
/// native cursor *into* the controller after a `Native` action; this pushes the
/// controller's cursor *back into* a rebuilt `Content`.
pub fn content_cursor_actions(controller: &RichEditController) -> Vec<Action> {
    use unicode_segmentation::UnicodeSegmentation;

    let text = controller.document().text();
    // The caret is the selection's end (where typing continues).
    let caret = controller.selection().end.min(text.len());
    let pos = byte_offset_to_position(text, caret);

    // Find the target line's text so we can count graphemes in its
    // prefix. `byte_offset_to_position` returns `pos.column` as a BYTE
    // index within the line; Motion::Right moves by GRAPHEME
    // CLUSTER, so the number of Rights required is the grapheme count of
    // the line-prefix `[0 .. pos.column]`, not `pos.column` itself.
    //
    // Counting chars instead would overshoot by one per combining mark and
    // by four per ZWJ family emoji (see tests); only counting graphemes in
    // the byte prefix produces the correct number of Rights.
    let line_text = text.split('\n').nth(pos.line).unwrap_or("");
    let byte_col = pos.index.min(line_text.len());
    let grapheme_rights = line_text[..byte_col].graphemes(true).count();

    let mut actions = Vec::with_capacity(1 + pos.line + grapheme_rights);
    // Collapse any native selection and anchor at the very start.
    actions.push(Action::Move(Motion::DocumentStart));
    for _ in 0..pos.line {
        actions.push(Action::Move(Motion::Down));
    }
    // At the target line, `Down` lands at column 0 (or clamps to the
    // line's end on a shorter final line); step right to the target byte
    // column by advancing one grapheme cluster at a time.
    for _ in 0..grapheme_rights {
        actions.push(Action::Move(Motion::Right));
    }
    actions
}

/// Requested default editor height in logical pixels; native row rounding applies.
/// Use [`EnzRichEdit::editor_height`] to choose a different viewport policy.
pub const DEFAULT_EDITOR_HEIGHT: f32 = 200.0;

/// Build a rich-text editing surface.
///
/// Returns an `Element` containing a formatting toolbar and a text area with
/// the editor requesting [`DEFAULT_EDITOR_HEIGHT`]. The `on_action` callback
/// receives [`RichEditAction`]s that the caller must apply to their
/// `RichEditController` and then rebuild their `Content`.
///
/// For a composable editor height (e.g. fill the remaining dialog space) use
/// [`enz_rich_edit_builder`].
///
/// # Example
///
/// ```ignore
/// enz_rich_edit(&self.controller, &self.content, Msg::RichEdit)
/// ```
pub fn enz_rich_edit<'a, Message: Clone + 'a>(
    controller: &'a RichEditController,
    content: &'a Content,
    on_action: impl Fn(RichEditAction) -> Message + 'a,
) -> Element<'a, Message> {
    enz_rich_edit_builder(controller, content, on_action).into()
}

/// Configure a rich-text editor with a toolbar, viewport policy and base typography.
///
/// Same toolbar + text-area composite as [`enz_rich_edit`], but the editor
/// height can be set with [`EnzRichEdit::editor_height`]. Defaults to
/// [`AreaHeight::Fixed`] at [`DEFAULT_EDITOR_HEIGHT`], so
/// `enz_rich_edit_builder(..).into()` is identical to `enz_rich_edit(..)`.
pub fn enz_rich_edit_builder<'a, Message: Clone + 'a>(
    controller: &'a RichEditController,
    content: &'a Content,
    on_action: impl Fn(RichEditAction) -> Message + 'a,
) -> EnzRichEdit<'a, Message> {
    EnzRichEdit {
        controller,
        content,
        on_action: std::rc::Rc::new(on_action),
        editor_height: AreaHeight::Fixed(DEFAULT_EDITOR_HEIGHT),
        editor_id: None,
        font_family: None,
        font_role: FontRole::Ui,
        text_size: None,
        styles: InlineStyle::ALL.to_vec(),
        density: RichEditToolbarDensity::Standard,
        paper: None,
        retain_selection: false,
        swatches: Vec::new(),
        on_color: None,
        toolbar_leading: Vec::new(),
        toolbar_trailing: Vec::new(),
        paragraph_alignment: true,
        floats: Vec::new(),
        show_toolbar: true,
        editor_tokens: None,
        editor_variant: TextAreaVariant::Field,
    }
}

/// Builder for the rich-edit composite (toolbar + editor) with a composable
/// editor height. Construct with [`enz_rich_edit_builder`].
/// Use [`Self::show_toolbar`] to place a standalone toolbar elsewhere.
#[must_use]
pub struct EnzRichEdit<'a, Message: Clone + 'a> {
    controller: &'a RichEditController,
    content: &'a Content,
    on_action: std::rc::Rc<dyn Fn(RichEditAction) -> Message + 'a>,
    editor_height: AreaHeight,
    editor_id: Option<iced::advanced::widget::Id>,
    font_family: Option<FontFamily>,
    font_role: FontRole,
    text_size: Option<f32>,
    styles: Vec<InlineStyle>,
    density: RichEditToolbarDensity,
    paper: Option<iced::Color>,
    retain_selection: bool,
    swatches: Vec<RichEditSwatch>,
    on_color: Option<std::rc::Rc<dyn Fn(Option<iced::Color>) -> Message + 'a>>,
    toolbar_leading: Vec<Element<'a, Message>>,
    toolbar_trailing: Vec<Element<'a, Message>>,
    paragraph_alignment: bool,
    show_toolbar: bool,
    editor_tokens: Option<TextAreaTokens>,
    editor_variant: TextAreaVariant,
    floats: Vec<crate::text_float::TextFloat<'a, Message>>,
}

impl<'a, Message: Clone + 'a> EnzRichEdit<'a, Message> {
    /// Include the formatting toolbar; defaults to true.
    /// False returns only the native editor, with no toolbar spacing or controls.
    /// Toolbar settings and caller-owned toolbar slots are ignored when false.
    pub fn show_toolbar(mut self, show: bool) -> Self {
        self.show_toolbar = show;
        self
    }

    /// Set editor chrome, body ink, selection fill and padding.
    /// Explicit tokens override the tokens supplied by [`Self::paper`] in either order.
    /// Document formatting is unchanged; the paper contrast policy still applies
    /// to authored run colours. Body ink also supplies the native caret colour.
    pub fn editor_tokens(mut self, tokens: TextAreaTokens) -> Self {
        self.editor_tokens = Some(tokens);
        self
    }

    /// Choose editor chrome; defaults to [`TextAreaVariant::Field`].
    /// Plain paints no background, radius or idle, hover or focus border.
    /// Text, selection and caret keep their native paint and input geometry.
    pub fn editor_variant(mut self, variant: TextAreaVariant) -> Self {
        self.editor_variant = variant;
        self
    }

    /// Pin the base family while retaining application regular weight and inline formatting.
    /// Named families must be loaded by the host; code spans keep their monospace family.
    pub fn font(mut self, family: impl Into<FontFamily>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// Choose the application catalog role for an unpinned base family; defaults to Ui.
    /// An explicit family pin retains precedence.
    pub fn font_role(mut self, role: FontRole) -> Self {
        self.font_role = role;
        self
    }

    /// Pin the selected role's family from this theme; select the role first.
    pub fn font_from_theme(mut self, theme: &EnzTheme) -> Self {
        self.font_family = Some(theme.font_catalog.family(self.font_role).clone());
        self
    }

    /// Pin absolute logical text size, matching [`crate::text_area::EnzTextArea::text_size_px`].
    /// Defaults follow desktop text scaling. An explicit value bypasses that scaling;
    /// apply [`EnzTheme::text_scale`] to a document size when both are wanted.
    /// Values are forwarded without validation.
    pub fn text_size_px(mut self, px: f32) -> Self {
        self.text_size = Some(px);
        self
    }

    /// Set the editor viewport height policy.
    ///
    /// [`AreaHeight::Fill`] makes the editor consume the height the parent
    /// offers (the composer-in-a-bounded-dialog pattern); [`AreaHeight::Fixed`]
    /// pins a pixel height; [`AreaHeight::Rows`] a whole number of text rows;
    /// [`AreaHeight::AutoGrow`] the chat-composer grow-with-content policy. The
    /// toolbar keeps its intrinsic height; only the editor is affected.
    pub fn editor_height(mut self, height: AreaHeight) -> Self {
        self.editor_height = height;
        self
    }

    /// Offer only `styles` in the toolbar, in the given order.
    ///
    /// Defaults to [`InlineStyle::ALL`]. Narrow the set when a style has no
    /// painted result in this host: a toggle that lights up and changes nothing
    /// on screen tells the user their formatting was applied when it was not.
    /// Only the inline-style group narrows; paragraph alignment is controlled
    /// separately by [`Self::paragraph_alignment`]. The document and emitted
    /// HTML retain omitted styles.
    pub fn styles(mut self, styles: Vec<InlineStyle>) -> Self {
        self.styles = styles;
        self
    }

    /// Choose the toolbar row height.
    pub fn density(mut self, density: RichEditToolbarDensity) -> Self {
        self.density = density;
        self
    }

    /// Resolve default editor tokens and authored-ink contrast against opaque `paper`.
    /// Field chrome paints the paper; Plain chrome leaves the host backdrop visible.
    /// Explicit [`Self::editor_tokens`] override the default page tokens.
    pub fn paper(mut self, paper: iced::Color) -> Self {
        self.paper = Some(paper);
        self
    }

    /// Keep the selection painted while the editor is unfocused, so it
    /// stays visible under a formatting menu or link dialog that holds focus.
    /// See [`EnzTextArea::retain_selection`](crate::text_area::EnzTextArea::retain_selection).
    pub fn retain_selection(mut self, retain: bool) -> Self {
        self.retain_selection = retain;
        self
    }

    /// Offer an inline colour strip in the toolbar row.
    ///
    /// The callback receives the ink the user picked (or `None` for the clear
    /// chip); forward it to [`RichEditController::set_color`].
    pub fn colors(
        mut self,
        swatches: Vec<RichEditSwatch>,
        on_color: impl Fn(Option<iced::Color>) -> Message + 'a,
    ) -> Self {
        self.swatches = swatches;
        self.on_color = Some(std::rc::Rc::new(on_color));
        self
    }

    /// Give the editor a stable widget id.
    ///
    /// Without it the editor inside the composite has no id a focus operation
    /// or accessibility action could address.
    pub fn editor_id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.editor_id = Some(id.into());
        self
    }

    /// Prepend a caller-owned control to the toolbar row, ahead of the style
    /// toggles.
    pub fn toolbar_leading(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.toolbar_leading.push(element.into());
        self
    }

    /// Append a caller-owned control after the formatting groups.
    ///
    /// It shares their line when the combined measured width fits, and wraps
    /// onto a following line otherwise. Compact density does not disable wrapping.
    pub fn toolbar_trailing(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.toolbar_trailing.push(element.into());
        self
    }

    /// Offer the paragraph-alignment control in the toolbar row.
    ///
    /// On by default: the editable surface moves the glyphs, the caret, the
    /// selection and the hit test with the alignment, so the control's result
    /// is visible where the user is typing. Narrow it away only in a host that
    /// renders the document somewhere alignment cannot survive.
    ///
    /// The control is single-select and shows [`ParagraphAlign`] for the
    /// paragraphs the selection touches, nothing when they disagree.
    pub fn paragraph_alignment(mut self, offered: bool) -> Self {
        self.paragraph_alignment = offered;
        self
    }

    /// Float `float` in the document, like an image placed against a
    /// paragraph: the paragraph flows beside it and continues at the full
    /// width below, as one editable document (see [`crate::text_float`]).
    pub fn float(mut self, float: crate::text_float::TextFloat<'a, Message>) -> Self {
        self.floats.push(float);
        self
    }

    /// Build the composite into an [`Element`].
    pub fn into_element(self) -> Element<'a, Message> {
        let on_action = self.on_action;

        let toolbar_cb = std::rc::Rc::clone(&on_action);
        let mut toolbar = enz_rich_edit_toolbar_builder(self.controller, move |style| {
            toolbar_cb(RichEditAction::ToggleStyle(style))
        })
        .styles(self.styles)
        .density(self.density);
        if self.paragraph_alignment {
            let align_cb = std::rc::Rc::clone(&on_action);
            toolbar =
                toolbar.alignment(move |align| align_cb(RichEditAction::SetParagraphAlign(align)));
        }
        for element in self.toolbar_leading {
            toolbar = toolbar.leading(element);
        }
        if let Some(on_color) = self.on_color {
            toolbar = toolbar.colors(self.swatches, move |color| on_color(color));
        }
        let action_cb = std::rc::Rc::clone(&on_action);
        toolbar = toolbar.actions(move |action| action_cb(action));
        for element in self.toolbar_trailing {
            toolbar = toolbar.trailing(element);
        }

        let mut paint = self.controller.paint();
        if let Some(paper) = self.paper {
            paint = paint.with_paper(paper);
        }

        let editor_cb = on_action;
        let editor = enz_text_area(self.content, move |action| {
            editor_cb(RichEditAction::from_editor_action(action))
        })
        .variant(self.editor_variant)
        .font_role(self.font_role)
        .rich_formatting(paint)
        .background_highlights(highlight_runs(self.controller))
        .retain_selection(self.retain_selection);
        let editor = self
            .floats
            .into_iter()
            .fold(editor, crate::text_area::EnzTextArea::float);
        let editor = match self.font_family {
            Some(family) => editor.with_font_family(family),
            None => editor,
        };
        let editor = match self.text_size {
            Some(px) => editor.text_size_px(px),
            None => editor,
        };
        let editor = match self.editor_id {
            Some(id) => editor.id(id),
            None => editor,
        };
        let editor = match self
            .editor_tokens
            .or_else(|| self.paper.map(TextAreaTokens::page))
        {
            Some(tokens) => editor.tokens(tokens),
            None => editor,
        };
        let editor = match self.editor_height {
            AreaHeight::Fill => editor.fill_height(),
            AreaHeight::Fixed(h) => editor.area_height(h),
            AreaHeight::Rows(n) => editor.rows(n),
            AreaHeight::AutoGrow { min_rows, max_rows } => editor.auto_grow(min_rows, max_rows),
        };

        if self.show_toolbar {
            column![toolbar.into_element(), editor].spacing(4).into()
        } else {
            editor.into()
        }
    }
}

// The marker colours to paint behind the text, as editor positions.
fn highlight_runs(
    controller: &RichEditController,
) -> Vec<(
    std::ops::Range<iced::widget::text_editor::Position>,
    iced::Color,
)> {
    let doc = controller.document();
    let text = doc.text();
    let mut runs = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let Some(color) = doc.formats[start].highlight() else {
            start += 1;
            continue;
        };
        let mut end = start + 1;
        while end < text.len() && doc.formats[end].highlight() == Some(color) {
            end += 1;
        }
        // A run never spans a break: the editor addresses lines, and the break
        // itself paints nothing.
        for body in super::document::line_body_ranges(text) {
            let a = body.start.max(start);
            let b = body.end.min(end);
            if a < b {
                runs.push((
                    byte_offset_to_position(text, a)..byte_offset_to_position(text, b),
                    color,
                ));
            }
        }
        start = end;
    }
    runs
}

impl<'a, Message: Clone + 'a> From<EnzRichEdit<'a, Message>> for Element<'a, Message> {
    fn from(builder: EnzRichEdit<'a, Message>) -> Self {
        builder.into_element()
    }
}
