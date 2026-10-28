//! The JSON Schema subset `ask_user` takes, turned into [`FormField`]s.
//!
//! The schema is an object schema whose `properties` are the fields, in
//! order, and whose `required` lists the mandatory ones. Each property is
//! one of:
//!
//! - `{"type": "string"}`, with `minLength` / `maxLength`; with
//!   `"format": "multiline"` (or `"x-multiline": true`) it is a text area;
//!   with `"format": "date"` a date picker and with `"format":
//!   "date-range"` a range picker (answered as `{"start", "end"}`), both
//!   bounded by `formatMinimum` / `formatMaximum` (or `minimum` /
//!   `maximum` given as dates). The formats `email`, `uri` and
//!   `date-time` are accepted and shown as plain text;
//! - `{"type": "number"}` or `{"type": "integer"}`, with `minimum` /
//!   `maximum`;
//! - `{"type": "boolean"}`;
//! - a single choice: `{"type": "string", "enum": [...]}`, with optional
//!   `enumNames` for the labels, or `{"oneOf": [{"const": ..., "title":
//!   ...}]}`;
//! - several choices: `{"type": "array", "items": <a single choice>}`, with
//!   `minItems` / `maxItems`.
//!
//! Every property may have a `title` (the label), a `description` and a
//! `default`.
//!
//! The converter is lenient where a form can be: a round trip to the model
//! costs more than a key it cannot enforce. Counts and bounds written as
//! whole floats (`1.0`) are whole numbers; `exclusiveMinimum` /
//! `exclusiveMaximum` become the next whole number for integers and an
//! inclusive bound for numbers; `"type": ["string", "null"]` is the
//! non-null type; annotation keys ([`IGNORED_KEYS`], any `x-*`) and
//! constraint keys on a field type that does not use them (`minLength` on
//! an integer) are ignored. What a form cannot show (`$ref`, `anyOf`,
//! `allOf`, `const`, nested objects, number enums, free lists, unknown
//! formats) is rejected with a [`SchemaError`] naming the property and
//! what is not supported, so the model can rewrite the call.

use std::fmt;

use serde_json::{Map, Value};

use super::{DateSpan, FieldKind, FormField, FormOption};

/// Why a schema cannot be shown as a form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct SchemaError {
    /// Where the problem is: `schema` or `properties.<name>`.
    pub at: String,
    /// What is wrong.
    pub reason: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.reason)
    }
}

fn error(at: &str, reason: impl Into<String>) -> SchemaError {
    SchemaError {
        at: at.to_owned(),
        reason: reason.into(),
    }
}

/// Keys accepted at the top of the schema.
const SCHEMA_KEYS: &[&str] = &[
    "type",
    "properties",
    "required",
    "title",
    "description",
    "$schema",
    "additionalProperties",
];

/// Keys accepted in a property.
const PROPERTY_KEYS: &[&str] = &[
    "type",
    "title",
    "description",
    "default",
    "enum",
    "enumNames",
    "oneOf",
    "format",
    "x-multiline",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "formatMinimum",
    "formatMaximum",
    "items",
    "minItems",
    "maxItems",
    "uniqueItems",
];

/// Keys a property may carry that the form neither shows nor enforces:
/// annotations, and constraints a form cannot check. Ignored, not
/// rejected.
const IGNORED_KEYS: &[&str] = &[
    "examples",
    "$comment",
    "$id",
    "readOnly",
    "writeOnly",
    "deprecated",
    "nullable",
    "pattern",
    "multipleOf",
    "contentMediaType",
    "contentEncoding",
];

/// Whether `key` is dropped silently wherever it appears.
fn ignored(key: &str) -> bool {
    IGNORED_KEYS.contains(&key) || (key.starts_with("x-") && key != "x-multiline")
}

/// String formats shown as one line of plain text.
const PLAIN_FORMATS: &[&str] = &["email", "uri", "date-time"];

/// The fields of the object schema `schema`, in the order of its
/// `properties`.
///
/// # Errors
///
/// [`SchemaError`] for the first construct outside the subset: a schema
/// that is not an object with properties, an unknown key, a `required`
/// name that is not a property, a type that does not fit its default or
/// bounds.
pub fn fields_from_schema(schema: &Value) -> Result<Vec<FormField>, SchemaError> {
    let at = "schema";
    let object = schema
        .as_object()
        .ok_or_else(|| error(at, "must be a JSON object"))?;
    check_keys(at, object, SCHEMA_KEYS)?;
    if let Some(kind) = object.get("type")
        && kind != "object"
    {
        return Err(error(at, format!("`type` must be \"object\", not {kind}")));
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| error(at, "needs `properties`, an object with one entry per field"))?;
    if properties.is_empty() {
        return Err(error(at, "`properties` is empty: a form needs a field"));
    }
    let required = match object.get("required") {
        None => Vec::new(),
        Some(Value::Array(names)) => names
            .iter()
            .map(|name| {
                name.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error(at, "`required` must list property names"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(error(at, "`required` must be an array of property names")),
    };
    if let Some(unknown) = required.iter().find(|name| !properties.contains_key(*name)) {
        return Err(error(
            at,
            format!("`required` names {unknown:?}, which is not a property"),
        ));
    }
    properties
        .iter()
        .map(|(name, property)| {
            let at = format!("properties.{name}");
            field(&at, name, property, required.contains(name))
        })
        .collect()
}

fn check_keys(at: &str, object: &Map<String, Value>, allowed: &[&str]) -> Result<(), SchemaError> {
    match object
        .keys()
        .find(|key| !allowed.contains(&key.as_str()) && !ignored(key))
    {
        Some(key) if key == "const" => Err(error(
            at,
            "`const` is not supported; use `enum` with one value",
        )),
        Some(key) => Err(error(
            at,
            format!(
                "`{key}` is not supported; use only {}",
                allowed
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        None => Ok(()),
    }
}

fn field(at: &str, name: &str, property: &Value, required: bool) -> Result<FormField, SchemaError> {
    let object = property
        .as_object()
        .ok_or_else(|| error(at, "must be a JSON object"))?;
    check_keys(at, object, PROPERTY_KEYS)?;
    let text = |key: &str| -> Result<Option<String>, SchemaError> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(error(at, format!("`{key}` must be a string"))),
        }
    };
    Ok(FormField {
        name: name.to_owned(),
        label: text("title")?,
        description: text("description")?,
        required,
        kind: kind(at, object)?,
    })
}

/// What `type` must be.
const TYPE_HINT: &str = "`type` must be one string (string, number, integer, boolean or array), \
                         or that string with \"null\"";

fn kind(at: &str, object: &Map<String, Value>) -> Result<FieldKind, SchemaError> {
    let choice = object.contains_key("enum") || object.contains_key("oneOf");
    let declared = match object.get("type") {
        None if choice => "string",
        None => return Err(error(at, "needs a `type`")),
        Some(Value::String(kind)) => kind.as_str(),
        // `["string", "null"]`: an optional string.
        Some(Value::Array(types)) if types.iter().all(Value::is_string) => {
            let mut named = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| *t != "null");
            match (named.next(), named.next()) {
                (Some(one), None) => one,
                _ => return Err(error(at, TYPE_HINT)),
            }
        }
        Some(_) => return Err(error(at, TYPE_HINT)),
    };
    match declared {
        "string" if choice => {
            let options = options(at, object)?;
            let default = match object.get("default") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return Err(error(at, "`default` must be one of the options")),
            };
            Ok(FieldKind::Enum { options, default })
        }
        "string" => string(at, object),
        "number" | "integer" => {
            // `exclusiveMinimum: true` (draft 4) only qualifies `minimum`.
            let numeric = |key: &str| match object.get(key) {
                None | Some(Value::Null | Value::Bool(_)) => None,
                Some(value) => Some(value),
            };
            let bound = |key: &str| -> Result<Option<f64>, SchemaError> {
                match numeric(key) {
                    None => Ok(None),
                    Some(Value::Number(n)) => Ok(n.as_f64()),
                    Some(_) => Err(error(at, format!("`{key}` must be a number"))),
                }
            };
            if declared == "number" {
                // An exclusive bound is kept inclusive: a form cannot show
                // "more than 0" better than "at least 0".
                return Ok(FieldKind::Number {
                    default: bound("default")?,
                    min: bound("minimum")?.or(bound("exclusiveMinimum")?),
                    max: bound("maximum")?.or(bound("exclusiveMaximum")?),
                });
            }
            let whole = |key: &str| -> Result<Option<i64>, SchemaError> {
                match numeric(key) {
                    None => Ok(None),
                    Some(Value::Number(n)) => whole_number(n)
                        .map(Some)
                        .ok_or_else(|| error(at, format!("`{key}` must be a whole number"))),
                    Some(_) => Err(error(at, format!("`{key}` must be a whole number"))),
                }
            };
            // The next whole number inside an exclusive bound.
            let exclusive = |key: &str, step: i64| -> Result<Option<i64>, SchemaError> {
                Ok(whole(key)?.map(|n| n.saturating_add(step)))
            };
            Ok(FieldKind::Integer {
                default: whole("default")?,
                min: whole("minimum")?.or(exclusive("exclusiveMinimum", 1)?),
                max: whole("maximum")?.or(exclusive("exclusiveMaximum", -1)?),
            })
        }
        "boolean" => {
            let default = match object.get("default") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => return Err(error(at, "`default` must be true or false")),
            };
            Ok(FieldKind::Boolean { default })
        }
        "array" => {
            let items = object
                .get("items")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    error(
                        at,
                        "an array needs `items` with the options (`enum` or `oneOf`)",
                    )
                })?;
            let items_at = format!("{at}.items");
            check_keys(&items_at, items, &["type", "enum", "enumNames", "oneOf"])?;
            if let Some(kind) = items.get("type")
                && kind != "string"
            {
                return Err(error(
                    &items_at,
                    "only arrays of string options are supported (several choices)",
                ));
            }
            if !items.contains_key("enum") && !items.contains_key("oneOf") {
                return Err(error(
                    &items_at,
                    "only arrays of options (`enum` or `oneOf`) are supported, not free lists",
                ));
            }
            let options = options(&items_at, items)?;
            let default = match object.get("default") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| error(at, "`default` must list options"))
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(error(at, "`default` must be an array of options")),
            };
            Ok(FieldKind::MultiEnum {
                options,
                default,
                min_items: count(at, object, "minItems")?,
                max_items: count(at, object, "maxItems")?,
            })
        }
        "object" => Err(error(
            at,
            "nested objects are not supported: flatten them into top-level properties",
        )),
        other => Err(error(
            at,
            format!(
                "type {other:?} is not supported (use string, number, integer, boolean or array)"
            ),
        )),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn f64_to_i64(v: f64) -> i64 {
    v as i64
}

/// `n` as a whole number: an integer, or a float without a fraction
/// (models often write `1.0`).
fn whole_number(n: &serde_json::Number) -> Option<i64> {
    n.as_i64()
        .or_else(|| n.as_f64().filter(|v| v.fract() == 0.0).map(f64_to_i64))
}

/// A length or an item count: a whole number ≥ 0.
fn count(at: &str, object: &Map<String, Value>, key: &str) -> Result<Option<u32>, SchemaError> {
    let bad = || error(at, format!("`{key}` must be a whole number ≥ 0 (1 or 1.0)"));
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => whole_number(n)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(bad),
        Some(_) => Err(bad()),
    }
}

fn string(at: &str, object: &Map<String, Value>) -> Result<FieldKind, SchemaError> {
    let multiline = match (object.get("format"), object.get("x-multiline")) {
        (_, Some(Value::Bool(true))) => true,
        (_, Some(Value::Bool(false)) | None) => match object.get("format") {
            None | Some(Value::Null) => false,
            Some(Value::String(format)) if format == "multiline" => true,
            Some(Value::String(format)) if format == "date" || format == "date-range" => false,
            Some(Value::String(format)) if PLAIN_FORMATS.contains(&format.as_str()) => false,
            Some(other) => {
                return Err(error(
                    at,
                    format!(
                        "format {other} is not supported (use multiline, date, date-range, \
                         email, uri or date-time)"
                    ),
                ));
            }
        },
        (_, Some(_)) => return Err(error(at, "`x-multiline` must be true or false")),
    };
    // ajv's `formatMinimum`, or `minimum` written as a date; anything else
    // there is ignored like any misplaced key.
    let date = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    if object.get("format").and_then(Value::as_str) == Some("date-range") {
        let default = match object.get("default") {
            None | Some(Value::Null) => None,
            Some(value) => Some(DateSpan::from_value(value).ok_or_else(|| {
                error(at, "`default` of a date-range must be {\"start\", \"end\"}")
            })?),
        };
        return Ok(FieldKind::DateRange {
            default,
            min: date("formatMinimum").or_else(|| date("minimum")),
            max: date("formatMaximum").or_else(|| date("maximum")),
        });
    }
    let default = match object.get("default") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(error(at, "`default` must be a string")),
    };
    if multiline {
        return Ok(FieldKind::Multiline {
            default,
            max_length: count(at, object, "maxLength")?,
        });
    }
    if object.get("format").and_then(Value::as_str) == Some("date") {
        return Ok(FieldKind::Date {
            default,
            min: date("formatMinimum").or_else(|| date("minimum")),
            max: date("formatMaximum").or_else(|| date("maximum")),
        });
    }
    Ok(FieldKind::String {
        default,
        min_length: count(at, object, "minLength")?,
        max_length: count(at, object, "maxLength")?,
    })
}

/// The options of a choice: `enum` (labels from `enumNames`) or `oneOf`
/// with `const` and `title`.
fn options(at: &str, object: &Map<String, Value>) -> Result<Vec<FormOption>, SchemaError> {
    let as_string = |value: &Value| -> Result<String, SchemaError> {
        value.as_str().map(str::to_owned).ok_or_else(|| {
            error(
                at,
                format!("option {value} is not a string: options must be strings"),
            )
        })
    };
    match (object.get("enum"), object.get("oneOf")) {
        (Some(_), Some(_)) => Err(error(at, "use either `enum` or `oneOf`, not both")),
        (Some(Value::Array(values)), None) => {
            let labels = match object.get("enumNames") {
                None => Vec::new(),
                Some(Value::Array(names)) if names.len() == values.len() => {
                    names.iter().map(as_string).collect::<Result<_, _>>()?
                }
                Some(_) => {
                    return Err(error(
                        at,
                        "`enumNames` must be strings, one per `enum` value",
                    ));
                }
            };
            values
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    Ok(FormOption {
                        value: as_string(value)?,
                        label: labels.get(i).cloned(),
                    })
                })
                .collect()
        }
        (None, Some(Value::Array(choices))) => choices
            .iter()
            .map(|choice| {
                let choice = choice.as_object().ok_or_else(|| {
                    error(at, "each `oneOf` entry must be {\"const\", \"title\"}")
                })?;
                check_keys(at, choice, &["const", "title", "description"])?;
                let value = choice
                    .get("const")
                    .ok_or_else(|| error(at, "each `oneOf` entry needs a `const`"))?;
                Ok(FormOption {
                    value: as_string(value)?,
                    label: choice
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
            })
            .collect(),
        _ => Err(error(at, "`enum` / `oneOf` must be an array of options")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::form::FormSpec;

    fn convert(schema: Value) -> Result<Vec<FormField>, SchemaError> {
        fields_from_schema(&schema)
    }

    #[test]
    fn every_supported_property_becomes_a_field_in_order() {
        let fields = convert(json!({
            "type": "object",
            "properties": {
                "label": {"type": "string", "title": "Label", "maxLength": 10,
                          "description": "Short name"},
                "notes": {"type": "string", "format": "multiline"},
                "story": {"type": "string", "x-multiline": true, "default": "a\nb"},
                "email": {"type": "string", "format": "email"},
                "alpha": {"type": "number", "default": 0.05, "minimum": 0, "maximum": 1},
                "n": {"type": "integer", "minimum": 1, "default": 3.0},
                "paired": {"type": "boolean", "default": false},
                "test": {"type": "string", "enum": ["welch", "student"],
                         "enumNames": ["Welch t", "Student t"], "default": "welch"},
                "tail": {"oneOf": [{"const": "two", "title": "Two-sided"}, {"const": "less"}]},
                "plots": {"type": "array", "items": {"type": "string", "enum": ["qq", "hist"]},
                          "maxItems": 2, "default": ["qq"], "uniqueItems": true}
            },
            "required": ["label", "test"]
        }))
        .unwrap();
        let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "label", "notes", "story", "email", "alpha", "n", "paired", "test", "tail", "plots"
            ]
        );
        assert_eq!(fields[0].label.as_deref(), Some("Label"));
        assert_eq!(fields[0].description.as_deref(), Some("Short name"));
        assert!(fields[0].required && fields[7].required && !fields[1].required);
        assert_eq!(
            fields[0].kind,
            FieldKind::String {
                default: None,
                min_length: None,
                max_length: Some(10)
            }
        );
        assert!(matches!(fields[1].kind, FieldKind::Multiline { .. }));
        assert!(matches!(
            fields[2].kind,
            FieldKind::Multiline {
                default: Some(_),
                ..
            }
        ));
        assert!(matches!(fields[3].kind, FieldKind::String { .. }));
        assert_eq!(
            fields[4].kind,
            FieldKind::Number {
                default: Some(0.05),
                min: Some(0.0),
                max: Some(1.0)
            }
        );
        assert_eq!(
            fields[5].kind,
            FieldKind::Integer {
                default: Some(3),
                min: Some(1),
                max: None
            }
        );
        assert_eq!(
            fields[6].kind,
            FieldKind::Boolean {
                default: Some(false)
            }
        );
        let FieldKind::Enum { options, default } = &fields[7].kind else {
            panic!("{:?}", fields[7].kind);
        };
        assert_eq!(options[1].label.as_deref(), Some("Student t"));
        assert_eq!(default.as_deref(), Some("welch"));
        let FieldKind::Enum { options, .. } = &fields[8].kind else {
            panic!("{:?}", fields[8].kind);
        };
        assert_eq!(options[0].value, "two");
        assert_eq!(options[0].label.as_deref(), Some("Two-sided"));
        assert_eq!(options[1].label, None);
        assert_eq!(
            fields[9].kind,
            FieldKind::MultiEnum {
                options: vec![
                    FormOption {
                        value: "qq".into(),
                        label: None
                    },
                    FormOption {
                        value: "hist".into(),
                        label: None
                    }
                ],
                default: vec!["qq".into()],
                min_items: None,
                max_items: Some(2)
            }
        );
        let spec = FormSpec {
            title: "t".into(),
            description: None,
            fields,
            submit_label: None,
            state: Default::default(),
            answer: None,
            reply: None,
        };
        assert_eq!(spec.validate(), Ok(()));
    }

    #[test]
    fn unsupported_constructs_are_named() {
        let cases = [
            (json!([1]), "schema", "JSON object"),
            (
                json!({"type": "array", "properties": {}}),
                "schema",
                "\"object\"",
            ),
            (json!({"properties": {}}), "schema", "empty"),
            (json!({"type": "object"}), "schema", "`properties`"),
            (
                json!({"properties": {"a": {"type": "string"}}, "anyOf": []}),
                "schema",
                "`anyOf`",
            ),
            (
                json!({"properties": {"a": {"type": "string"}}, "required": ["b"]}),
                "schema",
                "\"b\"",
            ),
            (
                json!({"properties": {"a": {"$ref": "#/x"}}}),
                "properties.a",
                "`$ref`",
            ),
            (
                json!({"properties": {"a": {}}}),
                "properties.a",
                "needs a `type`",
            ),
            (
                json!({"properties": {"a": {"type": "object"}}}),
                "properties.a",
                "nested",
            ),
            (
                json!({"properties": {"a": {"type": "null"}}}),
                "properties.a",
                "\"null\"",
            ),
            (
                json!({"properties": {"a": {"type": ["string", "number"]}}}),
                "properties.a",
                "one string",
            ),
            (
                json!({"properties": {"a": {"enum": [1, 2]}}}),
                "properties.a",
                "not a string",
            ),
            (
                json!({"properties": {"a": {"const": "x"}}}),
                "properties.a",
                "`enum` with one value",
            ),
            (
                json!({"properties": {"a": {"allOf": [{"type": "string"}]}}}),
                "properties.a",
                "`allOf`",
            ),
            (
                json!({"properties": {"a": {"type": "integer", "default": 1.5}}}),
                "properties.a",
                "whole number",
            ),
            (
                json!({"properties": {"a": {"type": "integer", "exclusiveMinimum": 0.5}}}),
                "properties.a",
                "whole number",
            ),
            (
                json!({"properties": {"a": {"type": "string", "minLength": 1.5}}}),
                "properties.a",
                "1 or 1.0",
            ),
            (
                json!({"properties": {"a": {"type": "string", "minLength": -1}}}),
                "properties.a",
                "1 or 1.0",
            ),
            (
                json!({"properties": {"a": {"type": "string", "format": "uuid"}}}),
                "properties.a",
                "format",
            ),
            (
                json!({"properties": {"a": {"type": "array", "items": {"type": "string"}}}}),
                "properties.a.items",
                "free lists",
            ),
            (
                json!({"properties": {"a": {"type": "array", "items": {"type": "number", "enum": [1]}}}}),
                "properties.a.items",
                "string options",
            ),
            (
                json!({"properties": {"a": {"type": "boolean", "default": "yes"}}}),
                "properties.a",
                "true or false",
            ),
            (
                json!({"properties": {"a": {"enum": ["x"], "enumNames": ["X", "Y"]}}}),
                "properties.a",
                "one per",
            ),
        ];
        for (schema, at, needle) in cases {
            let err = convert(schema.clone()).expect_err(&schema.to_string());
            assert_eq!(err.at, at, "{schema}: {err}");
            assert!(err.to_string().contains(needle), "{schema}: {err}");
        }
    }

    fn one(property: Value) -> FieldKind {
        convert(json!({"properties": {"a": property}}))
            .unwrap_or_else(|e| panic!("{e}"))
            .remove(0)
            .kind
    }

    #[test]
    fn whole_floats_are_counts() {
        assert_eq!(
            one(json!({"type": "string", "minLength": 1.0, "maxLength": 20.0})),
            FieldKind::String {
                default: None,
                min_length: Some(1),
                max_length: Some(20)
            }
        );
        assert_eq!(
            one(json!({"type": "array", "items": {"enum": ["a", "b"]},
                       "minItems": 0.0, "maxItems": 2.0})),
            FieldKind::MultiEnum {
                options: vec![
                    FormOption {
                        value: "a".into(),
                        label: None
                    },
                    FormOption {
                        value: "b".into(),
                        label: None
                    }
                ],
                default: Vec::new(),
                min_items: Some(0),
                max_items: Some(2)
            }
        );
    }

    #[test]
    fn nullable_type_arrays_take_the_non_null_type() {
        assert!(matches!(
            one(json!({"type": ["string", "null"]})),
            FieldKind::String { .. }
        ));
        assert!(matches!(
            one(json!({"type": ["null", "integer"], "minimum": 1})),
            FieldKind::Integer { min: Some(1), .. }
        ));
    }

    #[test]
    fn annotation_and_misplaced_keys_are_ignored() {
        assert_eq!(
            one(
                json!({"type": "integer", "minLength": 1, "pattern": "^a", "examples": [1],
                       "readOnly": false, "x-order": 2, "$comment": "c", "minimum": 1.0})
            ),
            FieldKind::Integer {
                default: None,
                min: Some(1),
                max: None
            }
        );
        assert_eq!(
            one(
                json!({"type": "string", "minimum": 0, "format": "multiline", "minLength": 1,
                       "maxLength": 9})
            ),
            FieldKind::Multiline {
                default: None,
                max_length: Some(9)
            }
        );
        assert!(matches!(
            one(json!({"type": "boolean", "minimum": 0, "nullable": true})),
            FieldKind::Boolean { default: None }
        ));
        // Annotations at the top and in `items` too.
        let fields = convert(json!({
            "$comment": "c", "examples": [{}],
            "properties": {"a": {"type": "array", "items": {"enum": ["x"], "examples": ["x"]}}}
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(fields.len(), 1);
    }

    #[test]
    fn date_formats_become_date_fields_with_their_bounds() {
        assert_eq!(
            one(
                json!({"type": "string", "format": "date", "formatMinimum": "2024-01-01",
                       "default": "2024-06-01"})
            ),
            FieldKind::Date {
                default: Some("2024-06-01".into()),
                min: Some("2024-01-01".into()),
                max: None
            }
        );
        assert_eq!(
            one(
                json!({"type": "string", "format": "date", "minimum": "2024-01-01",
                       "maximum": "2024-12-31"})
            ),
            FieldKind::Date {
                default: None,
                min: Some("2024-01-01".into()),
                max: Some("2024-12-31".into())
            }
        );
        assert!(matches!(
            one(json!({"type": "string", "format": "date-time"})),
            FieldKind::String { .. }
        ));
        assert_eq!(
            one(
                json!({"type": "string", "format": "date-range", "formatMinimum": "2026-01-01",
                       "default": {"start": "2026-05-10", "end": "2026-05-14"}})
            ),
            FieldKind::DateRange {
                default: Some(DateSpan {
                    start: "2026-05-10".into(),
                    end: "2026-05-14".into()
                }),
                min: Some("2026-01-01".into()),
                max: None
            }
        );
        let err = convert(
            json!({"properties": {"a": {"type": "string", "format": "date-range",
                                                     "default": "mañana"}}}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("start"), "{err}");
    }

    #[test]
    fn exclusive_bounds_are_coerced() {
        assert_eq!(
            one(json!({"type": "integer", "exclusiveMinimum": 0, "exclusiveMaximum": 10.0})),
            FieldKind::Integer {
                default: None,
                min: Some(1),
                max: Some(9)
            }
        );
        // `minimum` wins over `exclusiveMinimum`; a draft-4 flag is ignored.
        assert_eq!(
            one(json!({"type": "integer", "minimum": 5, "exclusiveMinimum": true})),
            FieldKind::Integer {
                default: None,
                min: Some(5),
                max: None
            }
        );
        assert_eq!(
            one(json!({"type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 1.5})),
            FieldKind::Number {
                default: None,
                min: Some(0.0),
                max: Some(1.5)
            }
        );
    }
}
