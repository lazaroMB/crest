//! Fakes of the model ports: [`LlmPort`](crate::ports::llm::LlmPort) driven
//! by a script of answers.
//!
//! A [`ScriptedResponse`] describes one model answer at a high level
//! (text, thinking, tool uses) and expands into the exact event sequence a
//! real adapter would stream: `MessageStart`, an initial `Usage`, each block
//! split into several deltas and closed with `BlockStop`, a final `Usage`
//! and `MessageStop`. Consumers therefore exercise
//! [`MessageAssembler`](crate::ports::llm::MessageAssembler) for real.

use crate::ports::mcp::{
    McpContent, McpError, McpPort, McpServerState, McpServerStatus, McpTool, McpToolResult,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use domain::conversation::{ContentPart, Role, StopReason, ToolUseId, Usage};
use domain::model::ModelId;
use futures::{StreamExt, stream};

use crate::ports::llm::{
    LlmError, LlmEvent, LlmPort, LlmRequest, LlmStream, ModelCatalogPort, ModelInfo,
};

/// Signature attached to scripted thinking blocks.
pub const FAKE_SIGNATURE: &str = "fake-signature";

/// One content block of a scripted answer.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptBlock {
    /// A text block.
    Text(String),
    /// An extended thinking block with its signature.
    Thinking {
        /// Thinking text.
        thinking: String,
        /// Signature sent after the thinking deltas.
        signature: String,
    },
    /// A client-side tool call.
    ToolUse {
        /// Provider id of the call.
        id: ToolUseId,
        /// Tool name.
        name: String,
        /// Tool input.
        input: serde_json::Value,
    },
}

/// One scripted model answer, built with the constructors below and
/// combined with the `then_*` methods.
///
/// ```ignore
/// # use application::testing::llm::ScriptedResponse;
/// let answer = ScriptedResponse::text("Busco la función.")
///     .then_tool_use("toolu_1", "grep", serde_json::json!({ "pattern": "fn main" }));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptedResponse {
    blocks: Vec<ScriptBlock>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    model: Option<ModelId>,
    raw: Option<Vec<LlmEvent>>,
    open_error: Option<LlmError>,
    cut: Option<Cut>,
}

/// How a scripted stream ends early.
#[derive(Debug, Clone, PartialEq)]
enum Cut {
    /// Yields `after` events, then `error`, then ends.
    Fail { after: usize, error: LlmError },
    /// Yields `after` events and then never yields again.
    Hang { after: usize },
}

impl ScriptedResponse {
    /// An answer with no blocks; add them with the `then_*` methods.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            blocks: Vec::new(),
            stop_reason: None,
            usage: None,
            model: None,
            raw: None,
            open_error: None,
            cut: None,
        }
    }

    /// An answer streamed exactly as `events`, with no generated framing;
    /// for retries, fallbacks or protocol violations.
    #[must_use]
    pub fn events(events: Vec<LlmEvent>) -> Self {
        Self {
            raw: Some(events),
            ..Self::empty()
        }
    }

    /// A call that fails before streaming: `stream` returns `error`.
    #[must_use]
    pub fn open_error(error: LlmError) -> Self {
        Self {
            open_error: Some(error),
            ..Self::empty()
        }
    }

    /// The stream yields its first `after` events, then `error`, then ends.
    #[must_use]
    pub fn fail_after(mut self, after: usize, error: LlmError) -> Self {
        self.cut = Some(Cut::Fail { after, error });
        self
    }

    /// The stream yields its first `after` events and then stays pending
    /// forever, to test cancellation by dropping it.
    #[must_use]
    pub fn hang_after(mut self, after: usize) -> Self {
        self.cut = Some(Cut::Hang { after });
        self
    }

    /// An answer made of one text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::empty().then_text(text)
    }

    /// An answer made of one tool use.
    #[must_use]
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::empty().then_tool_use(id, name, input)
    }

    /// An answer starting with a thinking block.
    #[must_use]
    pub fn thinking(thinking: impl Into<String>) -> Self {
        Self::empty().then_thinking(thinking)
    }

    /// Appends a text block.
    #[must_use]
    pub fn then_text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(ScriptBlock::Text(text.into()));
        self
    }

    /// Appends a tool use.
    #[must_use]
    pub fn then_tool_use(
        mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        self.blocks.push(ScriptBlock::ToolUse {
            id: ToolUseId::new(id),
            name: name.into(),
            input,
        });
        self
    }

    /// Appends a thinking block signed with [`FAKE_SIGNATURE`].
    #[must_use]
    pub fn then_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.blocks.push(ScriptBlock::Thinking {
            thinking: thinking.into(),
            signature: FAKE_SIGNATURE.to_owned(),
        });
        self
    }

    /// Overrides the stop reason (by default `ToolUse` if the answer has a
    /// tool use and `EndTurn` otherwise).
    #[must_use]
    pub fn with_stop_reason(mut self, stop_reason: StopReason) -> Self {
        self.stop_reason = Some(stop_reason);
        self
    }

    /// Overrides the final usage (by default estimated from the sizes of
    /// the request and of the answer, at about 4 bytes per token).
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Overrides the model reported in `MessageStart` (by default the
    /// primary model of the request).
    #[must_use]
    pub fn with_model(mut self, model: impl Into<ModelId>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// The content blocks of the answer.
    #[must_use]
    pub fn blocks(&self) -> &[ScriptBlock] {
        &self.blocks
    }

    /// The stop reason the answer ends with.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason.unwrap_or_else(|| {
            if self
                .blocks
                .iter()
                .any(|block| matches!(block, ScriptBlock::ToolUse { .. }))
            {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        })
    }

    /// The events a real adapter would stream for this answer to `req`.
    #[must_use]
    ///
    /// Answers built with [`events`](Self::events) return those events
    /// unchanged. Early cuts (`fail_after`, `hang_after`) are not applied.
    pub fn events_for(&self, req: &LlmRequest) -> Vec<LlmEvent> {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| req.model.spec.id.clone());
        let usage = self.usage.unwrap_or_else(|| Usage {
            input_tokens: estimate_input_tokens(req),
            output_tokens: self.estimate_output_tokens(),
            ..Usage::default()
        });
        let mut events = vec![
            LlmEvent::MessageStart { model },
            LlmEvent::Usage(Usage {
                output_tokens: 1,
                ..usage
            }),
        ];
        for (index, block) in self.blocks.iter().enumerate() {
            push_block_events(&mut events, index, block);
        }
        events.push(LlmEvent::Usage(usage));
        events.push(LlmEvent::MessageStop {
            stop_reason: self.stop_reason(),
        });
        events
    }

    fn estimate_output_tokens(&self) -> u64 {
        let bytes: usize = self
            .blocks
            .iter()
            .map(|block| match block {
                ScriptBlock::Text(text) => text.len(),
                ScriptBlock::Thinking { thinking, .. } => thinking.len(),
                ScriptBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            })
            .sum();
        tokens(bytes)
    }
}

/// Appends the events of one block: its start, its deltas and `BlockStop`.
fn push_block_events(events: &mut Vec<LlmEvent>, index: usize, block: &ScriptBlock) {
    match block {
        ScriptBlock::Text(text) => {
            events.extend(
                split(text, 3)
                    .into_iter()
                    .map(|text| LlmEvent::TextDelta { index, text }),
            );
        }
        ScriptBlock::Thinking {
            thinking,
            signature,
        } => {
            events.extend(
                split(thinking, 3)
                    .into_iter()
                    .map(|thinking| LlmEvent::ThinkingDelta { index, thinking }),
            );
            events.push(LlmEvent::SignatureDelta {
                index,
                signature: signature.clone(),
            });
        }
        ScriptBlock::ToolUse { id, name, input } => {
            events.push(LlmEvent::ToolUseStart {
                index,
                id: id.clone(),
                name: name.clone(),
            });
            events.extend(
                split(&input.to_string(), 3)
                    .into_iter()
                    .map(|partial_json| LlmEvent::ToolInputDelta {
                        index,
                        partial_json,
                    }),
            );
        }
    }
    events.push(LlmEvent::BlockStop { index });
}

/// Splits `text` into at most `parts` non-empty chunks on char boundaries;
/// an empty text gives one empty chunk, so the block still starts.
fn split(text: &str, parts: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    let size = chars.len().div_ceil(parts.max(1));
    chars.chunks(size).map(|c| c.iter().collect()).collect()
}

/// Rough token count of `bytes` bytes of text.
fn tokens(bytes: usize) -> u64 {
    u64::try_from(bytes.div_ceil(4)).unwrap_or(u64::MAX).max(1)
}

/// Rough input token count of a request: system prompt, history and tools.
fn estimate_input_tokens(req: &LlmRequest) -> u64 {
    let system: usize = req.system.iter().map(|block| block.text.len()).sum();
    let messages = serde_json::to_string(&req.messages).map_or(0, |s| s.len());
    let tools = serde_json::to_string(&req.tools).map_or(0, |s| s.len());
    tokens(system + messages + tools)
}

/// [`LlmPort`] that answers each call with the next [`ScriptedResponse`]
/// of a FIFO queue and records every request it receives.
///
/// Streams are plain in-memory streams: no timers, no runtime needed.
/// [`open_streams`](Self::open_streams) counts the streams not dropped
/// yet, to check that a cancelled turn really drops its stream.
/// When the queue is empty `stream` fails with
/// [`LlmError::InvalidRequest`] mentioning `script exhausted`.
#[derive(Debug, Default)]
pub struct ScriptedLlm {
    script: Mutex<VecDeque<ScriptedResponse>>,
    requests: Mutex<Vec<LlmRequest>>,
    open: Arc<AtomicUsize>,
    text_only: bool,
}

impl ScriptedLlm {
    /// A fake that plays `script` in order.
    #[must_use]
    pub fn new(script: impl IntoIterator<Item = ScriptedResponse>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            open: Arc::new(AtomicUsize::new(0)),
            text_only: false,
        }
    }

    /// Makes every model refuse images ([`LlmPort::accepts_images`]); by
    /// default the fake accepts them.
    #[must_use]
    pub fn text_only(mut self) -> Self {
        self.text_only = true;
        self
    }

    /// Appends one more answer to the end of the script.
    pub fn push(&self, response: ScriptedResponse) {
        lock(&self.script).push_back(response);
    }

    /// Every request received so far, oldest first.
    #[must_use]
    pub fn requests(&self) -> Vec<LlmRequest> {
        lock(&self.requests).clone()
    }

    /// The last request received, if any.
    #[must_use]
    pub fn last_request(&self) -> Option<LlmRequest> {
        lock(&self.requests).last().cloned()
    }

    /// Number of answers not consumed yet.
    #[must_use]
    pub fn remaining(&self) -> usize {
        lock(&self.script).len()
    }

    /// Streams returned and not dropped yet.
    #[must_use]
    pub fn open_streams(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmPort for ScriptedLlm {
    fn accepts_images(&self, _model: &ModelId) -> bool {
        !self.text_only
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        lock(&self.requests).push(req.clone());
        let response = lock(&self.script)
            .pop_front()
            .ok_or_else(|| LlmError::InvalidRequest {
                message: format!(
                    "script exhausted: ScriptedLlm has no answer for request #{}",
                    lock(&self.requests).len()
                ),
            })?;
        if let Some(error) = response.open_error {
            return Err(error);
        }
        let mut events: Vec<_> = response.events_for(&req).into_iter().map(Ok).collect();
        let hang = match response.cut {
            None => false,
            Some(Cut::Fail { after, error }) => {
                events.truncate(after);
                events.push(Err(error));
                false
            }
            Some(Cut::Hang { after }) => {
                events.truncate(after);
                true
            }
        };
        let events = stream::iter(events);
        let inner: LlmStream = if hang {
            Box::pin(events.chain(stream::pending()))
        } else {
            Box::pin(events)
        };
        let guard = OpenGuard::new(Arc::clone(&self.open));
        Ok(Box::pin(inner.map(move |item| {
            let _alive = &guard;
            item
        })))
    }
}

/// Counts a stream as open from creation until it is dropped.
#[derive(Debug)]
struct OpenGuard(Arc<AtomicUsize>);

impl OpenGuard {
    fn new(open: Arc<AtomicUsize>) -> Self {
        open.fetch_add(1, Ordering::SeqCst);
        Self(open)
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A script of [`RoutedLlm`] and the text that selects it.
#[derive(Debug)]
struct Route {
    needle: String,
    llm: ScriptedLlm,
}

/// [`LlmPort`] with one script per route, for flows whose calls may run
/// in any order (e.g. in parallel): each request goes to the first route
/// whose text appears in its system prompt or its first user message, and
/// takes the next answer of that route's queue. Requests no route matches
/// go to the fallback script, if any.
///
/// ```ignore
/// let llm = RoutedLlm::new()
///     .route("Observation stage", [ScriptedResponse::text("{…}")])
///     .route("Data stage", [ScriptedResponse::text("{…}")]);
/// ```
#[derive(Debug, Default)]
pub struct RoutedLlm {
    routes: Vec<Route>,
    fallback: Option<ScriptedLlm>,
}

impl RoutedLlm {
    /// No routes yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a route: requests whose system prompt or first user message
    /// contains `needle` are answered from `script`, in order. Routes are
    /// tried in the order they were added.
    #[must_use]
    pub fn route(
        mut self,
        needle: impl Into<String>,
        script: impl IntoIterator<Item = ScriptedResponse>,
    ) -> Self {
        self.routes.push(Route {
            needle: needle.into(),
            llm: ScriptedLlm::new(script),
        });
        self
    }

    /// Answers the requests no route matches from `script`.
    #[must_use]
    pub fn fallback(mut self, script: impl IntoIterator<Item = ScriptedResponse>) -> Self {
        self.fallback = Some(ScriptedLlm::new(script));
        self
    }

    /// Appends one more answer to the route of `needle` (or the fallback
    /// when no route has that needle).
    pub fn push(&self, needle: &str, response: ScriptedResponse) {
        match self.routes.iter().find(|r| r.needle == needle) {
            Some(route) => route.llm.push(response),
            None => {
                if let Some(fallback) = &self.fallback {
                    fallback.push(response);
                }
            }
        }
    }

    /// The requests the route of `needle` received (the fallback's when
    /// no route has that needle), oldest first.
    #[must_use]
    pub fn requests(&self, needle: &str) -> Vec<LlmRequest> {
        match self.routes.iter().find(|r| r.needle == needle) {
            Some(route) => route.llm.requests(),
            None => self
                .fallback
                .as_ref()
                .map(ScriptedLlm::requests)
                .unwrap_or_default(),
        }
    }

    /// Answers not consumed yet, over every route and the fallback.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.routes.iter().map(|r| r.llm.remaining()).sum::<usize>()
            + self.fallback.as_ref().map_or(0, ScriptedLlm::remaining)
    }

    /// The script that answers `req`.
    fn script_for(&self, req: &LlmRequest) -> Option<&ScriptedLlm> {
        let system: String = req.system.iter().map(|b| b.text.as_str()).collect();
        let first_user: String = req
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.routes
            .iter()
            .find(|r| system.contains(&r.needle) || first_user.contains(&r.needle))
            .map(|r| &r.llm)
            .or(self.fallback.as_ref())
    }
}

#[async_trait]
impl LlmPort for RoutedLlm {
    fn accepts_images(&self, _model: &ModelId) -> bool {
        true
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        match self.script_for(&req) {
            Some(llm) => llm.stream(req).await,
            None => {
                let start: String = req
                    .messages
                    .first()
                    .map(|m| format!("{:?}", m.content))
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect();
                Err(LlmError::InvalidRequest {
                    message: format!("RoutedLlm: no route matches the request {start}"),
                })
            }
        }
    }
}

/// How long [`BarrierLlm`] waits for the other calls by default.
pub const BARRIER_TIMEOUT: Duration = Duration::from_secs(2);

/// [`LlmPort`] that proves calls run at the same time: the first
/// `parties` calls each wait, before reaching the wrapped model, until all
/// `parties` have arrived. Calls run one after another never get there,
/// so the first one fails with [`LlmError::InvalidRequest`] after the
/// timeout. Later calls go straight through.
///
/// It needs no async runtime: the waits are oneshot channels and the
/// timeout a sleeping thread.
pub struct BarrierLlm {
    inner: Arc<dyn LlmPort>,
    parties: usize,
    arrived: AtomicUsize,
    /// Calls waiting at the barrier, released together.
    waiting: Mutex<Vec<futures::channel::oneshot::Sender<()>>>,
    timeout: Duration,
}

impl std::fmt::Debug for BarrierLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarrierLlm")
            .field("parties", &self.parties)
            .field("arrived", &self.arrived)
            .finish_non_exhaustive()
    }
}

impl BarrierLlm {
    /// Holds the first `parties` calls to `inner` until all have arrived.
    #[must_use]
    pub fn new(inner: Arc<dyn LlmPort>, parties: usize) -> Self {
        Self {
            inner,
            parties,
            arrived: AtomicUsize::new(0),
            waiting: Mutex::new(Vec::new()),
            timeout: BARRIER_TIMEOUT,
        }
    }

    /// Waits `timeout` for the other calls instead of [`BARRIER_TIMEOUT`].
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Calls that reached the barrier so far.
    #[must_use]
    pub fn arrived(&self) -> usize {
        self.arrived.load(Ordering::SeqCst)
    }

    /// Waits until the other parties arrive; `false` on timeout.
    async fn wait(&self) -> bool {
        let (release, released) = futures::channel::oneshot::channel();
        {
            let mut waiting = lock(&self.waiting);
            let n = self.arrived.fetch_add(1, Ordering::SeqCst) + 1;
            if n > self.parties {
                return true;
            }
            waiting.push(release);
            if n == self.parties {
                for waiter in waiting.drain(..) {
                    let _ = waiter.send(());
                }
            }
        }
        let (expire, expired) = futures::channel::oneshot::channel::<()>();
        let timeout = self.timeout;
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            let _ = expire.send(());
        });
        matches!(
            futures::future::select(released, expired).await,
            futures::future::Either::Left((Ok(()), _))
        )
    }
}

#[async_trait]
impl LlmPort for BarrierLlm {
    fn accepts_images(&self, model: &ModelId) -> bool {
        self.inner.accepts_images(model)
    }

    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        if self.arrived() < self.parties && !self.wait().await {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "BarrierLlm: only {} of {} calls arrived within {:?}; they do not run at \
                     the same time",
                    self.arrived(),
                    self.parties,
                    self.timeout
                ),
            });
        }
        self.inner.stream(req).await
    }
}

/// [`ModelCatalogPort`] with a fixed answer.
#[derive(Debug, Clone, PartialEq)]
pub struct FakeModelCatalog {
    answer: Result<Vec<ModelInfo>, LlmError>,
}

impl FakeModelCatalog {
    /// A catalog listing exactly `models`.
    #[must_use]
    pub fn new(models: Vec<ModelInfo>) -> Self {
        Self { answer: Ok(models) }
    }

    /// A catalog listing `ids`, each displayed under its own id.
    #[must_use]
    pub fn with_ids<I, T>(ids: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<ModelId>,
    {
        Self::new(
            ids.into_iter()
                .map(|id| {
                    let id = id.into();
                    ModelInfo {
                        display_name: id.to_string(),
                        id,
                        created_at: None,
                    }
                })
                .collect(),
        )
    }

    /// A catalog whose listing fails with `error`.
    #[must_use]
    pub fn failing(error: LlmError) -> Self {
        Self { answer: Err(error) }
    }
}

#[async_trait]
impl ModelCatalogPort for FakeModelCatalog {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        self.answer.clone()
    }
}

/// Handler of a [`FakeMcp`] tool: computes the result from the input.
pub type McpHandler =
    Arc<dyn Fn(serde_json::Value) -> Result<McpToolResult, McpError> + Send + Sync>;

/// A call received by [`FakeMcp`].
#[derive(Debug, Clone, PartialEq)]
pub struct McpCall {
    /// Server name.
    pub server: String,
    /// Unqualified tool name.
    pub name: String,
    /// Input sent.
    pub input: serde_json::Value,
}

/// A server of [`FakeMcp`] with its tools.
struct FakeServer {
    name: String,
    /// Explicit state; `None` means ready with all its tools.
    state: Option<McpServerState>,
    tools: Vec<(McpTool, McpHandler)>,
}

/// [`McpPort`] with tools registered per server, answered by closures.
///
/// Servers are reported in registration order. A server is `Ready` unless
/// another state was set with
/// [`with_server_state`](Self::with_server_state); tools of servers that
/// are not ready are not listed and calling them fails with
/// [`McpError::Unavailable`].
#[derive(Default)]
pub struct FakeMcp {
    servers: Vec<FakeServer>,
    calls: Mutex<Vec<McpCall>>,
}

impl std::fmt::Debug for FakeMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tools: Vec<String> = self
            .servers
            .iter()
            .flat_map(|server| server.tools.iter().map(|(tool, _)| tool.qualified_name()))
            .collect();
        f.debug_struct("FakeMcp").field("tools", &tools).finish()
    }
}

impl FakeMcp {
    /// A fake with no servers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `name` on `server` (created if new), answered by `handler`.
    /// The tool accepts any object as input.
    #[must_use]
    pub fn with_tool(
        mut self,
        server: &str,
        name: &str,
        description: &str,
        handler: impl Fn(serde_json::Value) -> Result<McpToolResult, McpError> + Send + Sync + 'static,
    ) -> Self {
        let tool = McpTool {
            server: server.to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema: serde_json::json!({ "type": "object" }),
            output_schema: None,
            ui: crate::ports::mcp::McpToolUi::default(),
        };
        self.server_mut(server)
            .tools
            .push((tool, Arc::new(handler)));
        self
    }

    /// Registers `spec` (its server created if new), answered by
    /// `handler`: for tools with an output schema or MCP Apps metadata.
    #[must_use]
    pub fn with_spec(
        mut self,
        spec: McpTool,
        handler: impl Fn(serde_json::Value) -> Result<McpToolResult, McpError> + Send + Sync + 'static,
    ) -> Self {
        let server = spec.server.clone();
        self.server_mut(&server)
            .tools
            .push((spec, Arc::new(handler)));
        self
    }

    /// Registers a tool that always answers with `text`.
    #[must_use]
    pub fn with_text_tool(self, server: &str, name: &str, text: &str) -> Self {
        let text = text.to_owned();
        self.with_tool(server, name, "Fake MCP tool", move |_| {
            Ok(text_result(text.clone()))
        })
    }

    /// Sets the state of `server` (created if new).
    #[must_use]
    pub fn with_server_state(mut self, server: &str, state: McpServerState) -> Self {
        self.server_mut(server).state = Some(state);
        self
    }

    /// Every call received so far, oldest first.
    #[must_use]
    pub fn calls(&self) -> Vec<McpCall> {
        lock(&self.calls).clone()
    }

    fn server_mut(&mut self, name: &str) -> &mut FakeServer {
        let index = match self.servers.iter().position(|s| s.name == name) {
            Some(index) => index,
            None => {
                self.servers.push(FakeServer {
                    name: name.to_owned(),
                    state: None,
                    tools: Vec::new(),
                });
                self.servers.len() - 1
            }
        };
        &mut self.servers[index]
    }
}

impl FakeServer {
    fn state(&self) -> McpServerState {
        self.state.clone().unwrap_or(McpServerState::Ready {
            tool_count: self.tools.len(),
        })
    }

    fn is_ready(&self) -> bool {
        matches!(self.state(), McpServerState::Ready { .. })
    }
}

/// A successful MCP result with one text item.
#[must_use]
pub fn text_result(text: impl Into<String>) -> McpToolResult {
    McpToolResult {
        content: vec![McpContent::text(text)],
        ..McpToolResult::default()
    }
}

#[async_trait]
impl McpPort for FakeMcp {
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        Ok(self
            .servers
            .iter()
            .filter(|server| server.is_ready())
            .flat_map(|server| server.tools.iter().map(|(tool, _)| tool.clone()))
            .collect())
    }

    async fn call_tool(
        &self,
        server: &str,
        name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        lock(&self.calls).push(McpCall {
            server: server.to_owned(),
            name: name.to_owned(),
            input: input.clone(),
        });
        let found = self
            .servers
            .iter()
            .find(|s| s.name == server)
            .ok_or_else(|| McpError::UnknownServer(server.to_owned()))?;
        if !found.is_ready() {
            return Err(McpError::Unavailable {
                server: server.to_owned(),
                message: format!("{:?}", found.state()),
            });
        }
        let (_, handler) = found
            .tools
            .iter()
            .find(|(tool, _)| tool.name == name)
            .ok_or_else(|| McpError::UnknownTool {
                server: server.to_owned(),
                name: name.to_owned(),
            })?;
        handler(input)
    }

    async fn servers(&self) -> Vec<McpServerStatus> {
        self.servers
            .iter()
            .map(|server| McpServerStatus {
                name: server.name.clone(),
                state: server.state(),
                transport: "fake".into(),
            })
            .collect()
    }
}

/// Locks `mutex`, ignoring poisoning: a panicking test must not hide the
/// recorded state from the others.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::llm::{AssembledMessage, MessageAssembler};
    use domain::conversation::{ContentPart, Message, Role};
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
    use futures::FutureExt;
    use serde_json::json;

    fn model() -> ResolvedModel {
        ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new("claude-sonnet-5-5", 16_000),
            source: SpecSource::Default,
            warnings: Vec::new(),
        }
    }

    fn request(text: &str) -> LlmRequest {
        LlmRequest::new(model(), vec![Message::user_text(text)])
    }

    fn assemble(events: Vec<LlmEvent>) -> AssembledMessage {
        let mut asm = MessageAssembler::new();
        for event in events {
            asm.push(event).unwrap();
        }
        asm.finish().unwrap()
    }

    #[test]
    fn split_keeps_chars_and_limits_parts() {
        assert_eq!(split("", 3), vec![String::new()]);
        assert_eq!(split("ab", 3), vec!["a", "b"]);
        assert_eq!(split("ñandú!", 3), vec!["ña", "nd", "ú!"]);
    }

    #[test]
    fn stop_reason_follows_the_blocks() {
        assert_eq!(
            ScriptedResponse::text("hola").stop_reason(),
            StopReason::EndTurn
        );
        let tool = ScriptedResponse::text("busco").then_tool_use("toolu_1", "grep", json!({}));
        assert_eq!(tool.stop_reason(), StopReason::ToolUse);
        let capped = ScriptedResponse::text("…").with_stop_reason(StopReason::MaxTokens);
        assert_eq!(capped.stop_reason(), StopReason::MaxTokens);
    }

    #[test]
    fn tool_use_input_is_streamed_in_several_deltas() {
        let input = json!({ "pattern": "fn main", "path": "src" });
        let events = ScriptedResponse::tool_use("toolu_1", "grep", input.clone())
            .events_for(&request("busca main"));
        let deltas = events
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolInputDelta { .. }))
            .count();
        assert_eq!(deltas, 3);
        assert!(matches!(
            events.first(),
            Some(LlmEvent::MessageStart { model }) if model.as_str() == "claude-sonnet-5-5"
        ));
        assert!(matches!(
            events.last(),
            Some(LlmEvent::MessageStop {
                stop_reason: StopReason::ToolUse
            })
        ));
        let out = assemble(events);
        assert_eq!(
            out.message.content,
            vec![ContentPart::ToolUse {
                id: ToolUseId::new("toolu_1"),
                name: "grep".into(),
                input,
            }]
        );
    }

    #[test]
    fn thinking_text_and_usage_assemble() {
        let out = assemble(
            ScriptedResponse::thinking("pienso un poco")
                .then_text("Hola, ¿en qué te ayudo?")
                .with_model("claude-haiku-5-5")
                .events_for(&request("hola")),
        );
        assert_eq!(out.message.role, Role::Assistant);
        assert_eq!(
            out.message.content,
            vec![
                ContentPart::Thinking {
                    thinking: "pienso un poco".into(),
                    signature: Some(FAKE_SIGNATURE.into()),
                },
                ContentPart::text("Hola, ¿en qué te ayudo?"),
            ]
        );
        assert_eq!(out.model, Some(ModelId::new("claude-haiku-5-5")));
        assert!(out.usage.input_tokens > 0 && out.usage.output_tokens > 1);
        assert_eq!(out.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn usage_override_is_the_final_usage() {
        let usage = Usage {
            input_tokens: 120,
            output_tokens: 7,
            cache_read_input_tokens: 100,
            cache_creation_input_tokens: 0,
        };
        let out = assemble(
            ScriptedResponse::text("ok")
                .with_usage(usage)
                .events_for(&request("x")),
        );
        assert_eq!(out.usage, usage);
    }

    /// Consumes one stream with `MessageAssembler`.
    async fn run(llm: &ScriptedLlm, req: LlmRequest) -> Result<AssembledMessage, LlmError> {
        let mut stream = llm.stream(req).await?;
        let mut asm = MessageAssembler::new();
        while let Some(event) = stream.next().await {
            asm.push(event?)?;
        }
        Ok(asm.finish()?)
    }

    #[tokio::test]
    async fn plays_a_script_of_three_answers() {
        let llm = ScriptedLlm::new([
            ScriptedResponse::text("Busco primero.").then_tool_use(
                "toolu_1",
                "grep",
                json!({ "pattern": "fn main", "path": "src" }),
            ),
            ScriptedResponse::tool_use("toolu_2", "read_file", json!({ "path": "src/main.rs" })),
            ScriptedResponse::text("`main` está en src/main.rs."),
        ]);

        let mut history = vec![Message::user_text("¿dónde está main?")];
        let mut stops = Vec::new();
        let mut answers = Vec::new();
        for tool_result in ["src/main.rs:1", "fn main() {}", ""] {
            let req = LlmRequest::new(model(), history.clone());
            let out = run(&llm, req).await.unwrap();
            stops.push(out.stop_reason);
            history.push(out.message.clone());
            answers.push(out.message.content);
            if out.stop_reason == StopReason::ToolUse {
                let id = match history.last().map(|m| m.content.last()) {
                    Some(Some(ContentPart::ToolUse { id, .. })) => id.clone(),
                    other => panic!("expected a tool use, got {other:?}"),
                };
                history.push(Message::new(
                    Role::User,
                    vec![ContentPart::tool_result_text(id, tool_result)],
                ));
            }
        }

        assert_eq!(
            stops,
            [
                StopReason::ToolUse,
                StopReason::ToolUse,
                StopReason::EndTurn
            ]
        );
        assert_eq!(
            answers,
            vec![
                vec![
                    ContentPart::text("Busco primero."),
                    ContentPart::ToolUse {
                        id: ToolUseId::new("toolu_1"),
                        name: "grep".into(),
                        input: json!({ "pattern": "fn main", "path": "src" }),
                    },
                ],
                vec![ContentPart::ToolUse {
                    id: ToolUseId::new("toolu_2"),
                    name: "read_file".into(),
                    input: json!({ "path": "src/main.rs" }),
                }],
                vec![ContentPart::text("`main` está en src/main.rs.")],
            ]
        );
        let sent: Vec<usize> = llm.requests().iter().map(|r| r.messages.len()).collect();
        assert_eq!(sent, [1, 3, 5]);
        assert_eq!(llm.remaining(), 0);
    }

    #[tokio::test]
    async fn exhausted_script_is_a_clear_error() {
        let llm = ScriptedLlm::new([]);
        let err = llm.stream(request("hola")).await.err().unwrap();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("script exhausted")),
            "{err}"
        );
        assert_eq!(llm.requests().len(), 1);
    }

    #[tokio::test]
    async fn error_in_the_middle_of_the_stream() {
        let llm = ScriptedLlm::new([ScriptedResponse::text("Hola, mundo")
            .fail_after(3, LlmError::Network("connection reset".into()))]);
        let items: Vec<_> = llm.stream(request("hola")).await.unwrap().collect().await;
        assert_eq!(items.len(), 4);
        assert!(items[..3].iter().all(Result::is_ok));
        assert!(matches!(items[2], Ok(LlmEvent::TextDelta { .. })));
        assert_eq!(items[3], Err(LlmError::Network("connection reset".into())));

        llm.push(ScriptedResponse::text("x").fail_after(0, LlmError::Overloaded));
        assert_eq!(
            run(&llm, request("otra")).await.err(),
            Some(LlmError::Overloaded)
        );
    }

    #[tokio::test]
    async fn error_opening_the_stream() {
        let llm = ScriptedLlm::new([
            ScriptedResponse::open_error(LlmError::Auth {
                message: "set ANTHROPIC_API_KEY".into(),
            }),
            ScriptedResponse::text("ok"),
        ]);
        assert!(matches!(
            llm.stream(request("hola")).await.err(),
            Some(LlmError::Auth { .. })
        ));
        assert_eq!(llm.open_streams(), 0);
        let out = run(&llm, request("hola")).await.unwrap();
        assert_eq!(out.message.content, vec![ContentPart::text("ok")]);
        assert_eq!(llm.requests().len(), 2);
    }

    #[tokio::test]
    async fn raw_events_are_streamed_unchanged() {
        let events = vec![
            LlmEvent::Retrying {
                attempt: 1,
                delay_ms: 500,
                reason: LlmError::Overloaded,
            },
            LlmEvent::MessageStart {
                model: ModelId::new("claude-haiku-5-5"),
            },
            LlmEvent::MessageStop {
                stop_reason: StopReason::Refusal,
            },
        ];
        let llm = ScriptedLlm::new([ScriptedResponse::events(events.clone())]);
        let got: Vec<_> = llm.stream(request("hola")).await.unwrap().collect().await;
        assert_eq!(got, events.into_iter().map(Ok).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn hanging_stream_is_released_on_drop() {
        let llm = ScriptedLlm::new([ScriptedResponse::text("Hola, mundo").hang_after(2)]);
        let mut stream = llm.stream(request("hola")).await.unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(LlmEvent::MessageStart { .. }))
        ));
        assert!(matches!(stream.next().await, Some(Ok(LlmEvent::Usage(_)))));
        assert!(stream.next().now_or_never().is_none(), "stream must hang");
        assert_eq!(llm.open_streams(), 1);
        drop(stream);
        assert_eq!(llm.open_streams(), 0);
    }

    #[tokio::test]
    async fn model_catalog_returns_its_fixed_answer() {
        let catalog: Arc<dyn ModelCatalogPort> = Arc::new(FakeModelCatalog::with_ids([
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
        ]));
        let ids: Vec<String> = catalog
            .list_models()
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id.0)
            .collect();
        assert_eq!(ids, ["claude-sonnet-5-5", "claude-haiku-5-5"]);

        let failing = FakeModelCatalog::failing(LlmError::Network("offline".into()));
        assert_eq!(
            failing.list_models().await,
            Err(LlmError::Network("offline".into()))
        );
    }

    fn fake_mcp() -> FakeMcp {
        FakeMcp::new()
            .with_tool("github", "create_issue", "Opens an issue", |input| {
                Ok(text_result(format!("created {}", input["title"])))
            })
            .with_text_tool("github", "whoami", "lazaro")
            .with_text_tool("jira", "search", "nothing")
            .with_server_state(
                "jira",
                McpServerState::Failed {
                    message: "exit 1".into(),
                },
            )
    }

    #[tokio::test]
    async fn fake_mcp_lists_ready_tools_and_routes_calls() {
        let mcp = Arc::new(fake_mcp());
        let port: Arc<dyn McpPort> = mcp.clone();
        let names: Vec<String> = port
            .list_tools()
            .await
            .unwrap()
            .iter()
            .map(McpTool::qualified_name)
            .collect();
        assert_eq!(names, ["mcp__github__create_issue", "mcp__github__whoami"]);

        let result = port
            .call_tool("github", "create_issue", json!({ "title": "bug" }))
            .await
            .unwrap();
        assert_eq!(result, text_result("created \"bug\""));
        assert_eq!(
            mcp.calls(),
            [McpCall {
                server: "github".into(),
                name: "create_issue".into(),
                input: json!({ "title": "bug" }),
            }]
        );

        let states: Vec<McpServerState> =
            port.servers().await.into_iter().map(|s| s.state).collect();
        assert_eq!(
            states,
            [
                McpServerState::Ready { tool_count: 2 },
                McpServerState::Failed {
                    message: "exit 1".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn fake_mcp_errors_are_typed() {
        let mcp = fake_mcp();
        let call = |server: &'static str, name: &'static str| {
            mcp.call_tool(server, name, serde_json::Value::Null)
        };
        assert_eq!(
            call("gitlab", "x").await,
            Err(McpError::UnknownServer("gitlab".into()))
        );
        assert_eq!(
            call("github", "delete_repo").await,
            Err(McpError::UnknownTool {
                server: "github".into(),
                name: "delete_repo".into(),
            })
        );
        assert!(matches!(
            call("jira", "search").await,
            Err(McpError::Unavailable { ref server, .. }) if server == "jira"
        ));
        assert_eq!(mcp.calls().len(), 3);
    }

    fn system_request(system: &str, text: &str) -> LlmRequest {
        let mut req = request(text);
        req.system = vec![crate::ports::llm::SystemBlock::new(system)];
        req
    }

    async fn answer(llm: &dyn LlmPort, req: LlmRequest) -> Result<String, LlmError> {
        let events: Vec<_> = llm.stream(req).await?.collect().await;
        let events = events.into_iter().collect::<Result<Vec<_>, _>>()?;
        let message = assemble(events).message;
        Ok(message
            .content
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect())
    }

    #[tokio::test]
    async fn routed_answers_come_from_the_matching_script() {
        let llm = RoutedLlm::new()
            .route("Observation", [ScriptedResponse::text("map")])
            .route("Data", [ScriptedResponse::text("history")])
            .fallback([ScriptedResponse::text("other")]);
        // Out of order: each route keeps its own queue.
        assert_eq!(
            answer(&llm, request("Data stage: gather")).await.unwrap(),
            "history"
        );
        assert_eq!(
            answer(&llm, system_request("Observation stage", "walk"))
                .await
                .unwrap(),
            "map"
        );
        assert_eq!(answer(&llm, request("anything")).await.unwrap(), "other");
        assert_eq!(llm.requests("Data").len(), 1);
        assert_eq!(llm.remaining(), 0);
        assert!(answer(&llm, request("Data again")).await.is_err());

        let strict = RoutedLlm::new().route("x", []);
        let err = answer(&strict, request("nothing")).await.unwrap_err();
        assert!(err.to_string().contains("no route"), "{err}");
    }

    #[tokio::test]
    async fn barrier_releases_concurrent_calls_and_times_out_serial_ones() {
        let inner = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("a"),
            ScriptedResponse::text("b"),
            ScriptedResponse::text("c"),
            ScriptedResponse::text("d"),
        ]));
        let llm = BarrierLlm::new(inner, 3);
        let (a, b, c) = futures::join!(
            answer(&llm, request("1")),
            answer(&llm, request("2")),
            answer(&llm, request("3")),
        );
        let mut got = vec![a.unwrap(), b.unwrap(), c.unwrap()];
        got.sort();
        assert_eq!(got, ["a", "b", "c"]);
        // Past the barrier, calls go straight through.
        assert_eq!(answer(&llm, request("4")).await.unwrap(), "d");

        let serial = BarrierLlm::new(Arc::new(ScriptedLlm::new([ScriptedResponse::text("a")])), 2)
            .with_timeout(Duration::from_millis(20));
        let err = answer(&serial, request("1")).await.unwrap_err();
        assert!(err.to_string().contains("1 of 2"), "{err}");
    }
}
