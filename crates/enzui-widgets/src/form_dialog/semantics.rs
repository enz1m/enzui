//! AccessKit [`Semantics`] for a rendered form: the grouping node
//! ([`field_group_semantics`]), the interactive-control node
//! ([`field_semantics`]) and the dialog container ([`form_semantics`]).
//!
//! These are pure functions of the field/value/error, but they carry real
//! behaviour — role selection, required/invalid flags, toggle state — and each
//! has direct unit tests.

use enzui_core::form::{FieldValue, FormField};
use enzui_core::semantics::{Role, Semantics, Toggle};

/// The grouping node for a rendered field.
///
/// The form dialog wraps every rendered control in a
/// [`SemanticsContainer`](crate::semantics_container::SemanticsContainer). That
/// wrapper must not be interactive: the control it wraps already publishes
/// its own interactive node (a `TextInput`, a `Switch`, a `ComboBox`). An
/// interactive wrapper would emit a second, contradictory contract for a
/// `Choice` (`RadioGroup` around a `ComboBox`). The wrapper is therefore a neutral
/// [`Role::Group`], carrying only the field-level metadata the inner control
/// does not know: the label, `required`, the validation `error`/`invalid`, and
/// the description/helper. The control keeps its own interactive role. One
/// interactive node plus one grouping node is the standard shape for a
/// labelled form field.
///
/// `required` and `invalid` are separate flags on purpose (see
/// [`Semantics::required`]): a required field announces the requirement from
/// the moment the form opens, while `invalid` is the current verdict on its
/// value. Announcing an untouched required field as an error would be wrong,
/// and withholding the requirement until the user had already failed to meet
/// it would be worse.
///
/// Value/toggled/editable/read-only are *not* published here — those are the
/// control's contract, not the group's, and the control publishes them itself.
/// (`disabled`/`read_only` are still forwarded to the group so an AT that walks
/// only to the group can still hear the field is inert.)
pub fn field_group_semantics(field: &FormField, error: Option<&str>) -> Semantics {
    let mut semantics = Semantics::new(Role::Group)
        .label(field.label.as_str())
        .disabled(!field.enabled)
        .read_only(field.read_only)
        .required(field.required)
        .invalid(error.is_some());

    // The error replaces the helper text as the description, because that is
    // what the visible field does: a screen-reader user hearing the helper
    // while the sighted user reads the error are being told different things
    // about the same field.
    if let Some(error) = error {
        semantics = semantics.description(error);
    } else if let Some(helper) = field.helper.as_deref() {
        semantics = semantics.description(helper);
    }

    semantics
}

/// Self-describing semantics for one field.
///
/// A free function so the render path and a test ask the same question, and
/// so the mapping is testable without building an element.
///
/// This produces a node carrying the field's interactive control role
/// (`control_role`) *and* its metadata — a single complete node. It is used
/// where there is no inner control to publish its own node, i.e.
/// `labelled_fallback`, the read-only stand-in a multiline field with no
/// editor renders. It does not wrap a real control — that would
/// duplicate the control's interactive node; see
/// [`field_group_semantics`] for the wrapper.
///
/// `required` and `invalid` are separate flags on purpose (see
/// [`Semantics::required`]): a required field announces the requirement from
/// the moment the form opens, while `invalid` is the current verdict on its
/// value. Announcing an untouched required field as an error would be wrong,
/// and withholding the requirement until the user had already failed to meet
/// it would be worse.
pub fn field_semantics(field: &FormField, value: &FieldValue, error: Option<&str>) -> Semantics {
    // The editing *capability* comes from the field's kind, not
    // from whether it currently holds a value. A blank required text field and
    // a secure field (whose value is deliberately withheld below) are both
    // typed into, and both advertised no `SetValue` while the gate was
    // `value.is_some()`. `read_only` / `disabled` still withdraw the action in
    // `to_accesskit_node`.
    let mut semantics = Semantics::new(field.kind.control_role())
        .label(field.label.as_str())
        .disabled(!field.enabled)
        .read_only(field.read_only)
        .required(field.required)
        .editable(field.kind.is_textual())
        .invalid(error.is_some());

    // The error replaces the helper text as the description, because that is
    // what the visible field does: a screen-reader user hearing the helper
    // while the sighted user reads the error are being told different things
    // about the same field.
    if let Some(error) = error {
        semantics = semantics.description(error);
    } else if let Some(helper) = field.helper.as_deref() {
        semantics = semantics.description(helper);
    }

    match value {
        FieldValue::Text(text) => {
            semantics = semantics.value(text.as_str());
        }
        // A secure value is deliberately *not* published: classification is
        // carried by the value itself, so even a mismatched caller-provided
        // field cannot make the plaintext reachable through accessibility.
        FieldValue::Secure(_) => {}
        FieldValue::Toggle(on) => {
            semantics = semantics.toggled(if *on { Toggle::True } else { Toggle::False });
        }
        FieldValue::Choice(selected) => {
            if let Some(selected) = selected {
                semantics = semantics.value(selected.as_str());
            }
        }
    }

    semantics
}

/// Semantics for the dialog surface itself.
pub fn form_semantics(title: &str, description: Option<&str>) -> Semantics {
    let mut semantics = Semantics::new(Role::Dialog).label(title).modal(true);
    // The description is the accessible *description*, not part of the name:
    // a screen reader announces the name on focus and the description after
    // it, so folding it into the label would make every announcement of this
    // dialog recite the whole paragraph.
    if let Some(description) = description {
        semantics = semantics.description(description);
    }
    semantics
}
