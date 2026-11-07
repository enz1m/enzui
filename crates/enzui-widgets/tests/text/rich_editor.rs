//! `RichEditor` keeps the document and the native editing surface in step:
//! native selection reaches the controller, commands reach the surface, and
//! toolbar commands ask for focus back.

use enzui_widgets::rich_edit::{InlineStyle, RichEditAction, RichEditor};
use iced::widget::text_editor::Action;

#[test]
fn a_native_selection_is_what_a_command_formats() {
    let mut editor = RichEditor::from_html("<p>hello world</p>").expect("html");
    assert!(
        !editor.update(RichEditAction::Native(Action::SelectAll)),
        "selecting keeps focus where it is"
    );
    assert!(
        editor.update(RichEditAction::ToggleStyle(InlineStyle::Bold)),
        "a toolbar command hands focus back to the editor"
    );
    assert_eq!(editor.to_html(), "<strong>hello world</strong>");
}

#[test]
fn typing_lands_in_the_document_and_the_surface() {
    use iced::widget::text_editor::Motion;
    let mut editor = RichEditor::from_html("ab").expect("html");
    let _ = editor.update(RichEditAction::Native(Action::Move(Motion::DocumentEnd)));
    let _ = editor.update(RichEditAction::Insert('c'));
    assert_eq!(editor.text(), "abc");
    // The surface was rebuilt with the typed text, so selecting everything
    // on it selects everything the document holds.
    let _ = editor.update(RichEditAction::Native(Action::SelectAll));
    assert_eq!(editor.controller().selection(), 0..3);
}

#[test]
fn formatting_keeps_the_native_surface_and_typing_replaces_it() {
    let mut editor = RichEditor::from_html("<p>hello</p><p>world</p>").expect("html");
    let _ = editor.update(RichEditAction::Native(Action::SelectAll));
    let before = editor.content().presentation_identity().expect("identity");

    let _ = editor.update(RichEditAction::ToggleStyle(InlineStyle::Bold));
    let after = editor.content().presentation_identity().expect("identity");
    assert!(
        std::sync::Arc::ptr_eq(&before, &after),
        "a style change keeps the shaped buffer"
    );
    assert_eq!(editor.controller().selection(), 0..editor.text().len());

    let _ = editor.update(RichEditAction::Insert('x'));
    let typed = editor.content().presentation_identity().expect("identity");
    assert!(
        !std::sync::Arc::ptr_eq(&after, &typed),
        "a text change rebuilds it"
    );
    assert_eq!(editor.content().text(), editor.text());
}

/// Enter in a list item is one action: one undo takes back both the break and
/// the next item's marker, and one redo puts both back.
#[test]
fn a_list_enter_is_one_undo_step() {
    use enzui_widgets::rich_edit::{ListKind, RichEditController, RichEditDocument};
    let mut controller = RichEditController::new(RichEditDocument::from_plain("milk"));
    controller.set_selection(0..0);
    controller.toggle_list(ListKind::Bullet);
    let end = controller.document().text().len();
    controller.set_selection(end..end);
    let before = controller.document().text().to_owned();
    let depth = controller.undo_depth();

    controller.enter();
    let after = controller.document().text().to_owned();
    assert!(
        after.len() > before.len() + 1,
        "a break and a marker: {after:?}"
    );
    assert_eq!(controller.undo_depth(), depth + 1);

    assert!(controller.undo());
    assert_eq!(controller.document().text(), before);
    assert!(controller.redo());
    assert_eq!(controller.document().text(), after);
}
