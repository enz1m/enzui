//! Renderer-neutral accessibility semantics for enzUI controls.
//!
//! Widgets build [`Semantics`] values, and the optional AccessKit adapter combines them with tree geometry and relationships.

use iced_core::widget::Id;

/// A renderer-neutral description of a control’s accessibility role, state, and capabilities.
///
/// Geometry and child relationships are supplied separately during tree collection.
#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub struct Semantics {
    /// The control's role.
    pub role: Role,
    /// The accessible name, required for interactive controls.
    pub label: Option<String>,
    /// Longer description / helper text (checkbox subtitle, field hint).
    pub description: Option<String>,
    /// A keyboard shortcut exposed through the platform accessibility property.
    pub keyboard_shortcut: Option<String>,
    /// Whether the current value is invalid.
    pub invalid: bool,
    /// Expansion state: `Some(true)` is expanded, `Some(false)` collapsed, and `None` not expandable.
    pub expanded: Option<bool>,
    /// Whether assistive technology may invoke expand or collapse.
    ///
    /// This capability is independent of [`Self::expanded`], defaults to `true`, and matters only when expansion state is present.
    pub expandable_action: bool,
    /// Text value for text-bearing controls.
    pub value: Option<String>,
    /// Numeric value contract for range controls.
    pub numeric: Option<NumericValue>,
    /// Checkbox / switch checked state.
    pub toggled: Option<Toggle>,
    /// Whether the control is currently selected (tab, list row, radio).
    pub selected: Option<bool>,
    /// The action’s consequence classification, or `None` for no special warning.
    ///
    /// It is also included in [`Semantics::accessible_description`] where no native property exists.
    pub action_disposition: Option<ActionDisposition>,
    /// Sort state for a column header.
    ///
    /// `None` means not sortable; `Some(Unsorted)` means sortable but currently unsorted. The latter is preserved in the accessible description when no native direction exists.
    pub sort: Option<SortDirection>,
    /// Whether the control is disabled and inert.
    pub disabled: bool,
    /// Whether the control is read-only, suppressing value-changing actions while retaining focusability unless separately disabled.
    pub read_only: bool,
    /// Whether the field requires a value, independently of [`Self::invalid`].
    pub required: bool,
    /// Whether the element is working on something (loading, installing):
    /// assistive technology reports it busy.
    pub busy: bool,
    /// Whether the control currently has keyboard focus.
    pub focused: bool,
    /// Explicit focus routing for roles that are not interactive by default. Defaults to false.
    pub supports_focus: bool,
    /// Orientation, for controls that have one.
    pub orientation: Option<Orientation>,
    /// Whether assistive technology may invoke the default activation action.
    pub supports_default_action: bool,
    /// Whether assistive technology may increment or decrement the numeric value.
    ///
    /// This is independent of [`Self::numeric`] and defaults to `false`.
    pub adjustable: bool,
    /// Whether assistive technology may set the value directly.
    ///
    /// This is independent of value disclosure, defaults to `false`, and is applied only to roles with a routed value payload.
    pub editable: bool,
    /// Whether assistive technology may open this control’s context menu.
    pub supports_context_menu: bool,
    /// Position within an enclosing [`Grid`](Role::Grid), for
    /// [`GridCell`](Role::GridCell)s.
    pub grid_position: Option<GridPosition>,
    /// The 1-based hierarchical depth of a tree item.
    pub level: Option<usize>,
    /// Position and total count within an ordered set, mapped to AccessKit `position_in_set` and `size_of_set`.
    pub set_position: Option<SetPosition>,
    /// Whether this surface makes everything outside it inert to assistive technology.
    pub modal: bool,
    /// The active item while keyboard focus remains on this node.
    ///
    /// The target uses a widget [`Id`] and may be a sibling; tree assembly drops the relation when the target was not collected.
    pub active_descendant: Option<Id>,
    /// Live-region politeness.
    ///
    /// `None` inherits from ancestors; status and alert roles receive polite and assertive defaults, and explicit `Off` suppresses inheritance.
    pub live: Option<Live>,
    /// An optional document-outline level independent of [`Self::role`].
    ///
    /// Promoted labels retain their collected role while the AccessKit adapter exposes them as headings.
    pub heading_level: Option<HeadingLevel>,
    /// An editable text node's laid-out text and selection, with the
    /// screen-space box its line positions start from.
    ///
    /// The collector fills it from the text input inside the node; the
    /// AccessKit adapter publishes it as text runs, so screen readers follow
    /// the caret and read the selection.
    pub text: Option<Box<(iced_core::Rectangle, TextLayout)>>,
    /// The widgets whose text names this node (a separate label).
    ///
    /// Like [`Self::active_descendant`], targets are widget [`Id`]s; tree
    /// assembly drops the ones not collected in the same frame.
    pub labelled_by: Vec<Id>,
    /// The widgets whose text describes this node (help or hint text).
    pub described_by: Vec<Id>,
    /// The widgets this node controls (a tab's panel, a search's results).
    pub controls: Vec<Id>,
    /// The widget that shows this node's error, when it is invalid.
    pub error_message: Option<Id>,
}

/// An input's text as assistive technology reads it.
pub use iced_core::widget::operation::text_input::{
    AccessibleLine as TextLine, AccessibleText as TextLayout,
};

#[cfg(feature = "accesskit")]
mod ak;
mod builder;
mod predicates;
mod vocabulary;

pub use vocabulary::{
    ActionDisposition, GridPosition, HeadingLevel, Live, NumericValue, Orientation, Role,
    SetPosition, SortDirection, Toggle,
};
