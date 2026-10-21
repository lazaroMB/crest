//! [`ResearchSession`]: the research flow of one workspace, as the chat
//! (window or headless) drives it.
//!
//! - `/research <problem>` → [`ResearchSession::start`]: a new flow runs
//!   to its first gate.
//! - While a flow waits at a gate ([`ResearchSession::is_active`]), every
//!   plain line of the chat answers it ([`ResearchSession::handle_line`]).
//!   Running, failed, finished or aborted flows leave the chat free.
//! - `/stage`, `/back`, `/skip`, `/retry` and `/abort` act on the last flow
//!   of the workspace whether it is active or not.
//! - A node that fails leaves the flow [`FlowStatus::Failed`] at its last
//!   checkpoint; `/research retry` ([`ResearchSession::resume`]) runs it
//!   again. The same works after a restart or a cancelled run.
//! - The ids of the flows of the workspace are kept in the same storage
//!   (an index session per workspace): [`ResearchSession::list`] shows
//!   them and [`ResearchSession::resume`] takes one by number or id.

use std::sync::Arc;

use domain::block::BlockKind;
use graph_flow::{Session, SessionStorage};

use super::builder::ResearchGraph;
use super::error::FlowError;
use super::flow::{FlowReply, Position, ResearchFlow};
use super::gate::FlowStatus;
use crate::flows::aside::{FlowKind, GateInfo};

/// Prefix of the index session of a workspace.
const INDEX_PREFIX: &str = "research-index:";
/// Key of the id of the last flow in the index session.
const KEY_LAST: &str = "last_flow";
/// Key of the ids of every flow of the workspace, oldest first.
const KEY_FLOWS: &str = "flows";
/// Task id of the index session (it never runs).
const INDEX_TASK: &str = "index";

/// A stored flow of the workspace, as [`ResearchSession::list`] shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSummary {
    /// 1-based number, newest first; `/research resume <n>` takes it.
    pub number: usize,
    /// Id of the stored session.
    pub id: String,
    /// The user's request.
    pub problem: String,
    /// Where it is.
    pub position: Position,
}

/// The research flow of one workspace.
pub struct ResearchSession {
    graph: ResearchGraph,
    storage: Arc<dyn SessionStorage>,
    workspace: String,
    active: Option<ResearchFlow>,
}

impl std::fmt::Debug for ResearchSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchSession")
            .field("workspace", &self.workspace)
            .field(
                "active",
                &self.active.as_ref().map(ResearchFlow::session_id),
            )
            .finish_non_exhaustive()
    }
}

impl ResearchSession {
    /// The flows of `workspace` (its root, as a string) over `graph`,
    /// stored in `storage`.
    #[must_use]
    pub fn new(
        graph: ResearchGraph,
        storage: Arc<dyn SessionStorage>,
        workspace: impl Into<String>,
    ) -> Self {
        Self {
            graph,
            storage,
            workspace: workspace.into(),
            active: None,
        }
    }

    /// Whether a flow waits at a gate and takes the plain lines of the chat.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The active flow, if any.
    #[must_use]
    pub fn active(&self) -> Option<&ResearchFlow> {
        self.active.as_ref()
    }

    /// The gate the active flow waits at, for a side chat; `None` when no
    /// flow is active (or its storage fails).
    pub async fn gate_info(&self) -> Option<GateInfo> {
        let flow = self.active.as_ref()?;
        let FlowStatus::WaitingForInput { stage, prompt } = flow.status().await.ok()? else {
            return None;
        };
        let topic = flow.problem().await.ok().filter(|p| !p.trim().is_empty());
        Some(GateInfo {
            flow: FlowKind::Research,
            stage: stage.name().to_owned(),
            prompt,
            topic,
        })
    }

    fn index_id(&self) -> String {
        format!("{INDEX_PREFIX}{}", self.workspace)
    }

    async fn last_flow_id(&self) -> Result<Option<String>, FlowError> {
        let index = self.storage.get(&self.index_id()).await?;
        Ok(index.and_then(|s| s.context.get::<String>(KEY_LAST)))
    }

    /// Ids of the flows of the workspace, oldest first.
    async fn flow_ids(&self) -> Result<Vec<String>, FlowError> {
        let Some(index) = self.storage.get(&self.index_id()).await? else {
            return Ok(Vec::new());
        };
        let mut ids: Vec<String> = index.context.get(KEY_FLOWS).unwrap_or_default();
        // Indexes written before the list existed only know the last one.
        if let Some(last) = index.context.get::<String>(KEY_LAST)
            && !ids.contains(&last)
        {
            ids.push(last);
        }
        Ok(ids)
    }

    async fn add_flow_id(&self, id: &str) -> Result<(), FlowError> {
        let mut ids = self.flow_ids().await?;
        let index = match self.storage.get(&self.index_id()).await? {
            Some(index) => index,
            None => Session::new_from_task(self.index_id(), INDEX_TASK),
        };
        ids.retain(|known| known != id);
        ids.push(id.to_owned());
        index.context.set(KEY_FLOWS, ids)?;
        index.context.set(KEY_LAST, id.to_owned())?;
        Ok(self.storage.save(index).await?)
    }

    /// Binds to the stored flow `id`; `None` when it is gone or belongs to
    /// another graph.
    async fn open(&self, id: String) -> Result<Option<ResearchFlow>, FlowError> {
        match ResearchFlow::resume(self.graph.clone(), Arc::clone(&self.storage), id).await {
            Ok(flow) => Ok(Some(flow)),
            Err(FlowError::SessionNotFound(_) | FlowError::UnknownTask(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The last flow of the workspace, if it is still stored.
    async fn last_flow(&self) -> Result<Option<ResearchFlow>, FlowError> {
        match self.last_flow_id().await? {
            Some(id) => self.open(id).await,
            None => Ok(None),
        }
    }

    /// Status of the last flow of the workspace, if there is one.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn last_status(&self) -> Result<Option<FlowStatus>, FlowError> {
        match self.last_flow().await? {
            Some(flow) => Ok(Some(flow.status().await?)),
            None => Ok(None),
        }
    }

    /// The last flow of the workspace, if it was left running, at a gate
    /// or failed, and where it is.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn pending(&self) -> Result<Option<Position>, FlowError> {
        let Some(flow) = self.last_flow().await? else {
            return Ok(None);
        };
        let position = flow.position().await?;
        Ok(position.status.is_open().then_some(position))
    }

    /// The stored flows of the workspace, newest first.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn list(&self) -> Result<Vec<FlowSummary>, FlowError> {
        let mut flows = Vec::new();
        for id in self.flow_ids().await?.into_iter().rev() {
            let Some(flow) = self.open(id.clone()).await? else {
                continue;
            };
            flows.push(FlowSummary {
                number: flows.len() + 1,
                id,
                problem: flow.problem().await?,
                position: flow.position().await?,
            });
        }
        Ok(flows)
    }

    /// Starts a flow for `problem` with a new session id (`now` in Unix
    /// epoch milliseconds) and runs it to its first gate. It replaces the
    /// active flow, if any, which stays stored.
    ///
    /// # Errors
    ///
    /// A storage error. A failing node is not an error: the flow comes
    /// back [`FlowStatus::Failed`].
    pub async fn start(&mut self, problem: &str, now: u64) -> Result<FlowStatus, FlowError> {
        let mut id = format!("research:{}:{now}", self.workspace);
        let mut n = 1;
        while self.storage.get(&id).await?.is_some() {
            n += 1;
            id = format!("research:{}:{now}-{n}", self.workspace);
        }
        let flow = ResearchFlow::start(self.graph.clone(), Arc::clone(&self.storage), &id, problem)
            .await?;
        self.add_flow_id(&id).await?;
        // Until it stops at a gate, the chat stays free: if this run is
        // dropped (cancelled), nothing is left holding the chat.
        self.active = None;
        let status = flow.run().await?;
        self.settle(flow, &status);
        Ok(status)
    }

    /// Continues a flow of the workspace: the last one, or `which` (its
    /// number in [`ResearchSession::list`] or its id). A failed or
    /// interrupted node runs again; at a gate its prompt is shown again.
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionNotFound`] when there is no such flow,
    /// [`FlowError::Aborted`] when it was aborted, [`FlowError::Completed`]
    /// when it finished (`/back <stage>` reopens it), or a storage error.
    pub async fn resume(&mut self, which: Option<&str>) -> Result<FlowStatus, FlowError> {
        let id = match which.map(str::trim).filter(|w| !w.is_empty()) {
            None => self
                .last_flow_id()
                .await?
                .ok_or_else(|| FlowError::SessionNotFound(self.index_id()))?,
            Some(which) => self.find(which).await?,
        };
        let flow = self
            .open(id.clone())
            .await?
            .ok_or(FlowError::SessionNotFound(id.clone()))?;
        self.add_flow_id(&id).await?;
        self.active = None;
        let status = match flow.status().await? {
            FlowStatus::WaitingForInput { stage, prompt } => {
                // The gate was shown before; show it again.
                let _ = self.graph.deps().emit(BlockKind::Text {
                    markdown: prompt.clone(),
                });
                FlowStatus::WaitingForInput { stage, prompt }
            }
            _ => flow.retry().await?,
        };
        self.settle(flow, &status);
        Ok(status)
    }

    /// The id of the flow `which` names: a number of the list or an id.
    async fn find(&self, which: &str) -> Result<String, FlowError> {
        let ids = self.flow_ids().await?;
        if let Ok(n) = which.parse::<usize>() {
            return ids
                .iter()
                .rev()
                .nth(n.saturating_sub(1))
                .filter(|_| n > 0)
                .cloned()
                .ok_or_else(|| FlowError::SessionNotFound(which.to_owned()));
        }
        ids.into_iter()
            .find(|id| id == which)
            .ok_or_else(|| FlowError::SessionNotFound(which.to_owned()))
    }

    /// A line of the chat for the research flow (see
    /// [`ResearchFlow::handle_line`]): a command such as `/back` or
    /// `/retry`, which works on the last flow even when it is not active,
    /// or the answer to the pending gate.
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionNotFound`] without a flow, or the errors of
    /// [`ResearchFlow::handle_line`].
    pub async fn handle_line(&mut self, line: &str) -> Result<FlowReply, FlowError> {
        let was_active = self.active.is_some();
        let flow = match self.active.take() {
            Some(flow) => flow,
            None => self
                .last_flow()
                .await?
                .ok_or_else(|| FlowError::SessionNotFound("research flow".into()))?,
        };
        let reply = flow.handle_line(line).await;
        match &reply {
            Ok(FlowReply::Status(status)) => self.settle(flow, status),
            Ok(FlowReply::Position(position)) => self.settle(flow, &position.status),
            Err(_) if was_active => self.active = Some(flow),
            Err(_) => {}
        }
        reply
    }

    /// Keeps the flow in the chat only while it waits at a gate.
    fn settle(&mut self, flow: ResearchFlow, status: &FlowStatus) {
        self.active = matches!(status, FlowStatus::WaitingForInput { .. }).then_some(flow);
    }
}

/// A short line for the status bar or the terminal about `status`.
#[must_use]
pub fn describe_status(status: &FlowStatus) -> String {
    match status {
        FlowStatus::Running { next } => format!(
            "research flow running: next is {} ({}/{})",
            next.name(),
            next.index(),
            domain::research::Stage::total()
        ),
        FlowStatus::WaitingForInput { stage, .. } => format!(
            "research flow at {} ({}/{}): answer in the chat",
            stage.name(),
            stage.index(),
            domain::research::Stage::total()
        ),
        FlowStatus::Completed => "research flow finished; `/simulate <spec>` runs its solution on \
                                  a board before `/implement <spec>`"
            .to_owned(),
        FlowStatus::Aborted => "research flow aborted".to_owned(),
        FlowStatus::Failed { stage, error } => format!(
            "research flow failed at {} ({}/{}): {error} — `/research retry` runs it again",
            stage.name(),
            stage.index(),
            domain::research::Stage::total()
        ),
    }
}

/// A short line about what a chat line did in the flow.
#[must_use]
pub fn describe_reply(reply: &FlowReply) -> String {
    match reply {
        FlowReply::Position(position) => position.to_string(),
        FlowReply::Status(status) => describe_status(status),
    }
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPolicy;
    use domain::research::Stage;
    use graph_flow::InMemorySessionStorage;

    use super::*;
    use crate::flows::research::builder::ResearchGraphBuilder;
    use crate::flows::research::fake::FakeNode;
    use crate::flows::research::node::NodeDeps;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::RecordingSink;

    fn graph() -> ResearchGraph {
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::new(RecordingSink::new()),
        );
        ResearchGraphBuilder::new(deps)
            .node(FakeNode::new(Stage::Intake))
            .node(FakeNode::new(Stage::Observation))
            .node(FakeNode::new(Stage::Spec))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn a_flow_takes_the_chat_until_it_ends_and_can_be_resumed() {
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let mut research = ResearchSession::new(graph(), Arc::clone(&storage), "/ws");
        assert!(research.pending().await.unwrap().is_none());
        assert!(matches!(
            research.resume(None).await,
            Err(FlowError::SessionNotFound(_))
        ));

        let status = research.start("adds wrong", 1).await.unwrap();
        assert!(matches!(
            status,
            FlowStatus::WaitingForInput {
                stage: Stage::Intake,
                ..
            }
        ));
        assert!(research.is_active());

        // A restart: a new session over the same storage finds it.
        let mut reopened = ResearchSession::new(graph(), Arc::clone(&storage), "/ws");
        assert!(!reopened.is_active());
        let pending = reopened.pending().await.unwrap().unwrap();
        assert_eq!(pending.stage, Stage::Intake);
        reopened.resume(None).await.unwrap();
        assert!(reopened.is_active());
        let reply = reopened.handle_line("yes").await.unwrap();
        assert_eq!(reply, FlowReply::Status(FlowStatus::Completed));
        assert!(!reopened.is_active());
        assert!(reopened.pending().await.unwrap().is_none());
        assert_eq!(
            reopened.last_status().await.unwrap(),
            Some(FlowStatus::Completed)
        );
        // A finished flow cannot be resumed, but `/back` reopens it, e.g.
        // to redo the Spec.
        assert!(matches!(
            reopened.resume(None).await,
            Err(FlowError::Completed)
        ));
        assert!(!reopened.is_active());
        let reply = reopened.handle_line("/back spec").await.unwrap();
        assert_eq!(reply, FlowReply::Status(FlowStatus::Completed));
        assert!(!reopened.is_active());
        // Another workspace has its own flows.
        let other = ResearchSession::new(graph(), storage, "/other");
        assert!(other.pending().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn abort_releases_the_chat() {
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let mut research = ResearchSession::new(graph(), storage, "/ws");
        research.start("x", 1).await.unwrap();
        let reply = research.handle_line("/abort").await.unwrap();
        assert_eq!(reply, FlowReply::Status(FlowStatus::Aborted));
        assert!(!research.is_active());
        assert!(describe_reply(&reply).contains("aborted"));
    }

    fn failing_graph() -> ResearchGraph {
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::new(RecordingSink::new()),
        );
        ResearchGraphBuilder::new(deps)
            .node(FakeNode::new(Stage::Intake).then_fail("offline"))
            .node(FakeNode::new(Stage::Spec))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn a_failed_flow_frees_the_chat_and_resumes() {
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let mut research = ResearchSession::new(failing_graph(), Arc::clone(&storage), "/ws");
        let status = research.start("adds wrong", 1).await.unwrap();
        assert!(matches!(
            status,
            FlowStatus::Failed {
                stage: Stage::Intake,
                ..
            }
        ));
        assert!(describe_status(&status).contains("/research retry"));
        // The chat is free, and the failure survives a restart.
        assert!(!research.is_active());
        let reopened = ResearchSession::new(failing_graph(), Arc::clone(&storage), "/ws");
        let pending = reopened.pending().await.unwrap().unwrap();
        assert_eq!(pending.stage, Stage::Intake);
        assert!(matches!(pending.status, FlowStatus::Failed { .. }));
        // Retrying runs the stage again, up to its gate.
        let status = research.resume(None).await.unwrap();
        assert!(matches!(
            status,
            FlowStatus::WaitingForInput {
                stage: Stage::Intake,
                ..
            }
        ));
        assert!(research.is_active());
    }

    #[tokio::test]
    async fn flows_are_listed_newest_first_and_resumed_by_number() {
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let mut research = ResearchSession::new(graph(), Arc::clone(&storage), "/ws");
        research.start("first", 1).await.unwrap();
        research.start("second", 2).await.unwrap();
        let flows = research.list().await.unwrap();
        let problems: Vec<_> = flows.iter().map(|f| f.problem.as_str()).collect();
        assert_eq!(problems, ["second", "first"]);
        assert_eq!(flows[1].number, 2);
        // Resuming the older one makes it the last again.
        research.resume(Some("2")).await.unwrap();
        let reply = research.handle_line("/stage").await.unwrap();
        assert!(describe_reply(&reply).contains("Intake"));
        assert_eq!(research.list().await.unwrap()[0].problem, "first");
        assert!(matches!(
            research.resume(Some("9")).await,
            Err(FlowError::SessionNotFound(_))
        ));
    }
}
