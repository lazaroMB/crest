//! Schema of `plan.json`, the implementation plan of a Spec.
//!
//! The Plan node of the research flow produces it with structured output
//! and validates it before writing; the implementation flow loads it and
//! runs its tasks in dependency order. `plan.md` is always generated from
//! it, never the other way around.

use std::cmp::Ordering;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::ModelRole;
use crate::research::{ConstraintId, TestKind, TestRef};

/// Id of a task of the plan, e.g. `T1`.
///
/// Ids order naturally, so `T2` sorts before `T10`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    /// Creates an id from anything string-like.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Ord for TaskId {
    fn cmp(&self, other: &Self) -> Ordering {
        natural_cmp(&self.0, &other.0)
    }
}

impl PartialOrd for TaskId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compares the text before the trailing digits, then the trailing digits
/// as a number, then the raw strings, so only equal strings are equal.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let split = |s: &str| {
        let head = s.trim_end_matches(|c: char| c.is_ascii_digit());
        let digits = s[head.len()..].trim_start_matches('0');
        (head.to_owned(), digits.len(), digits.to_owned())
    };
    let (ha, la, da) = split(a);
    let (hb, lb, db) = split(b);
    ha.cmp(&hb)
        .then(la.cmp(&lb))
        .then_with(|| da.cmp(&db))
        .then_with(|| a.cmp(b))
}

/// How risky a task is to implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Small, well understood change.
    Low,
    /// Touches several parts or has some unknowns.
    Medium,
    /// Large or uncertain; review closely.
    High,
}

impl Risk {
    /// The snake_case name used in `plan.json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

const fn default_role() -> ModelRole {
    ModelRole::Main
}

/// A task of the plan, implemented and committed on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Unique id, e.g. `T1`.
    pub id: TaskId,
    /// Short title, used in the commit message.
    pub title: String,
    /// What to do, in enough detail to implement it.
    pub description: String,
    /// Tasks that must be done first.
    pub depends_on: Vec<TaskId>,
    /// The only files the task may touch without asking.
    pub files: Vec<String>,
    /// Checks that must pass for the task to be done.
    pub acceptance: Vec<TestRef>,
    /// Model role that implements it.
    #[serde(default = "default_role")]
    pub role: ModelRole,
    /// Implementation risk.
    pub risk: Risk,
}

/// A command that verifies the hypothesis of the Spec once all tasks are
/// done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HypothesisTest {
    /// How to run it.
    pub kind: TestKind,
    /// What to run, e.g. `make bench-sessions`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Expected result, e.g. `p95 < 20ms`.
    pub expect: String,
}

/// A check that proves a constraint of the Spec holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConstraintTest {
    /// Constraint it proves, e.g. `C2`.
    pub constraint: ConstraintId,
    /// How to run it.
    pub kind: TestKind,
    /// What to run, e.g. `cargo test -p store offline`.
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Contents of `plan.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Folder name of the Spec, e.g. `0007-cache-de-sesiones`.
    pub spec: String,
    /// Tasks, in the order they were written.
    pub tasks: Vec<Task>,
    /// Checks of the hypothesis, run after every task.
    #[serde(default)]
    pub hypothesis_tests: Vec<HypothesisTest>,
    /// Checks of the constraints, run after every task.
    #[serde(default)]
    pub constraint_tests: Vec<ConstraintTest>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The `plan.json` example of `docs/PLAN.md`, verbatim.
    pub(super) const PLAN_EXAMPLE: &str = r#"{
  "spec": "0007-cache-de-sesiones",
  "tasks": [
    {
      "id": "T1",
      "title": "Agregar SessionCache al puerto de storage",
      "description": "...",
      "depends_on": [],
      "files": ["crates/app/src/ports/store.rs"],
      "acceptance": [
        { "kind": "test", "ref": "cargo test -p app session_cache" }
      ],
      "role": "main",
      "risk": "low"
    }
  ],
  "hypothesis_tests": [{ "kind": "command", "ref": "make bench-sessions", "expect": "p95 < 20ms" }],
  "constraint_tests": [{ "constraint": "C2", "kind": "test", "ref": "cargo test -p store offline" }]
}"#;

    #[test]
    fn the_documented_example_parses_and_roundtrips() {
        let plan: Plan = serde_json::from_str(PLAN_EXAMPLE).unwrap();
        assert_eq!(plan.spec, "0007-cache-de-sesiones");
        let task = &plan.tasks[0];
        assert_eq!(task.id, TaskId::new("T1"));
        assert_eq!(task.role, ModelRole::Main);
        assert_eq!(task.risk, Risk::Low);
        assert_eq!(
            task.acceptance[0].reference,
            "cargo test -p app session_cache"
        );
        assert_eq!(plan.hypothesis_tests[0].expect, "p95 < 20ms");
        assert_eq!(plan.constraint_tests[0].constraint.to_string(), "C2");

        let original: serde_json::Value = serde_json::from_str(PLAN_EXAMPLE).unwrap();
        assert_eq!(serde_json::to_value(&plan).unwrap(), original);
    }

    #[test]
    fn role_defaults_to_main_and_unknown_fields_are_rejected() {
        let mut task = json!({
            "id": "T1", "title": "t", "description": "d", "depends_on": [],
            "files": ["a.rs"], "acceptance": [], "risk": "high"
        });
        let parsed: Task = serde_json::from_value(task.clone()).unwrap();
        assert_eq!(parsed.role, ModelRole::Main);
        task["extra"] = json!(1);
        assert!(serde_json::from_value::<Task>(task).is_err());
    }

    #[test]
    fn constraint_tests_need_a_constraint_id() {
        let bad = json!({"constraint": "T2", "kind": "test", "ref": "x"});
        assert!(serde_json::from_value::<ConstraintTest>(bad).is_err());
    }

    #[test]
    fn task_ids_order_naturally() {
        let mut ids: Vec<TaskId> = ["T10", "T2", "A1", "T1", "T02", "T"]
            .into_iter()
            .map(TaskId::new)
            .collect();
        ids.sort();
        let sorted: Vec<&str> = ids.iter().map(TaskId::as_str).collect();
        assert_eq!(sorted, ["A1", "T", "T1", "T02", "T2", "T10"]);
    }

    #[test]
    fn plan_schema() {
        insta::assert_json_snapshot!("plan_schema", schemars::schema_for!(Plan));
    }
}
