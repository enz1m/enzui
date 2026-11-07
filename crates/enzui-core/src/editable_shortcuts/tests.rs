use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Save,
    Open,
}
fn ctrl(key: char) -> Shortcut {
    Shortcut::new(Modifier::CTRL, key)
}
fn defaults() -> Vec<ShortcutBinding<Command>> {
    vec![
        ShortcutBinding::new("file.save", Command::Save, ctrl('s'))
            .with_description("Save message")
            .with_section("File")
            .with_alternate(Shortcut::new(Modifier::CTRL | Modifier::SHIFT, 's')),
        ShortcutBinding::new("file.open", Command::Open, ctrl('o'))
            .with_description("Open message")
            .with_section("File"),
    ]
}

#[test]
fn conflict_is_refused_before_apply_and_names_the_owner() {
    let mut model = EditableShortcutCatalog::new(defaults());
    let before = model.rows().next().unwrap().primary.clone();
    let error = model
        .try_rebind("file.save", ctrl('o'), |_| ShortcutValidity::Valid)
        .unwrap_err();
    assert!(error.message().contains("Open message"));
    assert_eq!(model.rows().next().unwrap().primary, &before);
}
