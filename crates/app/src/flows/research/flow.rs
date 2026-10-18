//! [`ResearchFlow`]: runs a research graph for one session, one node per
//! checkpoint, and feeds it the user's answers at the gates.

use std::fmt;
use std::sync::Arc;

use domain::research::{Phase, ResearchState, Stage};
use graph_flow::{FlowRunner, Session, SessionStorage};

use super::builder::{GRAPH_ID, ResearchGraph};
use super::commands::{Command, parse_command};
use super::error::FlowError;
use super::gate::{
    FlowStatus, GateReply, KEY_BACKS, KEY_ENTERED, KEY_GATE, KEY_PROBLEM, KEY_REPLY, KEY_STATE,
    KEY_STATUS, stage_of_task, task_id,
};
use super::node::is_optional;

/// Most nodes one [`ResearchFlow::run`] goes through before returning,
/// even if no gate stops it (a guard against runaway loops).
pub const MAX_STEPS_PER_RUN: usize = 64;

/// Where the flow is, as `/stage` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Current (or last) stage.
    pub stage: Stage,
    /// Status of the flow.
    pub status: FlowStatus,
}

impl Position {
    /// 1-based position of the stage among the 14.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.stage.index()
    }

    /// Number of stages of the workflow (14).
    #[must_use]
    pub fn total(&self) -> u32 {
        Stage::total()
    }

    /// Phase of the stage.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.stage.phase()
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Stage {}/{}: {} ({})",
            self.index(),
            self.total(),
            self.stage.name(),
            self.phase().name()
        )?;
        match &self.status {
            FlowStatus::Running { .. } => write!(f, " · running"),
            FlowStatus::WaitingForInput { .. } => write!(f, " · waiting for your answer"),
            FlowStatus::Completed => write!(f, " · flow finished"),
            FlowStatus::Aborted => write!(f, " · flow aborted"),
        }
    }
}

/// What a chat line did ([`ResearchFlow::handle_line`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowReply {
    /// `/stage`: where the flow is.
    Position(Position),
    /// Status after the command or the answer, and the nodes it ran.
    Status(FlowStatus),
}

/// Appends a journal entry in the session and clears any pending gate.
fn log_and_clear_gate(
    session: &Session,
    at: u64,
    stage: Stage,
    text: &str,
) -> Result<(), FlowError> {
    let mut state: ResearchState = session.context.get(KEY_STATE).unwrap_or_default();
    state.log(at, stage, text);
    session.context.set(KEY_STATE, state)?;
    session.context.remove(KEY_GATE);
    session.context.remove(KEY_REPLY);
    Ok(())
}

/// A research flow bound to one stored session.
///
/// Every finished node is saved before the next one runs, so dropping the
/// flow (or the app) at any point and calling [`ResearchFlow::resume`]
/// continues at the pending node. Blocks go to the event sink of the
/// graph's [`NodeDeps`](super::node::NodeDeps).
#[derive(Clone)]
pub struct ResearchFlow {
    graph: ResearchGraph,
    storage: Arc<dyn SessionStorage>,
    runner: FlowRunner,
    session_id: String,
}

impl fmt::Debug for ResearchFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResearchFlow")
            .field("session_id", &self.session_id)
            .field("graph", &self.graph)
            .finish_non_exhaustive()
    }
}

impl ResearchFlow {
    fn bind(graph: ResearchGraph, storage: Arc<dyn SessionStorage>, session_id: String) -> Self {
        let runner = FlowRunner::new(Arc::clone(graph.graph()), Arc::clone(&storage));
        Self {
            graph,
            storage,
            runner,
            session_id,
        }
    }

    /// Creates the session `session_id` for `problem`, at the first node.
    /// Nothing runs until [`ResearchFlow::run`] or [`ResearchFlow::step`].
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionExists`], or a storage error.
    pub async fn start(
        graph: ResearchGraph,
        storage: Arc<dyn SessionStorage>,
        session_id: impl Into<String>,
        problem: impl Into<String>,
    ) -> Result<Self, FlowError> {
        let session_id = session_id.into();
        if storage.get(&session_id).await?.is_some() {
            return Err(FlowError::SessionExists(session_id));
        }
        let first = graph.first();
        let session =
            Session::new_from_task(session_id.clone(), &task_id(first)).with_graph_id(GRAPH_ID);
        session.context.set(KEY_PROBLEM, problem.into())?;
        session.context.set(KEY_STATE, ResearchState::default())?;
        session
            .context
            .set(KEY_STATUS, FlowStatus::Running { next: first })?;
        storage.save(session).await?;
        Ok(Self::bind(graph, storage, session_id))
    }

    /// Binds to the stored session `session_id`, e.g. after a restart.
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionNotFound`], [`FlowError::UnknownTask`] when the
    /// session is at a node this graph does not have, or a storage error.
    pub async fn resume(
        graph: ResearchGraph,
        storage: Arc<dyn SessionStorage>,
        session_id: impl Into<String>,
    ) -> Result<Self, FlowError> {
        let flow = Self::bind(graph, storage, session_id.into());
        flow.position().await?;
        Ok(flow)
    }

    /// Id of the stored session.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The graph the flow runs.
    #[must_use]
    pub fn graph(&self) -> &ResearchGraph {
        &self.graph
    }

    pub(crate) async fn load(&self) -> Result<Session, FlowError> {
        self.storage
            .get(&self.session_id)
            .await?
            .ok_or_else(|| FlowError::SessionNotFound(self.session_id.clone()))
    }

    pub(crate) async fn save(&self, session: Session) -> Result<(), FlowError> {
        Ok(self.storage.save(session).await?)
    }

    /// The stage the session points at.
    pub(crate) fn current_stage(&self, session: &Session) -> Result<Stage, FlowError> {
        stage_of_task(&session.current_task_id)
            .filter(|s| self.graph.contains(*s))
            .ok_or_else(|| FlowError::UnknownTask(session.current_task_id.clone()))
    }

    pub(crate) fn status_of(&self, session: &Session) -> Result<FlowStatus, FlowError> {
        let current = self.current_stage(session)?;
        Ok(match session.context.get(KEY_STATUS) {
            Some(FlowStatus::Running { .. }) | None => FlowStatus::Running { next: current },
            Some(other) => other,
        })
    }

    /// Where the flow is.
    ///
    /// # Errors
    ///
    /// As [`ResearchFlow::resume`].
    pub async fn position(&self) -> Result<Position, FlowError> {
        let session = self.load().await?;
        Ok(Position {
            stage: self.current_stage(&session)?,
            status: self.status_of(&session)?,
        })
    }

    /// Status of the flow.
    ///
    /// # Errors
    ///
    /// As [`ResearchFlow::resume`].
    pub async fn status(&self) -> Result<FlowStatus, FlowError> {
        Ok(self.position().await?.status)
    }

    /// Everything learned so far.
    ///
    /// # Errors
    ///
    /// As [`ResearchFlow::resume`].
    pub async fn state(&self) -> Result<ResearchState, FlowError> {
        let session = self.load().await?;
        Ok(session.context.get(KEY_STATE).unwrap_or_default())
    }

    /// The user's request that started the flow.
    ///
    /// # Errors
    ///
    /// As [`ResearchFlow::resume`].
    pub async fn problem(&self) -> Result<String, FlowError> {
        let session = self.load().await?;
        Ok(session.context.get(KEY_PROBLEM).unwrap_or_default())
    }

    /// Runs the pending node and saves the session (one checkpoint).
    /// Does nothing when the flow waits for input, finished or was aborted.
    ///
    /// # Errors
    ///
    /// The node's error (the session stays at the last checkpoint), or a
    /// storage error.
    pub async fn step(&self) -> Result<FlowStatus, FlowError> {
        let status = self.status().await?;
        if !status.is_running() {
            return Ok(status);
        }
        self.runner.run(&self.session_id).await?;
        self.status().await
    }

    /// Runs nodes, one checkpoint each, until a gate, the end, an abort or
    /// [`MAX_STEPS_PER_RUN`] nodes.
    ///
    /// # Errors
    ///
    /// As [`ResearchFlow::step`].
    pub async fn run(&self) -> Result<FlowStatus, FlowError> {
        let mut status = self.status().await?;
        for _ in 0..MAX_STEPS_PER_RUN {
            if !status.is_running() {
                break;
            }
            status = self.step().await?;
        }
        Ok(status)
    }

    /// Answers the pending gate with `text` and runs on.
    ///
    /// At a confirmation gate a short "yes" moves to the next node and
    /// anything else reruns the node with `text` as feedback; after a
    /// question the node reruns with `text` as its answer.
    ///
    /// # Errors
    ///
    /// [`FlowError::NotWaiting`], [`FlowError::Completed`] or
    /// [`FlowError::Aborted`] when no gate is pending, or as
    /// [`ResearchFlow::run`].
    pub async fn provide_input(&self, text: impl Into<String>) -> Result<FlowStatus, FlowError> {
        let session = self.load().await?;
        let stage = match self.status_of(&session)? {
            FlowStatus::WaitingForInput { stage, .. } => stage,
            FlowStatus::Running { .. } => return Err(FlowError::NotWaiting),
            FlowStatus::Completed => return Err(FlowError::Completed),
            FlowStatus::Aborted => return Err(FlowError::Aborted),
        };
        let text = text.into();
        session.context.set(KEY_REPLY, GateReply { stage, text })?;
        session
            .context
            .set(KEY_STATUS, FlowStatus::Running { next: stage })?;
        self.save(session).await?;
        self.run().await
    }

    fn now(&self) -> u64 {
        self.graph.deps().now()
    }

    /// `/back <nodo>`: moves the flow to `to`, an earlier stage or the
    /// current one (any stage once finished). Nothing runs until
    /// [`ResearchFlow::run`].
    ///
    /// # Errors
    ///
    /// [`FlowError::InvalidBack`], [`FlowError::Aborted`], or a storage
    /// error.
    pub async fn back(&self, to: Stage) -> Result<FlowStatus, FlowError> {
        let mut session = self.load().await?;
        let current = self.current_stage(&session)?;
        let status = self.status_of(&session)?;
        if status == FlowStatus::Aborted {
            return Err(FlowError::Aborted);
        }
        if !self.graph.contains(to) {
            return Err(FlowError::InvalidBack {
                stage: to,
                reason: "it is not a node of this flow",
            });
        }
        if status != FlowStatus::Completed && to > current {
            return Err(FlowError::InvalidBack {
                stage: to,
                reason: "it is ahead of the current stage",
            });
        }
        let text = format!("Manual return from {} (/back).", current.name());
        log_and_clear_gate(&session, self.now(), to, &text)?;
        session.context.remove(KEY_ENTERED);
        session.context.remove(KEY_BACKS);
        session.current_task_id = task_id(to);
        let status = FlowStatus::Running { next: to };
        session.context.set(KEY_STATUS, &status)?;
        self.save(session).await?;
        Ok(status)
    }

    /// `/skip`: jumps over the current stage if it is optional (see
    /// [`OPTIONAL_STAGES`](super::node::OPTIONAL_STAGES)). Nothing runs
    /// until [`ResearchFlow::run`].
    ///
    /// # Errors
    ///
    /// [`FlowError::NotOptional`], [`FlowError::Completed`],
    /// [`FlowError::Aborted`], or a storage error.
    pub async fn skip(&self) -> Result<FlowStatus, FlowError> {
        let mut session = self.load().await?;
        let current = self.current_stage(&session)?;
        match self.status_of(&session)? {
            FlowStatus::Completed => return Err(FlowError::Completed),
            FlowStatus::Aborted => return Err(FlowError::Aborted),
            FlowStatus::Running { .. } | FlowStatus::WaitingForInput { .. } => {}
        }
        if !is_optional(current) {
            return Err(FlowError::NotOptional(current));
        }
        log_and_clear_gate(&session, self.now(), current, "Stage skipped (/skip).")?;
        let status = match self.graph.next_after(current) {
            Some(next) => {
                session.current_task_id = task_id(next);
                FlowStatus::Running { next }
            }
            None => FlowStatus::Completed,
        };
        session.context.set(KEY_STATUS, &status)?;
        self.save(session).await?;
        Ok(status)
    }

    /// `/abort`: stops the flow for good; its state and journal stay
    /// stored. Aborting twice is a no-op.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn abort(&self) -> Result<FlowStatus, FlowError> {
        let session = self.load().await?;
        if self.status_of(&session)? == FlowStatus::Aborted {
            return Ok(FlowStatus::Aborted);
        }
        let current = self.current_stage(&session)?;
        log_and_clear_gate(&session, self.now(), current, "Flow aborted (/abort).")?;
        session.context.set(KEY_STATUS, FlowStatus::Aborted)?;
        self.save(session).await?;
        Ok(FlowStatus::Aborted)
    }

    /// Applies a parsed command without running any node.
    ///
    /// # Errors
    ///
    /// As the method of each command.
    pub async fn command(&self, command: Command) -> Result<FlowReply, FlowError> {
        Ok(match command {
            Command::Stage => FlowReply::Position(self.position().await?),
            Command::Back(to) => FlowReply::Status(self.back(to).await?),
            Command::Skip => FlowReply::Status(self.skip().await?),
            Command::Abort => FlowReply::Status(self.abort().await?),
        })
    }

    /// Handles a line typed in the chat while the flow is active: a
    /// research command (`/back` and `/skip` then run on to the next
    /// gate), or else the answer to the pending gate.
    ///
    /// # Errors
    ///
    /// [`FlowError::Command`] for a malformed command, or as
    /// [`ResearchFlow::command`] and [`ResearchFlow::provide_input`].
    pub async fn handle_line(&self, line: &str) -> Result<FlowReply, FlowError> {
        match parse_command(line) {
            Some(command) => {
                let command = command?;
                let reply = self.command(command).await?;
                if matches!(command, Command::Back(_) | Command::Skip) {
                    return Ok(FlowReply::Status(self.run().await?));
                }
                Ok(reply)
            }
            None => Ok(FlowReply::Status(self.provide_input(line).await?)),
        }
    }
}
