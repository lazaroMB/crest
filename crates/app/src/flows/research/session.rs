//! [`ResearchSession`]: the research flow of one workspace, as the chat
//! (window or headless) drives it.
//!
//! - `:research <problem>` → [`ResearchSession::start`]: a new flow runs
//!   to its first gate.
//! - While a flow is active ([`ResearchSession::is_active`]), every line of
//!   the chat goes to [`ResearchSession::handle_line`]: `/stage`, `/back`,
//!   `/skip`, `/abort`, or the answer to the pending gate. Once the flow
//!   finishes or is aborted the chat is the chat again.
//! - The id of the last flow of the workspace is kept in the same storage
//!   (an index session per workspace), so after a restart
//!   [`ResearchSession::pending`] finds a flow left halfway and
//!   [`ResearchSession::resume`] (`:research resume`) continues it at the
//!   pending node.

use std::sync::Arc;

use domain::block::BlockKind;
use graph_flow::{Session, SessionStorage};

use super::builder::ResearchGraph;
use super::error::FlowError;
use super::flow::{FlowReply, Position, ResearchFlow};
use super::gate::FlowStatus;

/// Prefix of the index session of a workspace.
const INDEX_PREFIX: &str = "research-index:";
/// Key of the id of the last flow in the index session.
const KEY_LAST: &str = "last_flow";
/// Task id of the index session (it never runs).
const INDEX_TASK: &str = "index";

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

/// Whether the flow still has work: running or at a gate.
fn is_open(status: &FlowStatus) -> bool {
    matches!(
        status,
        FlowStatus::Running { .. } | FlowStatus::WaitingForInput { .. }
    )
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

    /// Whether a flow takes the lines of the chat.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The active flow, if any.
    #[must_use]
    pub fn active(&self) -> Option<&ResearchFlow> {
        self.active.as_ref()
    }

    fn index_id(&self) -> String {
        format!("{INDEX_PREFIX}{}", self.workspace)
    }

    async fn last_flow_id(&self) -> Result<Option<String>, FlowError> {
        let index = self.storage.get(&self.index_id()).await?;
        Ok(index.and_then(|s| s.context.get::<String>(KEY_LAST)))
    }

    async fn set_last_flow_id(&self, id: &str) -> Result<(), FlowError> {
        let index = match self.storage.get(&self.index_id()).await? {
            Some(index) => index,
            None => Session::new_from_task(self.index_id(), INDEX_TASK),
        };
        index.context.set(KEY_LAST, id.to_owned())?;
        Ok(self.storage.save(index).await?)
    }

    /// Status of the last flow of the workspace, if there is one.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn last_status(&self) -> Result<Option<FlowStatus>, FlowError> {
        let Some(id) = self.last_flow_id().await? else {
            return Ok(None);
        };
        match ResearchFlow::resume(self.graph.clone(), Arc::clone(&self.storage), id).await {
            Ok(flow) => Ok(Some(flow.status().await?)),
            Err(FlowError::SessionNotFound(_) | FlowError::UnknownTask(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The last flow of the workspace, if it was left running or at a
    /// gate, and where it is.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn pending(&self) -> Result<Option<Position>, FlowError> {
        let Some(id) = self.last_flow_id().await? else {
            return Ok(None);
        };
        match ResearchFlow::resume(self.graph.clone(), Arc::clone(&self.storage), id).await {
            Ok(flow) => {
                let position = flow.position().await?;
                Ok(is_open(&position.status).then_some(position))
            }
            Err(FlowError::SessionNotFound(_) | FlowError::UnknownTask(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Starts a flow for `problem` with a new session id (`now` in Unix
    /// epoch milliseconds) and runs it to its first gate. It replaces the
    /// active flow, if any, which stays stored.
    ///
    /// # Errors
    ///
    /// A storage error, or the error of a node (the flow stays active at
    /// its last checkpoint).
    pub async fn start(&mut self, problem: &str, now: u64) -> Result<FlowStatus, FlowError> {
        let mut id = format!("research:{}:{now}", self.workspace);
        let mut n = 1;
        while self.storage.get(&id).await?.is_some() {
            n += 1;
            id = format!("research:{}:{now}-{n}", self.workspace);
        }
        let flow = ResearchFlow::start(self.graph.clone(), Arc::clone(&self.storage), &id, problem)
            .await?;
        self.set_last_flow_id(&id).await?;
        self.active = Some(flow.clone());
        let status = flow.run().await?;
        self.settle(&status);
        Ok(status)
    }

    /// Continues the last flow of the workspace at its pending node: runs
    /// it on, or shows its gate again. A finished flow is reopened only for
    /// `/back <nodo>` (e.g. back to the Spec when its plan proved
    /// unfeasible): it stays active until the next line.
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionNotFound`] when there is no flow to resume,
    /// [`FlowError::Aborted`] when it was aborted, a storage error or the
    /// error of a node.
    pub async fn resume(&mut self) -> Result<FlowStatus, FlowError> {
        let id = self
            .last_flow_id()
            .await?
            .ok_or_else(|| FlowError::SessionNotFound(self.index_id()))?;
        let flow = ResearchFlow::resume(self.graph.clone(), Arc::clone(&self.storage), id).await?;
        let status = match flow.status().await? {
            FlowStatus::Aborted => return Err(FlowError::Aborted),
            // Only `/back <nodo>` makes sense now (e.g. back to the Spec
            // when its plan proved unfeasible); any other line releases it.
            FlowStatus::Completed => {
                self.active = Some(flow);
                return Ok(FlowStatus::Completed);
            }
            FlowStatus::WaitingForInput { stage, prompt } => {
                // The gate was shown before the restart; show it again.
                let _ = self.graph.deps().emit(BlockKind::Text {
                    markdown: prompt.clone(),
                });
                FlowStatus::WaitingForInput { stage, prompt }
            }
            FlowStatus::Running { .. } => {
                self.active = Some(flow.clone());
                flow.run().await?
            }
        };
        self.active = Some(flow);
        self.settle(&status);
        Ok(status)
    }

    /// A line of the chat while a flow is active (see
    /// [`ResearchFlow::handle_line`]).
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionNotFound`] without an active flow, or the
    /// errors of [`ResearchFlow::handle_line`].
    pub async fn handle_line(&mut self, line: &str) -> Result<FlowReply, FlowError> {
        let flow = self
            .active
            .clone()
            .ok_or_else(|| FlowError::SessionNotFound("active research flow".into()))?;
        let reply = flow.handle_line(line).await;
        match &reply {
            Ok(FlowReply::Status(status)) => self.settle(status),
            Err(FlowError::Completed | FlowError::Aborted) => self.active = None,
            _ => {}
        }
        reply
    }

    /// Releases the chat once the flow ended.
    fn settle(&mut self, status: &FlowStatus) {
        if !is_open(status) {
            self.active = None;
        }
    }
}

/// A short line for the status bar or the terminal about `status`.
#[must_use]
pub fn describe_status(status: &FlowStatus) -> String {
    match status {
        FlowStatus::Running { next } => format!(
            "flujo investigativo en curso: sigue {} ({}/{})",
            next.name_es(),
            next.index(),
            domain::research::Stage::total()
        ),
        FlowStatus::WaitingForInput { stage, .. } => format!(
            "flujo investigativo en {} ({}/{}): responde en el chat",
            stage.name_es(),
            stage.index(),
            domain::research::Stage::total()
        ),
        FlowStatus::Completed => "flujo investigativo terminado".to_owned(),
        FlowStatus::Aborted => "flujo investigativo abortado".to_owned(),
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
            research.resume().await,
            Err(FlowError::SessionNotFound(_))
        ));

        let status = research.start("suma mal", 1).await.unwrap();
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
        reopened.resume().await.unwrap();
        assert!(reopened.is_active());
        let reply = reopened.handle_line("sí").await.unwrap();
        assert_eq!(reply, FlowReply::Status(FlowStatus::Completed));
        assert!(!reopened.is_active());
        assert!(reopened.pending().await.unwrap().is_none());
        assert_eq!(
            reopened.last_status().await.unwrap(),
            Some(FlowStatus::Completed)
        );
        // A finished flow reopens for `/back`, e.g. to redo the Spec.
        assert_eq!(reopened.resume().await.unwrap(), FlowStatus::Completed);
        assert!(reopened.is_active());
        let reply = reopened.handle_line("/back spec").await.unwrap();
        assert_eq!(reply, FlowReply::Status(FlowStatus::Completed));
        assert!(!reopened.is_active());
        // Another workspace has its own flows.
        let other = ResearchSession::new(graph(), storage, "/otro");
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
        assert!(describe_reply(&reply).contains("abortado"));
    }
}
