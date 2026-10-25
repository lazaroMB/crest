//! A form the model asks the user to fill (`ask_user`).
//!
//! The fields are a subset of JSON Schema, close to MCP elicitation's
//! `requestedSchema`: text (one line or several), number, integer, boolean,
//! one choice and several choices, each with a label, a description, a
//! default and bounds. The block keeps its state: pending while the tool
//! waits, then the submitted answer or a cancellation.

use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::block::BlockError;

pub mod schema;

pub use schema::{SchemaError, fields_from_schema};

/// A form shown in the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FormSpec {
    /// Heading.
    pub title: String,
    /// Text under the heading: why the model asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Fields, in display order.
    pub fields: Vec<FormField>,
    /// Text of the submit button; the UI picks one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_label: Option<String>,
    /// Whether the user answered.
    #[serde(default)]
    pub state: FormState,
    /// The submitted values by field name, once submitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<BTreeMap<String, Value>>,
}

/// Lifecycle of a form.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormState {
    /// Waiting for the user.
    #[default]
    Pending,
    /// Submitted; `answer` holds the values.
    Submitted,
    /// Dismissed without answering.
    Cancelled,
}

/// One field of a form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FormField {
    /// Key of the value in the answer.
    pub name: String,
    /// Label shown; the name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Help text under the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the form cannot be submitted without it.
    #[serde(default)]
    pub required: bool,
    /// Input type and its options.
    #[serde(flatten)]
    pub kind: FieldKind,
}

/// The input of a [`FormField`], tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    /// One line of text.
    String {
        /// Initial text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// Shortest accepted length, in characters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_length: Option<u32>,
        /// Longest accepted length, in characters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<u32>,
    },
    /// Several lines of text.
    Multiline {
        /// Initial text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// Longest accepted length, in characters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<u32>,
    },
    /// A real number.
    Number {
        /// Initial value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<f64>,
        /// Smallest accepted value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        /// Largest accepted value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    /// A whole number.
    Integer {
        /// Initial value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<i64>,
        /// Smallest accepted value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        /// Largest accepted value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    /// A checkbox.
    Boolean {
        /// Initial state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<bool>,
    },
    /// One choice among options.
    Enum {
        /// The options.
        options: Vec<FormOption>,
        /// Value of the option selected at first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },
    /// Any number of choices among options.
    MultiEnum {
        /// The options.
        options: Vec<FormOption>,
        /// Values selected at first.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        default: Vec<String>,
        /// Fewest choices accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_items: Option<u32>,
        /// Most choices accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_items: Option<u32>,
    },
}

/// An option of an enum field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FormOption {
    /// Value put in the answer.
    pub value: String,
    /// Text shown; the value when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn invalid(reason: String) -> BlockError {
    BlockError::Invalid {
        kind: "form",
        reason,
    }
}

fn ordered<T: PartialOrd + Copy>(min: Option<T>, value: Option<T>, max: Option<T>) -> bool {
    let le = |a: Option<T>, b: Option<T>| match (a, b) {
        (Some(a), Some(b)) => a <= b,
        _ => true,
    };
    le(min, value) && le(value, max) && le(min, max)
}

impl FormSpec {
    /// Checks that field names are unique and non-empty, every choice field
    /// has distinct options that include its defaults, and bounds are
    /// ordered with the default inside them.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let mut names = HashSet::new();
        for field in &self.fields {
            if field.name.trim().is_empty() {
                return Err(invalid("a field has an empty name".into()));
            }
            if !names.insert(field.name.as_str()) {
                return Err(invalid(format!("field {:?} appears twice", field.name)));
            }
            field.validate()?;
        }
        Ok(())
    }

    /// Checks a submitted answer: every required field present, each value
    /// of the field's type and within its bounds and options, and no
    /// unknown keys.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first bad field.
    pub fn check_answer(&self, answer: &BTreeMap<String, Value>) -> Result<(), BlockError> {
        if let Some(key) = answer
            .keys()
            .find(|k| !self.fields.iter().any(|f| &f.name == *k))
        {
            return Err(invalid(format!("unknown field {key:?}")));
        }
        for field in &self.fields {
            match answer.get(&field.name) {
                None | Some(Value::Null) if field.required => {
                    return Err(invalid(format!("{:?} is required", field.name)));
                }
                None | Some(Value::Null) => {}
                Some(value) => {
                    if !field.kind.accepts(value) {
                        return Err(invalid(format!("{:?} has an invalid value", field.name)));
                    }
                }
            }
        }
        Ok(())
    }
}

impl FormField {
    fn validate(&self) -> Result<(), BlockError> {
        let name = &self.name;
        let ok = match &self.kind {
            FieldKind::String {
                default,
                min_length,
                max_length,
            } => {
                let len = default
                    .as_ref()
                    .map(|d| u32::try_from(d.chars().count()).unwrap_or(u32::MAX));
                ordered(*min_length, len, *max_length)
            }
            FieldKind::Multiline {
                default,
                max_length,
            } => {
                let len = default
                    .as_ref()
                    .map(|d| u32::try_from(d.chars().count()).unwrap_or(u32::MAX));
                ordered(None, len, *max_length)
            }
            FieldKind::Number { default, min, max } => {
                [*default, *min, *max]
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite())
                    && ordered(*min, *default, *max)
            }
            FieldKind::Integer { default, min, max } => ordered(*min, *default, *max),
            FieldKind::Boolean { .. } => true,
            FieldKind::Enum { options, default } => {
                check_options(name, options)?;
                default
                    .as_ref()
                    .is_none_or(|d| options.iter().any(|o| &o.value == d))
            }
            FieldKind::MultiEnum {
                options,
                default,
                min_items,
                max_items,
            } => {
                check_options(name, options)?;
                default
                    .iter()
                    .all(|d| options.iter().any(|o| &o.value == d))
                    && ordered(*min_items, None, *max_items)
            }
        };
        if ok {
            Ok(())
        } else {
            Err(invalid(format!(
                "field {name:?} has bounds out of order or a default outside them"
            )))
        }
    }
}

fn check_options(name: &str, options: &[FormOption]) -> Result<(), BlockError> {
    if options.is_empty() {
        return Err(invalid(format!("field {name:?} has no options")));
    }
    let mut seen = HashSet::new();
    if let Some(dup) = options.iter().find(|o| !seen.insert(o.value.as_str())) {
        return Err(invalid(format!(
            "field {name:?} has option {:?} twice",
            dup.value
        )));
    }
    Ok(())
}

impl FieldKind {
    /// Whether `value` is a valid answer for this input.
    #[must_use]
    pub fn accepts(&self, value: &Value) -> bool {
        let chars = |s: &str| u32::try_from(s.chars().count()).unwrap_or(u32::MAX);
        match (self, value) {
            (
                Self::String {
                    min_length,
                    max_length,
                    ..
                },
                Value::String(s),
            ) => ordered(*min_length, Some(chars(s)), *max_length),
            (Self::Multiline { max_length, .. }, Value::String(s)) => {
                ordered(None, Some(chars(s)), *max_length)
            }
            (Self::Number { min, max, .. }, Value::Number(n)) => n
                .as_f64()
                .is_some_and(|v| v.is_finite() && ordered(*min, Some(v), *max)),
            (Self::Integer { min, max, .. }, Value::Number(n)) => {
                n.as_i64().is_some_and(|v| ordered(*min, Some(v), *max))
            }
            (Self::Boolean { .. }, Value::Bool(_)) => true,
            (Self::Enum { options, .. }, Value::String(s)) => options.iter().any(|o| &o.value == s),
            (
                Self::MultiEnum {
                    options,
                    min_items,
                    max_items,
                    ..
                },
                Value::Array(items),
            ) => {
                let count = u32::try_from(items.len()).unwrap_or(u32::MAX);
                ordered(*min_items, Some(count), *max_items)
                    && items.iter().all(|item| {
                        item.as_str()
                            .is_some_and(|s| options.iter().any(|o| o.value == s))
                    })
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn form() -> FormSpec {
        serde_json::from_value(json!({
            "title": "Run settings",
            "fields": [
                {"name": "label", "type": "string", "required": true, "max_length": 10},
                {"name": "notes", "type": "multiline"},
                {"name": "alpha", "type": "number", "default": 0.05, "min": 0, "max": 1},
                {"name": "n", "type": "integer", "min": 1},
                {"name": "paired", "type": "boolean", "default": false},
                {"name": "test", "type": "enum", "default": "welch",
                 "options": [{"value": "welch", "label": "Welch t"}, {"value": "student"}]},
                {"name": "plots", "type": "multi_enum", "max_items": 2,
                 "options": [{"value": "qq"}, {"value": "hist"}, {"value": "box"}]}
            ]
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn form_parses_with_flattened_field_types() {
        let form = form();
        assert_eq!(form.validate(), Ok(()));
        assert_eq!(form.state, FormState::Pending);
        assert!(matches!(form.fields[6].kind, FieldKind::MultiEnum { .. }));
        let value = serde_json::to_value(&form).unwrap();
        assert_eq!(value["fields"][0]["type"], "string");
        assert_eq!(value["fields"][0]["required"], true);
        let back: FormSpec = serde_json::from_value(value).unwrap();
        assert_eq!(back, form);
    }

    #[test]
    fn validate_rejects_bad_fields() {
        let mut dup = form();
        dup.fields[1].name = "label".into();
        assert!(dup.validate().is_err());
        let mut bad_default = form();
        bad_default.fields[2].kind = FieldKind::Number {
            default: Some(2.0),
            min: Some(0.0),
            max: Some(1.0),
        };
        assert!(bad_default.validate().is_err());
        let mut no_options = form();
        no_options.fields[5].kind = FieldKind::Enum {
            options: vec![],
            default: None,
        };
        assert!(no_options.validate().is_err());
    }

    #[test]
    fn answers_are_checked_against_the_fields() {
        let form = form();
        let answer = |v: Value| -> BTreeMap<String, Value> {
            serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
        };
        let good = answer(json!({"label": "a", "alpha": 0.1, "n": 3, "paired": true,
                                 "test": "student", "plots": ["qq", "box"]}));
        assert_eq!(form.check_answer(&good), Ok(()));
        for bad in [
            json!({"alpha": 0.1}),
            json!({"label": "much too long label"}),
            json!({"label": "a", "alpha": 2}),
            json!({"label": "a", "n": 1.5}),
            json!({"label": "a", "test": "other"}),
            json!({"label": "a", "plots": ["qq", "hist", "box"]}),
            json!({"label": "a", "extra": 1}),
        ] {
            assert!(form.check_answer(&answer(bad.clone())).is_err(), "{bad}");
        }
    }
}
