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
    /// The policy is read-only (subagents).
    ReadOnly,
    /// A user rule denies it.
    Rule {
        /// The rule that matched.
        rule: PermissionRule,
    },
}

impl fmt::Display for DenyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlanMode => f.write_str("plan mode is read-only"),
            Self::ReadOnly => f.write_str("this agent is read-only"),
            Self::Rule { rule } => write!(f, "denied by a rule for `{}`", rule.tool),
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

/// Tool name in a [`PermissionRule`] that matches every tool.
pub const ANY_TOOL: &str = "*";

/// Which subjects (paths or commands) a rule covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum RuleMatcher {
    /// Exactly this path or command.
    Exact(String),
    /// This path or command and anything below or after it, on a word or
    /// path boundary: `cargo test` covers `cargo test -p domain` but not
    /// `cargo testx`; `src` and `src/` cover `src/main.rs` but not `srcx`.
    Prefix(String),
    /// Every subject.
    Any,
}

impl RuleMatcher {
    /// Whether `subject` is covered by this matcher.
    #[must_use]
    pub fn matches(&self, subject: &str) -> bool {
        match self {
            Self::Exact(exact) => subject == exact,
            Self::Prefix(prefix) => prefix_matches(prefix, subject),
            Self::Any => true,
        }
    }
}

const fn is_boundary(c: char) -> bool {
    matches!(c, ' ' | '/')
}

fn prefix_matches(prefix: &str, subject: &str) -> bool {
    let Some(rest) = subject.strip_prefix(prefix) else {
        return false;
    };
    rest.is_empty() || prefix.ends_with(is_boundary) || rest.starts_with(is_boundary)
}

/// What a matching rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RuleDecision {
    /// "Allow always": turns an `Ask` into `Allow`.
    AllowAlways,
    /// Always deny, whatever the mode.
    Deny,
}

/// A user rule, persisted per workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRule {
    /// Tool name the rule applies to, or [`ANY_TOOL`].
    pub tool: String,
    /// Subjects covered.
    pub matcher: RuleMatcher,
    /// Effect of the rule.
    pub decision: RuleDecision,
}

impl PermissionRule {
    /// A rule for `tool` on subjects covered by `matcher`.
    #[must_use]
    pub fn new(tool: impl Into<String>, matcher: RuleMatcher, decision: RuleDecision) -> Self {
        Self {
            tool: tool.into(),
            matcher,
            decision,
        }
    }

    /// Whether the rule covers `request`.
    #[must_use]
    pub fn applies_to(&self, request: &ActionRequest) -> bool {
        (self.tool == ANY_TOOL || self.tool == request.tool)
            && self.matcher.matches(&request.subject)
    }
}

/// Decides, per action, whether to allow, ask or deny.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPolicy {
    /// Current mode.
    pub mode: PermissionMode,
    /// User rules, in the order they were added.
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    /// Locked to reads regardless of `mode` and rules (subagents).
    #[serde(default)]
    pub read_only: bool,
}

impl PermissionPolicy {
    /// A policy in `mode`.
    #[must_use]
    pub const fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            rules: Vec::new(),
            read_only: false,
        }
    }

    /// A locked read-only policy for subagents: everything that is not a
    /// read is denied, whatever the mode or rules, and the mode cannot be
    /// cycled.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            mode: PermissionMode::Plan,
            rules: Vec::new(),
            read_only: true,
        }
    }

    /// Adds `rule` ("allow always" or a denial), ignoring exact duplicates.
    /// Persisting it per workspace is the caller's job.
    pub fn remember(&mut self, rule: PermissionRule) {
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
    }

    /// Advances to the next mode (`<leader>p`) and returns it. A
    /// [`read_only`](Self::read_only) policy stays where it is.
    pub fn cycle_mode(&mut self) -> PermissionMode {
        if !self.read_only {
            self.mode = self.mode.next();
        }
        self.mode
    }

    /// Decides what to do with `request`.
    ///
    /// Priority: a matching `Deny` rule always wins (even for reads); then
    /// the mode decides, except that a matching `AllowAlways` rule turns an
    /// `Ask` into `Allow`. `AllowAlways` never lifts a mode denial.
    /// A read-only policy denies every non-read before looking at rules.
    #[must_use]
    pub fn decide(&self, request: &ActionRequest) -> Decision {
        if self.read_only && request.effect != ToolEffect::Read {
            return Decision::Deny {
                reason: DenyReason::ReadOnly,
            };
        }
        let mut matching = self.rules.iter().filter(|rule| rule.applies_to(request));
        if let Some(rule) = matching
            .clone()
            .find(|rule| rule.decision == RuleDecision::Deny)
        {
            return Decision::Deny {
                reason: DenyReason::Rule { rule: rule.clone() },
            };
        }
        match mode_decision(self.mode, request.effect) {
            Decision::Ask if matching.any(|rule| rule.decision == RuleDecision::AllowAlways) => {
                Decision::Allow
            }
            decision => decision,
        }
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

    #[test]
    fn matcher_table() {
        use RuleMatcher::{Any, Exact, Prefix};
        let p = |s: &str| Prefix(s.to_owned());
        let e = |s: &str| Exact(s.to_owned());
        let cases = [
            (e("cargo test"), "cargo test", true),
            (e("cargo test"), "cargo test -p domain", false),
            (e("src/main.rs"), "src/main.rs", true),
            (e("src/main.rs"), "src/main.rs.bak", false),
            (p("cargo test"), "cargo test", true),
            (p("cargo test"), "cargo test -p domain", true),
            (p("cargo test"), "cargo testx", false),
            (p("cargo test"), "cargo tes", false),
            (p("cargo "), "cargo build", true),
            (p("src/"), "src/main.rs", true),
            (p("src/"), "src/a/b.rs", true),
            (p("src"), "src/main.rs", true),
            (p("src"), "srcx/main.rs", false),
            (p("src/"), "src", false),
            (p(""), "anything", false),
            (Any, "anything", true),
            (Any, "", true),
        ];
        for (matcher, subject, expected) in cases {
            assert_eq!(
                matcher.matches(subject),
                expected,
                "{matcher:?} vs {subject:?}"
            );
        }
    }

    #[test]
    fn rule_applies_by_tool_and_subject() {
        let rule = |tool: &str| {
            PermissionRule::new(tool, RuleMatcher::Prefix("src/".into()), RuleDecision::Deny)
        };
        let cases = [
            (rule("edit_file"), "edit_file", "src/lib.rs", true),
            (rule("edit_file"), "write_file", "src/lib.rs", false),
            (rule("edit_file"), "edit_file", "docs/a.md", false),
            (rule(ANY_TOOL), "write_file", "src/lib.rs", true),
            (rule(ANY_TOOL), "write_file", "docs/a.md", false),
        ];
        for (rule, tool, subject, expected) in cases {
            let request = ActionRequest::new(tool, ToolEffect::Write, subject);
            assert_eq!(
                rule.applies_to(&request),
                expected,
                "{rule:?} vs {request:?}"
            );
        }
    }

    #[test]
    fn rule_serde_round_trip() {
        let rule = PermissionRule::new(
            "run_command",
            RuleMatcher::Prefix("cargo test".into()),
            RuleDecision::AllowAlways,
        );
        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "tool": "run_command",
                "matcher": { "kind": "prefix", "value": "cargo test" },
                "decision": "allow-always",
            })
        );
        assert_eq!(
            serde_json::from_value::<PermissionRule>(json).unwrap(),
            rule
        );
        let any = serde_json::to_value(RuleMatcher::Any).unwrap();
        assert_eq!(any, serde_json::json!({ "kind": "any" }));
        assert_eq!(
            serde_json::from_value::<RuleMatcher>(any).unwrap(),
            RuleMatcher::Any
        );
    }

    #[test]
    fn rule_priority_table() {
        use Decision::{Allow, Ask};
        use PermissionMode as M;
        use ToolEffect as E;
        let allow_cargo_test = PermissionRule::new(
            "run_command",
            RuleMatcher::Prefix("cargo test".into()),
            RuleDecision::AllowAlways,
        );
        let deny_rm = PermissionRule::new(
            "run_command",
            RuleMatcher::Prefix("rm".into()),
            RuleDecision::Deny,
        );
        let allow_all_commands =
            PermissionRule::new("run_command", RuleMatcher::Any, RuleDecision::AllowAlways);
        let allow_src = PermissionRule::new(
            ANY_TOOL,
            RuleMatcher::Prefix("src/".into()),
            RuleDecision::AllowAlways,
        );
        let deny_env = PermissionRule::new(
            ANY_TOOL,
            RuleMatcher::Exact(".env".into()),
            RuleDecision::Deny,
        );
        let deny = |rule: &PermissionRule| Decision::Deny {
            reason: DenyReason::Rule { rule: rule.clone() },
        };
        let rules = vec![
            allow_cargo_test.clone(),
            allow_all_commands.clone(),
            deny_rm.clone(),
            allow_src,
            deny_env.clone(),
        ];

        let cases = [
            // AllowAlways turns Ask into Allow.
            (
                M::Ask,
                "run_command",
                E::Execute,
                "cargo test -p domain",
                Allow,
            ),
            (M::AutoEdit, "run_command", E::Execute, "cargo test", Allow),
            (M::Ask, "edit_file", E::Write, "src/lib.rs", Allow),
            // No matching rule: the mode decides.
            (M::Ask, "edit_file", E::Write, "docs/PLAN.md", Ask),
            (M::AutoEdit, "edit_file", E::Write, "docs/PLAN.md", Allow),
            (M::Ask, "web_fetch", E::Network, "https://example.com", Ask),
            // Deny wins over a matching AllowAlways, in every mode.
            (
                M::Ask,
                "run_command",
                E::Execute,
                "rm -rf target",
                deny(&deny_rm),
            ),
            (
                M::AutoEdit,
                "run_command",
                E::Execute,
                "rm -rf target",
                deny(&deny_rm),
            ),
            (
                M::Plan,
                "run_command",
                E::Execute,
                "rm -rf target",
                deny(&deny_rm),
            ),
            // Deny also applies to reads.
            (M::Ask, "read_file", E::Read, ".env", deny(&deny_env)),
            (M::Ask, "read_file", E::Read, "src/lib.rs", Allow),
            // AllowAlways never lifts the plan-mode denial.
            (M::Plan, "run_command", E::Execute, "cargo test", PLAN_DENY),
            (M::Plan, "edit_file", E::Write, "src/lib.rs", PLAN_DENY),
        ];
        for (mode, tool, effect, subject, expected) in cases {
            let policy = PermissionPolicy {
                mode,
                rules: rules.clone(),
                read_only: false,
            };
            let request = ActionRequest::new(tool, effect, subject);
            assert_eq!(policy.decide(&request), expected, "{mode} / {request:?}");
        }
    }

    #[test]
    fn remember_allow_always_skips_future_asks() {
        let mut policy = PermissionPolicy::new(PermissionMode::Ask);
        let request = ActionRequest::new("run_command", ToolEffect::Execute, "cargo test");
        assert_eq!(policy.decide(&request), Decision::Ask);

        let rule = PermissionRule::new(
            "run_command",
            RuleMatcher::Exact("cargo test".into()),
            RuleDecision::AllowAlways,
        );
        policy.remember(rule.clone());
        policy.remember(rule);
        assert_eq!(policy.rules.len(), 1, "duplicates are ignored");
        assert_eq!(policy.decide(&request), Decision::Allow);
        let other = ActionRequest::new("run_command", ToolEffect::Execute, "cargo build");
        assert_eq!(policy.decide(&other), Decision::Ask);
    }

    #[test]
    fn policy_deserializes_without_rules() {
        let policy: PermissionPolicy =
            serde_json::from_value(serde_json::json!({ "mode": "auto-edit" })).unwrap();
        assert_eq!(policy, PermissionPolicy::new(PermissionMode::AutoEdit));
    }

    #[test]
    fn read_only_policy_table() {
        use PermissionMode as M;
        use ToolEffect as E;
        const RO_DENY: Decision = Decision::Deny {
            reason: DenyReason::ReadOnly,
        };
        let allow_everything =
            PermissionRule::new(ANY_TOOL, RuleMatcher::Any, RuleDecision::AllowAlways);
        let deny_env = PermissionRule::new(
            ANY_TOOL,
            RuleMatcher::Exact(".env".into()),
            RuleDecision::Deny,
        );
        let cases = [
            (E::Read, "src/lib.rs", Decision::Allow),
            (
                E::Read,
                ".env",
                Decision::Deny {
                    reason: DenyReason::Rule {
                        rule: deny_env.clone(),
                    },
                },
            ),
            (E::Write, "src/lib.rs", RO_DENY),
            (E::Execute, "cargo test", RO_DENY),
            (E::Network, "https://example.com", RO_DENY),
        ];
        for mode in M::ALL {
            let mut policy = PermissionPolicy::read_only();
            policy.mode = mode;
            policy.remember(allow_everything.clone());
            policy.remember(deny_env.clone());
            for (effect, subject, expected) in &cases {
                let request = ActionRequest::new("tool", *effect, *subject);
                assert_eq!(&policy.decide(&request), expected, "{mode} / {request:?}");
            }
        }
    }

    #[test]
    fn read_only_policy_cannot_cycle_mode() {
        let mut policy = PermissionPolicy::read_only();
        assert_eq!(policy.cycle_mode(), PermissionMode::Plan);
        assert_eq!(policy, PermissionPolicy::read_only());
    }
}
