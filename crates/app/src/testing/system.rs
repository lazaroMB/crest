//! Fakes of the system ports: events, approval, session store, Lua host and
//! telemetry.
//!
//! Every fake is `Send + Sync`, usable as `Arc<dyn Port>`, and records what
//! it received so tests can assert on it afterwards.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::ports::approval::{ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink};

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
