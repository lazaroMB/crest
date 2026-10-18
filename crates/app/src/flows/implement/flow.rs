//! [`ImplementFlow`]: runs an implementation graph for one session, one
//! step per checkpoint, and feeds it the user's answers at the gates.

use std::fmt;
use std::sync::Arc;

use graph_flow::{FlowRunner, GraphError, Session, SessionStorage};

use super::builder::{GRAPH_ID, ImplementGraph};
use super::state::{
    EntryKind, FlowStatus, GateReply, ImplementState, KEY_GATE, KEY_REPLY, KEY_STATE, KEY_STATUS,
    Step,
};

/// Most steps one [`ImplementFlow::run`] goes through before returning,
/// even if no gate stops it (each task takes three).
pub const MAX_STEPS_PER_RUN: usize = 256;

/// Why a flow operation failed.
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    /// The steps given to the builder do not form a valid graph.
    #[error("invalid implementation graph: {0}")]
    InvalidGraph(String),
    /// No stored session has that id.
    #[error("implementation session {0} not found")]
    SessionNotFound(String),
    /// `start` was called with the id of an existing session.
    #[error("implementation session {0} already exists")]
    SessionExists(String),
    /// The stored session points at a task this graph does not have.
    #[error("implementation session is at {0:?}, which is not a step of this graph")]
    UnknownTask(String),
    /// Input was given while no step waits for it.
    #[error("the implementation flow is not waiting for input")]
    NotWaiting,
    /// The flow already stopped (completed, aborted or back to the Spec).
    #[error("the implementation flow already stopped")]
    Finished,
    /// graph-flow or its storage failed (a step error included).
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// Where the flow is, as `/stage` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Current (or last) step.
    pub step: Step,
    /// Status of the flow.
    pub status: FlowStatus,
    /// The task being worked on, as `(id, 1-based index, total)`.
    pub task: Option<(String, usize, usize)>,
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Paso {}/{}: {}",
            self.step.index(),
            Step::total(),
            self.step.name_es()
        )?;
        if let Some((id, n, total)) = &self.task {
            write!(f, " · tarea {id} ({n}/{total})")?;
        }
        match &self.status {
            FlowStatus::Running { .. } => write!(f, " · en curso"),
            FlowStatus::WaitingForInput { .. } => write!(f, " · esperando tu respuesta"),
            FlowStatus::Completed => write!(f, " · flujo terminado"),
            FlowStatus::Aborted => write!(f, " · flujo abortado"),
            FlowStatus::BackToSpec { .. } => write!(f, " · de vuelta al Spec"),
        }
    }
}

/// What a chat line did ([`ImplementFlow::handle_line`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowReply {
    /// `/stage`: where the flow is.
    Position(Position),
    /// Status after the command or the answer.
    Status(FlowStatus),
}

/// An implementation flow bound to one stored session.
///
/// Every finished step is saved before the next one runs, so dropping the
/// flow (or the app) and calling [`ImplementFlow::resume`] continues at
/// the pending step.
#[derive(Clone)]
pub struct ImplementFlow {
    graph: ImplementGraph,
    storage: Arc<dyn SessionStorage>,
    runner: FlowRunner,
    session_id: String,
}

impl fmt::Debug for ImplementFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImplementFlow")
            .field("session_id", &self.session_id)
            .field("graph", &self.graph)
            .finish_non_exhaustive()
    }
}

impl ImplementFlow {
    fn bind(graph: ImplementGraph, storage: Arc<dyn SessionStorage>, session_id: String) -> Self {
        let runner = FlowRunner::new(Arc::clone(graph.graph()), Arc::clone(&storage));
        Self {
            graph,
            storage,
            runner,
            session_id,
        }
    }

    /// Creates the session `session_id` to implement the plan at
    /// `plan_path` (a workspace path), at the first step. Nothing runs
    /// until [`ImplementFlow::run`] or [`ImplementFlow::step`].
    ///
    /// # Errors
    ///
    /// [`FlowError::SessionExists`], or a storage error.
    pub async fn start(
        graph: ImplementGraph,
        storage: Arc<dyn SessionStorage>,
        session_id: impl Into<String>,
        plan_path: impl Into<String>,
    ) -> Result<Self, FlowError> {
        let session_id = session_id.into();
        if storage.get(&session_id).await?.is_some() {
            return Err(FlowError::SessionExists(session_id));
        }
        let first = graph.first();
        let session =
            Session::new_from_task(session_id.clone(), &first.task_id()).with_graph_id(GRAPH_ID);
        session
            .context
            .set(KEY_STATE, ImplementState::new(plan_path))?;
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
    /// [`FlowError::SessionNotFound`], [`FlowError::UnknownTask`], or a
    /// storage error.
    pub async fn resume(
        graph: ImplementGraph,
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

    async fn load(&self) -> Result<Session, FlowError> {
        self.storage
            .get(&self.session_id)
            .await?
            .ok_or_else(|| FlowError::SessionNotFound(self.session_id.clone()))
    }

    fn current_step(&self, session: &Session) -> Result<Step, FlowError> {
        Step::of_task(&session.current_task_id)
            .filter(|s| self.graph.contains(*s))
            .ok_or_else(|| FlowError::UnknownTask(session.current_task_id.clone()))
    }

    fn status_of(&self, session: &Session) -> Result<FlowStatus, FlowError> {
        let current = self.current_step(session)?;
        Ok(match session.context.get(KEY_STATUS) {
            Some(FlowStatus::Running { .. }) | None => FlowStatus::Running { next: current },
            Some(other) => other,
        })
    }

    /// Where the flow is.
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::resume`].
    pub async fn position(&self) -> Result<Position, FlowError> {
        let session = self.load().await?;
        let step = self.current_step(&session)?;
        let state: ImplementState = session.context.get(KEY_STATE).unwrap_or_default();
        let task = state
            .order
            .get(state.current)
            .filter(|_| step.is_per_task())
            .map(|id| (id.to_string(), state.current + 1, state.order.len()));
        Ok(Position {
            step,
            status: self.status_of(&session)?,
            task,
        })
    }

    /// Status of the flow.
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::resume`].
    pub async fn status(&self) -> Result<FlowStatus, FlowError> {
        Ok(self.position().await?.status)
    }

    /// Everything known so far: plan, progress, checks and journal.
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::resume`].
    pub async fn state(&self) -> Result<ImplementState, FlowError> {
        let session = self.load().await?;
        Ok(session.context.get(KEY_STATE).unwrap_or_default())
    }

    /// Runs the pending step and saves the session (one checkpoint). Does
    /// nothing when the flow waits for input or stopped.
    ///
    /// # Errors
    ///
    /// The step's error (the session stays at the last checkpoint), or a
    /// storage error.
    pub async fn step(&self) -> Result<FlowStatus, FlowError> {
        let status = self.status().await?;
        if !status.is_running() {
            return Ok(status);
        }
        self.runner.run(&self.session_id).await?;
        self.status().await
    }

    /// Runs steps, one checkpoint each, until a gate, the end or
    /// [`MAX_STEPS_PER_RUN`] steps.
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::step`].
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
    /// # Errors
    ///
    /// [`FlowError::NotWaiting`] or [`FlowError::Finished`] when no gate is
    /// pending, or as [`ImplementFlow::run`].
    pub async fn provide_input(&self, text: impl Into<String>) -> Result<FlowStatus, FlowError> {
        let session = self.load().await?;
        let step = match self.status_of(&session)? {
            FlowStatus::WaitingForInput { step, .. } => step,
            FlowStatus::Running { .. } => return Err(FlowError::NotWaiting),
            _ => return Err(FlowError::Finished),
        };
        session.context.set(
            KEY_REPLY,
            GateReply {
                step,
                text: text.into(),
            },
        )?;
        session
            .context
            .set(KEY_STATUS, FlowStatus::Running { next: step })?;
        self.storage.save(session).await?;
        self.run().await
    }

    /// `/abort`: stops the flow for good; its state and journal stay
    /// stored. Aborting a stopped flow is a no-op.
    ///
    /// # Errors
    ///
    /// A storage error.
    pub async fn abort(&self) -> Result<FlowStatus, FlowError> {
        let session = self.load().await?;
        let status = self.status_of(&session)?;
        if status.is_finished() {
            return Ok(status);
        }
        let step = self.current_step(&session)?;
        let mut state: ImplementState = session.context.get(KEY_STATE).unwrap_or_default();
        let now = self.graph.deps().now();
        state.log(now, step, None, EntryKind::Note, "Flujo abortado (/abort).");
        session.context.set(KEY_STATE, state)?;
        session.context.remove(KEY_GATE);
        session.context.remove(KEY_REPLY);
        session.context.set(KEY_STATUS, FlowStatus::Aborted)?;
        self.storage.save(session).await?;
        Ok(FlowStatus::Aborted)
    }

    /// Handles a line typed in the chat while the flow is active: `/stage`,
    /// `/abort`, or else the answer to the pending gate.
    ///
    /// # Errors
    ///
    /// As [`ImplementFlow::provide_input`] and [`ImplementFlow::abort`].
    pub async fn handle_line(&self, line: &str) -> Result<FlowReply, FlowError> {
        match line.trim() {
            "/stage" => Ok(FlowReply::Position(self.position().await?)),
            "/abort" => Ok(FlowReply::Status(self.abort().await?)),
            _ => Ok(FlowReply::Status(self.provide_input(line).await?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use graph_flow::InMemorySessionStorage;

    use super::*;
    use crate::flows::implement::builder::ImplementGraphBuilder;
    use crate::flows::implement::fake::FakeStep;
    use crate::flows::implement::node::StepOutcome;
    use crate::flows::implement::node::tests::deps;
    use crate::testing::system::RecordingSink;

    fn graph(load: FakeStep) -> ImplementGraph {
        ImplementGraphBuilder::new(deps(&Arc::new(RecordingSink::new())))
            .node(load)
            .node(FakeStep::new(Step::Implement))
            .build()
            .unwrap()
    }

    fn asking() -> FakeStep {
        FakeStep::new(Step::Load).then(StepOutcome::Ask {
            prompt: "¿Empezamos?".into(),
        })
    }

    #[tokio::test]
    async fn runs_to_the_gate_and_on_after_the_answer() {
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let flow = ImplementFlow::start(graph(asking()), Arc::clone(&storage), "s1", "a/plan.json")
            .await
            .unwrap();
        assert_eq!(flow.state().await.unwrap().plan_path, "a/plan.json");
        let status = flow.run().await.unwrap();
        assert!(matches!(
            status,
            FlowStatus::WaitingForInput {
                step: Step::Load,
                ..
            }
        ));
        assert!(matches!(
            flow.step().await.unwrap(),
            FlowStatus::WaitingForInput { .. }
        ));

        // A restart resumes at the gate.
        let flow = ImplementFlow::resume(graph(FakeStep::new(Step::Load)), storage, "s1")
            .await
            .unwrap();
        let reply = flow.handle_line("/stage").await.unwrap();
        let FlowReply::Position(position) = reply else {
            panic!("{reply:?}")
        };
        assert!(position.to_string().starts_with("Paso 1/6: Cargar plan"));
        assert_eq!(
            flow.handle_line("sí").await.unwrap(),
            FlowReply::Status(FlowStatus::Completed)
        );
        assert!(matches!(
            flow.provide_input("x").await,
            Err(FlowError::Finished)
        ));
    }

    #[tokio::test]
    async fn abort_stops_and_start_rejects_existing_sessions() {
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let flow = ImplementFlow::start(graph(asking()), Arc::clone(&storage), "s", "p.json")
            .await
            .unwrap();
        assert!(matches!(
            flow.provide_input("x").await,
            Err(FlowError::NotWaiting)
        ));
        flow.run().await.unwrap();
        assert_eq!(flow.abort().await.unwrap(), FlowStatus::Aborted);
        assert_eq!(flow.run().await.unwrap(), FlowStatus::Aborted);
        assert_eq!(flow.abort().await.unwrap(), FlowStatus::Aborted);
        let again = ImplementFlow::start(graph(asking()), storage, "s", "p.json").await;
        assert!(matches!(again, Err(FlowError::SessionExists(_))));
    }
}
