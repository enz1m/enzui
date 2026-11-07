//! Typed dynamic-form models, keyed draft sessions, validation, and submissions.
//!
//! Sessions preserve compatible drafts across retargeting, discard removed or
//! incompatible fields, and keep secure values in wipe-on-drop storage.

use std::collections::{BTreeMap, BTreeSet};

pub use enzui_security::secret::SecretText;
#[cfg(any(test, feature = "secret-observer"))]
#[doc(hidden)]
pub use enzui_security::secret::observer as secure_text_observer;

/// What `Debug` prints instead of a secure field's value.
pub const REDACTED: &str = "<redacted>";

/// A field's stable identity within a form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldId(String);

impl FieldId {
    /// Name a field.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for FieldId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for FieldId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for FieldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One choice within a [`FieldKind::Choice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// The value stored when this option is picked.
    pub value: String,
    /// What the user reads.
    pub label: String,
}

impl ChoiceOption {
    /// Creates an option from its stored value and display label.
    #[must_use]
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// What kind of control a field is, and the data only that kind has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// A single-line text field.
    Text {
        /// Ghost text shown while empty.
        placeholder: Option<String>,
    },
    /// A single-line field whose content is masked.
    Secure {
        /// Ghost text shown while empty.
        placeholder: Option<String>,
    },
    /// A multi-line text field.
    Multiline {
        /// Ghost text shown while empty.
        placeholder: Option<String>,
        /// Preferred visible line count.
        rows: u16,
    },
    /// An on/off field.
    Toggle,
    /// A single choice from a fixed set.
    Choice {
        /// The available options, in display order.
        options: Vec<ChoiceOption>,
    },
}

impl FieldKind {
    /// Whether this kind holds free text the user types into.
    #[must_use]
    pub const fn is_textual(&self) -> bool {
        matches!(
            self,
            Self::Text { .. } | Self::Secure { .. } | Self::Multiline { .. }
        )
    }

    /// Whether two kinds render the same shape of control, ignoring payload that does not
    /// change the control's shape.
    #[must_use]
    pub fn same_shape(&self, other: &Self) -> bool {
        core::mem::discriminant(self) == core::mem::discriminant(other)
    }

    /// The interactive role of the control this kind actually renders.
    #[must_use]
    pub const fn control_role(&self) -> crate::semantics::Role {
        use crate::semantics::Role;

        match self {
            Self::Text { .. } | Self::Multiline { .. } => Role::TextInput,
            Self::Secure { .. } => Role::PasswordInput,
            Self::Toggle => Role::Switch,
            Self::Choice { .. } => Role::ComboBox,
        }
    }
}

/// A field's value, in the shape its kind actually has.
#[derive(Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// Ordinary owned text for text and multiline fields.
    Text(String),
    /// Secure text, held in one shared allocation that is wiped when its last
    /// owner drops. Cloning shares custody and never copies plaintext bytes.
    Secure(SecretText),
    /// On or off, for toggles.
    Toggle(bool),
    /// An optional choice value; field normalization accepts it only when it matches an available option.
    Choice(Option<String>),
}

impl FieldValue {
    /// Moves owned plaintext into wipe-on-last-drop secure storage without copying it.
    #[must_use]
    pub fn secure(value: String) -> Self {
        Self::Secure(SecretText::new(value))
    }

    /// Borrows textual content without cloning secure plaintext.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            Self::Secure(t) => Some(t.expose_secret()),
            _ => None,
        }
    }

    /// Returns whether a value is absent for required validation; whitespace-only text is
    /// empty and toggles are always present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(t) => t.trim().is_empty(),
            Self::Secure(t) => t.expose_secret().trim().is_empty(),
            Self::Toggle(_) => false,
            Self::Choice(c) => c.is_none(),
        }
    }
}

impl std::fmt::Debug for FieldValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(value) => f.debug_tuple("Text").field(value).finish(),
            Self::Secure(_) => f.debug_tuple("Secure").field(&REDACTED).finish(),
            Self::Toggle(value) => f.debug_tuple("Toggle").field(value).finish(),
            Self::Choice(value) => f.debug_tuple("Choice").field(value).finish(),
        }
    }
}

/// Validates a present field value, returning an error message when it is ill-formed.
pub type FieldValidator = fn(&FieldValue) -> Option<String>;

/// One field in a form model.
#[derive(Clone)]
pub struct FormField {
    /// Stable identity, independent of position.
    pub id: FieldId,
    /// The accessible name, and the visible label.
    pub label: String,
    /// What kind of control, and its kind-specific data.
    pub kind: FieldKind,
    /// The value this field starts at when the presentation opens.
    pub initial: FieldValue,
    /// Supporting text shown under the field.
    pub helper: Option<String>,
    /// Whether the user may change the value.
    pub read_only: bool,
    /// Whether the field is enabled.
    pub enabled: bool,
    /// Whether an enabled, editable field must be non-empty at submission.
    pub required: bool,
    /// An optional well-formedness check on the value.
    pub validator: Option<FieldValidator>,
}

// Compare validator presence only; function-pointer addresses are neither stable debug
// output nor reliable function identity.
impl std::fmt::Debug for FormField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact secure initial values as well as drafts so no debug route exposes plaintext.
        let initial: &dyn std::fmt::Debug = if matches!(self.kind, FieldKind::Secure { .. }) {
            &REDACTED
        } else {
            &self.initial
        };
        f.debug_struct("FormField")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("kind", &self.kind)
            .field("initial", initial)
            .field("helper", &self.helper)
            .field("read_only", &self.read_only)
            .field("enabled", &self.enabled)
            .field("required", &self.required)
            .field("validator", &self.validator.map(|_| "<fn>"))
            .finish()
    }
}

impl PartialEq for FormField {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.kind == other.kind
            && self.initial == other.initial
            && self.helper == other.helper
            && self.read_only == other.read_only
            && self.enabled == other.enabled
            && self.required == other.required
            && self.validator.is_some() == other.validator.is_some()
    }
}

impl Eq for FormField {}

impl FormField {
    /// A single-line text field.
    #[must_use]
    pub fn text(id: impl Into<FieldId>, label: impl Into<String>) -> Self {
        Self::new(
            id,
            label,
            FieldKind::Text { placeholder: None },
            FieldValue::Text(String::new()),
        )
    }

    /// A masked single-line field.
    #[must_use]
    pub fn secure(id: impl Into<FieldId>, label: impl Into<String>) -> Self {
        Self::new(
            id,
            label,
            FieldKind::Secure { placeholder: None },
            FieldValue::secure(String::new()),
        )
    }

    /// A multi-line text field.
    #[must_use]
    pub fn multiline(id: impl Into<FieldId>, label: impl Into<String>, rows: u16) -> Self {
        Self::new(
            id,
            label,
            FieldKind::Multiline {
                placeholder: None,
                // Clamp multiline rows to one because a zero-row editor cannot expose its multiline
                // interaction.
                rows: rows.max(1),
            },
            FieldValue::Text(String::new()),
        )
    }

    /// An on/off field.
    #[must_use]
    pub fn toggle(id: impl Into<FieldId>, label: impl Into<String>, on: bool) -> Self {
        Self::new(id, label, FieldKind::Toggle, FieldValue::Toggle(on))
    }

    /// Constructs a choice field, returning `None` when `options` is empty.
    #[must_use]
    pub fn choice(
        id: impl Into<FieldId>,
        label: impl Into<String>,
        options: Vec<ChoiceOption>,
    ) -> Option<Self> {
        if options.is_empty() {
            return None;
        }
        Some(Self::new(
            id,
            label,
            FieldKind::Choice { options },
            FieldValue::Choice(None),
        ))
    }

    fn new(
        id: impl Into<FieldId>,
        label: impl Into<String>,
        kind: FieldKind,
        initial: FieldValue,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            initial,
            helper: None,
            read_only: false,
            enabled: true,
            required: false,
            validator: None,
        }
    }

    /// Sets the initial value when its shape matches the field kind; ordinary text supplied
    /// to a secure field is moved into secure storage.
    #[must_use]
    pub fn initial(mut self, value: FieldValue) -> Self {
        if let Some(value) = normalize_for_kind(value, &self.kind) {
            self.initial = value;
        }
        self
    }

    /// Sets placeholder text when the field kind supports it and otherwise leaves the field
    /// unchanged.
    #[must_use]
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        match &mut self.kind {
            FieldKind::Text { placeholder }
            | FieldKind::Secure { placeholder }
            | FieldKind::Multiline { placeholder, .. } => *placeholder = Some(text),
            FieldKind::Toggle | FieldKind::Choice { .. } => {}
        }
        self
    }

    /// Add supporting text.
    #[must_use]
    pub fn helper(mut self, text: impl Into<String>) -> Self {
        self.helper = Some(text.into());
        self
    }

    /// Sets whether the field is read-only.
    #[must_use]
    pub const fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Enable or disable the field.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Require a non-empty value before submit.
    #[must_use]
    pub const fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Attaches a validator that runs after required checking on non-empty enabled values.
    #[must_use]
    pub const fn validator(mut self, validator: FieldValidator) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Whether the user may currently change this field's value.
    #[must_use]
    pub const fn is_editable(&self) -> bool {
        self.enabled && !self.read_only
    }

    /// Returns whether Enter submits from this editable field; multiline fields return
    /// `false`.
    #[must_use]
    pub const fn enter_submits(&self) -> bool {
        self.is_editable() && !matches!(self.kind, FieldKind::Multiline { .. })
    }
}

// Move ordinary text into secure custody without cloning its plaintext allocation.
fn normalize_for_kind(value: FieldValue, kind: &FieldKind) -> Option<FieldValue> {
    match (value, kind) {
        (FieldValue::Text(value), FieldKind::Secure { .. }) => Some(FieldValue::secure(value)),
        (value, kind) if value_matches_kind(&value, kind) => Some(value),
        _ => None,
    }
}

fn default_value_for_kind(kind: &FieldKind) -> FieldValue {
    match kind {
        FieldKind::Text { .. } | FieldKind::Multiline { .. } => FieldValue::Text(String::new()),
        FieldKind::Secure { .. } => FieldValue::secure(String::new()),
        FieldKind::Toggle => FieldValue::Toggle(false),
        FieldKind::Choice { .. } => FieldValue::Choice(None),
    }
}

fn value_matches_kind(value: &FieldValue, kind: &FieldKind) -> bool {
    match (value, kind) {
        (FieldValue::Text(_), FieldKind::Text { .. } | FieldKind::Multiline { .. }) => true,
        (FieldValue::Secure(_), FieldKind::Secure { .. }) => true,
        (FieldValue::Toggle(_), FieldKind::Toggle) => true,
        (FieldValue::Choice(None), FieldKind::Choice { .. }) => true,
        (FieldValue::Choice(Some(v)), FieldKind::Choice { options }) => {
            options.iter().any(|o| &o.value == v)
        }
        _ => false,
    }
}

/// Describes a form-model construction error without panicking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormModelError {
    /// Two or more fields share an id.
    DuplicateId(FieldId),
}

impl std::fmt::Display for FormModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateId(id) => {
                write!(
                    f,
                    "duplicate field id `{id}`: two fields would share one draft"
                )
            }
        }
    }
}

impl std::error::Error for FormModelError {}

/// A validated set of fields.
#[derive(Debug, Clone, Default)]
pub struct FormModel {
    fields: Vec<FormField>,
    index: BTreeMap<FieldId, usize>,
}

// Compare only fields because `index` is derived entirely from them.
impl PartialEq for FormModel {
    fn eq(&self, other: &Self) -> bool {
        self.fields == other.fields
    }
}

impl Eq for FormModel {}

impl FormModel {
    /// Validates fields and constructs a model.
    ///
    /// # Errors
    ///
    /// Returns [`FormModelError::DuplicateId`] when field ids are not unique.
    pub fn new(fields: Vec<FormField>) -> Result<Self, FormModelError> {
        // Re-establish kind/value compatibility before retaining caller-mutated public fields.
        let fields: Vec<FormField> = fields
            .into_iter()
            .map(|field| {
                let FormField {
                    id,
                    label,
                    kind,
                    initial,
                    helper,
                    read_only,
                    enabled,
                    required,
                    validator,
                } = field;
                let initial = normalize_for_kind(initial, &kind)
                    .unwrap_or_else(|| default_value_for_kind(&kind));
                FormField {
                    id,
                    label,
                    kind,
                    initial,
                    helper,
                    read_only,
                    enabled,
                    required,
                    validator,
                }
            })
            .collect();

        let mut index: BTreeMap<FieldId, usize> = BTreeMap::new();
        for (position, field) in fields.iter().enumerate() {
            if index.insert(field.id.clone(), position).is_some() {
                return Err(FormModelError::DuplicateId(field.id.clone()));
            }
            // Diagnose an empty required read-only field at construction because runtime validation
            // cannot let an uneditable field block submission.
            debug_assert!(
                !(field.required && field.read_only && field.initial.is_empty()),
                "field `{}` is required and read-only but its initial value is empty: \
                 the user cannot supply a value and validate would deadlock the dialog, \
                 so submit will proceed as if the field were disabled",
                field.id,
            );
        }
        Ok(Self { fields, index })
    }

    /// The fields, in display order.
    #[must_use]
    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    /// How many fields there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the model has no fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Look up a field by id.
    #[must_use]
    pub fn field(&self, id: &FieldId) -> Option<&FormField> {
        self.index.get(id).and_then(|&i| self.fields.get(i))
    }
}

/// Why a submit was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// Which field.
    pub id: FieldId,
    /// What to tell the user, and what AT announces as the field's error.
    pub message: String,
}

/// Owns one presentation’s keyed drafts, validation errors, and secure-field reveal
/// state.
///
/// Compatible drafts survive retargeting; removed fields, errors, and reveal state do
/// not.
#[derive(Clone, PartialEq, Eq)]
pub struct FormSession {
    model: FormModel,
    // Keep drafts ordered so submitted iteration and debug output are deterministic.
    drafts: BTreeMap<FieldId, FieldValue>,
    errors: BTreeMap<FieldId, String>,
    // Store only revealed secure-field ids so absence always means masked.
    revealed: BTreeSet<FieldId>,
}

// Redact secure drafts and secure initial values from every `Debug` path.
impl std::fmt::Debug for FormSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Drafts<'a>(&'a FormSession);
        impl std::fmt::Debug for Drafts<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut map = f.debug_map();
                for (id, value) in &self.0.drafts {
                    if self.0.is_secure(id) {
                        map.entry(id, &REDACTED);
                    } else {
                        map.entry(id, value);
                    }
                }
                map.finish()
            }
        }

        f.debug_struct("FormSession")
            .field("model", &self.model)
            .field("drafts", &Drafts(self))
            .field("errors", &self.errors)
            .field("revealed", &self.revealed)
            .finish()
    }
}

impl FormSession {
    /// Begin a presentation of `model`.
    #[must_use]
    pub fn new(model: FormModel) -> Self {
        let drafts = model
            .fields()
            .iter()
            .map(|f| (f.id.clone(), f.initial.clone()))
            .collect();
        Self {
            model,
            drafts,
            errors: BTreeMap::new(),
            revealed: BTreeSet::new(),
        }
    }

    /// Reopen a submission the application refused after it validated: the
    /// submitted values return as the drafts, and `id` carries `message` as
    /// its error, shown and announced like a validation error until that
    /// field is edited.
    ///
    /// For refusals only the application can make (a name already taken, an
    /// address that cannot be played): the person corrects what they typed
    /// instead of typing it again. A submitted value that no longer fits its
    /// field's kind falls back to the field's initial value.
    #[must_use]
    pub fn refused(
        model: FormModel,
        submission: FormSubmission,
        id: &FieldId,
        message: impl Into<String>,
    ) -> Self {
        let mut session = Self::new(model);
        for field in session.model.fields() {
            if let Some(value) = submission
                .values
                .get(&field.id)
                .filter(|value| value_matches_kind(value, &field.kind))
            {
                session.drafts.insert(field.id.clone(), value.clone());
            }
        }
        if session.model.field(id).is_some() {
            session.errors.insert(id.clone(), message.into());
        }
        session
    }

    fn is_secure(&self, id: &FieldId) -> bool {
        self.model
            .field(id)
            .is_some_and(|field| matches!(field.kind, FieldKind::Secure { .. }))
    }

    /// Returns `true` only when `id` names a currently revealed secure field.
    #[must_use]
    pub fn is_revealed(&self, id: &FieldId) -> bool {
        self.revealed.contains(id)
    }

    /// How many secure fields are currently revealed.
    #[must_use]
    pub fn revealed_count(&self) -> usize {
        self.revealed.len()
    }

    /// Sets a secure field’s reveal state and returns whether it changed.
    ///
    /// Missing, non-secure, and disabled fields are refused without mutation.
    pub fn set_reveal(&mut self, id: &FieldId, revealed: bool) -> bool {
        let Some(field) = self.model.field(id) else {
            return false;
        };
        if !field.enabled || !matches!(field.kind, FieldKind::Secure { .. }) {
            return false;
        }
        if revealed {
            self.revealed.insert(id.clone())
        } else {
            self.revealed.remove(id)
        }
    }

    /// Toggles a secure field’s reveal state and returns whether it changed.
    pub fn toggle_reveal(&mut self, id: &FieldId) -> bool {
        self.set_reveal(id, !self.is_revealed(id))
    }

    /// Re-masks every revealed field.
    pub fn conceal_all(&mut self) {
        self.revealed.clear();
    }

    /// The model being presented.
    #[must_use]
    pub const fn model(&self) -> &FormModel {
        &self.model
    }

    /// The current value of `id`, if the model has that field.
    #[must_use]
    pub fn value(&self, id: &FieldId) -> Option<&FieldValue> {
        self.drafts.get(id)
    }

    /// How many drafts are held.
    #[must_use]
    pub fn draft_count(&self) -> usize {
        self.drafts.len()
    }

    /// Applies an edit and returns whether the value changed.
    ///
    /// Missing, read-only, disabled, and kind-incompatible edits are refused.
    pub fn edit(&mut self, id: &FieldId, value: FieldValue) -> bool {
        let Some(field) = self.model.field(id) else {
            return false;
        };
        let Some(value) = normalize_for_kind(value, &field.kind) else {
            return false;
        };
        if !field.is_editable() {
            // A refused secure edit is already protected and is wiped when this value drops.
            return false;
        }
        let Some(slot) = self.drafts.get_mut(id) else {
            return false;
        };
        if *slot == value {
            return false;
        }
        *slot = value;
        self.errors.remove(id);
        true
    }

    /// Replaces the model while preserving drafts whose ids and control shapes remain
    /// compatible.
    ///
    /// Removed or incompatible drafts are dropped, and errors and reveal state are cleared.
    pub fn retarget(&mut self, model: FormModel) {
        let old_kinds: BTreeMap<&FieldId, &FieldKind> = self
            .model
            .fields()
            .iter()
            .map(|old| (&old.id, &old.kind))
            .collect();
        let mut next = BTreeMap::new();
        for field in model.fields() {
            let carried = self.drafts.get(&field.id).filter(|value| {
                old_kinds
                    .get(&field.id)
                    .is_some_and(|old_kind| old_kind.same_shape(&field.kind))
                    && value_matches_kind(value, &field.kind)
            });
            next.insert(
                field.id.clone(),
                carried.map_or_else(|| field.initial.clone(), Clone::clone),
            );
        }
        self.errors.clear();
        self.revealed.clear();
        self.drafts = next;
        self.model = model;
    }

    /// Returns whether Enter in `id` submits; only editable non-multiline fields do.
    #[must_use]
    pub fn enter_submits(&self, id: &FieldId) -> bool {
        self.model.field(id).is_some_and(FormField::enter_submits)
    }

    /// Returns validation errors in model field order.
    #[must_use]
    pub fn errors(&self) -> Vec<FieldError> {
        self.model
            .fields()
            .iter()
            .filter_map(|field| {
                self.errors.get(&field.id).map(|message| FieldError {
                    id: field.id.clone(),
                    message: message.clone(),
                })
            })
            .collect()
    }

    /// How many fields currently carry an error.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// The error attached to one field, if any.
    #[must_use]
    pub fn error_for(&self, id: &FieldId) -> Option<&str> {
        self.errors.get(id).map(String::as_str)
    }

    /// Validates enabled drafts and records errors.
    ///
    /// Required emptiness takes precedence over custom validation; empty read-only fields
    /// cannot block submission.
    pub fn validate(&mut self) -> bool {
        self.errors = self
            .model
            .fields()
            .iter()
            .filter(|field| field.enabled)
            .filter_map(|field| {
                let value = self.drafts.get(&field.id);
                let empty = value.is_none_or(FieldValue::is_empty);

                if field.required && empty {
                    if field.read_only {
                        return None;
                    }
                    return Some((
                        field.id.clone(),
                        crate::localized!("enzui-form-required", "field" => field.label),
                    ));
                }

                if empty {
                    return None;
                }
                let message = field.validator.and_then(|check| value.and_then(check))?;
                Some((field.id.clone(), message))
            })
            .collect();
        self.errors.is_empty()
    }

    /// Returns the first invalid editable field in model order.
    #[must_use]
    pub fn first_invalid(&self) -> Option<&FieldId> {
        self.model
            .fields()
            .iter()
            .find(|field| field.is_editable() && self.errors.contains_key(&field.id))
            .map(|field| &field.id)
    }

    /// Validates and consumes the session on success, or returns it with errors on failure.
    ///
    /// # Errors
    ///
    /// The session itself, carrying the validation errors, when any field fails.
    pub fn submit(mut self) -> Result<FormSubmission, Self> {
        if !self.validate() {
            return Err(self);
        }
        Ok(FormSubmission {
            values: self.drafts,
        })
    }
}

/// The values a form submitted, captured atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormSubmission {
    values: BTreeMap<FieldId, FieldValue>,
}

impl FormSubmission {
    /// The submitted value for `id`.
    #[must_use]
    pub fn value(&self, id: &FieldId) -> Option<&FieldValue> {
        self.values.get(id)
    }

    /// The submitted text for `id`, if that field was textual.
    #[must_use]
    pub fn text(&self, id: &FieldId) -> Option<&str> {
        self.values.get(id).and_then(FieldValue::as_text)
    }

    /// Every submitted value, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&FieldId, &FieldValue)> {
        self.values.iter()
    }

    /// How many values were submitted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether nothing was submitted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}
