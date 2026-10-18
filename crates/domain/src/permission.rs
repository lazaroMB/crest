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

/// The kind of side effect a tool has; every tool declares one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ToolEffect {
    /// Reads the workspace or repo (`read_file`, `grep`, `git log`...).
    Read,
    /// Creates or modifies files (`edit_file`, `write_file`).
    Write,
    /// Runs a process (`run_command`).
    Execute,
    /// Talks to the network (web fetch, remote MCP servers).
    Network,
}

/// A concrete action a tool wants to perform, as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ActionRequest {
    /// Tool name, e.g. `edit_file`.
    pub tool: String,
    /// The tool's declared effect.
    pub effect: ToolEffect,
    /// Exact path or command the action applies to.
    pub subject: String,
}

impl ActionRequest {
    /// Builds a request for `tool` with `effect` on `subject`.
    #[must_use]
    pub fn new(tool: impl Into<String>, effect: ToolEffect, subject: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            effect,
            subject: subject.into(),
        }
    }
}

/// Why an action was denied.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DenyReason {
    /// The policy is in plan (read-only) mode.
    PlanMode,
}

impl fmt::Display for DenyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlanMode => f.write_str("plan mode is read-only"),
        }
    }
}

/// Outcome of evaluating an [`ActionRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum Decision {
    /// Run without asking.
    Allow,
    /// Ask the user, showing the diff or exact command.
    Ask,
    /// Refuse; the reason is reported back to the model.
    Deny {
        /// Why it was denied.
        reason: DenyReason,
    },
}

/// Decides, per action, whether to allow, ask or deny.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPolicy {
    /// Current mode.
    pub mode: PermissionMode,
}

impl PermissionPolicy {
    /// A policy in `mode`.
    #[must_use]
    pub const fn new(mode: PermissionMode) -> Self {
        Self { mode }
    }

    /// Advances to the next mode (`<leader>p`) and returns it.
    pub fn cycle_mode(&mut self) -> PermissionMode {
        self.mode = self.mode.next();
        self.mode
    }

    /// Decides what to do with `request`.
    #[must_use]
    pub fn decide(&self, request: &ActionRequest) -> Decision {
        mode_decision(self.mode, request.effect)
    }
}

/// Base decision table, before rules.
const fn mode_decision(mode: PermissionMode, effect: ToolEffect) -> Decision {
    use PermissionMode as M;
    use ToolEffect as E;
    match (effect, mode) {
        (E::Read, _) | (E::Write, M::AutoEdit) => Decision::Allow,
        (E::Write | E::Execute | E::Network, M::Ask) | (E::Execute | E::Network, M::AutoEdit) => {
            Decision::Ask
        }
        (E::Write | E::Execute | E::Network, M::Plan) => Decision::Deny {
            reason: DenyReason::PlanMode,
        },
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

    const PLAN_DENY: Decision = Decision::Deny {
        reason: DenyReason::PlanMode,
    };

    #[test]
    fn base_decision_table_by_mode_and_effect() {
        use Decision::{Allow, Ask};
        use PermissionMode as M;
        use ToolEffect as E;
        let cases = [
            (M::Ask, E::Read, Allow),
            (M::Ask, E::Write, Ask),
            (M::Ask, E::Execute, Ask),
            (M::Ask, E::Network, Ask),
            (M::AutoEdit, E::Read, Allow),
            (M::AutoEdit, E::Write, Allow),
            (M::AutoEdit, E::Execute, Ask),
            (M::AutoEdit, E::Network, Ask),
            (M::Plan, E::Read, Allow),
            (M::Plan, E::Write, PLAN_DENY),
            (M::Plan, E::Execute, PLAN_DENY),
            (M::Plan, E::Network, PLAN_DENY),
        ];
        for (mode, effect, expected) in cases {
            let request = ActionRequest::new("tool", effect, "subject");
            assert_eq!(
                PermissionPolicy::new(mode).decide(&request),
                expected,
                "{mode} / {effect:?}"
            );
        }
    }

    #[test]
    fn cycle_mode_changes_decisions() {
        let mut policy = PermissionPolicy::default();
        let write = ActionRequest::new("edit_file", ToolEffect::Write, "src/main.rs");
        let expected = [Decision::Allow, PLAN_DENY, Decision::Ask];
        for decision in expected {
            policy.cycle_mode();
            assert_eq!(policy.decide(&write), decision, "{}", policy.mode);
        }
    }
}
