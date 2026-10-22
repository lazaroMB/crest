//! `app.opt.shell`: the shell that runs whole command lines (`/!` commands
//! and plan checks with pipes or globs).
//!
//! ```lua
//! app.opt.shell = "pwsh"                                -- a known shell by name
//! app.opt.shell = { program = "bash", args = { "-lc" } } -- anything else
//! ```
//!
//! Unset, it is `sh -c` on Unix and PowerShell on Windows (`pwsh` when
//! installed, else the bundled `powershell`). A name alone gets its usual
//! arguments: `-NoProfile -NonInteractive -Command` for PowerShell, `/C`
//! for `cmd`, `-c` for anything else.

use serde_json::{Map, Value};

use crate::ports::plugin::AppOptions;
use crate::ports::shell::ShellSpec;

/// Key of the shell option in `app.opt`.
pub const SHELL_OPTION: &str = "shell";

const FIELDS: [&str; 2] = ["program", "args"];

/// A malformed `app.opt.shell`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellConfigError {
    /// Neither a string nor a table.
    #[error("app.opt.{SHELL_OPTION} must be a program name or a table")]
    WrongType,
    /// A field crest does not know.
    #[error("app.opt.{SHELL_OPTION}: unknown field `{0}` (known: {known})", known = FIELDS.join(", "))]
    UnknownField(String),
    /// A field of the wrong type, or `program` missing.
    #[error("app.opt.{SHELL_OPTION}.{field} {problem}")]
    BadField {
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: &'static str,
    },
}

/// The configured shell, or `None` when `app.opt.shell` is not set.
///
/// # Errors
///
/// [`ShellConfigError`] when it is malformed.
pub fn from_options(options: &AppOptions) -> Result<Option<ShellSpec>, ShellConfigError> {
    match options.extra.get(SHELL_OPTION) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => from_json(value).map(Some),
    }
}

/// Parses `app.opt.shell`: a program name, or `{ program, args }`.
///
/// # Errors
///
/// [`ShellConfigError`] when it is malformed.
pub fn from_json(value: &Value) -> Result<ShellSpec, ShellConfigError> {
    let empty = Map::new();
    let table = match value {
        Value::String(name) => return named(name),
        Value::Object(table) => table,
        Value::Array(items) if items.is_empty() => &empty,
        _ => return Err(ShellConfigError::WrongType),
    };
    if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(ShellConfigError::UnknownField(unknown.clone()));
    }
    let program = table
        .get("program")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())
        .ok_or(ShellConfigError::BadField {
            field: "program",
            problem: "must be a non-empty string",
        })?;
    let Some(args) = table.get("args") else {
        return named(program);
    };
    let bad_args = ShellConfigError::BadField {
        field: "args",
        problem: "must be a list of strings",
    };
    let args = match args {
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or(bad_args)?,
        // `{}` in Lua can come as an empty table.
        Value::Object(map) if map.is_empty() => Vec::new(),
        _ => return Err(bad_args),
    };
    Ok(ShellSpec {
        program: program.to_owned(),
        args,
    })
}

/// `name` with the arguments it usually takes before a command line.
fn named(name: &str) -> Result<ShellSpec, ShellConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ShellConfigError::BadField {
            field: "program",
            problem: "must be a non-empty string",
        });
    }
    let stem = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
    Ok(match stem {
        "pwsh" | "powershell" => ShellSpec::powershell(name),
        "cmd" => ShellSpec {
            program: name.to_owned(),
            args: vec!["/C".into()],
        },
        _ => ShellSpec {
            program: name.to_owned(),
            args: vec!["-c".into()],
        },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(program: &str, args: &[&str]) -> ShellSpec {
        ShellSpec {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    #[test]
    fn unset_means_the_platform_default() {
        assert_eq!(from_options(&AppOptions::default()), Ok(None));
    }

    #[test]
    fn names_get_their_usual_arguments() {
        assert_eq!(
            from_json(&json!("pwsh")),
            Ok(spec("pwsh", &["-NoProfile", "-NonInteractive", "-Command"]))
        );
        assert_eq!(
            from_json(&json!(r"C:\Windows\System32\cmd.exe")),
            Ok(spec(r"C:\Windows\System32\cmd.exe", &["/C"]))
        );
        assert_eq!(from_json(&json!("bash")), Ok(spec("bash", &["-c"])));
        assert_eq!(
            from_json(&json!({ "program": "zsh" })),
            Ok(spec("zsh", &["-c"]))
        );
    }

    #[test]
    fn tables_give_the_arguments() {
        assert_eq!(
            from_json(&json!({ "program": "bash", "args": ["-lc"] })),
            Ok(spec("bash", &["-lc"]))
        );
        assert_eq!(
            from_json(&json!({ "program": "nu", "args": {} })),
            Ok(spec("nu", &[]))
        );
    }

    #[test]
    fn rejects_malformed_values() {
        assert_eq!(from_json(&json!(3)), Err(ShellConfigError::WrongType));
        assert_eq!(
            from_json(&json!({ "shell": "x" })),
            Err(ShellConfigError::UnknownField("shell".into()))
        );
        assert!(from_json(&json!("")).is_err());
        assert!(from_json(&json!({})).is_err());
        assert!(from_json(&json!({ "program": "sh", "args": [1] })).is_err());
    }
}
