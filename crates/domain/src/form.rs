//! A form the model asks the user to fill (`ask_user`).
//!
//! The fields are a subset of JSON Schema, close to MCP elicitation's
//! `requestedSchema`: text (one line or several), number, integer, date,
//! boolean, one choice and several choices, each with a label, a
//! description, a default and bounds. The block keeps its state: pending
//! while the tool waits, then the submitted answer, the words the user
//! typed in the chat instead, or a cancellation.

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
    /// What the user wrote in the chat instead of filling the form, once
    /// replied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
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
    /// Answered in the chat instead; `reply` holds the words.
    Replied,
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
    /// A calendar date, `YYYY-MM-DD`.
    Date {
        /// Initial date.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// Earliest accepted date.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<String>,
        /// Latest accepted date.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<String>,
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

/// Whether `s` is a calendar date written `YYYY-MM-DD` (proleptic
/// Gregorian: `2024-02-29` is a date, `2023-02-29` is not). Dates written
/// so compare as text, which is what the bounds rely on.
#[must_use]
pub fn is_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        s.get(range)
            .filter(|part| part.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|part| part.parse().ok())
    };
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
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
            FieldKind::Date { default, min, max } => {
                if let Some(bad) = [default, min, max]
                    .into_iter()
                    .flatten()
                    .find(|d| !is_iso_date(d))
                {
                    return Err(invalid(format!(
                        "field {name:?} has date {bad:?}, which is not YYYY-MM-DD"
                    )));
                }
                ordered(min.as_deref(), default.as_deref(), max.as_deref())
            }
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
    /// What this input accepts, said to the user when an answer is
    /// rejected (`Enter a whole number between 1 and 9`).
    #[must_use]
    pub fn hint(&self) -> String {
        /// `between 1 and 5`, `at least 1`, `at most 5`, or nothing.
        fn bounds<T: std::fmt::Display>(min: Option<T>, max: Option<T>) -> Option<String> {
            match (min, max) {
                (Some(min), Some(max)) => Some(format!("between {min} and {max}")),
                (Some(min), None) => Some(format!("at least {min}")),
                (None, Some(max)) => Some(format!("at most {max}")),
                (None, None) => None,
            }
        }
        // `of at least 1` reads better after a noun than `at least 1`.
        let of = |b: String| {
            if b.starts_with("between") {
                b
            } else {
                format!("of {b}")
            }
        };
        match self {
            Self::String {
                min_length,
                max_length,
                ..
            } => match bounds(*min_length, *max_length) {
                Some(b) => format!("Enter {b} characters"),
                None => "Enter some text".to_owned(),
            },
            Self::Multiline { max_length, .. } => match max_length {
                Some(max) => format!("Enter at most {max} characters"),
                None => "Enter some text".to_owned(),
            },
            Self::Number { min, max, .. } => match bounds(*min, *max) {
                Some(b) => format!("Enter a number {}", of(b)),
                None => "Enter a number".to_owned(),
            },
            Self::Integer { min, max, .. } => match bounds(*min, *max) {
                Some(b) => format!("Enter a whole number {}", of(b)),
                None => "Enter a whole number".to_owned(),
            },
            Self::Date { min, max, .. } => match (min, max) {
                (Some(min), Some(max)) => format!("Pick a date between {min} and {max}"),
                (Some(min), None) => format!("Pick a date from {min}"),
                (None, Some(max)) => format!("Pick a date up to {max}"),
                (None, None) => "Pick a date (YYYY-MM-DD)".to_owned(),
            },
            Self::Boolean { .. } => "Answer yes or no".to_owned(),
            Self::Enum { .. } => "Pick one of the options".to_owned(),
            Self::MultiEnum {
                min_items,
                max_items,
                ..
            } => match bounds(*min_items, *max_items) {
                Some(b) => format!("Pick {b} options"),
                None => "Pick any of the options".to_owned(),
            },
        }
    }

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
            (Self::Date { min, max, .. }, Value::String(s)) => {
                is_iso_date(s) && ordered(min.as_deref(), Some(s.as_str()), max.as_deref())
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
                 "options": [{"value": "qq"}, {"value": "hist"}, {"value": "box"}]},
                {"name": "due", "type": "date", "min": "2024-01-01", "max": "2024-12-31"}
            ]
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn iso_dates_are_real_calendar_days() {
        for good in ["2024-02-29", "2000-02-29", "1999-12-31", "2026-04-30"] {
            assert!(is_iso_date(good), "{good}");
        }
        for bad in [
            "2023-02-29",
            "1900-02-29",
            "2024-04-31",
            "2024-13-01",
            "2024-00-10",
            "2024-1-5",
            "24-01-05",
            "2024/01/05",
            "2024-01-05T00:00",
            "yesterday",
            "",
        ] {
            assert!(!is_iso_date(bad), "{bad}");
        }
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
        assert_eq!(value["fields"][7]["type"], "date");
        let back: FormSpec = serde_json::from_value(value).unwrap();
        assert_eq!(back, form);
        let mut replied = form;
        replied.state = FormState::Replied;
        replied.reply = Some("mejor en Python".into());
        let value = serde_json::to_value(&replied).unwrap();
        assert_eq!(value["state"], "replied");
        assert_eq!(value["reply"], "mejor en Python");
        assert_eq!(serde_json::from_value::<FormSpec>(value).unwrap(), replied);
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
        let date = |default: &str, min: &str, max: &str| {
            let mut form = form();
            form.fields[7].kind = FieldKind::Date {
                default: Some(default.into()),
                min: Some(min.into()),
                max: Some(max.into()),
            };
            form.validate()
        };
        assert_eq!(date("2024-06-01", "2024-01-01", "2024-12-31"), Ok(()));
        assert!(date("2025-01-01", "2024-01-01", "2024-12-31").is_err());
        assert!(date("2024-06-01", "2024-12-31", "2024-01-01").is_err());
        let err = date("yesterday", "2024-01-01", "2024-12-31").unwrap_err();
        assert!(err.to_string().contains("YYYY-MM-DD"), "{err}");
    }

    #[test]
    fn answers_are_checked_against_the_fields() {
        let form = form();
        let answer = |v: Value| -> BTreeMap<String, Value> {
            serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
        };
        let good = answer(json!({"label": "a", "alpha": 0.1, "n": 3, "paired": true,
                                 "test": "student", "plots": ["qq", "box"],
                                 "due": "2024-02-29"}));
        assert_eq!(form.check_answer(&good), Ok(()));
        for bad in [
            json!({"alpha": 0.1}),
            json!({"label": "much too long label"}),
            json!({"label": "a", "alpha": 2}),
            json!({"label": "a", "n": 1.5}),
            json!({"label": "a", "test": "other"}),
            json!({"label": "a", "plots": ["qq", "hist", "box"]}),
            json!({"label": "a", "extra": 1}),
            json!({"label": "a", "due": "2023-02-29"}),
            json!({"label": "a", "due": "2024-1-5"}),
            json!({"label": "a", "due": "2025-01-01"}),
            json!({"label": "a", "due": 20240101}),
        ] {
            assert!(form.check_answer(&answer(bad.clone())).is_err(), "{bad}");
        }
    }

    #[test]
    fn hints_say_what_a_field_accepts() {
        let form = form();
        let hints: Vec<String> = form.fields.iter().map(|f| f.kind.hint()).collect();
        assert_eq!(
            hints,
            [
                "Enter at most 10 characters",
                "Enter some text",
                "Enter a number between 0 and 1",
                "Enter a whole number of at least 1",
                "Answer yes or no",
                "Pick one of the options",
                "Pick at most 2 options",
                "Pick a date between 2024-01-01 and 2024-12-31",
            ]
        );
        let open = FieldKind::Date {
            default: None,
            min: None,
            max: None,
        };
        assert_eq!(open.hint(), "Pick a date (YYYY-MM-DD)");
        let from = FieldKind::Date {
            default: None,
            min: Some("2024-01-01".into()),
            max: None,
        };
        assert_eq!(from.hint(), "Pick a date from 2024-01-01");
    }
}
