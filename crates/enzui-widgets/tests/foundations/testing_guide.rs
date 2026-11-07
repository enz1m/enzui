//! The examples in `docs/testing.md`, compiled and run, so the guide cannot
//! drift from the API it describes. Each test is one section of the guide.

use enzui_core::collect::{SemanticsCollector, collect_frame};
use enzui_core::semantics::Role;
use enzui_widgets::button::enz_button;
use enzui_widgets::checkbox::enz_checkbox;
use iced::advanced::widget::operation::focusable;
use iced::keyboard::key::Named;
use iced::widget::{column, text_input};
use iced::{Element, Size};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Message {
    Save,
    Remember(bool),
    Name(String),
}

fn form(remember: bool) -> Element<'static, Message> {
    column![
        text_input("Name", "").id("name").on_input(Message::Name),
        enz_checkbox(remember, "Remember me", Message::Remember).into_element(),
        enz_button("Save", Message::Save).into_element(),
    ]
    .spacing(8)
    .into()
}

/// "Simulate input": find a control by its text, click it, read the messages.
#[test]
fn guide_simulate_a_click() {
    let mut ui = Simulator::with_size(Default::default(), Size::new(400.0, 300.0), form(false));
    let _ = ui.click("Save").expect("a Save button");
    assert_eq!(ui.into_messages().collect::<Vec<_>>(), [Message::Save]);
}

/// "Focus and keys": focus moves through operations, which may chain; run
/// them with `enzui_core::chained_operation::operate_chained`, as the runtime
/// does.
#[test]
fn guide_focus_and_type() {
    let mut ui = Simulator::with_size(Default::default(), Size::new(400.0, 300.0), form(false));
    let mut focus = focusable::focus::<()>(iced::advanced::widget::Id::new("name"));
    let _ = enzui_core::chained_operation::operate_chained(
        |operation| ui.operate(operation),
        &mut focus,
    );
    let _ = ui.typewrite("Ada");
    let _ = ui.tap_key(Named::Enter);
    assert_eq!(
        ui.into_messages().last(),
        Some(Message::Name("Ada".to_owned()))
    );
}

/// "What assistive technology sees": collect the semantics tree of a frame
/// and assert on roles, names and states.
#[test]
fn guide_assert_semantics() {
    let frame = collect_frame(form(true), Size::new(400.0, 300.0));
    let checkbox = frame
        .collected()
        .iter()
        .find(|node| node.semantics.role == Role::CheckBox)
        .expect("a checkbox");
    assert_eq!(checkbox.semantics.label.as_deref(), Some("Remember me"));
    assert_eq!(
        checkbox.semantics.toggled,
        Some(enzui_core::semantics::Toggle::True)
    );
    // The same collector runs over a live simulator.
    let mut ui = Simulator::new(form(false));
    let mut collector = SemanticsCollector::new();
    ui.operate(&mut collector);
    assert!(
        collector
            .collected()
            .iter()
            .any(|node| node.semantics.role == Role::Button)
    );
}

/// "Audit accessibility": the audit a running app performs under
/// `ENZUI_DIAGNOSTICS=1`, as a test. It examines what publishes a probe: text
/// that elides, and the controls you wrap in `HitTargetScanProbe` or
/// `ContrastScanProbe`.
#[test]
fn guide_accessibility_audit() {
    use enzui_core::accessibility_scan::{AccessibilityDefectKind, AccessibilityScanCollector};
    let toolbar: Element<'static, Message> = iced::widget::row![
        enzui_widgets::HitTargetScanProbe::new(
            iced::widget::container(enz_button("Save", Message::Save).into_element()).width(80),
        ),
        // A 12 px close button: too small to hit reliably.
        enzui_widgets::HitTargetScanProbe::new(
            iced::widget::button(iced::widget::Space::new().width(12).height(12))
                .padding(0)
                .on_press(Message::Save),
        ),
    ]
    .spacing(8)
    .into();
    let mut ui = Simulator::with_size(Default::default(), Size::new(400.0, 300.0), toolbar);
    let mut audit = AccessibilityScanCollector::new(
        &enzui_widgets::hit_target_deficit,
        &enzui_widgets::contrast_deficit,
        &enzui_widgets::elision_clamp,
    );
    ui.operate(&mut audit);
    assert!(
        audit.subjects_examined() >= 2,
        "the audit looked at both controls"
    );
    let small: Vec<_> = audit
        .findings()
        .iter()
        .filter(|finding| {
            matches!(
                finding.kind,
                AccessibilityDefectKind::UndersizedTarget { .. }
            )
        })
        .collect();
    assert_eq!(
        small.len(),
        1,
        "only the 12 px button is too small: {:?}",
        audit.findings()
    );
}

/// "Desktop preferences, locale and theme": scope them around the code
/// under test instead of changing the process.
#[test]
fn guide_scoped_preferences_and_locale() {
    let reduced = iced_core::preferences::Preferences {
        reduced_motion: true,
        ..iced_core::preferences::get()
    };
    let seen =
        iced_core::preferences::scoped(reduced, || iced_core::preferences::get().reduced_motion);
    assert!(seen);
    let german = enzui_core::localization::Localizer::new(
        Default::default(),
        vec!["de-DE".parse().expect("locale")],
        enzui_core::localization::Locale::default(),
    );
    let label = german.scope(|| enzui_core::localization::text("enzui-time-picker-choose"));
    assert_eq!(label, "Uhrzeit wählen");
}
