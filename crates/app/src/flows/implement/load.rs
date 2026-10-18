//! Step 1, Cargar plan: reads `plan.json`, validates it, orders its tasks
//! by dependencies and creates the branch of the Spec.

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, MermaidSpec, TableSpec};
use domain::plan::{Plan, PlanError, TaskId};
use domain::workspace::{PathError, WorkspacePath};

use super::gate::{GateChoice, parse_choice};
use super::node::{ImplementNode, NodeError, StepContext, StepOutcome};
use super::state::{EntryKind, Step};
use crate::ports::events::NoticeLevel;

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

/// Step 1, Cargar plan.
///
/// Loads and validates `plan.json`, shows the tasks in dependency order
/// (a table and the Mermaid graph) and stops at the first gate: «sí»
/// creates the branch `spec/<carpeta>` and starts. A plan that does not
/// validate is a problem of the Spec, so the flow offers to go back there.
#[derive(Debug, Default)]
pub struct LoadNode;

impl LoadNode {
    /// The step.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn show(ctx: &StepContext, plan: &Plan, order: &[TaskId]) -> Result<(), NodeError> {
        let position = |id: &TaskId| order.iter().position(|o| o == id).unwrap_or(0) + 1;
        let mut tasks: Vec<_> = plan.tasks.iter().collect();
        tasks.sort_by_key(|t| position(&t.id));
        let rows = tasks
            .iter()
            .map(|t| {
                let deps: Vec<_> = t.depends_on.iter().map(TaskId::as_str).collect();
                vec![
                    Cell::Number(f64::from(u32::try_from(position(&t.id)).unwrap_or(0))),
                    Cell::Text(t.id.to_string()),
                    Cell::Text(t.title.clone()),
                    Cell::Text(deps.join(", ")),
                    Cell::Text(t.files.join(", ")),
                    Cell::Text(t.role.as_str().to_owned()),
                    Cell::Text(t.risk.as_str().to_owned()),
                ]
            })
            .collect();
        let columns = [
            "#",
            "Tarea",
            "Título",
            "Depende de",
            "Archivos",
            "Rol",
            "Riesgo",
        ]
        .into_iter()
        .map(Column::new)
        .collect();
        ctx.emit(BlockKind::Table(TableSpec { columns, rows }))?;
        ctx.emit(BlockKind::Mermaid(MermaidSpec {
            source: plan.to_mermaid(),
            title: Some(format!("Dependencias de {}", plan.spec)),
        }))?;
        Ok(())
    }

    fn prompt(plan: &Plan, order: &[TaskId], branch: &str) -> String {
        let order: Vec<_> = order.iter().map(TaskId::as_str).collect();
        format!(
            "Plan **{}**: {} tareas, en este orden: {}. Las implementaré una a una en la \
             rama `{branch}`, con un commit por tarea. Responde «sí» para crear la rama y \
             empezar, «abortar» o «volver al spec» para corregir el plan.",
            plan.spec,
            order.len(),
            order.join(" → ")
        )
    }
}

#[async_trait]
impl ImplementNode for LoadNode {
    fn step(&self) -> Step {
        Step::Load
    }

    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError> {
        let Some(plan) = ctx.state().plan.clone() else {
            let path = ctx.state().plan_path.clone();
            let loaded = match load_plan(ctx.fs().as_ref(), &path).await {
                Ok(loaded) => loaded,
                Err(err @ (LoadError::Parse { .. } | LoadError::Invalid(_))) => {
                    return Ok(StepOutcome::BackToSpec {
                        reason: err.to_string(),
                    });
                }
                Err(err) => return Err(NodeError::Failed(err.to_string())),
            };
            let LoadedPlan { plan, order } = loaded;
            Self::show(ctx, &plan, &order)?;
            let ids: Vec<_> = order.iter().map(TaskId::as_str).collect();
            ctx.log(
                EntryKind::Note,
                format!(
                    "Plan cargado de {path}: {} tareas; orden {}.",
                    ids.len(),
                    ids.join(", ")
                ),
            );
            let prompt = Self::prompt(&plan, &order, &branch_name(&plan));
            let state = ctx.state_mut();
            state.plan = Some(plan);
            state.order = order;
            state.current = 0;
            return Ok(StepOutcome::Ask { prompt });
        };
        let branch = branch_name(&plan);
        let order = ctx.state().order.clone();
        match parse_choice(ctx.user_reply().unwrap_or_default()) {
            GateChoice::Continue => {}
            GateChoice::Abort => {
                return Ok(StepOutcome::Abort {
                    reason: "el usuario no aprobó el plan".to_owned(),
                });
            }
            GateChoice::BackToSpec => {
                return Ok(StepOutcome::BackToSpec {
                    reason: "el usuario pidió corregir el plan".to_owned(),
                });
            }
            GateChoice::Retry | GateChoice::Skip | GateChoice::Other(_) => {
                return Ok(StepOutcome::Ask {
                    prompt: format!(
                        "No cambio el plan aquí: para corregirlo, «volver al spec». {}",
                        Self::prompt(&plan, &order, &branch)
                    ),
                });
            }
        }
        match create_branch(ctx.shell().as_ref(), &branch).await {
            Ok(outcome) => {
                let text = match outcome {
                    BranchOutcome::Created => format!("Rama `{branch}` creada."),
                    BranchOutcome::Existing => format!("Rama `{branch}` ya existía; sigo en ella."),
                };
                ctx.log(EntryKind::Note, text.clone());
                ctx.notice(NoticeLevel::Info, text);
                ctx.state_mut().branch = Some(branch);
                Ok(StepOutcome::Next)
            }
            Err(err) => {
                ctx.log(
                    EntryKind::Failure,
                    format!("No se pudo crear la rama `{branch}`: {err}"),
                );
                Ok(StepOutcome::Ask {
                    prompt: format!(
                        "No pude crear la rama `{branch}`: {err}. Corrige el repositorio y \
                         responde «sí» para reintentar, o «abortar»."
                    ),
                })
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

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

    use crate::flows::implement::node::ImplementDeps;
    use crate::flows::implement::state::ImplementState;
    use crate::testing::system::RecordingSink;

    fn ctx(fs: InMemoryFs, shell: ScriptedShell, sink: &Arc<RecordingSink>) -> StepContext {
        let deps = ImplementDeps::new(Arc::clone(sink) as _, Arc::new(fs), Arc::new(shell));
        StepContext::new(
            Step::Load,
            deps,
            ImplementState::new(".crest/specs/0007/plan.json"),
        )
    }

    #[tokio::test]
    async fn node_shows_the_order_asks_and_then_creates_the_branch() {
        let sink = Arc::new(RecordingSink::new());
        let fs = fs_with(&plan_json(&[("T2", &["T1"]), ("T1", &[])]));
        let shell =
            ScriptedShell::new().on("git", ["switch", "-c", "spec/0007-cache-de-sesiones"], ok());
        let mut first = ctx(fs, shell, &sink);
        let out = LoadNode::new().run(&mut first).await.unwrap();
        let StepOutcome::Ask { prompt } = out else {
            panic!("{out:?}")
        };
        assert!(prompt.contains("T1 → T2"), "{prompt}");
        assert!(prompt.contains("spec/0007-cache-de-sesiones"), "{prompt}");
        assert_eq!(ids(&first.state().order), ["T1", "T2"]);
        assert_eq!(sink.kinds().len(), 2, "table and mermaid");

        let state = first.into_state();
        let fs = fs_with("{}");
        let shell = Arc::new(ScriptedShell::new().on(
            "git",
            ["switch", "-c", "spec/0007-cache-de-sesiones"],
            ok(),
        ));
        let deps = ImplementDeps::new(
            Arc::clone(&sink) as _,
            Arc::new(fs),
            Arc::clone(&shell) as _,
        );
        let mut again = StepContext::new(Step::Load, deps.clone(), state.clone()).with_reply("sí");
        assert_eq!(
            LoadNode::new().run(&mut again).await.unwrap(),
            StepOutcome::Next
        );
        assert_eq!(
            again.state().branch.as_deref(),
            Some("spec/0007-cache-de-sesiones")
        );
        assert_eq!(shell.calls().len(), 1);

        let mut back = StepContext::new(Step::Load, deps, state).with_reply("volver al spec");
        assert!(matches!(
            LoadNode::new().run(&mut back).await.unwrap(),
            StepOutcome::BackToSpec { .. }
        ));
    }

    #[tokio::test]
    async fn invalid_plans_go_back_to_the_spec() {
        let sink = Arc::new(RecordingSink::new());
        let fs = fs_with(&plan_json(&[("T1", &["T2"]), ("T2", &["T1"])]));
        let mut c = ctx(fs, ScriptedShell::new(), &sink);
        let out = LoadNode::new().run(&mut c).await.unwrap();
        assert!(matches!(out, StepOutcome::BackToSpec { reason } if reason.contains("cycle")));
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
