//! Fakes of the system ports: events, approval, session store, Lua host and
//! telemetry.
//!
//! Every fake is `Send + Sync`, usable as `Arc<dyn Port>`, and records what
//! it received so tests can assert on it afterwards.

use std::sync::{Mutex, MutexGuard, PoisonError};

use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};

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
