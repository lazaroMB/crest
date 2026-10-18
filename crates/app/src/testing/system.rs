//! Fakes of the system ports: events, approval, session store, Lua host and
//! telemetry.
//!
//! Every fake is `Send + Sync`, usable as `Arc<dyn Port>`, and records what
//! it received so tests can assert on it afterwards.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::conversation::SessionId;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::ports::approval::{ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink};
use crate::ports::store::{SessionStorePort, SessionSummary, StoreError, StoredSession};

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
        }
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
