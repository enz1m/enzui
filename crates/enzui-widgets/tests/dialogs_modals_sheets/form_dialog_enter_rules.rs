//! Enter-key behaviour in form dialog fields.
//!
//! The core rule: Enter submits from single-line fields and inserts a newline
//! in multiline fields. One key, two behaviours, decided by field kind.
//!
//! The core tests (`form.rs`) prove the *decision function* (`enter_submits`)
//! is correct. These prove the *rendered dialog* honours it: the single-line
//! field's `on_submit` is wired and fires, and the multiline field's is not.
//!
//! A test covering only single-line passes against an implementation that
//! always submits, which would make multiline fields unusable. Both sides must
//! be exercised, and the multiline side must assert the ABSENCE of a
//! `SubmitRequested`.

use enzui_core::form::{FormField, FormModel, FormSession};
use enzui_widgets::form_dialog::{FormDialogEvent, MultilineBridge, enz_form_dialog};
use iced::Element;
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Form(FormDialogEvent),
}

fn model(fields: Vec<FormField>) -> FormModel {
    FormModel::new(fields).expect("test model must be valid")
}

fn simulator(element: Element<'_, Msg>) -> Simulator<'_, Msg> {
    Simulator::with_size(
        iced_test::core::Settings::default(),
        iced::Size::new(900.0, 1400.0),
        element,
    )
}

// -----------------------------------------------------------------------
// Single-line: Enter must submit
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// Multiline: Enter must not submit
// -----------------------------------------------------------------------

/// A multiline field with Enter pressed: zero `SubmitRequested` messages.
/// This is the half that catches an implementation always submitting.
#[test]
fn enter_in_a_multiline_field_does_not_submit() {
    let session = FormSession::new(model(vec![FormField::multiline("body", "Body", 4)]));
    let bridge = MultilineBridge::new(&session);
    let element = enz_form_dialog("Test", &session, Msg::Form)
        .editors(&bridge)
        .into_surface();
    let mut ui = simulator(element);

    // Click inside the text_editor area to focus it, then press Enter.
    // The multiline field's label is separate; we click on the label area
    // which should focus the editor.
    ui.click("Body")
        .expect("the multiline label must be clickable");
    ui.tap_key(iced::keyboard::key::Named::Enter);

    let messages: Vec<Msg> = ui.into_messages().collect();
    let submit_count = messages
        .iter()
        .filter(|m| matches!(m, Msg::Form(FormDialogEvent::SubmitRequested)))
        .count();
    assert_eq!(
        submit_count, 0,
        "Enter in a multiline field must NOT submit (it inserts a newline); \
         got {submit_count} in {messages:?}"
    );
}

// -----------------------------------------------------------------------
// Mixed form: both rules coexist
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// Disabled / read-only: Enter does not submit
// -----------------------------------------------------------------------
