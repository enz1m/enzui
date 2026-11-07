//! The state a host keeps for one rich-text editor.

use super::widget::{
    EnzRichEdit, RichEditAction, enz_rich_edit_builder, sync_content_cursor_from_controller,
    sync_selection_from_content,
};
use super::{FromHtmlError, RichEditController, RichEditDocument, from_html, to_html};
use iced::widget::text_editor::Content;

/// A rich-text editor's state: the document with its formatting, selection
/// and undo history ([`RichEditController`]), and the native editing surface
/// kept in step with it.
///
/// Keep one in the application state, show it with [`RichEditor::view`],
/// and hand every [`RichEditAction`] to [`RichEditor::update`]:
///
/// ```no_run
/// use enzui_widgets::rich_edit::{RichEditAction, RichEditor};
///
/// struct Notes {
///     editor: RichEditor,
/// }
///
/// #[derive(Debug, Clone)]
/// enum Message {
///     Edit(RichEditAction),
/// }
///
/// impl Notes {
///     fn new() -> Self {
///         Self {
///             editor: RichEditor::from_html("<p>Hello <b>world</b></p>").expect("valid HTML"),
///         }
///     }
///
///     fn update(&mut self, message: Message) {
///         let Message::Edit(action) = message;
///         // `true` asks for focus back on the editor after a toolbar press;
///         // focus the id given to `.editor_id(..)` when it is.
///         let _refocus = self.editor.update(action);
///     }
///
///     fn view(&self) -> iced::Element<'_, Message> {
///         self.editor.view(Message::Edit).into_element()
///     }
///
///     fn save(&self) -> String {
///         self.editor.to_html()
///     }
/// }
/// ```
#[derive(Debug)]
pub struct RichEditor {
    controller: RichEditController,
    content: Content,
}

impl RichEditor {
    /// An editor holding `document`.
    #[must_use]
    pub fn new(document: RichEditDocument) -> Self {
        let controller = RichEditController::new(document);
        let mut content = Content::with_text(controller.document().text());
        // One caret from the start: the native editor's matches the controller's.
        sync_content_cursor_from_controller(&controller, &mut content);
        Self {
            controller,
            content,
        }
    }

    /// An editor holding the document `html` describes (see [`from_html`]).
    ///
    /// # Errors
    /// Returns [`FromHtmlError`] for HTML the document model cannot hold.
    pub fn from_html(html: &str) -> Result<Self, FromHtmlError> {
        from_html(html).map(Self::new)
    }

    /// Apply one action from the editor or its toolbar. Returns whether the
    /// host should give the editor keyboard focus back (a toolbar press took
    /// it), as [`RichEditAction::returns_to_editor`].
    pub fn update(&mut self, action: RichEditAction) -> bool {
        if let RichEditAction::Native(native) = action {
            self.content.perform(native);
            sync_selection_from_content(&mut self.controller, &self.content);
            return false;
        }
        let refocus = action.returns_to_editor();
        // The native selection can move without an action reaching here (an
        // assistive technology setting it, a focus operation); a command acts
        // on the selection the user sees.
        sync_selection_from_content(&mut self.controller, &self.content);
        if action.apply(&mut self.controller) {
            // A formatting-only change keeps the native buffer and its shaping;
            // the paint projection carries the new formatting.
            if !shows_text(&self.content, self.controller.document().text()) {
                self.content = Content::with_text(self.controller.document().text());
            }
            sync_content_cursor_from_controller(&self.controller, &mut self.content);
        }
        refocus
    }

    /// The editor and its toolbar, sending actions through `on_action`.
    pub fn view<'a, Message: Clone + 'a>(
        &'a self,
        on_action: impl Fn(RichEditAction) -> Message + 'a,
    ) -> EnzRichEdit<'a, Message> {
        enz_rich_edit_builder(&self.controller, &self.content, on_action)
    }

    /// The document and its selection, for reading formatting state or
    /// running commands directly.
    #[must_use]
    pub fn controller(&self) -> &RichEditController {
        &self.controller
    }

    /// The native editing surface, for reading its cursor, selection or
    /// scroll metrics.
    #[must_use]
    pub fn content(&self) -> &Content {
        &self.content
    }

    /// The document as HTML (see [`to_html`]).
    #[must_use]
    pub fn to_html(&self) -> String {
        to_html(self.controller.document())
    }

    /// The document's plain text.
    #[must_use]
    pub fn text(&self) -> &str {
        self.controller.document().text()
    }
}

/// Whether `content` holds exactly `text`, line by line; stops at the first
/// difference.
fn shows_text(content: &Content, text: &str) -> bool {
    let mut lines = content.lines();
    text.split('\n')
        .all(|expected| lines.next().is_some_and(|line| line.text == expected))
        && lines.next().is_none()
}
