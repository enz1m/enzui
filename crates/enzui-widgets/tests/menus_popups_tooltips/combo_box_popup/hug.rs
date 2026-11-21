/// A hugging trigger is as wide as its text, can show a short form of the
/// selection, and its popup takes the width asked for rather than the
/// trigger's, with the full labels.
#[test]
fn a_hugging_trigger_fits_its_text_and_its_popup_is_wider() {
    let mut host = Host::with(
        vec!["Apple".into(), "A much longer pear".into()],
        Some("Apple".into()),
    );
    host.hug = true;
    let field = host
        .collect()
        .into_iter()
        .find(|n| n.semantics.role == Role::ComboBox)
        .expect("the field is published");
    assert_eq!(
        field.semantics.value.as_deref(),
        Some("Ap"),
        "the trigger shows the short form"
    );
    let trigger = field.bounds;
    assert!(
        trigger.width < 120.0,
        "the trigger hugs its text: {trigger:?}"
    );
    let _ = host.click_at(Point::new(
        trigger.x + trigger.width / 2.0,
        trigger.y + trigger.height / 2.0,
    ));
    let list = host
        .collect()
        .into_iter()
        .find(|n| n.semantics.role == Role::ListBox)
        .expect("a click opens the list")
        .bounds;
    assert!(
        (list.width - 260.0).abs() < 0.5,
        "the popup takes the asked width: {list:?}"
    );
}
