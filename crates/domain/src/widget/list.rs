//! A filterable list of records: log lines, HTTP requests, test cases.
//!
//! Each item has a free text, an optional level and timestamp, and named
//! fields. The UI filters items by text (or a `/regex/`) and by chips
//! over the level and the `facets` fields, and shows `columns` fields
//! next to the text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{invalid, unique};
use crate::block::BlockError;

/// Items a list may have.
pub const MAX_ITEMS: usize = 500_000;

const KIND: &str = "filter_list";

/// Severity class of a level name, for colors, ordering and chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LevelClass {
    /// `fatal`, `critical`, `panic`, `emerg`, `alert`.
    Fatal,
    /// `error`, `err`, `fail`.
    Error,
    /// `warn`, `warning`.
    Warn,
    /// `info`, `notice`.
    Info,
    /// `debug`.
    Debug,
    /// `trace`, `verbose`.
    Trace,
    /// Anything else.
    Other,
}

impl LevelClass {
    /// The class of `level`, ignoring case (`WARN`, `Warning`, `E`).
    #[must_use]
    pub fn of(level: &str) -> Self {
        match level.trim().to_ascii_lowercase().as_str() {
            "fatal" | "critical" | "crit" | "panic" | "emerg" | "emergency" | "alert" | "f" => {
                Self::Fatal
            }
            "error" | "err" | "fail" | "failed" | "e" => Self::Error,
            "warn" | "warning" | "w" => Self::Warn,
            "info" | "information" | "notice" | "i" => Self::Info,
            "debug" | "dbg" | "d" => Self::Debug,
            "trace" | "verbose" | "t" | "v" => Self::Trace,
            _ => Self::Other,
        }
    }
}

/// One record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ListItem {
    /// Severity (`error`, `warn`, `info`...); colors the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Timestamp as written by the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    /// The message.
    pub text: String,
    /// Named fields, shown when the row is opened.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub fields: Map<String, Value>,
}

impl ListItem {
    /// Field `name` as text (strings without quotes), or `None`.
    #[must_use]
    pub fn field_text(&self, name: &str) -> Option<String> {
        self.fields.get(name).map(value_text)
    }

    /// The item as one line: `ts LEVEL text key=value...`.
    #[must_use]
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(ts) = &self.ts {
            parts.push(ts.clone());
        }
        if let Some(level) = &self.level {
            parts.push(level.to_ascii_uppercase());
        }
        parts.push(self.text.clone());
        for (key, value) in &self.fields {
            let value = value_text(value);
            if value.contains(char::is_whitespace) || value.is_empty() {
                parts.push(format!("{key}={value:?}"));
            } else {
                parts.push(format!("{key}={value}"));
            }
        }
        parts.join(" ")
    }
}

/// A JSON value as plain text: strings without quotes.
#[must_use]
pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Records with a text filter and facet chips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ListSpec {
    /// Caption above the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Fields shown as columns between the level and the text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// The records, oldest first.
    pub items: Vec<ListItem>,
    /// Fields offered as filter chips (besides the level).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<String>,
    /// Start following the tail (scrolled to the last item).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub follow: bool,
}

impl ListSpec {
    /// Checks the size and that column and facet names are set and
    /// distinct.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.items.len() > MAX_ITEMS {
            return Err(invalid(
                KIND,
                format!(
                    "{} items; at most {MAX_ITEMS} can be shown",
                    self.items.len()
                ),
            ));
        }
        for (what, names) in [("column", &self.columns), ("facet", &self.facets)] {
            if names.iter().any(|n| n.trim().is_empty()) {
                return Err(invalid(KIND, format!("a {what} name is empty")));
            }
            unique(KIND, what, names.iter().map(String::as_str))?;
        }
        Ok(())
    }

    /// The items at `indices` as lines ([`ListItem::line`]).
    #[must_use]
    pub fn to_text(&self, indices: impl IntoIterator<Item = usize>) -> String {
        indices
            .into_iter()
            .filter_map(|ix| self.items.get(ix))
            .map(|item| item.line() + "\n")
            .collect()
    }

    /// Every item as lines.
    #[must_use]
    pub fn all_text(&self) -> String {
        self.to_text(0..self.items.len())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_and_writes_lines() {
        let spec: ListSpec = serde_json::from_value(json!({
            "columns": ["service"],
            "facets": ["service"],
            "items": [
                { "level": "warn", "ts": "12:00:01", "text": "slow query",
                  "fields": { "service": "db", "ms": 812, "sql": "SELECT 1" } },
                { "text": "plain" }
            ]
        }))
        .unwrap();
        spec.validate().unwrap();
        assert_eq!(
            spec.all_text(),
            "12:00:01 WARN slow query service=db ms=812 sql=\"SELECT 1\"\nplain\n"
        );
        assert_eq!(spec.items[0].field_text("ms").as_deref(), Some("812"));
        assert!(!spec.follow);
    }

    #[test]
    fn rejects_bad_names() {
        let mut spec = ListSpec {
            title: None,
            columns: vec!["a".into(), "a".into()],
            items: Vec::new(),
            facets: Vec::new(),
            follow: false,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("appears twice")
        );
        spec.columns = vec![" ".into()];
        assert!(spec.validate().unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn levels_are_classified() {
        assert_eq!(LevelClass::of("WARNING"), LevelClass::Warn);
        assert_eq!(LevelClass::of("err"), LevelClass::Error);
        assert_eq!(LevelClass::of("Critical"), LevelClass::Fatal);
        assert_eq!(LevelClass::of("notice"), LevelClass::Info);
        assert_eq!(LevelClass::of("audit"), LevelClass::Other);
        assert!(LevelClass::Fatal < LevelClass::Trace);
    }
}
