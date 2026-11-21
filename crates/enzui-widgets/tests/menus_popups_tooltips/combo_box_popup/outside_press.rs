/// A press on another control closes the list even when that control
/// captures the press: only a press on the list itself keeps it open.
#[test]
fn a_press_another_control_captures_still_closes_the_list() {
    let mut host = Host::with(vec!["Apple".into(), "Pear".into()], Some("Apple".into()));
    host.button_above = true;
    let trigger = host
        .collect()
        .into_iter()
        .find(|n| n.semantics.role == Role::ComboBox)
        .expect("the field is published")
        .bounds;
    let _ = host.click_at(Point::new(trigger.x + 20.0, trigger.y + trigger.height / 2.0));
    let open = |host: &mut Host| host.collect().iter().any(|n| n.semantics.role == Role::ListBox);
    assert!(open(&mut host), "sanity: a click on the field opens the list");
    let picked = host.click_at(Point::new(20.0, 20.0));
    assert!(picked.contains(&Msg::Pick("button".into())), "the button still gets its press");
    assert!(!open(&mut host), "a press another control captured closes the list");
}
