//! Permission policy: decides whether a tool action is allowed, needs the
//! user's approval, or is denied.
//!
//! Pure rules only; persisting "allow always" rules per workspace and asking
//! the user are done elsewhere (`ApprovalPort`, session store).

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Permission mode, cycled by the user with `<leader>p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Ask for approval before writing or executing.
    #[default]
    Ask,
    /// Edit without asking; executing still asks.
    AutoEdit,
    /// Read-only: writing and executing are denied.
    Plan,
}

impl PermissionMode {
    /// All modes, in cycling order.
    pub const ALL: [Self; 3] = [Self::Ask, Self::AutoEdit, Self::Plan];

    /// Next mode in the cycle `ask → auto-edit → plan → ask`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Ask => Self::AutoEdit,
            Self::AutoEdit => Self::Plan,
            Self::Plan => Self::Ask,
        }
    }

    /// Stable name, as shown in the status bar and used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AutoEdit => "auto-edit",
            Self::Plan => "plan",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error parsing a [`PermissionMode`] from text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown permission mode `{0}` (expected ask, auto-edit or plan)")]
pub struct ParsePermissionModeError(pub String);

impl FromStr for PermissionMode {
    type Err = ParsePermissionModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == s)
            .ok_or_else(|| ParsePermissionModeError(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_next_cycles_through_all_modes() {
        let cases = [
            (PermissionMode::Ask, PermissionMode::AutoEdit),
            (PermissionMode::AutoEdit, PermissionMode::Plan),
            (PermissionMode::Plan, PermissionMode::Ask),
        ];
        for (mode, expected) in cases {
            assert_eq!(mode.next(), expected, "{mode}.next()");
        }
    }

    #[test]
    fn mode_display_from_str_and_serde_agree() {
        let cases = [
            (PermissionMode::Ask, "ask"),
            (PermissionMode::AutoEdit, "auto-edit"),
            (PermissionMode::Plan, "plan"),
        ];
        for (mode, text) in cases {
            assert_eq!(mode.to_string(), text);
            assert_eq!(text.parse::<PermissionMode>(), Ok(mode));
            assert_eq!(serde_json::to_value(mode).unwrap(), serde_json::json!(text));
        }
    }

    #[test]
    fn mode_from_str_rejects_unknown_names() {
        for text in ["", "Ask", "auto_edit", "readonly"] {
            assert_eq!(
                text.parse::<PermissionMode>(),
                Err(ParsePermissionModeError(text.to_owned())),
                "{text:?}"
            );
        }
    }

    #[test]
    fn default_mode_is_ask() {
        assert_eq!(PermissionMode::default(), PermissionMode::Ask);
    }
}
