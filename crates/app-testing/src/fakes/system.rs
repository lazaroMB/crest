//! Fakes of the system ports: events, approval, session store, Lua host and
//! telemetry.
//!
//! Every fake is `Send + Sync`, usable as `Arc<dyn Port>`, and records what
//! it received so tests can assert on it afterwards.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::conversation::{SessionId, StopReason, Usage};
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use serde_json::Value;

use crate::ports::approval::{ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink};
use crate::ports::plugin::{
    CallbackId, ConfigSources, HookEvent, HookKind, HookOutcome, LoadedConfig, PluginError,
    PluginHostPort, PluginOutput,
};
use crate::ports::projects::RecentWorkspace;
use crate::ports::store::{SessionStorePort, SessionSummary, StoreError, StoredSession};
use crate::ports::telemetry::{LlmCallRecord, SpanId, TelemetryPort, ToolCallRecord, TurnSpan};

/// Locks `mutex`, ignoring poisoning: a panicking test must not hide the
/// recorded data from the next assertion.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// EventSink
// ---------------------------------------------------------------------------

/// [`EventSink`] that records every event, optionally forwarding it to a
/// channel so a test UI can consume them live.
#[derive(Debug, Default)]
pub struct RecordingSink {
    events: Mutex<Vec<DomainEvent>>,
    tx: Option<UnboundedSender<DomainEvent>>,
}

impl RecordingSink {
    /// Creates a sink that only records.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a sink that records and also forwards each event to the
    /// returned receiver. A dropped receiver is ignored.
    #[must_use]
    pub fn with_channel() -> (Self, UnboundedReceiver<DomainEvent>) {
        let (tx, rx) = mpsc::unbounded();
        let sink = Self {
            events: Mutex::default(),
            tx: Some(tx),
        };
        (sink, rx)
    }

    /// Events recorded so far, in emission order.
    #[must_use]
    pub fn events(&self) -> Vec<DomainEvent> {
        lock(&self.events).clone()
    }

    /// Returns the recorded events and clears the record.
    pub fn take(&self) -> Vec<DomainEvent> {
        std::mem::take(&mut *lock(&self.events))
    }

    /// Serde tag of each recorded event (e.g. `"turn_started"`), in order;
    /// handy to assert the sequence of a turn.
    #[must_use]
    pub fn kinds(&self) -> Vec<String> {
        lock(&self.events).iter().map(event_kind).collect()
    }
}

/// Serde tag of `event`, the snake_case name of its variant.
fn event_kind(event: &DomainEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

impl EventSink for RecordingSink {
    fn emit(&self, event: DomainEvent) {
        if let Some(tx) = &self.tx {
            // A closed receiver is not an error: the UI may have gone away.
            let _ = tx.unbounded_send(event.clone());
        }
        lock(&self.events).push(event);
    }
}

#[cfg(test)]
mod sink_tests {
    use std::sync::Arc;

    use futures::StreamExt;

    use super::*;
    use crate::ports::events::NoticeLevel;

    fn notice(message: &str) -> DomainEvent {
        DomainEvent::Notice {
            level: NoticeLevel::Info,
            message: message.into(),
        }
    }

    #[test]
    fn records_events_in_order() {
        let sink = RecordingSink::new();
        sink.emit(notice("a"));
        sink.emit(DomainEvent::Retrying {
            attempt: 1,
            delay_ms: 10,
        });
        assert_eq!(sink.kinds(), ["notice", "retrying"]);
        assert_eq!(sink.events()[0], notice("a"));
    }

    #[test]
    fn take_clears_the_record() {
        let sink = RecordingSink::new();
        sink.emit(notice("a"));
        assert_eq!(sink.take(), [notice("a")]);
        assert!(sink.events().is_empty());
    }

    #[tokio::test]
    async fn channel_receives_events_live() {
        let (sink, mut rx) = RecordingSink::with_channel();
        let sink: Arc<dyn EventSink> = Arc::new(sink);
        sink.emit(notice("live"));
        assert_eq!(rx.next().await, Some(notice("live")));
    }

    #[test]
    fn dropped_receiver_is_ignored() {
        let (sink, rx) = RecordingSink::with_channel();
        drop(rx);
        sink.emit(notice("a"));
        assert_eq!(sink.events().len(), 1);
    }
}

// ---------------------------------------------------------------------------
// ApprovalPort
// ---------------------------------------------------------------------------

/// One programmed answer of a [`ScriptedApproval`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptedAnswer {
    /// Resolve with this result.
    Reply(Result<ApprovalResponse, ApprovalError>),
    /// Never resolve, as a user who never answers; to test cancellation.
    Pending,
}

/// [`ApprovalPort`] that answers from a FIFO queue of programmed answers,
/// falling back to a default when the queue is empty.
///
/// With an empty queue and no default it fails with
/// [`ApprovalError::Unavailable`]. Every request is recorded, including
/// those left pending.
#[derive(Debug, Default)]
pub struct ScriptedApproval {
    queue: Mutex<VecDeque<ScriptedAnswer>>,
    default: Mutex<Option<ScriptedAnswer>>,
    requests: Mutex<Vec<ApprovalRequest>>,
}

impl ScriptedApproval {
    /// Creates an approval with no answers and no default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an approval that answers `response` to every request not
    /// covered by the queue.
    #[must_use]
    pub fn always(response: ApprovalResponse) -> Self {
        Self::with_default(ScriptedAnswer::Reply(Ok(response)))
    }

    /// Creates an approval that never answers unless an answer is queued.
    #[must_use]
    pub fn pending_forever() -> Self {
        Self::with_default(ScriptedAnswer::Pending)
    }

    /// Creates an approval with `answer` as default.
    #[must_use]
    pub fn with_default(answer: ScriptedAnswer) -> Self {
        let approval = Self::default();
        approval.set_default(Some(answer));
        approval
    }

    /// Replaces the default answer; `None` makes an empty queue fail.
    pub fn set_default(&self, answer: Option<ScriptedAnswer>) {
        *lock(&self.default) = answer;
    }

    /// Queues `response` for the next request.
    pub fn push(&self, response: ApprovalResponse) {
        self.push_answer(ScriptedAnswer::Reply(Ok(response)));
    }

    /// Queues `error` for the next request.
    pub fn push_error(&self, error: ApprovalError) {
        self.push_answer(ScriptedAnswer::Reply(Err(error)));
    }

    /// Queues an answer that never resolves.
    pub fn push_pending(&self) {
        self.push_answer(ScriptedAnswer::Pending);
    }

    /// Queues any answer.
    pub fn push_answer(&self, answer: ScriptedAnswer) {
        lock(&self.queue).push_back(answer);
    }

    /// Requests received so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<ApprovalRequest> {
        lock(&self.requests).clone()
    }

    /// Number of queued answers not consumed yet.
    #[must_use]
    pub fn remaining(&self) -> usize {
        lock(&self.queue).len()
    }

    fn next_answer(&self) -> Option<ScriptedAnswer> {
        let queued = lock(&self.queue).pop_front();
        queued.or_else(|| lock(&self.default).clone())
    }
}

#[async_trait]
impl ApprovalPort for ScriptedApproval {
    async fn request(&self, req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        lock(&self.requests).push(req);
        match self.next_answer() {
            Some(ScriptedAnswer::Reply(result)) => result,
            Some(ScriptedAnswer::Pending) => futures::future::pending().await,
            None => Err(ApprovalError::Unavailable(
                "no scripted approval answer".into(),
            )),
        }
    }
}

#[cfg(test)]
mod approval_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use domain::conversation::ToolUseId;
    use domain::permission::ToolEffect;

    use super::*;
    use crate::ports::approval::{ApprovalPreview, ApprovalRequestId};

    fn req(id: u64) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalRequestId(id),
            tool_use_id: ToolUseId::new(format!("toolu_{id}")),
            tool: "run_command".into(),
            effect: ToolEffect::Execute,
            subject: "cargo test".into(),
            preview: ApprovalPreview::Text {
                text: "cargo test".into(),
            },
        }
    }

    fn deny() -> ApprovalResponse {
        ApprovalResponse::Deny { reason: None }
    }

    #[tokio::test]
    async fn answers_in_fifo_order_then_default() {
        let approval = ScriptedApproval::always(deny());
        approval.push(ApprovalResponse::Approve);
        approval.push_error(ApprovalError::Cancelled);
        let port: Arc<dyn ApprovalPort> = Arc::new(approval);
        assert_eq!(port.request(req(1)).await, Ok(ApprovalResponse::Approve));
        assert_eq!(port.request(req(2)).await, Err(ApprovalError::Cancelled));
        assert_eq!(port.request(req(3)).await, Ok(deny()));
    }

    #[tokio::test]
    async fn empty_queue_without_default_is_unavailable() {
        let approval = ScriptedApproval::new();
        let result = approval.request(req(1)).await;
        assert!(matches!(result, Err(ApprovalError::Unavailable(_))));
    }

    #[tokio::test]
    async fn records_every_request() {
        let approval = ScriptedApproval::new();
        approval.push(ApprovalResponse::Approve);
        let _ = approval.request(req(1)).await;
        let _ = approval.request(req(2)).await;
        let ids: Vec<_> = approval.requests().iter().map(|r| r.id).collect();
        assert_eq!(ids, [ApprovalRequestId(1), ApprovalRequestId(2)]);
        assert_eq!(approval.remaining(), 0);
    }

    #[tokio::test]
    async fn pending_answer_never_resolves() {
        let approval = ScriptedApproval::pending_forever();
        let wait = tokio::time::timeout(Duration::from_millis(20), approval.request(req(1)));
        assert!(wait.await.is_err(), "pending approval resolved");
        assert_eq!(approval.requests().len(), 1);
    }

    #[tokio::test]
    async fn queued_answer_wins_over_pending_default() {
        let approval = ScriptedApproval::pending_forever();
        approval.push(ApprovalResponse::Approve);
        assert_eq!(
            approval.request(req(1)).await,
            Ok(ApprovalResponse::Approve)
        );
    }
}

// ---------------------------------------------------------------------------
// SessionStorePort
// ---------------------------------------------------------------------------

/// [`SessionStorePort`] over a `HashMap`, with the semantics of the real
/// store: `NotFound` on missing ids, listings most recent first.
///
/// [`fail_next`](Self::fail_next) makes the next operation fail, to test how
/// use cases handle a broken backend.
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    sessions: Mutex<HashMap<SessionId, StoredSession>>,
    fail_next: Mutex<Option<StoreError>>,
}

impl InMemorySessionStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes the next operation, whatever it is, fail with `error`.
    pub fn fail_next(&self, error: StoreError) {
        *lock(&self.fail_next) = Some(error);
    }

    /// Number of stored sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn check_failure(&self) -> Result<(), StoreError> {
        lock(&self.fail_next).take().map_or(Ok(()), Err)
    }

    /// Sessions of `root`, most recently updated first; ties are broken by
    /// creation time and then id so the order is deterministic.
    fn sorted_for(&self, root: &str) -> Vec<StoredSession> {
        let mut found: Vec<StoredSession> = lock(&self.sessions)
            .values()
            .filter(|s| s.session.workspace_root == root)
            .cloned()
            .collect();
        found.sort_by(|a, b| {
            let key =
                |s: &StoredSession| (s.session.updated_at, s.session.created_at, s.session.id);
            key(b).cmp(&key(a))
        });
        found
    }
}

#[async_trait]
impl SessionStorePort for InMemorySessionStore {
    async fn save(&self, session: &StoredSession) -> Result<(), StoreError> {
        self.check_failure()?;
        lock(&self.sessions).insert(session.session.id, session.clone());
        Ok(())
    }

    async fn load(&self, id: SessionId) -> Result<StoredSession, StoreError> {
        self.check_failure()?;
        lock(&self.sessions)
            .get(&id)
            .cloned()
            .ok_or(StoreError::NotFound(id))
    }

    async fn latest_for_workspace(&self, root: &str) -> Result<Option<StoredSession>, StoreError> {
        self.check_failure()?;
        Ok(self.sorted_for(root).into_iter().next())
    }

    async fn list(&self, root: &str) -> Result<Vec<SessionSummary>, StoreError> {
        self.check_failure()?;
        Ok(self
            .sorted_for(root)
            .iter()
            .map(StoredSession::summary)
            .collect())
    }

    async fn delete(&self, id: SessionId) -> Result<(), StoreError> {
        self.check_failure()?;
        lock(&self.sessions)
            .remove(&id)
            .map(|_| ())
            .ok_or(StoreError::NotFound(id))
    }

    async fn recent_workspaces(&self, limit: usize) -> Result<Vec<RecentWorkspace>, StoreError> {
        self.check_failure()?;
        let mut latest: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for stored in lock(&self.sessions).values() {
            let at = latest
                .entry(stored.session.workspace_root.clone())
                .or_default();
            *at = (*at).max(stored.session.updated_at);
        }
        let mut recent: Vec<RecentWorkspace> = latest
            .into_iter()
            .map(|(root, updated_at)| RecentWorkspace { root, updated_at })
            .collect();
        recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.root.cmp(&b.root)));
        recent.truncate(limit);
        Ok(recent)
    }
}

#[cfg(test)]
mod store_tests {
    use std::sync::Arc;

    use domain::conversation::Session;

    use super::*;

    fn stored(root: &str, updated_at: u64) -> StoredSession {
        StoredSession {
            session: Session::new(root, updated_at),
            timeline: Vec::new(),
            permission_rules: Vec::new(),
            prompt_history: Vec::new(),
        }
    }

    #[tokio::test]
    async fn recent_workspaces_are_grouped_and_newest_first() {
        let store = InMemorySessionStore::new();
        for (root, at) in [("/a", 5), ("/b", 9), ("/a", 12), ("/c", 1)] {
            store.save(&stored(root, at)).await.unwrap();
        }
        let recent = store.recent_workspaces(2).await.unwrap();
        let roots: Vec<(&str, u64)> = recent
            .iter()
            .map(|r| (r.root.as_str(), r.updated_at))
            .collect();
        assert_eq!(roots, [("/a", 12), ("/b", 9)]);
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let store: Arc<dyn SessionStorePort> = Arc::new(InMemorySessionStore::new());
        let session = stored("/ws", 1);
        store.save(&session).await.unwrap();
        assert_eq!(store.load(session.session.id).await.unwrap(), session);
    }

    #[tokio::test]
    async fn save_replaces_the_same_id() {
        let store = InMemorySessionStore::new();
        let mut session = stored("/ws", 1);
        store.save(&session).await.unwrap();
        session.session.title = Some("renamed".into());
        store.save(&session).await.unwrap();
        assert_eq!(store.len(), 1);
        let loaded = store.load(session.session.id).await.unwrap();
        assert_eq!(loaded.session.title.as_deref(), Some("renamed"));
    }

    #[tokio::test]
    async fn missing_ids_are_not_found() {
        let store = InMemorySessionStore::new();
        let id = SessionId::new();
        assert_eq!(store.load(id).await, Err(StoreError::NotFound(id)));
        assert_eq!(store.delete(id).await, Err(StoreError::NotFound(id)));
    }

    #[tokio::test]
    async fn list_is_per_workspace_and_most_recent_first() {
        let store = InMemorySessionStore::new();
        let old = stored("/ws", 1);
        let new = stored("/ws", 5);
        let other = stored("/other", 9);
        for s in [&old, &new, &other] {
            store.save(s).await.unwrap();
        }
        let ids: Vec<_> = store
            .list("/ws")
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, [new.session.id, old.session.id]);
        let latest = store.latest_for_workspace("/ws").await.unwrap();
        assert_eq!(latest, Some(new));
        assert_eq!(store.latest_for_workspace("/none").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_removes_the_session() {
        let store = InMemorySessionStore::new();
        let session = stored("/ws", 1);
        store.save(&session).await.unwrap();
        store.delete(session.session.id).await.unwrap();
        assert!(store.is_empty());
        assert!(store.list("/ws").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fail_next_fails_only_once() {
        let store = InMemorySessionStore::new();
        store.fail_next(StoreError::Backend("disk full".into()));
        let session = stored("/ws", 1);
        assert_eq!(
            store.save(&session).await,
            Err(StoreError::Backend("disk full".into()))
        );
        assert!(store.is_empty());
        store.save(&session).await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// PluginHostPort
// ---------------------------------------------------------------------------

/// Programmed body of a fake Lua callback or plugin tool.
pub type FakeHandler = Arc<dyn Fn(Value) -> Result<PluginOutput, PluginError> + Send + Sync>;

/// Programmed body of a fake hook.
pub type FakeHook = Arc<dyn Fn(&HookEvent) -> Result<HookOutcome, PluginError> + Send + Sync>;

/// A call received by a [`FakePluginHost`].
#[derive(Debug, Clone, PartialEq)]
pub enum PluginInvocation {
    /// [`PluginHostPort::load_config`].
    LoadConfig(ConfigSources),
    /// [`PluginHostPort::invoke_callback`].
    Callback {
        /// Callback invoked.
        id: CallbackId,
        /// Its arguments.
        args: Value,
    },
    /// [`PluginHostPort::call_tool`].
    Tool {
        /// Tool called.
        name: String,
        /// Its input.
        input: Value,
    },
    /// [`PluginHostPort::run_hook`].
    Hook(HookEvent),
}

/// [`PluginHostPort`] without Lua: a fixed [`LoadedConfig`], callbacks and
/// tools programmed as closures, and hooks with the real chaining rules
/// (the first `Deny` stops; each `ReplaceInput` feeds the next hook).
///
/// Unknown callbacks and tools fail with [`PluginError::NotFound`]. Every
/// call is recorded, see [`invocations`](Self::invocations).
#[derive(Default)]
pub struct FakePluginHost {
    config: Option<Result<LoadedConfig, PluginError>>,
    callbacks: HashMap<CallbackId, FakeHandler>,
    tools: HashMap<String, FakeHandler>,
    hooks: Vec<(HookKind, FakeHook)>,
    invocations: Mutex<Vec<PluginInvocation>>,
}

impl fmt::Debug for FakePluginHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakePluginHost")
            .field("config", &self.config)
            .field("callbacks", &self.callbacks.keys().collect::<Vec<_>>())
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .field("hooks", &self.hooks.len())
            .finish_non_exhaustive()
    }
}

impl FakePluginHost {
    /// Creates a host that loads [`LoadedConfig::default`] and knows no
    /// callbacks, tools or hooks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `load_config` return `config`.
    #[must_use]
    pub fn with_config(mut self, config: LoadedConfig) -> Self {
        self.config = Some(Ok(config));
        self
    }

    /// Makes `load_config` fail with `error`.
    #[must_use]
    pub fn with_load_error(mut self, error: PluginError) -> Self {
        self.config = Some(Err(error));
        self
    }

    /// Registers the body of the callback `id`.
    #[must_use]
    pub fn on_callback(
        mut self,
        id: CallbackId,
        f: impl Fn(Value) -> Result<PluginOutput, PluginError> + Send + Sync + 'static,
    ) -> Self {
        self.callbacks.insert(id, Arc::new(f));
        self
    }

    /// Registers the handler of the tool `name`.
    #[must_use]
    pub fn on_tool(
        mut self,
        name: impl Into<String>,
        f: impl Fn(Value) -> Result<PluginOutput, PluginError> + Send + Sync + 'static,
    ) -> Self {
        self.tools.insert(name.into(), Arc::new(f));
        self
    }

    /// Appends a hook for events of `kind`; hooks run in registration order.
    #[must_use]
    pub fn on_hook(
        mut self,
        kind: HookKind,
        f: impl Fn(&HookEvent) -> Result<HookOutcome, PluginError> + Send + Sync + 'static,
    ) -> Self {
        self.hooks.push((kind, Arc::new(f)));
        self
    }

    /// Appends a `before_tool` hook that denies the tool `name`.
    #[must_use]
    pub fn deny_tool(self, name: impl Into<String>, reason: Option<String>) -> Self {
        let name = name.into();
        self.on_hook(HookKind::BeforeTool, move |event| match event {
            HookEvent::BeforeTool { name: tool, .. } if *tool == name => Ok(HookOutcome::Deny {
                reason: reason.clone(),
            }),
            _ => Ok(HookOutcome::Continue),
        })
    }

    /// Calls received so far, in order.
    #[must_use]
    pub fn invocations(&self) -> Vec<PluginInvocation> {
        lock(&self.invocations).clone()
    }

    fn record(&self, invocation: PluginInvocation) {
        lock(&self.invocations).push(invocation);
    }
}

/// Replaces the input carried by `event`.
fn set_hook_input(event: &mut HookEvent, value: Value) {
    match event {
        HookEvent::BeforeTool { input, .. } | HookEvent::AfterTool { input, .. } => *input = value,
    }
}

#[async_trait]
impl PluginHostPort for FakePluginHost {
    async fn load_config(&self, sources: ConfigSources) -> Result<LoadedConfig, PluginError> {
        self.record(PluginInvocation::LoadConfig(sources));
        self.config
            .clone()
            .unwrap_or_else(|| Ok(LoadedConfig::default()))
    }

    async fn invoke_callback(
        &self,
        id: CallbackId,
        args: Value,
    ) -> Result<PluginOutput, PluginError> {
        self.record(PluginInvocation::Callback {
            id,
            args: args.clone(),
        });
        let handler = self
            .callbacks
            .get(&id)
            .ok_or_else(|| PluginError::NotFound(format!("callback {id}")))?;
        handler(args)
    }

    async fn call_tool(&self, name: &str, input: Value) -> Result<PluginOutput, PluginError> {
        self.record(PluginInvocation::Tool {
            name: name.to_owned(),
            input: input.clone(),
        });
        let handler = self
            .tools
            .get(name)
            .ok_or_else(|| PluginError::NotFound(format!("tool {name}")))?;
        handler(input)
    }

    async fn run_hook(&self, event: HookEvent) -> Result<HookOutcome, PluginError> {
        self.record(PluginInvocation::Hook(event.clone()));
        let kind = event.kind();
        let mut event = event;
        let mut replaced = None;
        for (_, hook) in self.hooks.iter().filter(|(k, _)| *k == kind) {
            match hook(&event)? {
                HookOutcome::Continue => {}
                deny @ HookOutcome::Deny { .. } => return Ok(deny),
                HookOutcome::ReplaceInput(value) => {
                    set_hook_input(&mut event, value.clone());
                    replaced = Some(value);
                }
            }
        }
        Ok(replaced.map_or(HookOutcome::Continue, HookOutcome::ReplaceInput))
    }
}

#[cfg(test)]
mod plugin_tests {
    use serde_json::json;

    use super::*;
    use crate::ports::plugin::AppOptions;

    fn before(name: &str, input: Value) -> HookEvent {
        HookEvent::BeforeTool {
            name: name.into(),
            input,
        }
    }

    #[tokio::test]
    async fn load_config_returns_the_fixed_config() {
        let config = LoadedConfig {
            options: AppOptions {
                theme: Some("tokyonight".into()),
                ..AppOptions::default()
            },
            ..LoadedConfig::default()
        };
        let host: Arc<dyn PluginHostPort> =
            Arc::new(FakePluginHost::new().with_config(config.clone()));
        let loaded = host.load_config(ConfigSources::default()).await.unwrap();
        assert_eq!(loaded, config);
    }

    #[tokio::test]
    async fn load_config_can_fail() {
        let host = FakePluginHost::new().with_load_error(PluginError::Unavailable("gone".into()));
        let result = host.load_config(ConfigSources::default()).await;
        assert_eq!(result, Err(PluginError::Unavailable("gone".into())));
    }

    #[tokio::test]
    async fn callbacks_and_tools_run_their_closures() {
        let host = FakePluginHost::new()
            .on_callback(CallbackId(1), |args| Ok(PluginOutput::Value(args)))
            .on_tool("echo", |input| {
                Ok(PluginOutput::Value(json!({ "got": input })))
            })
            .on_tool("boom", |_| Err(PluginError::Runtime("nil value".into())));
        assert_eq!(
            host.invoke_callback(CallbackId(1), json!([1])).await,
            Ok(PluginOutput::Value(json!([1])))
        );
        assert_eq!(
            host.call_tool("echo", json!("x")).await,
            Ok(PluginOutput::Value(json!({ "got": "x" })))
        );
        assert_eq!(
            host.call_tool("boom", Value::Null).await,
            Err(PluginError::Runtime("nil value".into()))
        );
    }

    #[tokio::test]
    async fn unknown_callbacks_and_tools_are_not_found() {
        let host = FakePluginHost::new();
        let callback = host.invoke_callback(CallbackId(9), Value::Null).await;
        assert!(matches!(callback, Err(PluginError::NotFound(_))));
        let tool = host.call_tool("nope", Value::Null).await;
        assert!(matches!(tool, Err(PluginError::NotFound(_))));
    }

    #[tokio::test]
    async fn deny_tool_only_denies_that_tool() {
        let host = FakePluginHost::new().deny_tool("rm", Some("no".into()));
        assert_eq!(
            host.run_hook(before("rm", Value::Null)).await,
            Ok(HookOutcome::Deny {
                reason: Some("no".into())
            })
        );
        assert_eq!(
            host.run_hook(before("grep", Value::Null)).await,
            Ok(HookOutcome::Continue)
        );
    }

    #[tokio::test]
    async fn replaced_input_feeds_the_next_hook() {
        let host = FakePluginHost::new()
            .on_hook(HookKind::BeforeTool, |_| {
                Ok(HookOutcome::ReplaceInput(json!({ "step": 1 })))
            })
            .on_hook(HookKind::BeforeTool, |event| match event {
                HookEvent::BeforeTool { input, .. } if *input == json!({ "step": 1 }) => {
                    Ok(HookOutcome::ReplaceInput(json!({ "step": 2 })))
                }
                _ => Ok(HookOutcome::Continue),
            });
        assert_eq!(
            host.run_hook(before("edit", Value::Null)).await,
            Ok(HookOutcome::ReplaceInput(json!({ "step": 2 })))
        );
    }

    #[tokio::test]
    async fn deny_stops_the_chain_and_errors_propagate() {
        let host = FakePluginHost::new()
            .deny_tool("rm", None)
            .on_hook(HookKind::BeforeTool, |_| Err(PluginError::InstructionLimit));
        assert_eq!(
            host.run_hook(before("rm", Value::Null)).await,
            Ok(HookOutcome::Deny { reason: None })
        );
        assert_eq!(
            host.run_hook(before("ls", Value::Null)).await,
            Err(PluginError::InstructionLimit)
        );
    }

    #[tokio::test]
    async fn hooks_only_run_for_their_kind() {
        let host = FakePluginHost::new().on_hook(HookKind::AfterTool, |_| {
            Ok(HookOutcome::Deny { reason: None })
        });
        assert_eq!(
            host.run_hook(before("ls", Value::Null)).await,
            Ok(HookOutcome::Continue)
        );
    }

    #[tokio::test]
    async fn records_every_invocation() {
        let host = FakePluginHost::new();
        let _ = host.load_config(ConfigSources::default()).await;
        let _ = host.call_tool("t", json!(1)).await;
        let _ = host.invoke_callback(CallbackId(2), json!(2)).await;
        let _ = host.run_hook(before("t", json!(3))).await;
        assert_eq!(
            host.invocations(),
            [
                PluginInvocation::LoadConfig(ConfigSources::default()),
                PluginInvocation::Tool {
                    name: "t".into(),
                    input: json!(1)
                },
                PluginInvocation::Callback {
                    id: CallbackId(2),
                    args: json!(2)
                },
                PluginInvocation::Hook(before("t", json!(3))),
            ]
        );
    }
}

// ---------------------------------------------------------------------------
// TelemetryPort
// ---------------------------------------------------------------------------

/// A call received by a [`RecordingTelemetry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryRecord {
    /// [`TelemetryPort::turn_started`], with the span it returned.
    TurnStarted {
        /// Span handed out.
        span: SpanId,
        /// What was reported.
        turn: TurnSpan,
    },
    /// [`TelemetryPort::llm_call`].
    LlmCall(LlmCallRecord),
    /// [`TelemetryPort::tool_call`].
    ToolCall(ToolCallRecord),
    /// [`TelemetryPort::turn_finished`].
    TurnFinished {
        /// Span closed.
        span: SpanId,
        /// Total usage of the turn.
        usage: Usage,
        /// Stop reason, `None` when the turn failed.
        stop_reason: Option<StopReason>,
    },
}

/// [`TelemetryPort`] that records every call; spans are numbered 1, 2, ...
#[derive(Debug, Default)]
pub struct RecordingTelemetry {
    next_span: AtomicU64,
    records: Mutex<Vec<TelemetryRecord>>,
}

impl RecordingTelemetry {
    /// Creates an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Calls received so far, in order.
    #[must_use]
    pub fn records(&self) -> Vec<TelemetryRecord> {
        lock(&self.records).clone()
    }

    fn record(&self, record: TelemetryRecord) {
        lock(&self.records).push(record);
    }
}

impl TelemetryPort for RecordingTelemetry {
    fn turn_started(&self, turn: TurnSpan) -> SpanId {
        let span = SpanId(self.next_span.fetch_add(1, Ordering::Relaxed) + 1);
        self.record(TelemetryRecord::TurnStarted { span, turn });
        span
    }

    fn llm_call(&self, record: LlmCallRecord) {
        self.record(TelemetryRecord::LlmCall(record));
    }

    fn tool_call(&self, record: ToolCallRecord) {
        self.record(TelemetryRecord::ToolCall(record));
    }

    fn turn_finished(&self, span: SpanId, usage: Usage, stop_reason: Option<StopReason>) {
        self.record(TelemetryRecord::TurnFinished {
            span,
            usage,
            stop_reason,
        });
    }
}

#[cfg(test)]
mod telemetry_tests {
    use domain::conversation::TurnId;
    use domain::model::{ModelId, ModelRole};

    use super::*;

    fn turn() -> TurnSpan {
        TurnSpan {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            role: ModelRole::Main,
            model: ModelId::new("claude-sonnet-5-5"),
        }
    }

    #[test]
    fn spans_are_incremental() {
        let telemetry = RecordingTelemetry::new();
        assert_eq!(telemetry.turn_started(turn()), SpanId(1));
        assert_eq!(telemetry.turn_started(turn()), SpanId(2));
    }

    #[test]
    fn records_calls_in_order() {
        let telemetry: Arc<RecordingTelemetry> = Arc::new(RecordingTelemetry::new());
        let port: Arc<dyn TelemetryPort> = telemetry.clone();
        let t = turn();
        let span = port.turn_started(t.clone());
        let tool = ToolCallRecord {
            parent: span,
            name: "grep".into(),
            duration_ms: 3,
            ok: false,
        };
        port.tool_call(tool.clone());
        let llm = LlmCallRecord {
            parent: span,
            model: ModelId::new("m"),
            usage: Usage::default(),
            latency_ms: 10,
            error: Some("overloaded".into()),
        };
        port.llm_call(llm.clone());
        port.turn_finished(span, Usage::default(), None);
        assert_eq!(
            telemetry.records(),
            [
                TelemetryRecord::TurnStarted { span, turn: t },
                TelemetryRecord::ToolCall(tool),
                TelemetryRecord::LlmCall(llm),
                TelemetryRecord::TurnFinished {
                    span,
                    usage: Usage::default(),
                    stop_reason: None
                },
            ]
        );
    }
}
