//! The contract of an implementation step: [`ImplementNode`], what it gets
//! ([`StepContext`], over [`ImplementDeps`]) and what it answers
//! ([`StepOutcome`]).
//!
//! Errors are the research flow's [`NodeError`]: the flow stays at the
//! last checkpoint and the error is shown.

use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{Block, BlockId, BlockKind};
use domain::conversation::{ContentPart, Role, Session};
use domain::model::ModelPolicy;
use domain::workspace::WorkspaceRoot;

use super::state::{EntryKind, ImplementState, Step};
pub use crate::flows::research::node::NodeError;
use crate::flows::research::node::system_now_ms;
use crate::ports::events::{DomainEvent, EventSink, NoticeLevel};
use crate::ports::fs::FileSystemPort;
use crate::ports::shell::ShellPort;
use crate::tools::registry::ToolRegistry;
use crate::usecases::run_turn::{AgentDeps, AgentService, SharedPermissions};

/// How a step run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// Go on with the next step of the graph (the flow ends after the
    /// last one).
    Next,
    /// Go to `step` instead, e.g. back to `Implement` for the next task.
    GoTo(Step),
    /// Stop at a gate: show `prompt` and run the step again with the
    /// user's answer (see [`StepContext::user_reply`]).
    Ask {
        /// Question shown in the chat.
        prompt: String,
    },
    /// The flow is done.
    Finish,
    /// The plan is not feasible: stop and offer to go back to the Spec.
    BackToSpec {
        /// Why; written to the journal.
        reason: String,
    },
    /// The user aborted the flow.
    Abort {
        /// Why; written to the journal.
        reason: String,
    },
}

/// What the implementation agent is built from. Every task gets its own
/// [`AgentService`] over these, with the file tools restricted to the
/// task's files.
#[derive(Clone)]
pub struct AgentKit {
    /// Model, events, approvals and telemetry of the agent.
    pub deps: AgentDeps,
    /// Tools offered to the agent (the file tools included).
    pub tools: ToolRegistry,
    /// Model selection; each task uses its own role.
    pub models: ModelPolicy,
    /// Permission policy, shared with the chat.
    pub permissions: SharedPermissions,
    /// Extra system prompt (project instructions), appended to the step's.
    pub system_prompt: String,
    /// Limit of model calls per turn, if not the default.
    pub max_iterations: Option<u32>,
}

impl std::fmt::Debug for AgentKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentKit")
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

impl AgentKit {
    /// A kit with no extra system prompt and the default limits.
    #[must_use]
    pub fn new(
        deps: AgentDeps,
        tools: ToolRegistry,
        models: ModelPolicy,
        permissions: SharedPermissions,
    ) -> Self {
        Self {
            deps,
            tools,
            models,
            permissions,
            system_prompt: String::new(),
            max_iterations: None,
        }
    }

    /// An agent over `tools` (usually [`AgentKit::tools`] restricted to a
    /// task) whose system prompt is `system` and then the kit's own.
    #[must_use]
    pub fn service(&self, tools: ToolRegistry, system: &str) -> AgentService {
        let prompt = if self.system_prompt.trim().is_empty() {
            system.to_owned()
        } else {
            format!("{system}\n\n{}", self.system_prompt)
        };
        let service = AgentService::new(
            self.deps.clone(),
            tools,
            self.models.clone(),
            Arc::clone(&self.permissions),
        )
        .with_system_prompt(prompt);
        match self.max_iterations {
            Some(limit) => service.with_max_iterations(limit),
            None => service,
        }
    }
}

/// The ports shared by every step.
#[derive(Clone)]
pub struct ImplementDeps {
    events: Arc<dyn EventSink>,
    fs: Arc<dyn FileSystemPort>,
    shell: Arc<dyn ShellPort>,
    agent: Option<AgentKit>,
    clock: fn() -> u64,
}

impl std::fmt::Debug for ImplementDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImplementDeps")
            .field("agent", &self.agent)
            .finish_non_exhaustive()
    }
}

impl ImplementDeps {
    /// Deps that show blocks on `events`, read and write files through
    /// `fs` and run checks and git through `shell`.
    #[must_use]
    pub fn new(
        events: Arc<dyn EventSink>,
        fs: Arc<dyn FileSystemPort>,
        shell: Arc<dyn ShellPort>,
    ) -> Self {
        Self {
            events,
            fs,
            shell,
            agent: None,
            clock: system_now_ms,
        }
    }

    /// Lets the steps run agent turns built from `kit`.
    #[must_use]
    pub fn with_agent(mut self, kit: AgentKit) -> Self {
        self.agent = Some(kit);
        self
    }

    /// Replaces the wall clock (Unix epoch milliseconds), for tests.
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> u64) -> Self {
        self.clock = clock;
        self
    }

    pub(crate) fn now(&self) -> u64 {
        (self.clock)()
    }

    pub(crate) fn root(&self) -> &WorkspaceRoot {
        self.fs.root()
    }

    pub(crate) fn emit(&self, kind: BlockKind) -> Result<BlockId, NodeError> {
        let block = Block::new(kind);
        block.validate()?;
        let id = block.id;
        self.events.emit(DomainEvent::BlockAdded { block });
        Ok(id)
    }

    pub(crate) fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.events.emit(DomainEvent::Notice {
            level,
            message: message.into(),
        });
    }
}

/// What a step sees while it runs.
///
/// The [`ImplementState`] is a copy: the frame writes it back to the flow
/// after the step returns and drops it if the step fails.
pub struct StepContext {
    step: Step,
    deps: ImplementDeps,
    state: ImplementState,
    reply: Option<String>,
}

impl std::fmt::Debug for StepContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepContext")
            .field("step", &self.step)
            .field("reply", &self.reply)
            .finish_non_exhaustive()
    }
}

impl StepContext {
    /// A context outside any flow, to test a step on its own.
    #[must_use]
    pub fn new(step: Step, deps: ImplementDeps, state: ImplementState) -> Self {
        Self {
            step,
            deps,
            state,
            reply: None,
        }
    }

    pub(crate) fn in_flow(
        step: Step,
        deps: ImplementDeps,
        state: ImplementState,
        reply: Option<String>,
    ) -> Self {
        Self {
            step,
            deps,
            state,
            reply,
        }
    }

    /// The same context as a rerun after the user answered `reply`.
    #[must_use]
    pub fn with_reply(mut self, reply: impl Into<String>) -> Self {
        self.reply = Some(reply.into());
        self
    }

    /// The state as the step left it.
    #[must_use]
    pub fn into_state(self) -> ImplementState {
        self.state
    }

    /// The step being run.
    #[must_use]
    pub fn step(&self) -> Step {
        self.step
    }

    /// Everything known so far.
    #[must_use]
    pub fn state(&self) -> &ImplementState {
        &self.state
    }

    /// Everything known so far, to record progress.
    pub fn state_mut(&mut self) -> &mut ImplementState {
        &mut self.state
    }

    /// The user's answer when this run follows a gate of this step.
    #[must_use]
    pub fn user_reply(&self) -> Option<&str> {
        self.reply.as_deref()
    }

    /// Current time in Unix epoch milliseconds.
    #[must_use]
    pub fn now(&self) -> u64 {
        self.deps.now()
    }

    /// Appends `text` to the journal under this step and, for the
    /// per-task steps, the current task.
    pub fn log(&mut self, kind: EntryKind, text: impl Into<String>) {
        let at = self.deps.now();
        let task = if self.step.is_per_task() {
            self.state.current_task().map(|t| t.id.clone())
        } else {
            None
        };
        self.state.log(at, self.step, task, kind, text);
    }

    /// Shows a block in the timeline.
    ///
    /// # Errors
    ///
    /// [`NodeError::Block`] when the block does not validate.
    pub fn emit(&self, kind: BlockKind) -> Result<BlockId, NodeError> {
        self.deps.emit(kind)
    }

    /// Shows a status-bar message.
    pub fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.deps.notice(level, message);
    }

    /// The event sink.
    #[must_use]
    pub fn events(&self) -> &Arc<dyn EventSink> {
        &self.deps.events
    }

    /// The workspace files.
    #[must_use]
    pub fn fs(&self) -> &Arc<dyn FileSystemPort> {
        &self.deps.fs
    }

    /// The shell, for checks and git.
    #[must_use]
    pub fn shell(&self) -> &Arc<dyn ShellPort> {
        &self.deps.shell
    }

    /// What the agent is built from.
    ///
    /// # Errors
    ///
    /// [`NodeError::Unavailable`] when the flow was built without one.
    pub fn agent_kit(&self) -> Result<&AgentKit, NodeError> {
        self.deps
            .agent
            .as_ref()
            .ok_or(NodeError::Unavailable("the implementation agent"))
    }

    /// A fresh conversation for an agent turn in this workspace.
    #[must_use]
    pub fn new_session(&self) -> Session {
        let root = self.deps.fs.root().as_path().display().to_string();
        Session::new(root, self.now())
    }
}

/// Text of the last assistant message of the session's last turn.
#[must_use]
pub fn final_text(session: &Session) -> String {
    session
        .turns
        .last()
        .and_then(|turn| {
            turn.messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
        })
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// A step of the implementation graph, wrapped by
/// [`StepTask`](super::task::StepTask).
#[async_trait]
pub trait ImplementNode: Send + Sync {
    /// The step this node implements.
    fn step(&self) -> Step;

    /// Runs the step. Called again after each gate answer, with
    /// [`StepContext::user_reply`] set.
    ///
    /// # Errors
    ///
    /// [`NodeError`] when it cannot finish; the flow stays at the last
    /// checkpoint and the error is shown.
    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError>;
}

#[cfg(test)]
pub(crate) mod tests {
    use domain::workspace::WorkspaceRoot;

    use super::*;
    use crate::testing::fs::{InMemoryFs, ScriptedShell};
    use crate::testing::system::RecordingSink;

    pub(crate) fn deps(sink: &Arc<RecordingSink>) -> ImplementDeps {
        let fs = InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap());
        ImplementDeps::new(
            Arc::clone(sink) as _,
            Arc::new(fs),
            Arc::new(ScriptedShell::new()),
        )
        .with_clock(|| 9)
    }

    #[test]
    fn log_writes_the_journal_under_the_step() {
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = StepContext::new(Step::Verify, deps(&sink), ImplementState::new("p"));
        ctx.log(EntryKind::Achievement, "all good");
        let entry = &ctx.state().journal[0];
        assert_eq!(
            (entry.at, entry.step, entry.kind),
            (9, Step::Verify, EntryKind::Achievement)
        );
        assert_eq!(entry.task, None);
        assert!(ctx.agent_kit().is_err());
    }

    #[test]
    fn emit_validates_blocks() {
        let sink = Arc::new(RecordingSink::new());
        let ctx =
            StepContext::new(Step::Load, deps(&sink), ImplementState::new("p")).with_reply("yes");
        assert_eq!(ctx.user_reply(), Some("yes"));
        ctx.emit(BlockKind::Text {
            markdown: "hello".into(),
        })
        .unwrap();
        let bad = ctx.emit(BlockKind::StageHeader {
            stage: "x".into(),
            index: 9,
            total: 3,
        });
        assert!(matches!(bad, Err(NodeError::Block(_))));
        assert_eq!(sink.events().len(), 1);
    }
}
