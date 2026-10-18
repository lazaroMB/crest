//! The agent loop: send the conversation to the model, run the tools it
//! asks for, send the results back and repeat until it ends its turn.
//!
//! Everything visible happens through [`DomainEvent`]s, in this order for
//! each model call: `BlockAdded` (`Text`, `Thinking` or `ToolCall`) when a
//! content block starts, its `TextDelta`s / `ThinkingDelta`s, and
//! `UsageUpdated` when the answer is complete. Then, for each tool call in
//! order: `BlockUpdated` with its input and summary, the optional extra
//! block of the tool (e.g. a `Diff`), `ToolStatusChanged` to `Running` and
//! to its final status. The turn is framed by `TurnStarted` and
//! `TurnFinished` (or `TurnFailed`).
//!
//! Between preparing and running a call the [`PermissionPolicy`] decides:
//! `Allow` runs it, `Deny` answers the model with an error without asking,
//! and `Ask` emits `ToolStatusChanged(AwaitingApproval)` and
//! `ApprovalRequested` and waits for the [`ApprovalPort`] (see
//! [`super::approval`]). A rejected call ends with status `Denied`.
//!
//! With a plugin host ([`AgentService::with_plugin_host`]) the
//! `before_tool` hooks run before each call is prepared: `Deny` answers the
//! model with `denied by hook: <reason>` without asking (status `Denied`)
//! and `ReplaceInput` prepares the call with the new input. After a call
//! runs, the `after_tool` hooks get its input and output
//! (`{ text, is_error }`) to observe; calls that were denied do not reach
//! them. A hook that fails is reported as a warning `Notice` and the turn
//! goes on.
//!
//! Cancelling [`TurnOptions::cancel`] interrupts the turn at any await
//! point: the model stream or the running tool future is dropped (which
//! closes the connection or kills the process), the partial answer is kept
//! without its tool uses, every tool use already in the history gets a
//! `cancelled by user` result, open tool calls end as `Cancelled`, and the
//! turn finishes with [`StopReason::Interrupted`]. The session can then be
//! sent to the model again.
//!
//! With a [`CompactionPolicy`] ([`AgentService::with_compaction`]) the
//! context is estimated before each model call (the input tokens reported
//! by the previous call plus the messages added since, or ~4 bytes per
//! token when nothing was reported yet). Past the threshold, the oldest
//! finished turns are summarized by the `compact` role (see
//! [`super::compact`]) and the call goes on with the shorter history; an
//! `Info` `Notice` reports it. If the model still answers
//! [`LlmError::ContextTooLong`], the history is compacted and the call
//! retried once. A failed summary is a warning `Notice`, not an error.
//!
//! With a [`CostTracker`] ([`AgentService::with_cost_tracker`]) every
//! answer, the summaries of compaction included, is priced and
//! `UsageUpdated` carries the session's cost; without a price for the
//! model it stays `None`. With a spending cap, before each model call
//! (compaction included) a session that reached it pauses: a warning
//! `Notice` reports it and the [`ApprovalPort`] is asked (tool
//! `spend_cap`). "Approve" raises the cap by its amount, "approve always"
//! lifts it for the session, and a denial (or a failed request) ends the
//! turn with [`StopReason::Interrupted`] and a warning `Notice`; the
//! history is left ready to be sent again.
//!
//! [`PermissionPolicy`]: domain::permission::PermissionPolicy

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use domain::block::{ApprovalState, Block, BlockId, BlockKind, ToolStatus};
use domain::conversation::{
    ContentPart, Message, Role, Session, SessionId, StopReason, ToolUseId, Turn, TurnId, Usage,
};
use domain::model::{ModelId, ModelPolicy, ModelRole, ResolveRequest, ResolvedModel};
use domain::permission::{ActionRequest, Decision};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::cost::CostTracker;
use crate::ports::approval::{ApprovalPort, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink, NoticeLevel};
use crate::ports::llm::{
    AssembledMessage, LlmError, LlmEvent, LlmPort, LlmRequest, MessageAssembler, RequestMetadata,
    SystemBlock,
};
use crate::ports::plugin::{HookEvent, HookOutcome, PluginError, PluginHostPort};
use crate::ports::telemetry::{LlmCallRecord, SpanId, TelemetryPort, ToolCallRecord, TurnSpan};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, ToolRegistry};

pub use super::approval::SharedPermissions;
use super::approval::{ApprovalGate, PendingApproval, UserVerdict, policy_denial};
use super::cancel::{CancelToken, Interrupted};
pub use super::compact::CompactionPolicy;
use super::compact::{
    compactable_turns, context_tokens, estimate_tokens, replace_turns,
    summarize as summarize_history, summary_request, summary_turn, transcript,
};

/// Default limit of model calls in one turn.
pub const DEFAULT_MAX_ITERATIONS: u32 = 50;

/// Longest output summary sent in `ToolStatusChanged`, in characters.
const OUTPUT_SUMMARY_CHARS: usize = 120;

/// The ports the agent loop talks to.
#[derive(Clone)]
pub struct AgentDeps {
    /// The model.
    pub llm: Arc<dyn LlmPort>,
    /// Where domain events go (the UI).
    pub events: Arc<dyn EventSink>,
    /// Who approves calls with side effects.
    pub approval: Arc<dyn ApprovalPort>,
    /// Traces of turns, model calls and tool calls.
    pub telemetry: Arc<dyn TelemetryPort>,
}

/// Options of one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOptions {
    /// Role the turn uses (usually `main`).
    pub role: ModelRole,
    /// `/model …` override for this turn: alias, role name or model id.
    pub turn_override: Option<String>,
    /// Role pinned by a tool or plugin (e.g. a subagent asking `explore`).
    pub pinned_role: Option<ModelRole>,
    /// Interrupts the turn when cancelled (`Ctrl+C`).
    pub cancel: CancelToken,
}

impl Default for TurnOptions {
    fn default() -> Self {
        Self::new(ModelRole::Main)
    }
}

impl TurnOptions {
    /// Options for `role` without overrides.
    #[must_use]
    pub fn new(role: ModelRole) -> Self {
        Self {
            role,
            turn_override: None,
            pinned_role: None,
            cancel: CancelToken::new(),
        }
    }

    /// Returns the same options with a `/model` override.
    #[must_use]
    pub fn with_override(mut self, name: impl Into<String>) -> Self {
        self.turn_override = Some(name.into());
        self
    }

    /// Returns the same options interrupted by `cancel`.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnOutcome {
    /// The turn, now the last one of the session.
    pub turn_id: TurnId,
    /// Why the last model call stopped.
    pub stop_reason: StopReason,
    /// Tokens used by every model call of the turn.
    pub usage: Usage,
    /// Number of model calls made.
    pub model_calls: u32,
}

/// Why a turn was abandoned. The turn is still appended to the session,
/// ending with a user message so the history can be sent again.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TurnError {
    /// The model call failed (after the adapter's retries and fallbacks),
    /// or its stream broke the protocol.
    #[error(transparent)]
    Llm(#[from] LlmError),
    /// The model kept calling tools past the configured limit.
    #[error("the model made {limit} calls without ending its turn; stopping")]
    TooManyIterations {
        /// Configured limit of model calls.
        limit: u32,
    },
}

/// Runs turns of the agent: the use case behind the input box.
pub struct AgentService {
    deps: AgentDeps,
    tools: ToolRegistry,
    models: ModelPolicy,
    gate: ApprovalGate,
    plugins: Option<Arc<dyn PluginHostPort>>,
    compaction: Option<CompactionPolicy>,
    cost: Option<Arc<CostTracker>>,
    system_prompt: String,
    max_iterations: u32,
    clock: fn() -> u64,
}

impl std::fmt::Debug for AgentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentService")
            .field("tools", &self.tools)
            .field("max_iterations", &self.max_iterations)
            .finish_non_exhaustive()
    }
}

/// Wall clock in Unix epoch milliseconds.
fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl AgentService {
    /// A service with no system prompt and the default iteration limit.
    #[must_use]
    pub fn new(
        deps: AgentDeps,
        tools: ToolRegistry,
        models: ModelPolicy,
        permissions: SharedPermissions,
    ) -> Self {
        Self {
            tools,
            models,
            gate: ApprovalGate::new(Arc::clone(&deps.approval), permissions),
            plugins: None,
            compaction: None,
            cost: None,
            deps,
            system_prompt: String::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            clock: system_now_ms,
        }
    }

    /// Sets the system prompt, sent with a cache breakpoint.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Runs the `before_tool` and `after_tool` hooks of `host` around
    /// every tool call.
    #[must_use]
    pub fn with_plugin_host(mut self, host: Arc<dyn PluginHostPort>) -> Self {
        self.plugins = Some(host);
        self
    }

    /// Compacts the history with the `compact` role when it passes the
    /// threshold of `policy`, or when the model reports it too long.
    #[must_use]
    pub fn with_compaction(mut self, policy: CompactionPolicy) -> Self {
        self.compaction = Some(policy);
        self
    }

    /// Prices every answer with `tracker` and reports the session's cost
    /// in `UsageUpdated`.
    #[must_use]
    pub fn with_cost_tracker(mut self, tracker: impl Into<Arc<CostTracker>>) -> Self {
        self.cost = Some(tracker.into());
        self
    }

    /// Sets the limit of model calls per turn.
    #[must_use]
    pub fn with_max_iterations(mut self, limit: u32) -> Self {
        self.max_iterations = limit.max(1);
        self
    }

    /// Replaces the wall clock (Unix epoch milliseconds), for tests.
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> u64) -> Self {
        self.clock = clock;
        self
    }

    /// The permission policy, shared with whoever cycles the mode.
    #[must_use]
    pub fn permissions(&self) -> &SharedPermissions {
        self.gate.permissions()
    }

    /// The tools offered to the model.
    #[must_use]
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    fn emit(&self, event: DomainEvent) {
        self.deps.events.emit(event);
    }

    /// Runs one turn answering `input` and appends it to `session`.
    ///
    /// # Errors
    ///
    /// [`TurnError`] when the model fails or the iteration limit is hit; a
    /// `TurnFailed` event is emitted and the partial turn is still
    /// appended. Tool failures are not errors: they go back to the model
    /// as `tool_result`s with `is_error`.
    pub async fn run_turn(
        &self,
        session: &mut Session,
        input: impl Into<String>,
        opts: TurnOptions,
    ) -> Result<TurnOutcome, TurnError> {
        let model = self.models.resolve(&ResolveRequest {
            role: opts.role,
            turn_override: opts.turn_override,
            pinned_role: opts.pinned_role,
        });
        for warning in &model.warnings {
            self.emit(DomainEvent::Notice {
                level: NoticeLevel::Warning,
                message: warning.to_string(),
            });
        }
        let mut turn = Turn::new(Message::user_text(input));
        let turn_id = turn.id;
        self.emit(DomainEvent::TurnStarted {
            session_id: session.id,
            turn_id,
            model: model.spec.id.clone(),
            role: model.role,
        });
        let span = self.deps.telemetry.turn_started(TurnSpan {
            session_id: session.id,
            turn_id,
            role: model.role,
            model: model.spec.id.clone(),
        });
        let mut run = TurnRun {
            service: self,
            session,
            turn: &mut turn,
            model: &model,
            span,
            model_calls: 0,
            context: None,
            cancel: opts.cancel,
        };
        let result = run.run().await;
        let model_calls = run.model_calls;
        let usage = turn.usage;
        let result = match result {
            Ok(stop_reason) => {
                turn.stop_reason = Some(stop_reason);
                self.emit(DomainEvent::TurnFinished {
                    turn_id,
                    stop_reason,
                });
                self.deps
                    .telemetry
                    .turn_finished(span, usage, Some(stop_reason));
                Ok(TurnOutcome {
                    turn_id,
                    stop_reason,
                    usage,
                    model_calls,
                })
            }
            Err(err) => {
                self.emit(DomainEvent::TurnFailed {
                    turn_id,
                    error: err.to_string(),
                });
                self.deps.telemetry.turn_finished(span, usage, None);
                Err(err)
            }
        };
        session.push_turn(turn, (self.clock)());
        result
    }
}

/// State of one running turn.
struct TurnRun<'a> {
    service: &'a AgentService,
    session: &'a mut Session,
    turn: &'a mut Turn,
    model: &'a ResolvedModel,
    span: SpanId,
    model_calls: u32,
    /// Context reported by the last model call, for compaction.
    context: Option<ContextMark>,
    cancel: CancelToken,
}

/// Context size reported by a model call.
#[derive(Debug, Clone, Copy)]
struct ContextMark {
    /// Tokens read and written by the call.
    tokens: u64,
    /// Messages of the turn the call already accounts for.
    messages: usize,
}

/// What an attempt to compact the history did.
enum Compaction {
    /// The oldest turns were replaced by a summary.
    Done,
    /// Nothing to compact, or the summary failed.
    Skipped,
    /// The turn was cancelled meanwhile.
    Interrupted,
}

/// One answer of the model, complete or cut by a cancellation.
struct Answer {
    assembled: AssembledMessage,
    blocks: StreamBlocks,
    interrupted: bool,
}

/// Timeline blocks opened while streaming one answer.
#[derive(Default)]
struct StreamBlocks {
    /// Block of each content index of the answer.
    by_index: HashMap<usize, BlockId>,
    /// `ToolCall` block of each tool use, by provider id.
    tool_calls: HashMap<ToolUseId, BlockId>,
}

impl TurnRun<'_> {
    fn emit(&self, event: DomainEvent) {
        self.service.emit(event);
    }

    fn session_id(&self) -> SessionId {
        self.session.id
    }

    /// The loop: model, tools, model… until the model stops calling tools.
    async fn run(&mut self) -> Result<StopReason, TurnError> {
        loop {
            if self.cancel.is_cancelled() {
                return Ok(StopReason::Interrupted);
            }
            if self.model_calls >= self.service.max_iterations {
                return Err(TurnError::TooManyIterations {
                    limit: self.service.max_iterations,
                });
            }
            if !self.spending_allowed().await {
                return Ok(StopReason::Interrupted);
            }
            if self.compact_if_needed().await {
                return Ok(StopReason::Interrupted);
            }
            let answer = match self.call_model().await {
                Err(TurnError::Llm(LlmError::ContextTooLong)) => {
                    match self.compact_after_overflow().await? {
                        Some(answer) => answer,
                        None => return Ok(StopReason::Interrupted),
                    }
                }
                other => other?,
            };
            let Answer {
                assembled,
                blocks,
                interrupted,
            } = answer;
            if interrupted {
                for block_id in blocks.tool_calls.values() {
                    self.emit_cancelled(*block_id);
                }
                if !assembled.message.content.is_empty() {
                    self.turn.messages.push(assembled.message);
                }
                return Ok(StopReason::Interrupted);
            }
            let stop_reason = assembled.stop_reason;
            let tool_uses: Vec<_> = assembled
                .message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();
            self.turn.messages.push(assembled.message);
            if tool_uses.is_empty() {
                return Ok(stop_reason);
            }
            let mut results = Vec::with_capacity(tool_uses.len());
            let mut tool_uses = tool_uses.into_iter();
            while let Some((id, name, input)) = tool_uses.next() {
                let block_id = blocks.tool_calls.get(&id).copied();
                let run = self.execute_tool(id.clone(), name, input, block_id);
                if let Ok(result) = self.cancel.run(run).await {
                    results.push(result);
                    continue;
                }
                // Every tool use must get a result, or the history could
                // not be sent to the model again.
                let unanswered = std::iter::once(id).chain(tool_uses.map(|(id, ..)| id));
                for id in unanswered {
                    if let Some(block_id) = blocks.tool_calls.get(&id) {
                        self.emit_cancelled(*block_id);
                    }
                    results.push(ContentPart::ToolResult {
                        tool_use_id: id,
                        content: ToolOutput::from(ToolError::Cancelled).content,
                        is_error: true,
                    });
                }
                self.turn.messages.push(Message::new(Role::User, results));
                return Ok(StopReason::Interrupted);
            }
            self.turn.messages.push(Message::new(Role::User, results));
        }
    }

    fn emit_cancelled(&self, block_id: BlockId) {
        self.emit(DomainEvent::ToolStatusChanged {
            block_id,
            status: ToolStatus::Cancelled,
            output: Some(ToolError::Cancelled.to_string()),
        });
    }

    fn request(&self) -> LlmRequest {
        let messages = self
            .session
            .history()
            .chain(&self.turn.messages)
            .cloned()
            .collect();
        let mut req = LlmRequest::new(self.model.clone(), messages);
        if !self.service.system_prompt.is_empty() {
            req.system = vec![SystemBlock::cached(self.service.system_prompt.clone())];
        }
        req.tools = self.service.tools.definitions();
        if let Some(last) = req.tools.pop() {
            req.tools.push(last.cached());
        }
        req.metadata = RequestMetadata {
            session_id: Some(self.session_id()),
            turn_id: Some(self.turn.id),
        };
        req
    }

    /// One model call: streams the answer into events and assembles it.
    async fn call_model(&mut self) -> Result<Answer, TurnError> {
        self.model_calls += 1;
        let started = Instant::now();
        let result = self.stream_answer().await;
        let (model, usage, error) = match &result {
            Ok(answer) => (
                answer
                    .assembled
                    .model
                    .clone()
                    .unwrap_or_else(|| self.model.spec.id.clone()),
                answer.assembled.usage,
                answer.interrupted.then(|| Interrupted.to_string()),
            ),
            Err(err) => (
                self.model.spec.id.clone(),
                Usage::default(),
                Some(err.to_string()),
            ),
        };
        self.service.deps.telemetry.llm_call(LlmCallRecord {
            parent: self.span,
            model: model.clone(),
            usage,
            latency_ms: elapsed_ms(started),
            error,
        });
        let answer = result?;
        let cost_usd = self.record_cost(&model, &usage);
        let usage = answer.assembled.usage;
        let tokens = context_tokens(&usage).saturating_add(usage.output_tokens);
        if tokens > 0 {
            self.context = Some(ContextMark {
                tokens,
                messages: self.turn.messages.len() + 1,
            });
        }
        self.turn.usage += answer.assembled.usage;
        self.emit(DomainEvent::UsageUpdated {
            turn: self.turn.usage,
            session: self.session.total_usage() + self.turn.usage,
            cost_usd,
        });
        Ok(answer)
    }

    /// Whether the next model call may spend: when the session reached
    /// its cap, asks the user to go on.
    async fn spending_allowed(&self) -> bool {
        let Some(tracker) = &self.service.cost else {
            return true;
        };
        let session = self.session_id();
        let Some(reached) = tracker.cap_reached(session) else {
            return true;
        };
        let amounts = format!("${:.2} de ${:.2}", reached.spent, reached.cap);
        self.emit(DomainEvent::Notice {
            level: NoticeLevel::Warning,
            message: format!("tope de gasto alcanzado ({amounts}); esperando confirmación"),
        });
        let asked = self.service.deps.approval.request(tracker.request(reached));
        let Ok(answer) = self.cancel.run(asked).await else {
            return false;
        };
        match answer {
            Ok(ApprovalResponse::Approve) => tracker.raise_cap(session),
            Ok(ApprovalResponse::ApproveAlways { .. }) => tracker.lift_cap(session),
            Ok(ApprovalResponse::Deny { .. }) | Err(_) => {
                self.emit(DomainEvent::Notice {
                    level: NoticeLevel::Warning,
                    message: format!("turno detenido por el tope de gasto ({amounts})"),
                });
                return false;
            }
        }
        true
    }

    /// Adds the cost of one answer; the session's cost when it is known.
    fn record_cost(&self, model: &ModelId, usage: &Usage) -> Option<f64> {
        self.service
            .cost
            .as_ref()?
            .record(self.session_id(), model, usage)
    }

    /// Estimated tokens of the next request.
    fn context_estimate(&self) -> u64 {
        if let Some(mark) = self.context {
            let added = self.turn.messages.get(mark.messages..).unwrap_or_default();
            return mark.tokens.saturating_add(estimate_tokens(added));
        }
        let history = estimate_tokens(self.session.history().chain(&self.turn.messages));
        let tools = serde_json::to_string(&self.service.tools.definitions()).map_or(0, |s| s.len());
        let fixed = self.service.system_prompt.len() + tools;
        history.saturating_add(u64::try_from(fixed.div_ceil(4)).unwrap_or(u64::MAX))
    }

    /// Compacts before a model call when the history passes the threshold.
    /// Returns `true` when the turn was cancelled meanwhile.
    async fn compact_if_needed(&mut self) -> bool {
        let Some(policy) = self.service.compaction else {
            return false;
        };
        if !policy.exceeded(self.context_estimate()) {
            return false;
        }
        matches!(self.compact(policy).await, Compaction::Interrupted)
    }

    /// The model said the prompt is too long: compacts and retries once.
    /// `None` when the turn was cancelled meanwhile.
    async fn compact_after_overflow(&mut self) -> Result<Option<Answer>, TurnError> {
        let Some(policy) = self.service.compaction else {
            return Err(LlmError::ContextTooLong.into());
        };
        match self.compact(policy).await {
            Compaction::Done => self.call_model().await.map(Some),
            Compaction::Skipped => Err(LlmError::ContextTooLong.into()),
            Compaction::Interrupted => Ok(None),
        }
    }

    /// Replaces the oldest finished turns of the session by a summary made
    /// with the `compact` role.
    async fn compact(&mut self, policy: CompactionPolicy) -> Compaction {
        let Some(count) = compactable_turns(self.session, policy.keep_recent_turns) else {
            return Compaction::Skipped;
        };
        let model = self.service.models.resolve(&ResolveRequest {
            role: ModelRole::Compact,
            turn_override: None,
            pinned_role: Some(ModelRole::Compact),
        });
        let req = summary_request(model.clone(), &transcript(&self.session.turns[..count]));
        let started = Instant::now();
        let call = summarize_history(self.service.deps.llm.as_ref(), req);
        let Ok(result) = self.cancel.run(call).await else {
            return Compaction::Interrupted;
        };
        let answered_by = result
            .as_ref()
            .ok()
            .and_then(|summary| summary.model.clone())
            .unwrap_or_else(|| model.spec.id.clone());
        let usage = result.as_ref().map(|s| s.usage).unwrap_or_default();
        self.service.deps.telemetry.llm_call(LlmCallRecord {
            parent: self.span,
            model: answered_by.clone(),
            usage,
            latency_ms: elapsed_ms(started),
            error: result.as_ref().err().map(ToString::to_string),
        });
        let cost_usd = self.record_cost(&answered_by, &usage);
        let summary = match result {
            Ok(summary) => summary,
            Err(err) => {
                self.emit(DomainEvent::Notice {
                    level: NoticeLevel::Warning,
                    message: format!("no se pudo compactar el historial: {err}"),
                });
                return Compaction::Skipped;
            }
        };
        let turn = summary_turn(&summary.text, &self.session.turns[..count], summary.usage);
        replace_turns(self.session, count, turn);
        self.context = None;
        self.emit(DomainEvent::Notice {
            level: NoticeLevel::Info,
            message: format!("historial compactado: {count} turnos → resumen"),
        });
        self.emit(DomainEvent::UsageUpdated {
            turn: self.turn.usage,
            session: self.session.total_usage() + self.turn.usage,
            cost_usd,
        });
        Compaction::Done
    }

    /// Streams one answer. On cancellation the stream is dropped (closing
    /// the connection) and what arrived is kept, without tool uses.
    async fn stream_answer(&self) -> Result<Answer, LlmError> {
        let mut assembler = MessageAssembler::new();
        let mut blocks = StreamBlocks::default();
        let interrupted = |assembler: MessageAssembler, blocks| Answer {
            assembled: assembler.interrupt(),
            blocks,
            interrupted: true,
        };
        let opened = self.service.deps.llm.stream(self.request());
        let Ok(stream) = self.cancel.run(opened).await else {
            return Ok(interrupted(assembler, blocks));
        };
        let mut stream = stream?;
        loop {
            let Ok(next) = self.cancel.run(stream.next()).await else {
                drop(stream);
                return Ok(interrupted(assembler, blocks));
            };
            let Some(event) = next else { break };
            let event = event?;
            self.forward(&mut blocks, &event);
            assembler.push(event)?;
        }
        Ok(Answer {
            assembled: assembler.finish()?,
            blocks,
            interrupted: false,
        })
    }

    /// Emits the domain events of one stream event.
    fn forward(&self, blocks: &mut StreamBlocks, event: &LlmEvent) {
        match event {
            LlmEvent::TextDelta { index, text } => {
                let block_id = self.open_block(blocks, *index, || BlockKind::Text {
                    markdown: String::new(),
                });
                self.emit(DomainEvent::TextDelta {
                    block_id,
                    text: text.clone(),
                });
            }
            LlmEvent::ThinkingDelta { index, thinking } => {
                let block_id = self.open_block(blocks, *index, || BlockKind::Thinking {
                    text: String::new(),
                    collapsed: true,
                });
                self.emit(DomainEvent::ThinkingDelta {
                    block_id,
                    text: thinking.clone(),
                });
            }
            LlmEvent::ToolUseStart { index, id, name } => {
                let block_id = self.open_block(blocks, *index, || {
                    tool_call_kind(id, name, Value::Null, String::new())
                });
                blocks.tool_calls.insert(id.clone(), block_id);
            }
            LlmEvent::Retrying {
                attempt, delay_ms, ..
            } => self.emit(DomainEvent::Retrying {
                attempt: *attempt,
                delay_ms: *delay_ms,
            }),
            LlmEvent::ModelFallback { from, to, .. } => self.emit(DomainEvent::ModelFallback {
                from: from.clone(),
                to: to.clone(),
            }),
            LlmEvent::MessageStart { .. }
            | LlmEvent::SignatureDelta { .. }
            | LlmEvent::RedactedThinking { .. }
            | LlmEvent::ToolInputDelta { .. }
            | LlmEvent::ServerToolUse { .. }
            | LlmEvent::ProviderBlock { .. }
            | LlmEvent::Citation { .. }
            | LlmEvent::BlockStop { .. }
            | LlmEvent::Usage(_)
            | LlmEvent::MessageStop { .. } => {}
        }
    }

    /// The block of content index `index`, adding it on first use.
    fn open_block(
        &self,
        blocks: &mut StreamBlocks,
        index: usize,
        kind: impl FnOnce() -> BlockKind,
    ) -> BlockId {
        *blocks.by_index.entry(index).or_insert_with(|| {
            let block = Block::new(kind());
            let id = block.id;
            self.emit(DomainEvent::BlockAdded { block });
            id
        })
    }

    /// Runs one tool call and returns its `tool_result`.
    async fn execute_tool(
        &self,
        tool_use_id: ToolUseId,
        name: String,
        input: Value,
        block_id: Option<BlockId>,
    ) -> ContentPart {
        let block_id = block_id.unwrap_or_else(|| {
            let block = Block::new(tool_call_kind(
                &tool_use_id,
                &name,
                input.clone(),
                String::new(),
            ));
            let id = block.id;
            self.emit(DomainEvent::BlockAdded { block });
            id
        });
        let ctx = ToolContext::new(self.session_id(), self.turn.id, tool_use_id.clone());
        let (output, status) = self.prepare_and_run(&ctx, &name, input, block_id).await;
        self.emit(DomainEvent::ToolStatusChanged {
            block_id,
            status,
            output: Some(summarize(&output.text_content())),
        });
        ContentPart::ToolResult {
            tool_use_id,
            content: output.content,
            is_error: output.is_error,
        }
    }

    /// Prepares the call, gets it approved and runs it; returns its output
    /// and final status.
    async fn prepare_and_run(
        &self,
        ctx: &ToolContext,
        name: &str,
        input: Value,
        block_id: BlockId,
    ) -> (ToolOutput, ToolStatus) {
        let failed = |err: ToolError| (ToolOutput::from(err), ToolStatus::Failed);
        let Some(tool) = self.service.tools.get(name) else {
            return failed(ToolError::UnknownTool(name.to_owned()));
        };
        let input = match self.before_tool(name, input.clone()).await {
            Ok(input) => input,
            Err(denial) => {
                self.emit(DomainEvent::BlockUpdated {
                    block: Block {
                        id: block_id,
                        kind: tool_call_kind(&ctx.tool_use_id, name, input, String::new()),
                    },
                });
                return (ToolOutput::error(denial), ToolStatus::Denied);
            }
        };
        let effect = tool.effect(&input);
        let mut prepared = match tool.prepare(input.clone(), ctx).await {
            Ok(prepared) => prepared,
            Err(err) => return failed(err),
        };
        let summary = prepared.subject.clone();
        self.emit(DomainEvent::BlockUpdated {
            block: Block {
                id: block_id,
                kind: tool_call_kind(&ctx.tool_use_id, name, input.clone(), summary),
            },
        });
        let action = ActionRequest::new(name, effect, prepared.subject.clone());
        match self.service.gate.decide(&action) {
            Decision::Deny { reason } => (
                ToolOutput::error(policy_denial(&reason)),
                ToolStatus::Denied,
            ),
            Decision::Allow => {
                let extra = prepared.block.take().map(|kind| self.add_block(kind));
                self.run_prepared(name, &input, prepared, block_id, extra)
                    .await
            }
            Decision::Ask => {
                let mut extra = prepared.block.take().map(|kind| self.add_block(kind));
                if let Some(block) = &mut extra {
                    self.set_diff_approval(block, ApprovalState::Pending);
                }
                let pending = PendingApproval {
                    action,
                    tool_use_id: ctx.tool_use_id.clone(),
                    preview: prepared.preview.take(),
                    tool_block: block_id,
                    shown_in: extra.as_ref().map_or(block_id, |block| block.id),
                };
                let verdict = self
                    .service
                    .gate
                    .ask(pending, self.service.deps.events.as_ref())
                    .await;
                match verdict {
                    UserVerdict::Approved => {
                        if let Some(block) = &mut extra {
                            self.set_diff_approval(block, ApprovalState::Approved);
                        }
                        self.run_prepared(name, &input, prepared, block_id, extra)
                            .await
                    }
                    UserVerdict::Rejected(message) => {
                        if let Some(block) = &mut extra {
                            self.set_diff_approval(block, ApprovalState::Rejected);
                        }
                        (ToolOutput::error(message), ToolStatus::Denied)
                    }
                }
            }
        }
    }

    /// Runs the `before_tool` hooks and returns the input to prepare the
    /// call with, or the denial to send to the model.
    async fn before_tool(&self, name: &str, input: Value) -> Result<Value, String> {
        let Some(host) = &self.service.plugins else {
            return Ok(input);
        };
        let event = HookEvent::BeforeTool {
            name: name.to_owned(),
            input: input.clone(),
        };
        match host.run_hook(event).await {
            Ok(HookOutcome::Continue) => Ok(input),
            Ok(HookOutcome::ReplaceInput(replaced)) => Ok(replaced),
            Ok(HookOutcome::Deny { reason }) => Err(hook_denial(reason.as_deref())),
            Err(err) => {
                self.hook_failed("before_tool", name, &err);
                Ok(input)
            }
        }
    }

    /// Runs the `after_tool` hooks of a call that ran. They only observe:
    /// what they return is ignored.
    async fn after_tool(&self, name: &str, input: &Value, output: &ToolOutput) {
        let Some(host) = &self.service.plugins else {
            return;
        };
        let event = HookEvent::AfterTool {
            name: name.to_owned(),
            input: input.clone(),
            output: json!({ "text": output.text_content(), "is_error": output.is_error }),
        };
        if let Err(err) = host.run_hook(event).await {
            self.hook_failed("after_tool", name, &err);
        }
    }

    /// Reports a hook that failed; the turn goes on.
    fn hook_failed(&self, hook: &str, tool: &str, err: &PluginError) {
        self.emit(DomainEvent::Notice {
            level: NoticeLevel::Warning,
            message: format!("{hook} hook failed for `{tool}`: {err}"),
        });
    }

    /// Adds a block to the timeline and returns it.
    fn add_block(&self, kind: BlockKind) -> Block {
        let block = Block::new(kind);
        self.emit(DomainEvent::BlockAdded {
            block: block.clone(),
        });
        block
    }

    /// Marks a `Diff` block with the user's decision; other kinds are left
    /// as they are.
    fn set_diff_approval(&self, block: &mut Block, state: ApprovalState) {
        if let BlockKind::Diff(spec) = &mut block.kind {
            spec.approval = Some(state);
            self.emit(DomainEvent::BlockUpdated {
                block: block.clone(),
            });
        }
    }

    /// Runs an approved call, then its `after_tool` hooks.
    async fn run_prepared(
        &self,
        name: &str,
        input: &Value,
        prepared: PreparedCall,
        block_id: BlockId,
        extra: Option<Block>,
    ) -> (ToolOutput, ToolStatus) {
        self.emit(DomainEvent::ToolStatusChanged {
            block_id,
            status: ToolStatus::Running,
            output: None,
        });
        let started = Instant::now();
        let output = prepared.run.await.unwrap_or_else(ToolOutput::from);
        let mut output = self.service.tools.budget().apply(output);
        self.service.deps.telemetry.tool_call(ToolCallRecord {
            parent: self.span,
            name: name.to_owned(),
            duration_ms: elapsed_ms(started),
            ok: !output.is_error,
        });
        if let Some(kind) = output.block_update.take() {
            let event = match extra {
                Some(block) => DomainEvent::BlockUpdated {
                    block: Block { id: block.id, kind },
                },
                None => DomainEvent::BlockAdded {
                    block: Block::new(kind),
                },
            };
            self.emit(event);
        }
        self.after_tool(name, input, &output).await;
        let status = if output.is_error {
            ToolStatus::Failed
        } else {
            ToolStatus::Succeeded
        };
        (output, status)
    }
}

/// A `ToolCall` block payload in `Pending` state.
fn tool_call_kind(id: &ToolUseId, name: &str, input: Value, summary: String) -> BlockKind {
    BlockKind::ToolCall {
        tool_use_id: id.as_str().to_owned(),
        name: name.to_owned(),
        summary,
        input,
        status: ToolStatus::Pending,
        output: None,
    }
}

/// Message for the model when a hook denies a call.
fn hook_denial(reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("denied by hook: {reason}"),
        None => "denied by hook".to_owned(),
    }
}

/// First line of `text`, cut to [`OUTPUT_SUMMARY_CHARS`].
fn summarize(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut summary: String = line.chars().take(OUTPUT_SUMMARY_CHARS).collect();
    if summary.len() < line.len() || text.lines().nth(1).is_some() {
        summary.push('…');
    }
    summary
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod approval_tests;
#[cfg(test)]
mod cancel_tests;
#[cfg(test)]
mod compact_tests;
#[cfg(test)]
mod cost_tests;
#[cfg(test)]
mod hook_tests;
#[cfg(test)]
mod tests;
