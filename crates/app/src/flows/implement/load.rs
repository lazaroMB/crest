//! Step 1, Cargar plan: reads `plan.json`, validates it, orders its tasks
//! by dependencies and creates the branch of the Spec.

use domain::plan::{Plan, PlanError, TaskId};
use domain::workspace::{PathError, WorkspacePath};

use crate::ports::fs::{FileSystemPort, FsError, ReadFileRequest};
use crate::ports::shell::{RunCommandRequest, ShellError, ShellPort};

/// Why `plan.json` cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// The path is not a workspace path.
    #[error("invalid plan path {path:?}: {source}")]
    Path {
        /// The path given.
        path: String,
        /// Why it is invalid.
        source: PathError,
    },
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The path given.
        path: String,
        /// The file system error.
        source: FsError,
    },
    /// The file is not a plan.
    #[error("{path} is not a valid plan.json: {message}")]
    Parse {
        /// The path given.
        path: String,
        /// The JSON error.
        message: String,
    },
    /// The plan does not validate (see [`Plan::validate`]).
    #[error("the plan is not valid: {}", join_errors(.0))]
    Invalid(Vec<PlanError>),
}

fn join_errors(errors: &[PlanError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A validated plan and its tasks in dependency order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPlan {
    /// The plan.
    pub plan: Plan,
    /// Ids of its tasks, every dependency first.
    pub order: Vec<TaskId>,
}

/// Reads and validates the plan at `path` and orders its tasks.
///
/// # Errors
///
/// [`LoadError`] when the file cannot be read, is not a plan, or the plan
/// does not validate (unknown dependencies, cycles, tasks without files…).
pub async fn load_plan(fs: &dyn FileSystemPort, path: &str) -> Result<LoadedPlan, LoadError> {
    let workspace_path = WorkspacePath::parse(path).map_err(|source| LoadError::Path {
        path: path.to_owned(),
        source,
    })?;
    let mut request = ReadFileRequest::new(workspace_path);
    request.limit = Some(usize::MAX);
    let content = fs
        .read_file(request)
        .await
        .map_err(|source| LoadError::Read {
            path: path.to_owned(),
            source,
        })?;
    parse_plan(path, &content.lines.join("\n"))
}

/// Parses, validates and orders the plan in `json` (read from `path`).
///
/// # Errors
///
/// As [`load_plan`], except reading.
pub fn parse_plan(path: &str, json: &str) -> Result<LoadedPlan, LoadError> {
    let plan: Plan = serde_json::from_str(json).map_err(|e| LoadError::Parse {
        path: path.to_owned(),
        message: e.to_string(),
    })?;
    plan.validate().map_err(LoadError::Invalid)?;
    let order = plan
        .topological_order()
        .map_err(|e| LoadError::Invalid(vec![e]))?
        .into_iter()
        .map(|t| t.id.clone())
        .collect();
    Ok(LoadedPlan { plan, order })
}

/// The branch of the Spec: `spec/<folder>`, with anything git would reject
/// in a ref name replaced by `-`.
#[must_use]
pub fn branch_name(plan: &Plan) -> String {
    let mut name = String::new();
    for c in plan.spec.trim().chars() {
        let ok = c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
        if ok {
            name.push(c);
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.replace("..", ".");
    let name = name.strip_suffix(".lock").unwrap_or(&name);
    let name = name.trim_matches(|c| c == '-' || c == '.');
    if name.is_empty() {
        "spec/plan".to_owned()
    } else {
        format!("spec/{name}")
    }
}

/// What [`create_branch`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchOutcome {
    /// The branch was created from the current `HEAD`.
    Created,
    /// The branch already existed (e.g. a resumed flow) and was checked out.
    Existing,
}

/// Why the branch could not be created.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BranchError {
    /// git could not be run.
    #[error(transparent)]
    Shell(#[from] ShellError),
    /// git refused; its stderr.
    #[error("git switch failed: {0}")]
    Git(String),
}

/// `git switch -c <branch>`, or `git switch <branch>` when it exists.
///
/// # Errors
///
/// [`BranchError`] when git cannot run or refuses both.
pub async fn create_branch(
    shell: &dyn ShellPort,
    branch: &str,
) -> Result<BranchOutcome, BranchError> {
    let create = shell
        .run(RunCommandRequest::new("git", ["switch", "-c", branch]))
        .await?;
    if create.success() {
        return Ok(BranchOutcome::Created);
    }
    let existing = shell
        .run(RunCommandRequest::new("git", ["switch", branch]))
        .await?;
    if existing.success() {
        return Ok(BranchOutcome::Existing);
    }
    let stderr = if existing.stderr.trim().is_empty() {
        create.stderr
    } else {
        existing.stderr
    };
    Err(BranchError::Git(stderr.trim().to_owned()))
}

#[cfg(test)]
pub(crate) mod tests {
    use domain::workspace::WorkspaceRoot;
    use serde_json::json;

    use super::*;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::{InMemoryFs, ScriptedShell};

    /// A plan with tasks `(id, depends_on)`, each with one file and one test.
    pub(crate) fn plan_json(tasks: &[(&str, &[&str])]) -> String {
        let tasks: Vec<_> = tasks
            .iter()
            .map(|(id, deps)| {
                json!({
                    "id": id,
                    "title": format!("Hacer {id}"),
                    "description": format!("Descripción de {id}"),
                    "depends_on": deps,
                    "files": [format!("src/{}.rs", id.to_lowercase())],
                    "acceptance": [{ "kind": "test", "ref": format!("cargo test {}", id.to_lowercase()) }],
                    "role": "main",
                    "risk": "low"
                })
            })
            .collect();
        json!({ "spec": "0007-cache-de-sesiones", "tasks": tasks }).to_string()
    }

    fn fs_with(json: &str) -> InMemoryFs {
        InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
            .with_file(".crest/specs/0007/plan.json", json)
    }

    fn ids(order: &[TaskId]) -> Vec<&str> {
        order.iter().map(TaskId::as_str).collect()
    }

    #[tokio::test]
    async fn orders_tasks_with_every_dependency_first() {
        let json = plan_json(&[
            ("T1", &[]),
            ("T2", &["T3"]),
            ("T3", &["T1"]),
            ("T4", &[]),
            ("T5", &["T2", "T4"]),
        ]);
        let fs = fs_with(&json);
        let loaded = load_plan(&fs, ".crest/specs/0007/plan.json").await.unwrap();
        assert_eq!(ids(&loaded.order), ["T1", "T3", "T2", "T4", "T5"]);
        assert_eq!(loaded.plan.tasks.len(), 5);
    }

    #[tokio::test]
    async fn rejects_cycles_unknown_dependencies_and_bad_json() {
        let cycle = fs_with(&plan_json(&[("T1", &["T2"]), ("T2", &["T1"])]));
        let err = load_plan(&cycle, ".crest/specs/0007/plan.json")
            .await
            .unwrap_err();
        assert!(matches!(&err, LoadError::Invalid(e) if matches!(e[0], PlanError::Cycle { .. })));
        assert!(err.to_string().contains("cycle"), "{err}");

        let unknown = fs_with(&plan_json(&[("T1", &["T9"])]));
        let err = load_plan(&unknown, ".crest/specs/0007/plan.json")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("T9"), "{err}");

        let bad = fs_with("{\"spec\": 1}");
        let err = load_plan(&bad, ".crest/specs/0007/plan.json")
            .await
            .unwrap_err();
        assert!(matches!(err, LoadError::Parse { .. }));

        let err = load_plan(&bad, "otro/plan.json").await.unwrap_err();
        assert!(matches!(err, LoadError::Read { .. }));
        let err = load_plan(&bad, "../plan.json").await.unwrap_err();
        assert!(matches!(err, LoadError::Path { .. }));
    }

    #[test]
    fn branch_names_are_valid_refs() {
        let mut plan = parse_plan("p", &plan_json(&[("T1", &[])])).unwrap().plan;
        assert_eq!(branch_name(&plan), "spec/0007-cache-de-sesiones");
        plan.spec = " 0008 caché: ¿rápida?..lock".into();
        assert_eq!(branch_name(&plan), "spec/0008-caché-rápida");
        plan.spec = "~~".into();
        assert_eq!(branch_name(&plan), "spec/plan");
    }

    fn ok() -> CommandOutput {
        CommandOutput {
            status: Some(0),
            ..CommandOutput::default()
        }
    }

    fn failed(stderr: &str) -> CommandOutput {
        CommandOutput {
            status: Some(128),
            stderr: stderr.into(),
            ..CommandOutput::default()
        }
    }

    #[tokio::test]
    async fn creates_the_branch_or_switches_to_it() {
        let shell = ScriptedShell::new().on("git", ["switch", "-c", "spec/x"], ok());
        assert_eq!(
            create_branch(&shell, "spec/x").await,
            Ok(BranchOutcome::Created)
        );

        let shell = ScriptedShell::new()
            .on("git", ["switch", "-c", "spec/x"], failed("already exists"))
            .on("git", ["switch", "spec/x"], ok());
        assert_eq!(
            create_branch(&shell, "spec/x").await,
            Ok(BranchOutcome::Existing)
        );
        assert_eq!(shell.calls().len(), 2);

        let shell = ScriptedShell::new().with_default(failed("fatal: not a git repository"));
        assert_eq!(
            create_branch(&shell, "spec/x").await,
            Err(BranchError::Git("fatal: not a git repository".into()))
        );
    }
}
