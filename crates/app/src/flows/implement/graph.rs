//! The implementation graph assembled ([`implement_graph`]) and one
//! implementation of a Spec as the chat and the headless mode drive it
//! ([`ImplementSession`]).

use std::path::Path;
use std::sync::Arc;

use graph_flow::{Session, SessionStorage};

use super::builder::{ImplementGraph, ImplementGraphBuilder};
use super::flow::{FlowError, FlowReply, ImplementFlow};
use super::load::LoadNode;
use super::node::ImplementDeps;
use super::nodes::commit::CommitTask;
use super::nodes::implement::ImplementTask;
use super::nodes::report::WriteReport;
use super::nodes::test::TestTask;
use super::nodes::verify::VerifyPlan;
use super::state::{FlowStatus, KEY_STATUS};
use crate::flows::aside::{FlowKind, GateInfo};
use crate::ports::events::NoticeLevel;

/// Name of the plan inside a Spec folder.
pub const PLAN_FILE: &str = "plan.json";

/// Folder of the Specs in the workspace.
pub const SPECS_DIR: &str = ".crest/specs";

/// The six steps of the implementation flow chained in order: Load plan,
/// Implement task, Test and fix, Commit, Verify hypotheses
/// and Report and next steps.
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

/// Why the target of `/implement` names no plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// Nothing was given.
    #[error("missing the Spec folder or the path to plan.json")]
    Empty,
    /// An absolute path outside the workspace.
    #[error("{0} is outside the workspace")]
    OutsideWorkspace(String),
}

/// The workspace path of the plan named by `target`, as typed after
/// `/implement` or `--implement`:
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
        FlowStatus::Completed => {
            Some((NoticeLevel::Info, "implementation flow finished".to_owned()))
        }
        FlowStatus::Aborted => Some((
            NoticeLevel::Warning,
            "implementation flow aborted; its journal is kept".to_owned(),
        )),
        FlowStatus::BackToSpec { reason } => Some((
            NoticeLevel::Warning,
            format!(
                "the plan turned out to be unworkable ({reason}); go back to the Spec node \
                 of the research flow with `/back spec` to fix it, and run `/implement` \
                 again"
            ),
        )),
    }
}

/// Key of the last plan path in the implement index session.
const KEY_LAST_PLAN: &str = "last_plan";

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

    /// Id of the index session that remembers the last plan opened in the
    /// workspace `root`.
    fn index_id(root: &Path) -> String {
        format!("implement-index:{}", root.display())
    }

    /// The plan of the last implementation opened in the workspace `root`,
    /// if its flow is still stored and has not finished, with its status:
    /// what `/implement resume` continues.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn pending(
        storage: Arc<dyn SessionStorage>,
        root: &Path,
    ) -> Result<Option<(String, FlowStatus)>, FlowError> {
        let Some(index) = storage.get(&Self::index_id(root)).await? else {
            return Ok(None);
        };
        let Some(plan) = index.context.get::<String>(KEY_LAST_PLAN) else {
            return Ok(None);
        };
        let Some(session) = storage.get(&Self::session_id(root, &plan)).await? else {
            return Ok(None);
        };
        let status: Option<FlowStatus> = session.context.get(KEY_STATUS);
        Ok(status
            .filter(|status| !status.is_finished())
            .map(|status| (plan, status)))
    }

    async fn remember(
        storage: &Arc<dyn SessionStorage>,
        root: &Path,
        plan_path: &str,
    ) -> Result<(), FlowError> {
        let id = Self::index_id(root);
        let index = match storage.get(&id).await? {
            Some(index) => index,
            None => Session::new_from_task(id, "index"),
        };
        index.context.set(KEY_LAST_PLAN, plan_path.to_owned())?;
        Ok(storage.save(index).await?)
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
        let root = deps.root().as_path().to_path_buf();
        let id = Self::session_id(&root, &plan_path);
        Self::remember(&storage, &root, &plan_path).await?;
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

    /// The gate the flow waits at, for a side chat; `None` when it does not
    /// wait (or its storage fails).
    pub async fn gate_info(&self) -> Option<GateInfo> {
        let FlowStatus::WaitingForInput { step, prompt } = self.status().await.ok()? else {
            return None;
        };
        Some(GateInfo {
            flow: FlowKind::Implement,
            stage: step.name().to_owned(),
            prompt,
            topic: Some(format!("implementing the plan {}", self.plan_path)),
        })
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
            path("/other/plan.json"),
            Err(TargetError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn only_a_stopped_flow_has_a_notice() {
        let running = FlowStatus::Running { next: Step::Load };
        assert_eq!(status_notice(&running), None);
        let back = FlowStatus::BackToSpec {
            reason: "the API is missing".into(),
        };
        let (level, text) = status_notice(&back).unwrap();
        assert_eq!(level, NoticeLevel::Warning);
        assert!(
            text.contains("the API is missing")
                && text.contains("/back spec")
                && text.contains("/implement"),
            "{text}"
        );
        assert!(status_notice(&FlowStatus::Completed).is_some());
    }

    #[tokio::test]
    async fn opening_the_same_plan_resumes_until_it_finishes() {
        let shell = ScriptedShell::new().on(
            "git",
            ["switch", "-c", "spec/0007-session-cache"],
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

        let root = h.deps().root().as_path().to_path_buf();
        assert_eq!(
            ImplementSession::pending(Arc::clone(&storage), &root)
                .await
                .unwrap(),
            None
        );
        let first = ImplementSession::open(h.deps(), Arc::clone(&storage), plan)
            .await
            .unwrap();
        assert!(!first.resumed());
        // `/implement resume` finds it.
        let (pending, _) = ImplementSession::pending(Arc::clone(&storage), &root)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending, plan);
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
        let reply = again.handle_line("abort").await.unwrap();
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
