//! `app.opt.rtk`: whether `run_command` runs the compact equivalent of
//! common commands through [rtk](https://github.com/rtk-ai/rtk) to save
//! tokens.
//!
//! ```lua
//! app.opt.rtk = {
//!   enabled = true,   -- off by default
//!   path = "rtk",     -- the binary: a name on PATH or a path
//! }
//! ```
//!
//! rtk is an external binary; when it is enabled but missing, commands
//! run uncompacted and the app says so at startup.

use serde_json::{Map, Value};

use crate::ports::plugin::AppOptions;

/// Key of the rtk options in `app.opt`.
pub const RTK_OPTION: &str = "rtk";

/// Default of [`RtkOptions::path`].
pub const DEFAULT_RTK_PATH: &str = "rtk";

const FIELDS: [&str; 2] = ["enabled", "path"];

/// A malformed `app.opt.rtk`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtkConfigError {
    /// Not a table.
    #[error("app.opt.{RTK_OPTION} must be a table")]
    NotATable,
    /// A field crest does not know.
    #[error("app.opt.{RTK_OPTION}: unknown field `{0}` (known: {known})", known = FIELDS.join(", "))]
    UnknownField(String),
    /// A field of the wrong type.
    #[error("app.opt.{RTK_OPTION}.{field} {problem}")]
    BadField {
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: &'static str,
    },
}

/// The rtk options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtkOptions {
    /// Whether commands go through rtk.
    pub enabled: bool,
    /// The rtk binary.
    pub path: String,
}

impl Default for RtkOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            path: DEFAULT_RTK_PATH.to_owned(),
        }
    }
}

impl RtkOptions {
    /// The `rtk` option of `app.opt`, or the defaults when it is not set.
    ///
    /// # Errors
    ///
    /// [`RtkConfigError`] when it is malformed.
    pub fn from_options(options: &AppOptions) -> Result<Self, RtkConfigError> {
        match options.extra.get(RTK_OPTION) {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(value) => Self::from_json(value),
        }
    }

    /// Parses the `app.opt.rtk` table (`true` is short for
    /// `{ enabled = true }`).
    ///
    /// # Errors
    ///
    /// [`RtkConfigError`] when it is malformed.
    pub fn from_json(value: &Value) -> Result<Self, RtkConfigError> {
        let empty = Map::new();
        let table = match value {
            Value::Bool(enabled) => {
                return Ok(Self {
                    enabled: *enabled,
                    ..Self::default()
                });
            }
            Value::Object(table) => table,
            // `{}` in Lua can come as an empty list.
            Value::Array(items) if items.is_empty() => &empty,
            _ => return Err(RtkConfigError::NotATable),
        };
        if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(RtkConfigError::UnknownField(unknown.clone()));
        }
        let mut out = Self::default();
        if let Some(enabled) = table.get("enabled") {
            out.enabled = enabled.as_bool().ok_or(RtkConfigError::BadField {
                field: "enabled",
                problem: "must be true or false",
            })?;
        }
        if let Some(path) = table.get("path") {
            out.path = path
                .as_str()
                .filter(|p| !p.trim().is_empty())
                .ok_or(RtkConfigError::BadField {
                    field: "path",
                    problem: "must be a non-empty string",
                })?
                .to_owned();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn off_unless_enabled() {
        assert_eq!(
            RtkOptions::from_options(&AppOptions::default()),
            Ok(RtkOptions::default())
        );
        assert!(!RtkOptions::default().enabled);
        assert!(RtkOptions::from_json(&json!(true)).unwrap().enabled);
        assert_eq!(
            RtkOptions::from_json(&json!({ "enabled": true, "path": "/opt/rtk" })),
            Ok(RtkOptions {
                enabled: true,
                path: "/opt/rtk".into()
            })
        );
    }

    #[test]
    fn rejects_malformed_tables() {
        assert_eq!(
            RtkOptions::from_json(&json!("yes")),
            Err(RtkConfigError::NotATable)
        );
        assert_eq!(
            RtkOptions::from_json(&json!({ "ultra": true })),
            Err(RtkConfigError::UnknownField("ultra".into()))
        );
        assert!(RtkOptions::from_json(&json!({ "path": "" })).is_err());
    }
}
