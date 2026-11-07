//! Public bulk range edits are one bounded, undoable editor action.

use iced::widget::text_editor::{
    Action, Content, Cursor, Edit, Motion, Position, ReplaceBatch, Replacement,
};
use std::sync::Arc;

fn replace_batch(replacements: Vec<Replacement>, cursor_after: usize) -> Action {
    Action::Edit(Edit::Replace(Arc::new(ReplaceBatch {
        replacements,
        cursor_after: Cursor {
            position: Position {
                line: 0,
                index: cursor_after,
            },
            selection: None,
        },
    })))
}

/// One undo restores every replacement and the exact pre-action caret; the
/// next undo still reaches the edit that preceded the batch.
#[test]
fn replace_batch_is_one_undo_step_without_discarding_history() {
    let original = "alpha cat beta cat omega";
    let mut content: Content = Content::with_text(original);

    content.perform(Action::Move(Motion::DocumentEnd));
    content.perform(Action::Edit(Edit::Insert('!')));
    content.move_to(Cursor {
        position: Position { line: 0, index: 6 },
        selection: None,
    });
    let before = content.cursor();

    let delta = content
        .perform(replace_batch(
            vec![
                Replacement::new(
                    Position { line: 0, index: 6 }..Position { line: 0, index: 9 },
                    "elephant",
                ),
                Replacement::new(
                    Position { line: 0, index: 15 }..Position { line: 0, index: 18 },
                    "dog",
                ),
            ],
            11,
        ))
        .expect("two replacements change the document");

    assert_eq!(delta.items().len(), 4, "two delete/insert pairs");
    assert_eq!(content.text(), "alpha elephant beta dog omega!");
    assert_eq!(content.cursor().position, Position { line: 0, index: 11 });

    content.perform(Action::Edit(Edit::Undo));
    assert_eq!(content.text(), "alpha cat beta cat omega!");
    assert_eq!(content.cursor(), before, "undo restores the prior caret");

    content.perform(Action::Edit(Edit::Undo));
    assert_eq!(
        content.text(),
        original,
        "the preceding edit remains the next history entry"
    );

    content.perform(Action::Edit(Edit::Redo));
    content.perform(Action::Edit(Edit::Redo));
    assert_eq!(content.text(), "alpha elephant beta dog omega!");
    assert_eq!(
        content.cursor().position,
        Position { line: 0, index: 11 },
        "redo restores the batch's requested final caret"
    );
}
