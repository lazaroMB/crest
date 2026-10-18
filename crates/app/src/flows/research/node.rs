//! The contract of a research node: [`ResearchNode`], what it gets
//! ([`NodeContext`]) and what it answers ([`NodeOutcome`]).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use domain::block::{Block, BlockError, BlockId, BlockKind};
use domain::conversation::{ContentPart, Message, Session};
use domain::model::{ModelPolicy, ModelRole, ResolveRequest, ResolvedModel};
use domain::research::{ResearchState, Stage};
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::gate::RESERVED_PREFIX;
use super::json::{final_text, invalid_answer, parse_valid, retry_message, text_of};
use crate::ports::events::{DomainEvent, EventSink, NoticeLevel};
use crate::ports::llm::{Citation, LlmError, LlmPort, LlmRequest, MessageAssembler, SystemBlock};
use crate::usecases::run_turn::{AgentService, TurnError, TurnOptions};

/// Stages that `/skip` may jump over: the optional ones.
///
/// - `Data` (3): there may be no history, logs or metrics worth reading.
/// - `Prototype` (11): the spike is only needed when the hard part is
///   uncertain.
pub const OPTIONAL_STAGES: [Stage; 2] = [Stage::Data, Stage::Prototype];

/// Whether `/skip` may jump over `stage`.
#[must_use]
pub fn is_optional(stage: Stage) -> bool {
    OPTIONAL_STAGES.contains(&stage)
}

/// How a node run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeOutcome {
    /// The node finished. Gated nodes then ask the user to confirm.
    Done,
    /// The node needs an answer from the user; it runs again with it
    /// (see [`NodeContext::user_reply`]).
    NeedsInput {
        /// Question shown in the chat.
        prompt: String,
    },
    /// Something an earlier stage should have produced is missing: the
    /// flow goes back to `to` instead of inventing it.
    Back {
        /// Earlier stage to go back to.
        to: Stage,
        /// What is missing; written to the journal and shown.
        reason: String,
    },
}

/// Why a node failed. The flow stays at the last checkpoint.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// A model call failed.
    #[error(transparent)]
    Llm(#[from] LlmError),
    /// An agent turn failed.
    #[error(transparent)]
    Turn(#[from] TurnError),
    /// A block to emit is invalid.
    #[error("invalid block: {0}")]
    Block(#[from] BlockError),
    /// The model answered without text.
    #[error("the model answered no text")]
    EmptyAnswer,
    /// A port the node needs was not configured.
    #[error("{0} is not available to research nodes")]
    Unavailable(&'static str),
    /// The graph-flow context could not be read or written.
    #[error("flow context: {0}")]
    Context(String),
    /// Any other failure of the node.
    #[error("{0}")]
    Failed(String),
}

/// Wall clock in Unix epoch milliseconds.
pub(crate) fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The ports shared by every node.
///
/// Built with [`NodeDeps::new`] and the `with_*` methods, so new optional
/// ports can be added without breaking callers.
#[derive(Clone)]
pub struct NodeDeps {
    llm: Arc<dyn LlmPort>,
    models: ModelPolicy,
    events: Arc<dyn EventSink>,
    agent: Option<Arc<AgentService>>,
    explore_agent: Option<Arc<AgentService>>,
    workspace_root: String,
    clock: fn() -> u64,
}

impl std::fmt::Debug for NodeDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeDeps")
            .field("agent", &self.agent.is_some())
            .field("explore_agent", &self.explore_agent.is_some())
            .field("workspace_root", &self.workspace_root)
            .finish_non_exhaustive()
    }
}

impl NodeDeps {
    /// Deps with a model, its policy and an event sink.
    #[must_use]
    pub fn new(llm: Arc<dyn LlmPort>, models: ModelPolicy, events: Arc<dyn EventSink>) -> Self {
        Self {
            llm,
            models,
            events,
            agent: None,
            explore_agent: None,
            workspace_root: ".".to_owned(),
            clock: system_now_ms,
        }
    }

    /// Lets nodes run full agent turns (with tools) through `agent`.
    #[must_use]
    pub fn with_agent(mut self, agent: Arc<AgentService>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// The read-only agent of the stages that only explore (Observación,
    /// Datos, Investigación); see [`explore_agent`](super::agents::explore_agent).
    /// Without it they use the agent of [`NodeDeps::with_agent`].
    #[must_use]
    pub fn with_explore_agent(mut self, agent: Arc<AgentService>) -> Self {
        self.explore_agent = Some(agent);
        self
    }

    /// The workspace root the agent sessions of the nodes are opened in
    /// (`.` by default).
    #[must_use]
    pub fn with_workspace_root(mut self, root: impl Into<String>) -> Self {
        self.workspace_root = root.into();
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

    pub(crate) fn emit(&self, kind: BlockKind) -> Result<BlockId, BlockError> {
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

/// What a node sees while it runs.
///
/// The [`ResearchState`] is a copy: the frame writes it back to the flow
/// after the node returns, whatever the outcome, and drops it if the node
/// fails.
pub struct NodeContext {
    stage: Stage,
    deps: NodeDeps,
    state: ResearchState,
    problem: String,
    reply: Option<String>,
    flow: graph_flow::Context,
    stages: Option<Arc<[Stage]>>,
}

impl std::fmt::Debug for NodeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeContext")
            .field("stage", &self.stage)
            .field("reply", &self.reply)
            .finish_non_exhaustive()
    }
}

impl NodeContext {
    /// A context outside any flow, to test a node on its own: the node
    /// sees `problem` and `state` and its data stays in this context.
    #[must_use]
    pub fn new(
        stage: Stage,
        deps: NodeDeps,
        problem: impl Into<String>,
        state: ResearchState,
    ) -> Self {
        Self::in_flow(
            stage,
            deps,
            state,
            problem.into(),
            None,
            graph_flow::Context::new(),
        )
    }

    /// The same context as a rerun after the user answered `reply`.
    #[must_use]
    pub fn with_reply(mut self, reply: impl Into<String>) -> Self {
        self.reply = Some(reply.into());
        self
    }

    pub(crate) fn in_flow(
        stage: Stage,
        deps: NodeDeps,
        state: ResearchState,
        problem: String,
        reply: Option<String>,
        flow: graph_flow::Context,
    ) -> Self {
        Self {
            stage,
            deps,
            state,
            problem,
            reply,
            flow,
            stages: None,
        }
    }

    /// The same context inside a graph of `stages` (in order).
    #[must_use]
    pub fn with_stages(mut self, stages: Arc<[Stage]>) -> Self {
        self.stages = Some(stages);
        self
    }

    /// The state as the node left it.
    #[must_use]
    pub fn into_state(self) -> ResearchState {
        self.state
    }

    /// The stage being run.
    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The stages of the graph, in order; all of them outside a flow.
    #[must_use]
    pub fn stages(&self) -> &[Stage] {
        self.stages.as_deref().unwrap_or(&Stage::ALL)
    }

    /// Whether the graph has a node for `stage` (where `Back` may go).
    #[must_use]
    pub fn has_stage(&self, stage: Stage) -> bool {
        self.stages().contains(&stage)
    }

    /// The first of `candidates` that the graph has, else the first one:
    /// where to go back for something several stages could provide.
    #[must_use]
    pub fn first_stage_of(&self, candidates: &[Stage]) -> Stage {
        candidates
            .iter()
            .copied()
            .find(|s| self.has_stage(*s))
            .or_else(|| candidates.first().copied())
            .unwrap_or(Stage::Intake)
    }

    /// The workspace root (as given to [`NodeDeps::with_workspace_root`]).
    #[must_use]
    pub fn workspace_root(&self) -> &str {
        &self.deps.workspace_root
    }

    /// A new agent session in the workspace, for one node run.
    #[must_use]
    pub fn new_session(&self) -> Session {
        Session::new(self.deps.workspace_root.clone(), self.now())
    }

    /// Model role of the stage ([`Stage::default_role`]).
    #[must_use]
    pub fn role(&self) -> ModelRole {
        self.stage.default_role()
    }

    /// The user's original request that started the flow.
    #[must_use]
    pub fn problem(&self) -> &str {
        &self.problem
    }

    /// Everything learned so far.
    #[must_use]
    pub fn state(&self) -> &ResearchState {
        &self.state
    }

    /// Everything learned so far, to add evidence, hypotheses…
    pub fn state_mut(&mut self) -> &mut ResearchState {
        &mut self.state
    }

    /// The user's answer when this run follows a gate of this node: the
    /// answer to [`NodeOutcome::NeedsInput`], or the feedback given at the
    /// confirmation gate instead of approving. `None` on a first run.
    #[must_use]
    pub fn user_reply(&self) -> Option<&str> {
        self.reply.as_deref()
    }

    /// Current time in Unix epoch milliseconds.
    #[must_use]
    pub fn now(&self) -> u64 {
        self.deps.now()
    }

    /// Appends `text` to the research journal under this stage.
    pub fn log(&mut self, text: impl Into<String>) {
        let at = self.deps.now();
        self.state.log(at, self.stage, text);
    }

    /// Shows a block in the timeline (`Sources`, `Chart`, `Table`,
    /// `Mermaid`, `Markdown`…). The `StageHeader` is emitted by the frame.
    ///
    /// # Errors
    ///
    /// [`NodeError::Block`] when the block does not validate.
    pub fn emit(&self, kind: BlockKind) -> Result<BlockId, NodeError> {
        Ok(self.deps.emit(kind)?)
    }

    /// Shows a status-bar message.
    pub fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.deps.notice(level, message);
    }

    /// The event sink, for streaming (`TextDelta`, `BlockUpdated`).
    #[must_use]
    pub fn events(&self) -> &Arc<dyn EventSink> {
        &self.deps.events
    }

    /// The model port, for requests built by the node.
    #[must_use]
    pub fn llm(&self) -> &Arc<dyn LlmPort> {
        &self.deps.llm
    }

    /// The model of this stage's role.
    #[must_use]
    pub fn model(&self) -> ResolvedModel {
        self.deps.models.resolve(&ResolveRequest {
            role: self.role(),
            turn_override: None,
            pinned_role: None,
        })
    }

    /// A request to this stage's model with `system` and `messages`.
    #[must_use]
    pub fn request(&self, system: &str, messages: Vec<Message>) -> LlmRequest {
        let mut req = LlmRequest::new(self.model(), messages);
        if !system.is_empty() {
            req.system = vec![SystemBlock::new(system)];
        }
        req
    }

    /// Sends `req` and returns the text of the answer, trimmed.
    ///
    /// # Errors
    ///
    /// The model's [`LlmError`], or [`NodeError::EmptyAnswer`].
    pub async fn complete(&self, req: LlmRequest) -> Result<String, NodeError> {
        let (text, _) = self.complete_with_citations(req).await?;
        if text.is_empty() {
            return Err(NodeError::EmptyAnswer);
        }
        Ok(text)
    }

    /// Sends `req` (e.g. with web search) and returns the text of the
    /// answer, trimmed, and its citations.
    ///
    /// # Errors
    ///
    /// The model's [`LlmError`], or [`NodeError::EmptyAnswer`] when the
    /// answer has neither text nor citations.
    pub async fn complete_with_citations(
        &self,
        req: LlmRequest,
    ) -> Result<(String, Vec<Citation>), NodeError> {
        let mut stream = self.deps.llm.stream(req).await?;
        let mut assembler = MessageAssembler::new();
        while let Some(event) = stream.next().await {
            assembler
                .push(event?)
                .map_err(|e| LlmError::Decode(e.to_string()))?;
        }
        let assembled = assembler
            .finish()
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        let text = text_of(&assembled.message.content);
        if text.is_empty() && assembled.citations.is_empty() {
            return Err(NodeError::EmptyAnswer);
        }
        Ok((text, assembled.citations))
    }

    /// One question to this stage's model, without tools.
    ///
    /// # Errors
    ///
    /// As [`NodeContext::complete`].
    pub async fn ask(&self, system: &str, prompt: &str) -> Result<String, NodeError> {
        self.complete(self.request(system, vec![Message::user_text(prompt)]))
            .await
    }

    /// Asks the stage's model for a JSON object of type `T`, without tools
    /// (see [`NodeContext::ask_parsed`]).
    ///
    /// # Errors
    ///
    /// As [`NodeContext::ask_parsed`].
    pub async fn ask_json<T: DeserializeOwned>(
        &self,
        system: &str,
        prompt: &str,
        validate: impl Fn(&T) -> Result<(), String>,
    ) -> Result<T, NodeError> {
        self.ask_parsed(system, prompt, |text| parse_valid(text, &validate))
            .await
    }

    /// Asks `prompt` and turns the answer into `T` with `parse`; an answer
    /// it rejects is sent back once with the error, in the same
    /// conversation, and a second rejection is a [`NodeError::Failed`].
    ///
    /// # Errors
    ///
    /// The model's errors, or the invalid answer.
    pub async fn ask_parsed<T>(
        &self,
        system: &str,
        prompt: &str,
        parse: impl Fn(&str) -> Result<T, String>,
    ) -> Result<T, NodeError> {
        let answer = self.ask(system, prompt).await?;
        self.fix_parsed(system, vec![Message::user_text(prompt)], answer, parse)
            .await
    }

    /// [`NodeContext::fix_parsed`] for a JSON object of type `T`.
    ///
    /// # Errors
    ///
    /// As [`NodeContext::fix_parsed`].
    pub async fn fix_json<T: DeserializeOwned>(
        &self,
        system: &str,
        messages: Vec<Message>,
        answer: String,
        validate: impl Fn(&T) -> Result<(), String>,
    ) -> Result<T, NodeError> {
        self.fix_parsed(system, messages, answer, |text| {
            parse_valid(text, &validate)
        })
        .await
    }

    /// Turns `answer`, the model's reply to `messages`, into `T` with
    /// `parse`; if it is rejected, sends the error back once in the same
    /// conversation (without tools).
    ///
    /// # Errors
    ///
    /// The model's errors, or [`NodeError::Failed`] when the second answer
    /// is rejected too.
    pub async fn fix_parsed<T>(
        &self,
        system: &str,
        mut messages: Vec<Message>,
        answer: String,
        parse: impl Fn(&str) -> Result<T, String>,
    ) -> Result<T, NodeError> {
        let error = match parse(&answer) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        messages.push(Message::assistant(vec![ContentPart::text(answer)]));
        messages.push(Message::user_text(retry_message(&error)));
        let again = self.complete(self.request(system, messages)).await?;
        parse(&again).map_err(|e| invalid_answer(self.stage, &e))
    }

    /// Runs one turn of `agent` in `session` with `input` and turns its
    /// last answer into `T` with `parse`; a rejected answer gets a second
    /// turn in the same session with the error.
    ///
    /// # Errors
    ///
    /// The turn's errors, or [`NodeError::Failed`] when the second answer
    /// is rejected too.
    pub async fn agent_parsed<T>(
        &self,
        agent: &AgentService,
        session: &mut Session,
        input: impl Into<String>,
        parse: impl Fn(&str) -> Result<T, String>,
    ) -> Result<T, NodeError> {
        agent.run_turn(session, input, self.turn_options()).await?;
        let error = match parse(&final_text(session)) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        agent
            .run_turn(session, retry_message(&error), self.turn_options())
            .await?;
        parse(&final_text(session)).map_err(|e| invalid_answer(self.stage, &e))
    }

    /// [`NodeContext::agent_parsed`] for a JSON object of type `T`.
    ///
    /// # Errors
    ///
    /// As [`NodeContext::agent_parsed`].
    pub async fn agent_json<T: DeserializeOwned>(
        &self,
        agent: &AgentService,
        session: &mut Session,
        input: impl Into<String>,
        validate: impl Fn(&T) -> Result<(), String>,
    ) -> Result<T, NodeError> {
        self.agent_parsed(agent, session, input, |text| parse_valid(text, &validate))
            .await
    }

    /// The agent loop, to run turns with tools.
    ///
    /// # Errors
    ///
    /// [`NodeError::Unavailable`] when the flow was built without one.
    pub fn agent(&self) -> Result<&Arc<AgentService>, NodeError> {
        self.deps
            .agent
            .as_ref()
            .ok_or(NodeError::Unavailable("the agent service"))
    }

    /// The read-only agent of [`NodeDeps::with_explore_agent`], else the
    /// agent of [`NodeDeps::with_agent`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Unavailable`] when the flow has neither.
    pub fn explore_agent(&self) -> Result<&Arc<AgentService>, NodeError> {
        match &self.deps.explore_agent {
            Some(agent) => Ok(agent),
            None => self.agent(),
        }
    }

    /// Turn options for [`AgentService::run_turn`] with this stage's role.
    #[must_use]
    pub fn turn_options(&self) -> TurnOptions {
        TurnOptions::new(self.role())
    }

    /// Data a node stored with [`NodeContext::set_data`], from any node.
    #[must_use]
    pub fn data<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.flow.get(key)
    }

    /// Stores `value` in the flow under `key`; it is saved with the
    /// checkpoint. Use keys like `spec.folder` (namespaced by node).
    ///
    /// # Errors
    ///
    /// [`NodeError::Context`] for keys starting with `research.` (owned by
    /// the frame) or values that do not serialize.
    pub fn set_data(&self, key: &str, value: impl Serialize) -> Result<(), NodeError> {
        if key.starts_with(RESERVED_PREFIX) {
            return Err(NodeError::Context(format!(
                "the key {key:?} is reserved for the research frame"
            )));
        }
        self.flow
            .set(key, value)
            .map_err(|e| NodeError::Context(e.to_string()))
    }
}

/// A node of the research graph. Implemented once per stage (T5.5–T5.9)
/// and wrapped by [`NodeTask`](super::task::NodeTask).
#[async_trait]
pub trait ResearchNode: Send + Sync {
    /// The stage this node implements.
    fn stage(&self) -> Stage;

    /// Whether the node asks the user to confirm when it finishes.
    fn has_gate(&self) -> bool {
        self.stage().has_gate()
    }

    /// Runs the stage. Called again after each gate answer that is not an
    /// approval, with [`NodeContext::user_reply`] set.
    ///
    /// # Errors
    ///
    /// [`NodeError`] when it cannot finish; the flow stays at the last
    /// checkpoint and the error is shown.
    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn context(llm: Arc<ScriptedLlm>, sink: Arc<RecordingSink>) -> NodeContext {
        let deps = NodeDeps::new(llm, ModelPolicy::default(), sink).with_clock(|| 7);
        NodeContext::new(
            Stage::Hypotheses,
            deps,
            "el login es lento",
            ResearchState::default(),
        )
    }

    #[tokio::test]
    async fn ask_uses_the_role_of_the_stage() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            "  H1: la caché  ",
        )]));
        let ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()));
        assert_eq!(
            ctx.ask("sistema", "pregunta").await.unwrap(),
            "H1: la caché"
        );
        let req = llm.last_request().unwrap();
        assert_eq!(req.model.role, ModelRole::Plan);
        assert_eq!(req.system[0].text, "sistema");
    }

    #[tokio::test]
    async fn empty_answers_are_errors() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text("   ")]));
        let ctx = context(llm, Arc::new(RecordingSink::new()));
        assert!(matches!(
            ctx.ask("", "x").await,
            Err(NodeError::EmptyAnswer)
        ));
    }

    #[test]
    fn log_writes_the_journal_under_the_stage() {
        let mut ctx = context(
            Arc::new(ScriptedLlm::new([])),
            Arc::new(RecordingSink::new()),
        );
        ctx.log("dos hipótesis");
        let entry = &ctx.state().journal[0];
        assert_eq!((entry.at, entry.stage), (7, Stage::Hypotheses));
        assert_eq!(entry.text, "dos hipótesis");
    }

    #[test]
    fn emit_validates_blocks() {
        let sink = Arc::new(RecordingSink::new());
        let ctx = context(Arc::new(ScriptedLlm::new([])), Arc::clone(&sink));
        ctx.emit(BlockKind::Markdown {
            title: None,
            source: "# H".into(),
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

    #[test]
    fn node_data_cannot_use_reserved_keys() {
        let ctx = context(
            Arc::new(ScriptedLlm::new([])),
            Arc::new(RecordingSink::new()),
        );
        ctx.set_data("spec.folder", ".crest/specs/0001").unwrap();
        assert_eq!(
            ctx.data::<String>("spec.folder").as_deref(),
            Some(".crest/specs/0001")
        );
        assert!(ctx.set_data("research.state", 1).is_err());
        assert!(ctx.agent().is_err());
    }

    #[test]
    fn reruns_see_the_reply_and_return_the_state() {
        let mut ctx = context(
            Arc::new(ScriptedLlm::new([])),
            Arc::new(RecordingSink::new()),
        )
        .with_reply("agrega H2");
        assert_eq!(ctx.user_reply(), Some("agrega H2"));
        assert_eq!(ctx.problem(), "el login es lento");
        ctx.state_mut().problem_statements.push("p".into());
        assert_eq!(ctx.into_state().problem_statements, ["p"]);
    }

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Items {
        items: Vec<String>,
    }

    fn non_empty(a: &Items) -> Result<(), String> {
        if a.items.is_empty() {
            Err("`items` está vacío".into())
        } else {
            Ok(())
        }
    }

    #[tokio::test]
    async fn ask_json_retries_once_with_the_error() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("{\"items\": []}"),
            ScriptedResponse::text("```json\n{\"items\": [\"a\"]}\n```"),
        ]));
        let ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()));
        let answer: Items = ctx.ask_json("s", "dame items", non_empty).await.unwrap();
        assert_eq!(answer.items, ["a"]);
        let retry = llm.last_request().unwrap();
        assert_eq!(retry.messages.len(), 3);
        let ContentPart::Text { text } = &retry.messages[2].content[0] else {
            panic!("expected text");
        };
        assert!(text.contains("`items` está vacío"), "{text}");
    }

    #[tokio::test]
    async fn ask_json_fails_after_two_invalid_answers() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("no"),
            ScriptedResponse::text("tampoco"),
        ]));
        let ctx = context(llm, Arc::new(RecordingSink::new()));
        let err = ctx
            .ask_json::<Items>("s", "p", non_empty)
            .await
            .unwrap_err();
        assert!(matches!(err, NodeError::Failed(m) if m.contains("Hipótesis")));
    }

    #[test]
    fn stages_default_to_all_and_follow_the_graph() {
        let ctx = context(
            Arc::new(ScriptedLlm::new([])),
            Arc::new(RecordingSink::new()),
        );
        assert_eq!(ctx.stages(), Stage::ALL);
        assert_eq!(
            ctx.first_stage_of(&[Stage::Data, Stage::Observation]),
            Stage::Data
        );
        assert_eq!(ctx.workspace_root(), ".");
        assert!(ctx.explore_agent().is_err());
        let ctx = ctx.with_stages([Stage::Observation, Stage::Hypotheses].into());
        assert!(!ctx.has_stage(Stage::Data));
        assert_eq!(
            ctx.first_stage_of(&[Stage::Data, Stage::Observation]),
            Stage::Observation
        );
    }

    #[test]
    fn only_data_and_prototype_are_optional() {
        let optional: Vec<_> = Stage::ALL.into_iter().filter(|s| is_optional(*s)).collect();
        assert_eq!(optional, [Stage::Data, Stage::Prototype]);
    }
}
