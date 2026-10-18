//! The implementation graph assembled ([`implement_graph`]) and one
//! implementation of a Spec as the chat and the headless mode drive it
//! ([`ImplementSession`]).

use std::path::Path;
use std::sync::Arc;

use graph_flow::SessionStorage;

use super::builder::{ImplementGraph, ImplementGraphBuilder};
use super::flow::{FlowError, FlowReply, ImplementFlow};
use super::load::LoadNode;
use super::node::ImplementDeps;
use super::nodes::commit::CommitTask;
use super::nodes::implement::ImplementTask;
use super::nodes::report::WriteReport;
use super::nodes::test::TestTask;
use super::nodes::verify::VerifyPlan;
use super::state::FlowStatus;
use crate::ports::events::NoticeLevel;

/// Name of the plan inside a Spec folder.
pub const PLAN_FILE: &str = "plan.json";

/// Folder of the Specs in the workspace.
pub const SPECS_DIR: &str = ".crest/specs";

/// The six steps of the implementation flow chained in order: Cargar plan,
/// Implementar tarea, Pruebas y corrección, Commit, Verificar hipótesis
/// and Reporte y futuro.
///
/// # Errors
///
/// [`FlowError::InvalidGraph`] if graph-flow rejects the graph.
pub fn implement_graph(deps: ImplementDeps) -> Result<ImplementGraph, FlowError> {
    ImplementGraphBuilder::new(deps)
        .node(LoadNode::new())
        .node(ImplementTask::new())
        .node(TestTask::new())
        .node(CommitTask::new())
        .node(VerifyPlan::new())
        .node(WriteReport::new())
        .build()
}

/// Why the target of `:implement` names no plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// Nothing was given.
    #[error("falta la carpeta del Spec o la ruta a plan.json")]
    Empty,
    /// An absolute path outside the workspace.
    #[error("{0} está fuera del workspace")]
    OutsideWorkspace(String),
}

/// The workspace path of the plan named by `target`, as typed after
/// `:implement` or `--implement`:
///
/// - a path ending in `.json` is the plan itself;
/// - a path with a `/` is the Spec folder, and the plan its `plan.json`;
/// - a bare name (`0001-cache`) is a folder of `.crest/specs/`.
///
/// Absolute paths must be inside `root`.
///
/// # Errors
///
/// [`TargetError`] when `target` is empty or outside the workspace.
pub fn plan_path(root: &Path, target: &str) -> Result<String, TargetError> {
    let target = target.trim();
    if target.is_empty() {
        return Err(TargetError::Empty);
    }
    let relative = if Path::new(target).is_absolute() {
        Path::new(target)
            .strip_prefix(root)
            .map_err(|_| TargetError::OutsideWorkspace(target.to_owned()))?
            .to_string_lossy()
            .into_owned()
    } else {
        target.strip_prefix("./").unwrap_or(target).to_owned()
    };
    let relative = relative.trim_end_matches('/');
    if relative.is_empty() || relative == "." {
        return Ok(PLAN_FILE.to_owned());
    }
    Ok(if relative.ends_with(".json") {
        relative.to_owned()
    } else if relative.contains('/') {
        format!("{relative}/{PLAN_FILE}")
    } else {
        format!("{SPECS_DIR}/{relative}/{PLAN_FILE}")
    })
}

/// What the chat shows when the flow stops: done, aborted, or back to the
/// Spec (with how to get there). `None` while it runs or waits at a gate,
/// whose prompt is already in the timeline.
#[must_use]
pub fn status_notice(status: &FlowStatus) -> Option<(NoticeLevel, String)> {
    match status {
        FlowStatus::Running { .. } | FlowStatus::WaitingForInput { .. } => None,
        FlowStatus::Completed => Some((
            NoticeLevel::Info,
            "flujo de implementación terminado".to_owned(),
        )),
        FlowStatus::Aborted => Some((
            NoticeLevel::Warning,
            "flujo de implementación abortado; su diario queda guardado".to_owned(),
        )),
        FlowStatus::BackToSpec { reason } => Some((
            NoticeLevel::Warning,
            format!(
                "el plan resultó inviable ({reason}); vuelve al nodo Spec del flujo \
                 investigativo con `:research resume` y luego `/back spec` para corregirlo, \
                 y vuelve a correr :implement"
            ),
        )),
    }
}

/// One implementation of a Spec: the flow of its plan, stored under an id
/// derived from the workspace and the plan, so opening the same plan again
/// resumes where it stopped.
#[derive(Debug, Clone)]
pub struct ImplementSession {
    flow: ImplementFlow,
    plan_path: String,
    resumed: bool,
}

impl ImplementSession {
    /// Id of the stored session of `plan_path` in the workspace `root`.
    #[must_use]
    pub fn session_id(root: &Path, plan_path: &str) -> String {
        format!("implement:{}:{plan_path}", root.display())
    }

    /// Opens the implementation of the plan at `plan_path` (a workspace
    /// path, see [`plan_path`]): resumes its stored session if it has not
    /// finished, otherwise starts a new one. Nothing runs until
    /// [`ImplementSession::run`].
    ///
    /// # Errors
    ///
    /// A storage error, or a stored session this graph cannot run.
    pub async fn open(
        deps: ImplementDeps,
        storage: Arc<dyn SessionStorage>,
        plan_path: impl Into<String>,
    ) -> Result<Self, FlowError> {
        let plan_path = plan_path.into();
        let id = Self::session_id(deps.root().as_path(), &plan_path);
        let graph = implement_graph(deps)?;
        if storage.get(&id).await?.is_some() {
            let flow = ImplementFlow::resume(graph.clone(), Arc::clone(&storage), &id).await;
            match flow {
                Ok(flow) if !flow.status().await?.is_finished() => {
                    return Ok(Self {
                        flow,
                        plan_path,
                        resumed: true,
                    });
                }
                // Finished, or from another version of the graph: start over.
                Ok(_) | Err(FlowError::UnknownTask(_)) => storage.delete(&id).await?,
                Err(err) => return Err(err),
            }
        }
        let flow = ImplementFlow::start(graph, storage, id, plan_path.clone()).await?;
        Ok(Self {
            flow,
            plan_path,
            resumed: false,
        })
    }

    /// The flow, for its state and position.
    #[must_use]
    pub fn flow(&self) -> &ImplementFlow {
        &self.flow
    }

    /// Workspace path of the plan.
    #[must_use]
    pub fn plan_path(&self) -> &str {
        &self.plan_path
    }

    /// Whether [`ImplementSession::open`] resumed a stored session.
    #[must_use]
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// Runs until the next gate or the end.
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::run`].
    pub async fn run(&self) -> Result<FlowStatus, FlowError> {
        self.flow.run().await
    }

    /// A line typed while the flow is active: `/stage`, `/abort` or the
    /// answer to the pending gate (see [`ImplementFlow::handle_line`]).
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::handle_line`].
    pub async fn handle_line(&self, line: &str) -> Result<FlowReply, FlowError> {
        self.flow.handle_line(line).await
    }

    /// Status of the flow.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn status(&self) -> Result<FlowStatus, FlowError> {
        self.flow.status().await
    }
}

#[cfg(test)]
mod tests {
    use graph_flow::InMemorySessionStorage;
    use serde_json::json;

    use super::*;
    use crate::flows::implement::load::tests::plan_json;
    use crate::flows::implement::nodes::tests::{Harness, exit};
    use crate::flows::implement::state::Step;
    use crate::testing::fs::ScriptedShell;
    use crate::testing::llm::ScriptedResponse;

    #[test]
    fn targets_name_the_plan() {
        let root = Path::new("/ws");
        let path = |t| plan_path(root, t);
        assert_eq!(path("0001-x").unwrap(), ".crest/specs/0001-x/plan.json");
        assert_eq!(
            path(".crest/specs/0001-x/").unwrap(),
            ".crest/specs/0001-x/plan.json"
        );
        assert_eq!(path("./a/b/plan.json").unwrap(), "a/b/plan.json");
        assert_eq!(path("/ws/specs/p.json").unwrap(), "specs/p.json");
        assert_eq!(path("/ws").unwrap(), "plan.json");
        assert_eq!(path("  "), Err(TargetError::Empty));
        assert!(matches!(
            path("/otro/plan.json"),
            Err(TargetError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn only_a_stopped_flow_has_a_notice() {
        let running = FlowStatus::Running { next: Step::Load };
        assert_eq!(status_notice(&running), None);
        let back = FlowStatus::BackToSpec {
            reason: "falta la API".into(),
        };
        let (level, text) = status_notice(&back).unwrap();
        assert_eq!(level, NoticeLevel::Warning);
        assert!(
            text.contains("falta la API")
                && text.contains(":research resume")
                && text.contains("/back spec"),
            "{text}"
        );
        assert!(status_notice(&FlowStatus::Completed).is_some());
    }

    #[tokio::test]
    async fn opening_the_same_plan_resumes_until_it_finishes() {
        let shell = ScriptedShell::new().on(
            "git",
            ["switch", "-c", "spec/0007-cache-de-sesiones"],
            exit(0, ""),
        );
        let h = Harness::new(
            [ScriptedResponse::tool_use(
                "toolu_1",
                "write_file",
                json!({ "path": "src/t1.rs", "content": "x" }),
            )],
            Arc::new(shell),
        );
        let plan = ".crest/specs/0007/plan.json";
        h.fs.set_file(plan, plan_json(&[("T1", &[])]));
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());

        let first = ImplementSession::open(h.deps(), Arc::clone(&storage), plan)
            .await
            .unwrap();
        assert!(!first.resumed());
        assert!(matches!(
            first.run().await.unwrap(),
            FlowStatus::WaitingForInput {
                step: Step::Load,
                ..
            }
        ));

        let again = ImplementSession::open(h.deps(), Arc::clone(&storage), plan)
            .await
            .unwrap();
        assert!(again.resumed());
        assert_eq!(again.plan_path(), plan);
        let reply = again.handle_line("abortar").await.unwrap();
        assert_eq!(reply, FlowReply::Status(FlowStatus::Aborted));

        let fresh = ImplementSession::open(h.deps(), storage, plan)
            .await
            .unwrap();
        assert!(!fresh.resumed());
        assert_eq!(
            fresh.status().await.unwrap(),
            FlowStatus::Running { next: Step::Load }
        );
    }
}
