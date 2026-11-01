//! `app.opt.privacy`: what the redactor masks.
//!
//! ```lua
//! app.opt.privacy = {
//!   enabled = true,                    -- false masks nothing (global config only)
//!   pii = { email = true, card = true, iban = true, national_id = true, phone = true },
//!   patterns = { { name = "ticket", regex = "ACME-[0-9]{6}" } },
//!   allow = { "me@company.com" },      -- values never masked (global config only)
//!   prompts = true,                    -- also mask the user's prompts
//! }
//! ```
//!
//! Every field is optional. A workspace config can only make the
//! protection stricter (see [`tighten`]), so a cloned repository cannot
//! switch it off.

use regex::Regex;
use serde_json::{Map, Value};

use crate::ports::plugin::AppOptions;

/// Key of the privacy options in `app.opt`.
pub const PRIVACY_OPTION: &str = "privacy";

const FIELDS: [&str; 5] = ["enabled", "pii", "patterns", "allow", "prompts"];
const PII_FIELDS: [&str; 5] = ["email", "card", "iban", "national_id", "phone"];

/// A malformed `app.opt.privacy`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrivacyConfigError {
    /// Not a table.
    #[error("app.opt.{PRIVACY_OPTION} must be a table")]
    NotATable,
    /// A field crest does not know.
    #[error("app.opt.{PRIVACY_OPTION}: unknown field `{0}` (known: {known})", known = FIELDS.join(", "))]
    UnknownField(String),
    /// An unknown personal data category.
    #[error("app.opt.{PRIVACY_OPTION}.pii: unknown category `{0}` (known: {known})", known = PII_FIELDS.join(", "))]
    UnknownCategory(String),
    /// A field of the wrong type.
    #[error("app.opt.{PRIVACY_OPTION}.{field} {problem}")]
    BadField {
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: String,
    },
    /// A pattern that does not compile.
    #[error("app.opt.{PRIVACY_OPTION}.patterns: `{name}` is not a valid regex: {message}")]
    BadPattern {
        /// Name of the pattern.
        name: String,
        /// The regex error.
        message: String,
    },
}

/// Which personal data is masked; secrets always are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PiiCategories {
    /// Email addresses (test and service addresses excepted).
    pub email: bool,
    /// Card numbers passing the Luhn check.
    pub card: bool,
    /// IBANs passing the mod-97 check.
    pub iban: bool,
    /// Spanish DNI/NIE (with their control letter) and US SSNs.
    pub national_id: bool,
    /// International phone numbers (`+34 600 123 456`).
    pub phone: bool,
}

impl Default for PiiCategories {
    fn default() -> Self {
        Self {
            email: true,
            card: true,
            iban: true,
            national_id: true,
            phone: true,
        }
    }
}

/// A pattern of the user: the whole match is masked, or its first group
/// when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomPattern {
    /// Tag of the placeholder.
    pub name: String,
    /// The regex (Rust `regex` syntax).
    pub regex: String,
}

impl CustomPattern {
    /// The compiled regex.
    ///
    /// # Errors
    ///
    /// [`PrivacyConfigError::BadPattern`] when it does not compile.
    pub fn compile(&self) -> Result<Regex, PrivacyConfigError> {
        super::compile(&self.regex).map_err(|err| PrivacyConfigError::BadPattern {
            name: self.name.clone(),
            message: err.to_string(),
        })
    }
}

/// The privacy options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyOptions {
    /// Whether anything is masked.
    pub enabled: bool,
    /// Personal data categories.
    pub pii: PiiCategories,
    /// Extra patterns.
    pub patterns: Vec<CustomPattern>,
    /// Values never masked.
    pub allow: Vec<String>,
    /// Whether the user's prompts are masked too.
    pub prompts: bool,
}

impl Default for PrivacyOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            pii: PiiCategories::default(),
            patterns: Vec::new(),
            allow: Vec::new(),
            prompts: true,
        }
    }
}

impl PrivacyOptions {
    /// The `privacy` option of `app.opt`, or the defaults when it is not set.
    ///
    /// # Errors
    ///
    /// [`PrivacyConfigError`] when it is malformed.
    pub fn from_options(options: &AppOptions) -> Result<Self, PrivacyConfigError> {
        match options.extra.get(PRIVACY_OPTION) {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(value) => Self::from_json(value),
        }
    }

    /// Parses the `app.opt.privacy` table, compiling its patterns.
    ///
    /// # Errors
    ///
    /// [`PrivacyConfigError`] when it is malformed.
    pub fn from_json(value: &Value) -> Result<Self, PrivacyConfigError> {
        let empty = Map::new();
        let table = table(value).unwrap_or(&empty);
        if table.is_empty() && !is_table(value) {
            return Err(PrivacyConfigError::NotATable);
        }
        if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(PrivacyConfigError::UnknownField(unknown.clone()));
        }
        let mut out = Self::default();
        if let Some(v) = table.get("enabled") {
            out.enabled = flag(v, "enabled")?;
        }
        if let Some(v) = table.get("prompts") {
            out.prompts = flag(v, "prompts")?;
        }
        if let Some(pii) = table
            .get("pii")
            .map(|v| table_of(v, "pii"))
            .transpose()?
            .flatten()
        {
            for (key, v) in pii {
                let on = flag(v, "pii")?;
                match key.as_str() {
                    "email" => out.pii.email = on,
                    "card" => out.pii.card = on,
                    "iban" => out.pii.iban = on,
                    "national_id" => out.pii.national_id = on,
                    "phone" => out.pii.phone = on,
                    other => return Err(PrivacyConfigError::UnknownCategory(other.to_owned())),
                }
            }
        }
        if let Some(v) = table.get("allow") {
            out.allow = strings(v, "allow")?;
        }
        if let Some(v) = table.get("patterns") {
            out.patterns = patterns(v)?;
        }
        Ok(out)
    }
}

/// The `privacy` value after a workspace layer: the workspace may add
/// patterns, switch categories and prompt masking on, but it cannot turn
/// anything off nor add allowed values. Malformed workspace values are
/// dropped (the parser already reported them).
#[must_use]
pub fn tighten(base: Option<&Value>, workspace: &Value) -> Value {
    let mut out = base.and_then(table).cloned().unwrap_or_default();
    let Some(ws) = table(workspace) else {
        return Value::Object(out);
    };
    if ws.get("prompts") == Some(&Value::Bool(true)) {
        out.insert("prompts".into(), Value::Bool(true));
    }
    if let Some(ws_pii) = ws.get("pii").and_then(table) {
        let mut pii = out.get("pii").and_then(table).cloned().unwrap_or_default();
        for (key, v) in ws_pii {
            if v == &Value::Bool(true) && PII_FIELDS.contains(&key.as_str()) {
                pii.insert(key.clone(), Value::Bool(true));
            }
        }
        out.insert("pii".into(), Value::Object(pii));
    }
    if let Some(Value::Array(extra)) = ws.get("patterns")
        && patterns(&Value::Array(extra.clone())).is_ok()
    {
        let mut list = match out.get("patterns") {
            Some(Value::Array(list)) => list.clone(),
            _ => Vec::new(),
        };
        list.extend(extra.iter().cloned());
        out.insert("patterns".into(), Value::Array(list));
    }
    Value::Object(out)
}

/// `{}` in Lua can come as an empty list.
fn is_table(value: &Value) -> bool {
    matches!(value, Value::Object(_)) || matches!(value, Value::Array(items) if items.is_empty())
}

fn table(value: &Value) -> Option<&Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn table_of<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<Option<&'a Map<String, Value>>, PrivacyConfigError> {
    match value {
        Value::Object(map) => Ok(Some(map)),
        Value::Array(items) if items.is_empty() => Ok(None),
        _ => Err(PrivacyConfigError::BadField {
            field,
            problem: "must be a table".into(),
        }),
    }
}

fn flag(value: &Value, field: &'static str) -> Result<bool, PrivacyConfigError> {
    value.as_bool().ok_or(PrivacyConfigError::BadField {
        field,
        problem: "must be true or false".into(),
    })
}

fn strings(value: &Value, field: &'static str) -> Result<Vec<String>, PrivacyConfigError> {
    let bad = || PrivacyConfigError::BadField {
        field,
        problem: "must be a list of strings".into(),
    };
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(bad))
            .collect(),
        Value::Object(map) if map.is_empty() => Ok(Vec::new()),
        _ => Err(bad()),
    }
}

fn patterns(value: &Value) -> Result<Vec<CustomPattern>, PrivacyConfigError> {
    let bad = || PrivacyConfigError::BadField {
        field: "patterns",
        problem: "must be a list of { name = \"…\", regex = \"…\" }".into(),
    };
    let items = match value {
        Value::Array(items) => items,
        Value::Object(map) if map.is_empty() => return Ok(Vec::new()),
        _ => return Err(bad()),
    };
    items
        .iter()
        .map(|item| {
            let entry = table(item).ok_or_else(bad)?;
            let name = entry.get("name").and_then(Value::as_str);
            let regex = entry.get("regex").and_then(Value::as_str);
            let (Some(name), Some(regex)) = (name, regex) else {
                return Err(bad());
            };
            let valid_name = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !valid_name || entry.len() != 2 {
                return Err(bad());
            }
            let pattern = CustomPattern {
                name: name.to_owned(),
                regex: regex.to_owned(),
            };
            pattern.compile()?;
            Ok(pattern)
        })
        .collect()
}
