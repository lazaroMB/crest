//! Schema of `plan.json`, the implementation plan of a Spec.
//!
//! The Plan node of the research flow produces it with structured output
//! and validates it before writing; the implementation flow loads it and
//! runs its tasks in dependency order. `plan.md` is always generated from
//! it, never the other way around.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
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

/// Why a plan cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The plan has no tasks.
    #[error("the plan has no tasks")]
    NoTasks,
    /// The `spec` field is empty.
    #[error("the plan does not name its spec")]
    EmptySpec,
    /// A task has an empty or blank id.
    #[error("task #{position} has an empty id")]
    EmptyTaskId {
        /// 1-based position of the task in `tasks`.
        position: usize,
    },
    /// Two tasks share an id.
    #[error("task id `{id}` is used by more than one task")]
    DuplicateTaskId {
        /// The repeated id.
        id: TaskId,
    },
    /// A task depends on an id that no task has.
    #[error("task `{task}` depends on unknown task `{dependency}`")]
    UnknownDependency {
        /// Task with the bad dependency.
        task: TaskId,
        /// The missing id.
        dependency: TaskId,
    },
    /// A task lists no files, so it could not touch anything.
    #[error("task `{task}` lists no files")]
    NoFiles {
        /// The task.
        task: TaskId,
    },
    /// A task lists an empty or blank file path.
    #[error("task `{task}` lists an empty file path")]
    EmptyFilePath {
        /// The task.
        task: TaskId,
    },
    /// A constraint test names a constraint the research did not define.
    #[error("constraint test refers to unknown constraint `{constraint}`")]
    UnknownConstraint {
        /// The missing constraint.
        constraint: ConstraintId,
    },
    /// The dependencies form a cycle.
    #[error("tasks form a dependency cycle: {}", join_ids(path))]
    Cycle {
        /// The cycle in execution order: each task must run before the
        /// next one, and the last id repeats the first.
        path: Vec<TaskId>,
    },
}

fn join_ids(ids: &[TaskId]) -> String {
    ids.iter()
        .map(TaskId::as_str)
        .collect::<Vec<_>>()
        .join(" -> ")
}

impl Plan {
    /// Checks everything the flows rely on: a named spec, at least one
    /// task, non-empty unique ids, known dependencies without cycles and
    /// at least one non-empty file per task. Constraint ids are already
    /// well formed by type; [`Plan::check_constraints`] checks that they
    /// exist.
    ///
    /// # Errors
    ///
    /// Returns every problem found, in a stable order.
    pub fn validate(&self) -> Result<(), Vec<PlanError>> {
        let mut errors = Vec::new();
        if self.spec.trim().is_empty() {
            errors.push(PlanError::EmptySpec);
        }
        if self.tasks.is_empty() {
            errors.push(PlanError::NoTasks);
        }
        for (i, task) in self.tasks.iter().enumerate() {
            if task.id.as_str().trim().is_empty() {
                errors.push(PlanError::EmptyTaskId { position: i + 1 });
            }
        }
        let duplicates = self.duplicate_ids();
        let unique = duplicates.is_empty();
        errors.extend(
            duplicates
                .into_iter()
                .map(|id| PlanError::DuplicateTaskId { id }),
        );
        errors.extend(self.unknown_dependencies());
        for task in &self.tasks {
            if task.files.is_empty() {
                errors.push(PlanError::NoFiles {
                    task: task.id.clone(),
                });
            } else if task.files.iter().any(|f| f.trim().is_empty()) {
                errors.push(PlanError::EmptyFilePath {
                    task: task.id.clone(),
                });
            }
        }
        if unique && let Err(path) = self.order_ignoring_unknown() {
            errors.push(PlanError::Cycle { path });
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Checks that every constraint test names one of `known`.
    ///
    /// # Errors
    ///
    /// Returns one [`PlanError::UnknownConstraint`] per missing id, without
    /// repeats, in the order the tests list them.
    pub fn check_constraints(&self, known: &[ConstraintId]) -> Result<(), Vec<PlanError>> {
        let mut missing: Vec<ConstraintId> = Vec::new();
        for test in &self.constraint_tests {
            if !known.contains(&test.constraint) && !missing.contains(&test.constraint) {
                missing.push(test.constraint);
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing
                .into_iter()
                .map(|constraint| PlanError::UnknownConstraint { constraint })
                .collect())
        }
    }

    /// The tasks in an order that runs every dependency first; ties are
    /// broken by task id, so the order is stable.
    ///
    /// # Errors
    ///
    /// Returns the first [`PlanError::DuplicateTaskId`] or
    /// [`PlanError::UnknownDependency`] found, since the order is not
    /// defined then, and [`PlanError::Cycle`] when the dependencies loop.
    pub fn topological_order(&self) -> Result<Vec<&Task>, PlanError> {
        if let Some(id) = self.duplicate_ids().into_iter().next() {
            return Err(PlanError::DuplicateTaskId { id });
        }
        if let Some(err) = self.unknown_dependencies().into_iter().next() {
            return Err(err);
        }
        self.order_ignoring_unknown()
            .map_err(|path| PlanError::Cycle { path })
    }

    /// Ids used by more than one task, each once, in natural order.
    fn duplicate_ids(&self) -> Vec<TaskId> {
        let mut seen = BTreeSet::new();
        let mut repeated = BTreeSet::new();
        for task in &self.tasks {
            if !seen.insert(&task.id) {
                repeated.insert(task.id.clone());
            }
        }
        repeated.into_iter().collect()
    }

    /// One error per dependency that names no task, in task order.
    fn unknown_dependencies(&self) -> Vec<PlanError> {
        let ids: BTreeSet<&TaskId> = self.tasks.iter().map(|t| &t.id).collect();
        self.tasks
            .iter()
            .flat_map(|task| {
                task.depends_on
                    .iter()
                    .filter(|dep| !ids.contains(dep))
                    .map(|dep| PlanError::UnknownDependency {
                        task: task.id.clone(),
                        dependency: dep.clone(),
                    })
            })
            .collect()
    }

    /// Kahn's algorithm over the known dependencies, taking the smallest
    /// ready id first. Assumes unique ids; on a cycle returns its path.
    fn order_ignoring_unknown(&self) -> Result<Vec<&Task>, Vec<TaskId>> {
        let index: BTreeMap<&TaskId, usize> = self
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| (&t.id, i))
            .collect();
        let deps: Vec<BTreeSet<usize>> = self
            .tasks
            .iter()
            .map(|t| {
                t.depends_on
                    .iter()
                    .filter_map(|d| index.get(d).copied())
                    .collect()
            })
            .collect();
        let mut dependents = vec![Vec::new(); self.tasks.len()];
        for (task, task_deps) in deps.iter().enumerate() {
            for &dep in task_deps {
                dependents[dep].push(task);
            }
        }
        let mut pending: Vec<usize> = deps.iter().map(BTreeSet::len).collect();
        let mut ready: BTreeSet<&TaskId> = self
            .tasks
            .iter()
            .zip(&pending)
            .filter(|(_, n)| **n == 0)
            .map(|(t, _)| &t.id)
            .collect();
        let mut order = Vec::with_capacity(self.tasks.len());
        while let Some(id) = ready.pop_first() {
            let Some(&i) = index.get(id) else { continue };
            order.push(&self.tasks[i]);
            for &next in &dependents[i] {
                pending[next] -= 1;
                if pending[next] == 0 {
                    ready.insert(&self.tasks[next].id);
                }
            }
        }
        if order.len() == self.tasks.len() {
            Ok(order)
        } else {
            Err(self.cycle_path(&deps, &pending))
        }
    }

    /// Walks from the smallest blocked task through its smallest blocked
    /// dependency until a task repeats; that loop is the cycle.
    fn cycle_path(&self, deps: &[BTreeSet<usize>], pending: &[usize]) -> Vec<TaskId> {
        let blocked = |i: usize| pending[i] > 0;
        let smallest = |candidates: &mut dyn Iterator<Item = usize>| {
            candidates.min_by(|&a, &b| self.tasks[a].id.cmp(&self.tasks[b].id))
        };
        let mut walk: Vec<usize> = Vec::new();
        let mut current = smallest(&mut (0..self.tasks.len()).filter(|&i| blocked(i)));
        while let Some(i) = current {
            if let Some(start) = walk.iter().position(|&w| w == i) {
                let mut cycle: Vec<TaskId> = walk[start..]
                    .iter()
                    .map(|&w| self.tasks[w].id.clone())
                    .collect();
                cycle.push(self.tasks[i].id.clone());
                // The walk follows "depends on"; flip it to execution order.
                cycle.reverse();
                return cycle;
            }
            walk.push(i);
            current = smallest(&mut deps[i].iter().copied().filter(|&d| blocked(d)));
        }
        Vec::new()
    }
}

impl Plan {
    /// Mermaid flowchart of the task dependencies, arrows from each
    /// dependency to the task that needs it.
    ///
    /// Nodes are named `t1`, `t2`, ... by position, so any task id is safe,
    /// and labelled `<id>: <title>`. Dependencies on unknown tasks are left
    /// out; [`Plan::validate`] reports them.
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        let node: BTreeMap<&TaskId, usize> = self
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| (&t.id, i + 1))
            .collect();
        let mut out = String::from("flowchart TD\n");
        for (i, task) in self.tasks.iter().enumerate() {
            let label = mermaid_label(&format!("{}: {}", task.id, task.title));
            out.push_str(&format!("    t{}[\"{label}\"]\n", i + 1));
        }
        for (i, task) in self.tasks.iter().enumerate() {
            let mut seen = BTreeSet::new();
            for dep in task.depends_on.iter().filter(|d| seen.insert(*d)) {
                if let Some(from) = node.get(dep) {
                    out.push_str(&format!("    t{from} --> t{}\n", i + 1));
                }
            }
        }
        out
    }
}

/// Text safe inside a quoted Mermaid label: one line, entities for the
/// characters Mermaid would parse.
fn mermaid_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("#quot;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

impl Plan {
    /// Human-readable `plan.md`, generated from the plan and never parsed
    /// back.
    ///
    /// Tasks follow [`Plan::topological_order`] when it exists and the
    /// written order otherwise, so the output is the same for the same
    /// plan.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let tasks = self
            .topological_order()
            .unwrap_or_else(|_| self.tasks.iter().collect());
        let mut out = format!(
            "# Plan: {}\n\n<!-- Generado desde plan.json; no editar a mano. -->\n\n",
            one_line(&self.spec)
        );
        let mermaid = self.to_mermaid();
        let fence = "`".repeat(longest_tick_run(&mermaid).max(2) + 1);
        out.push_str(&format!(
            "## Dependencias\n\n{fence}mermaid\n{mermaid}{fence}\n\n"
        ));
        out.push_str("## Tareas\n");
        for task in tasks {
            push_task(&mut out, task);
        }
        out.push_str("\n## Pruebas de hipótesis\n\n");
        if self.hypothesis_tests.is_empty() {
            out.push_str("Ninguna.\n");
        } else {
            out.push_str("| Tipo | Comando | Esperado |\n| --- | --- | --- |\n");
            for test in &self.hypothesis_tests {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    kind_name(test.kind),
                    cell(&code(&test.reference)),
                    cell(&test.expect)
                ));
            }
        }
        out.push_str("\n## Pruebas de restricciones\n\n");
        if self.constraint_tests.is_empty() {
            out.push_str("Ninguna.\n");
        } else {
            out.push_str("| Restricción | Tipo | Comando |\n| --- | --- | --- |\n");
            for test in &self.constraint_tests {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    test.constraint,
                    kind_name(test.kind),
                    cell(&code(&test.reference))
                ));
            }
        }
        out
    }
}

fn push_task(out: &mut String, task: &Task) {
    out.push_str(&format!(
        "\n### {}. {}\n\n",
        one_line(task.id.as_str()),
        one_line(&task.title)
    ));
    let description = task.description.trim();
    if !description.is_empty() {
        out.push_str(description);
        out.push_str("\n\n");
    }
    let deps = if task.depends_on.is_empty() {
        "ninguna".to_owned()
    } else {
        task.depends_on
            .iter()
            .map(TaskId::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.push_str(&format!("- **Depende de**: {}\n", one_line(&deps)));
    out.push_str(&format!("- **Rol**: `{}`\n", task.role));
    out.push_str(&format!("- **Riesgo**: {}\n", risk_name(task.risk)));
    out.push_str("- **Archivos**:\n");
    for file in &task.files {
        out.push_str(&format!("  - {}\n", code(file)));
    }
    out.push_str("- **Aceptación**:");
    if task.acceptance.is_empty() {
        out.push_str(" ninguna\n");
    } else {
        out.push('\n');
        for test in &task.acceptance {
            out.push_str(&format!(
                "  - {}: {}\n",
                kind_name(test.kind),
                code(&test.reference)
            ));
        }
    }
}

const fn risk_name(risk: Risk) -> &'static str {
    match risk {
        Risk::Low => "bajo",
        Risk::Medium => "medio",
        Risk::High => "alto",
    }
}

const fn kind_name(kind: TestKind) -> &'static str {
    match kind {
        TestKind::Test => "test",
        TestKind::Command => "command",
    }
}

/// The text on a single line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text safe inside a table cell.
fn cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}

/// An inline code span that survives backticks in `text`.
fn code(text: &str) -> String {
    let text = one_line(text);
    let ticks = "`".repeat(longest_tick_run(&text) + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{text}{pad}{ticks}")
}

fn longest_tick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The `plan.json` example of `docs/flujos.md`, verbatim.
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

    fn task(id: &str, deps: &[&str]) -> Task {
        Task {
            id: TaskId::new(id),
            title: format!("Tarea {id}"),
            description: String::new(),
            depends_on: deps.iter().copied().map(TaskId::new).collect(),
            files: vec![format!("src/{id}.rs")],
            acceptance: vec![],
            role: ModelRole::Main,
            risk: Risk::Low,
        }
    }

    fn plan(tasks: Vec<Task>) -> Plan {
        Plan {
            spec: "0001-demo".into(),
            tasks,
            hypothesis_tests: vec![],
            constraint_tests: vec![],
        }
    }

    fn order(plan: &Plan) -> Vec<&str> {
        plan.topological_order()
            .unwrap()
            .into_iter()
            .map(|t| t.id.as_str())
            .collect()
    }

    fn cycle(plan: &Plan) -> Vec<String> {
        match plan.topological_order() {
            Err(PlanError::Cycle { path }) => path.iter().map(ToString::to_string).collect(),
            other => panic!("expected a cycle, got {other:?}"),
        }
    }

    #[test]
    fn dependencies_run_first_and_ties_go_by_id() {
        let p = plan(vec![
            task("T10", &[]),
            task("T3", &["T1"]),
            task("T2", &["T1"]),
            task("T1", &[]),
            task("T4", &["T3", "T2", "T2"]),
        ]);
        assert_eq!(order(&p), ["T1", "T2", "T3", "T4", "T10"]);
    }

    #[test]
    fn cycles_are_reported_in_execution_order() {
        let p = plan(vec![
            task("T1", &["T3"]),
            task("T2", &["T1"]),
            task("T3", &["T2"]),
            task("T4", &[]),
        ]);
        assert_eq!(cycle(&p), ["T1", "T2", "T3", "T1"]);
        let err = p.topological_order().unwrap_err();
        assert_eq!(
            err.to_string(),
            "tasks form a dependency cycle: T1 -> T2 -> T3 -> T1"
        );
    }

    #[test]
    fn a_task_blocked_behind_a_cycle_is_not_part_of_it() {
        let p = plan(vec![
            task("T1", &["T2"]),
            task("T2", &["T3"]),
            task("T3", &["T2"]),
        ]);
        assert_eq!(cycle(&p), ["T2", "T3", "T2"]);
        assert_eq!(cycle(&plan(vec![task("T1", &["T1"])])), ["T1", "T1"]);
    }

    #[test]
    fn ordering_needs_unique_ids_and_known_dependencies() {
        let dup = plan(vec![task("T1", &[]), task("T1", &[])]);
        assert_eq!(
            dup.topological_order().unwrap_err(),
            PlanError::DuplicateTaskId {
                id: TaskId::new("T1")
            }
        );
        let unknown = plan(vec![task("T1", &["T9"])]);
        assert_eq!(
            unknown.topological_order().unwrap_err().to_string(),
            "task `T1` depends on unknown task `T9`"
        );
    }

    #[test]
    fn the_documented_example_is_valid() {
        let plan: Plan = serde_json::from_str(PLAN_EXAMPLE).unwrap();
        assert_eq!(plan.validate(), Ok(()));
        let c2 = "C2".parse().unwrap();
        assert_eq!(plan.check_constraints(&[c2]), Ok(()));
        assert_eq!(
            plan.check_constraints(&[]),
            Err(vec![PlanError::UnknownConstraint { constraint: c2 }])
        );
    }

    #[test]
    fn a_plan_with_a_cycle_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(PLAN_EXAMPLE).unwrap();
        let mut second = value["tasks"][0].clone();
        second["id"] = json!("T2");
        second["depends_on"] = json!(["T1"]);
        value["tasks"][0]["depends_on"] = json!(["T2"]);
        value["tasks"].as_array_mut().unwrap().push(second);
        let plan: Plan = serde_json::from_value(value).unwrap();
        let path = vec![TaskId::new("T1"), TaskId::new("T2"), TaskId::new("T1")];
        assert_eq!(plan.validate(), Err(vec![PlanError::Cycle { path }]));
    }

    #[test]
    fn validation_reports_every_problem() {
        let mut no_files = task("T2", &["T9"]);
        no_files.files.clear();
        let mut blank_file = task("T3", &[]);
        blank_file.files.push("  ".into());
        let p = Plan {
            spec: " ".into(),
            ..plan(vec![
                task("", &[]),
                task("T1", &[]),
                task("T1", &[]),
                no_files,
                blank_file,
            ])
        };
        assert_eq!(
            p.validate(),
            Err(vec![
                PlanError::EmptySpec,
                PlanError::EmptyTaskId { position: 1 },
                PlanError::DuplicateTaskId {
                    id: TaskId::new("T1")
                },
                PlanError::UnknownDependency {
                    task: TaskId::new("T2"),
                    dependency: TaskId::new("T9"),
                },
                PlanError::NoFiles {
                    task: TaskId::new("T2")
                },
                PlanError::EmptyFilePath {
                    task: TaskId::new("T3")
                },
            ])
        );
        assert_eq!(plan(vec![]).validate(), Err(vec![PlanError::NoTasks]));
    }

    #[test]
    fn mermaid_draws_one_edge_per_known_dependency() {
        let mut t3 = task("T3", &["T1", "T2", "T1", "T9"]);
        t3.title = "Usa \"cache\" <rápido>\nya".into();
        let p = plan(vec![task("T1", &[]), task("T2", &["T1"]), t3]);
        assert_eq!(
            p.to_mermaid(),
            "flowchart TD\n\
             \x20   t1[\"T1: Tarea T1\"]\n\
             \x20   t2[\"T2: Tarea T2\"]\n\
             \x20   t3[\"T3: Usa #quot;cache#quot; #lt;rápido#gt; ya\"]\n\
             \x20   t1 --> t2\n\
             \x20   t1 --> t3\n\
             \x20   t2 --> t3\n"
        );
    }

    #[test]
    fn markdown_of_the_documented_example() {
        let plan: Plan = serde_json::from_str(PLAN_EXAMPLE).unwrap();
        insta::assert_snapshot!("plan_example_markdown", plan.to_markdown());
    }

    #[test]
    fn markdown_follows_dependency_order_and_escapes() {
        let mut t1 = task("T1", &["T2"]);
        t1.title = "Usa la | tabla".into();
        t1.description = "Línea uno.\n\nLínea dos.".into();
        t1.risk = Risk::High;
        t1.role = ModelRole::Plan;
        t1.acceptance = vec![TestRef {
            kind: TestKind::Command,
            reference: "echo `x`".into(),
        }];
        let mut p = plan(vec![t1, task("T2", &[])]);
        p.constraint_tests = vec![ConstraintTest {
            constraint: "C1".parse().unwrap(),
            kind: TestKind::Test,
            reference: "cargo test a|b".into(),
        }];
        insta::assert_snapshot!("plan_ordered_markdown", p.to_markdown());
    }
}
