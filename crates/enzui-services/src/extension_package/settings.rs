//! Typed extension settings: the schema a manifest declares, saved values,
//! and an editable draft that keeps invalid input visible and applies all
//! fields or none.
//!
//! Render [`SettingsDraft::fields`] with the toolkit's settings widgets, one
//! `preferences_group` per extension: a toggle as a switch
//! `preferences_row`, a choice as a value-menu `preferences_row`, text and
//! integers as a `text_field` row with the field's [`SettingProblem`] as
//! its error caption. Enable Apply only while [`SettingsDraft::apply`]
//! succeeds, then save the values with `Store::set_settings`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The most settings one extension may declare.
pub const SETTING_LIMIT: usize = 16;
/// The longest text setting, in bytes.
pub const TEXT_LIMIT: usize = 1024;

/// One declared setting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingSpec {
    /// Its identifier, unique in the manifest.
    pub id: String,
    /// Its label.
    pub label: String,
    /// Its type, default and bounds.
    pub value: SettingKind,
}

/// A setting's type, default and bounds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SettingKind {
    /// On or off.
    Toggle {
        /// The default.
        default: bool,
    },
    /// One line of text, at most [`TEXT_LIMIT`] bytes.
    Text {
        /// The default.
        default: String,
    },
    /// A whole number in `min..=max`.
    Integer {
        /// The default.
        default: i64,
        /// The smallest value.
        min: i64,
        /// The largest value.
        max: i64,
    },
    /// One of a fixed list.
    Choice {
        /// The default, one of `choices`.
        default: String,
        /// The choices, in display order.
        choices: Vec<String>,
    },
}

impl SettingSpec {
    /// The default as a stored value.
    #[must_use]
    pub fn default_value(&self) -> Value {
        match &self.value {
            SettingKind::Toggle { default } => Value::Bool(*default),
            SettingKind::Text { default } | SettingKind::Choice { default, .. } => {
                Value::String(default.clone())
            }
            SettingKind::Integer { default, .. } => Value::from(*default),
        }
    }

    /// Whether `value` is a valid stored value for this setting.
    #[must_use]
    pub fn accepts(&self, value: &Value) -> bool {
        self.check(value).is_ok()
    }

    fn check(&self, value: &Value) -> Result<(), SettingProblem> {
        match &self.value {
            SettingKind::Toggle { .. } if value.is_boolean() => Ok(()),
            SettingKind::Text { .. } => match value.as_str() {
                Some(text) if text.len() > TEXT_LIMIT => Err(SettingProblem::TooLong),
                Some(text) if text.chars().any(char::is_control) => {
                    Err(SettingProblem::ControlCharacter)
                }
                Some(_) => Ok(()),
                None => Err(SettingProblem::WrongType),
            },
            SettingKind::Integer { min, max, .. } => match value.as_i64() {
                Some(n) if (*min..=*max).contains(&n) => Ok(()),
                Some(_) => Err(SettingProblem::OutOfRange {
                    min: *min,
                    max: *max,
                }),
                None => Err(SettingProblem::NotANumber),
            },
            SettingKind::Choice { choices, .. } => match value.as_str() {
                Some(choice) if choices.iter().any(|c| c == choice) => Ok(()),
                _ => Err(SettingProblem::NotAChoice),
            },
            SettingKind::Toggle { .. } => Err(SettingProblem::WrongType),
        }
    }
}

/// Validates a manifest's settings list.
pub(super) fn validate(settings: &[SettingSpec]) -> Result<(), String> {
    if settings.len() > SETTING_LIMIT {
        return Err("The extension declares too many settings".into());
    }
    let mut ids = BTreeSet::new();
    for setting in settings {
        if !super::identifier(&setting.id)
            || !ids.insert(&setting.id)
            || !super::display_text(&setting.label, 80)
        {
            return Err("Invalid or duplicate extension setting".into());
        }
        if let SettingKind::Choice { choices, .. } = &setting.value
            && (choices.is_empty()
                || choices.len() > 16
                || choices.iter().any(|c| !super::display_text(c, 80))
                || choices.iter().collect::<BTreeSet<_>>().len() != choices.len())
        {
            return Err("Invalid setting choices".into());
        }
        if let SettingKind::Integer { min, max, .. } = &setting.value
            && min > max
        {
            return Err("Invalid setting range".into());
        }
        if !setting.accepts(&setting.default_value()) {
            return Err("A setting's default is outside its own bounds".into());
        }
    }
    Ok(())
}

/// Every declared setting's value: the saved one when it is still valid,
/// otherwise the default. Saved values for undeclared settings are dropped.
#[must_use]
pub fn resolve(specs: &[SettingSpec], saved: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    specs
        .iter()
        .map(|spec| {
            let value = saved
                .get(&spec.id)
                .filter(|value| spec.accepts(value))
                .cloned()
                .unwrap_or_else(|| spec.default_value());
            (spec.id.clone(), value)
        })
        .collect()
}

/// Checks a complete set of values to save.
///
/// # Errors
///
/// An undeclared setting, or a value its setting rejects.
pub fn check_values(specs: &[SettingSpec], values: &BTreeMap<String, Value>) -> Result<(), String> {
    for (id, value) in values {
        let Some(spec) = specs.iter().find(|spec| spec.id == *id) else {
            return Err(format!("{id:?} is not a setting of this extension"));
        };
        if let Err(problem) = spec.check(value) {
            return Err(format!("{}: {problem}", spec.label));
        }
    }
    Ok(())
}

/// Why a draft field cannot be applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingProblem {
    /// Not a whole number.
    NotANumber,
    /// Outside the setting's bounds.
    OutOfRange {
        /// The smallest value.
        min: i64,
        /// The largest value.
        max: i64,
    },
    /// Longer than [`TEXT_LIMIT`].
    TooLong,
    /// Contains a control character.
    ControlCharacter,
    /// Not one of the choices.
    NotAChoice,
    /// The wrong kind of value for the setting.
    WrongType,
}

impl std::fmt::Display for SettingProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotANumber => f.write_str("Enter a whole number"),
            Self::OutOfRange { min, max } => write!(f, "Enter a number from {min} to {max}"),
            Self::TooLong => write!(f, "Use at most {TEXT_LIMIT} characters"),
            Self::ControlCharacter => f.write_str("Remove line breaks and control characters"),
            Self::NotAChoice => f.write_str("Choose one of the options"),
            Self::WrongType => f.write_str("This value does not fit the setting"),
        }
    }
}

/// One field of a settings form, as the user is editing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftValue {
    /// A toggle.
    Toggle(bool),
    /// Free text, or an integer field's raw text: kept as typed even when
    /// it does not parse, so the field shows what the user wrote.
    Text(String),
    /// A selected choice.
    Choice(String),
}

/// One field of a settings form, ready to render with the toolkit's toggle,
/// text-field, number-field or dropdown widget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingField<'a> {
    /// The declared setting.
    pub spec: &'a SettingSpec,
    /// What the field shows now.
    pub value: &'a DraftValue,
    /// Why it cannot be applied, shown under the field.
    pub problem: Option<SettingProblem>,
    /// Whether it differs from the saved value.
    pub changed: bool,
}

/// An editable copy of an extension's settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsDraft {
    saved: BTreeMap<String, Value>,
    values: BTreeMap<String, DraftValue>,
}

impl SettingsDraft {
    /// A draft of the resolved `saved` values.
    #[must_use]
    pub fn new(specs: &[SettingSpec], saved: &BTreeMap<String, Value>) -> Self {
        let saved = resolve(specs, saved);
        let values = specs
            .iter()
            .map(|spec| (spec.id.clone(), draft_of(spec, &saved[&spec.id])))
            .collect();
        Self { saved, values }
    }

    /// Replaces one field as the user edits it; unknown ids are ignored.
    pub fn edit(&mut self, id: &str, value: DraftValue) {
        if let Some(slot) = self.values.get_mut(id) {
            *slot = value;
        }
    }

    /// Restores every field to the saved value.
    pub fn revert(&mut self, specs: &[SettingSpec]) {
        *self = Self::new(specs, &self.saved.clone());
    }

    /// The form's fields in declaration order.
    #[must_use]
    pub fn fields<'a>(&'a self, specs: &'a [SettingSpec]) -> Vec<SettingField<'a>> {
        specs
            .iter()
            .filter_map(|spec| {
                let value = self.values.get(&spec.id)?;
                let parsed = parse(spec, value);
                Some(SettingField {
                    spec,
                    value,
                    problem: parsed.as_ref().err().copied(),
                    changed: parsed.is_err() || parsed.ok() != self.saved.get(&spec.id).cloned(),
                })
            })
            .collect()
    }

    /// Whether any field differs from the saved values.
    #[must_use]
    pub fn is_changed(&self, specs: &[SettingSpec]) -> bool {
        self.fields(specs).iter().any(|field| field.changed)
    }

    /// Every field's value when all are valid; otherwise each invalid
    /// field's problem, and nothing is applied.
    ///
    /// # Errors
    ///
    /// The invalid fields, by id.
    pub fn apply(
        &self,
        specs: &[SettingSpec],
    ) -> Result<BTreeMap<String, Value>, Vec<(String, SettingProblem)>> {
        let mut values = BTreeMap::new();
        let mut problems = Vec::new();
        for spec in specs {
            let Some(value) = self.values.get(&spec.id) else {
                continue;
            };
            match parse(spec, value) {
                Ok(value) => {
                    values.insert(spec.id.clone(), value);
                }
                Err(problem) => problems.push((spec.id.clone(), problem)),
            }
        }
        if problems.is_empty() {
            Ok(values)
        } else {
            Err(problems)
        }
    }

    /// Records a successful save of `values` as the new saved state.
    pub fn saved(&mut self, specs: &[SettingSpec], values: &BTreeMap<String, Value>) {
        *self = Self::new(specs, values);
    }
}

fn draft_of(spec: &SettingSpec, value: &Value) -> DraftValue {
    match &spec.value {
        SettingKind::Toggle { .. } => DraftValue::Toggle(value.as_bool().unwrap_or_default()),
        SettingKind::Text { .. } => DraftValue::Text(value.as_str().unwrap_or_default().into()),
        SettingKind::Integer { .. } => {
            DraftValue::Text(value.as_i64().map(|n| n.to_string()).unwrap_or_default())
        }
        SettingKind::Choice { .. } => DraftValue::Choice(value.as_str().unwrap_or_default().into()),
    }
}

fn parse(spec: &SettingSpec, draft: &DraftValue) -> Result<Value, SettingProblem> {
    let value = match (&spec.value, draft) {
        (SettingKind::Toggle { .. }, DraftValue::Toggle(on)) => Value::Bool(*on),
        (SettingKind::Text { .. }, DraftValue::Text(text)) => Value::String(text.clone()),
        (SettingKind::Integer { .. }, DraftValue::Text(text)) => Value::from(
            text.trim()
                .parse::<i64>()
                .map_err(|_| SettingProblem::NotANumber)?,
        ),
        (SettingKind::Choice { .. }, DraftValue::Choice(choice)) => Value::String(choice.clone()),
        _ => return Err(SettingProblem::WrongType),
    };
    spec.check(&value)?;
    Ok(value)
}
