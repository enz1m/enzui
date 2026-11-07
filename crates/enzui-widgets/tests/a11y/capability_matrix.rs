//! semantic capability contract, cross-check — the tree gate and the router gate are one
//! predicate expressed twice, so this enumerates the whole product of the
//! inputs either of them reads and asserts they agree action-for-action.
//!
//! `Semantics::to_accesskit_node` decides what an assistive technology is
//! *told* it can do; `resolve_action` decides what the runtime will *honour*.
//! A disagreement is invisible from either side alone: the AT invokes an
//! advertised action and the router silently drops it (`Unhandled`), which is
//! the advertised-action routing defect, or the router honours a step the node never offered.
//!
//! `action_capability.rs` compares the two for four *real widgets*. That
//! is the right test for "does the shipped progress bar do this", and it is
//! blind to a combination no current widget happens to publish. This file is
//! the complement: hand-built `Semantics` over every
//! `role x adjustable x editable x read_only x disabled x numeric x value`
//! combination — 35 roles x 2^6 = 2240 cases — driven through the real
//! collector so the router sees exactly the descriptor the node was built
//! from.

use accesskit::{Action, ActionData};
use enzui_core::a11y_action::{AccessibilityAction, resolve_action};
use enzui_core::collect::{SemanticsCollector, SemanticsNode, node_id_of_widget};
use enzui_core::semantics::{NumericValue, Role, Semantics};
use iced::Rectangle;
use iced::advanced::widget::{Id, Operation};

/// Every `Role` variant. Written out rather than derived so adding a role to
/// the enum without adding it here is a visible omission rather than a silent
/// coverage hole.
const ROLES: &[Role] = &[
    Role::Button,
    Role::Link,
    Role::CheckBox,
    Role::Switch,
    Role::RadioButton,
    Role::RadioGroup,
    Role::Slider,
    Role::SpinButton,
    Role::TextInput,
    Role::PasswordInput,
    Role::ProgressIndicator,
    Role::ListItem,
    Role::Tab,
    Role::TabList,
    Role::Label,
    Role::Image,
    Role::Disclosure,
    Role::Splitter,
    Role::Menu,
    Role::MenuItem,
    Role::MenuItemCheckBox,
    Role::Grid,
    Role::GridRow,
    Role::GridCell,
    Role::ColumnHeader,
    Role::Tree,
    Role::TreeItem,
    Role::DateInput,
    Role::Dialog,
    Role::AlertDialog,
    Role::ComboBox,
    Role::ListBox,
    Role::ListBoxOption,
    Role::Group,
];

const NUMERIC: NumericValue = NumericValue {
    value: 5.0,
    min: 0.0,
    max: 10.0,
    step: Some(1.0),
};

/// One case of the product, in a form that prints usefully on failure.
#[derive(Debug, Clone, Copy)]
struct Case {
    role: Role,
    adjustable: bool,
    editable: bool,
    read_only: bool,
    disabled: bool,
    numeric: bool,
    value: bool,
}

impl Case {
    fn semantics(self) -> Semantics {
        let mut s = Semantics::new(self.role)
            .label("case")
            .adjustable(self.adjustable)
            .editable(self.editable)
            .read_only(self.read_only)
            .disabled(self.disabled);
        if self.numeric {
            s = s.numeric(NUMERIC);
        }
        if self.value {
            s = s.value("v");
        }
        s
    }
}

fn all_cases() -> Vec<Case> {
    let mut out = Vec::new();
    for &role in ROLES {
        for bits in 0u8..64 {
            out.push(Case {
                role,
                adjustable: bits & 1 != 0,
                editable: bits & 2 != 0,
                read_only: bits & 4 != 0,
                disabled: bits & 8 != 0,
                numeric: bits & 16 != 0,
                value: bits & 32 != 0,
            });
        }
    }
    out
}

/// What the *router* would do with `action` against this descriptor, published
/// through the real collector exactly as a widget's `operate()` does.
fn routed(sem: &Semantics, action: Action, data: Option<&ActionData>) -> AccessibilityAction {
    let id = Id::new("case");
    let mut collector = SemanticsCollector::new();
    let mut carrier = SemanticsNode::new(sem.clone());
    Operation::<()>::custom(
        &mut collector,
        Some(&id),
        Rectangle::new([0.0, 0.0].into(), [100.0, 20.0].into()),
        &mut carrier,
    );
    let (_update, map) = collector.to_tree_update_with_ids();
    resolve_action(action, node_id_of_widget(&id), data, &map).expect("the node is in the map")
}

fn accepted(sem: &Semantics, action: Action, data: Option<&ActionData>) -> bool {
    !matches!(
        routed(sem, action, data),
        AccessibilityAction::Unhandled { .. }
    )
}

/// `SetValue` with the string payload an AT sends when typing into a field.
/// The tree advertises one `SetValue`; the router splits it into a text and a
/// numeric contract, and `ActionData::Value` is accepted by both.
#[test]
fn tree_and_router_agree_on_set_value_with_a_string_payload_over_the_whole_product() {
    let payload = ActionData::Value("typed".into());
    let cases = all_cases();
    let mut disagreements = Vec::new();

    for case in &cases {
        let sem = case.semantics();
        let node = sem.to_accesskit_node();
        let advertised = node.supports_action(Action::SetValue);
        let honoured = accepted(&sem, Action::SetValue, Some(&payload));
        if advertised != honoured {
            disagreements.push(format!(
                "{case:?}: tree advertises SetValue={advertised}, router honours={honoured}"
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "{} of {} cases disagree on SetValue.\nFirst 10:\n{}",
        disagreements.len(),
        cases.len(),
        disagreements
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
