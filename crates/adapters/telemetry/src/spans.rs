//! [`TelemetryPort`] backed by `tracing` spans.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use application::ports::telemetry::{
    LlmCallRecord, SpanId, TelemetryPort, ToolCallRecord, TurnSpan,
};
use domain::conversation::{StopReason, Usage};
use tracing::field::Empty;
use tracing::{Span, info_span};

/// Target of every span emitted by this adapter.
pub const TARGET: &str = "crest::telemetry";

/// Longest error message kept in a span, in bytes.
const MAX_ERROR_LEN: usize = 512;

/// [`TelemetryPort`] that records turns, model calls and tool calls as
/// `tracing` spans.
///
/// The `turn` span stays open from [`TelemetryPort::turn_started`] until
/// [`TelemetryPort::turn_finished`]; model and tool calls are explicit
/// children of it, so the hierarchy does not depend on which span is
/// entered on the current thread.
#[derive(Debug, Default)]
pub struct TracingTelemetry {
    next_id: AtomicU64,
    turns: Mutex<HashMap<SpanId, Span>>,
}

impl TracingTelemetry {
    /// Creates the adapter with no open turns.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of turns opened and not yet finished.
    #[must_use]
    pub fn open_turns(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SpanId, Span>> {
        // A poisoned map only means a panic happened while holding it; the
        // spans inside are still valid.
        self.turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The `tracing` id of an open turn, or `None` when the turn is unknown
    /// (the child then becomes a root span instead of being lost).
    fn parent_of(&self, span: SpanId) -> Option<tracing::Id> {
        let parent = self.lock().get(&span).and_then(Span::id);
        if parent.is_none() {
            tracing::debug!(target: TARGET, %span, "telemetry record for an unknown turn");
        }
        parent
    }
}

impl TelemetryPort for TracingTelemetry {
    fn turn_started(&self, turn: TurnSpan) -> SpanId {
        let id = SpanId(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let span = info_span!(
            target: TARGET,
            parent: None,
            "turn",
            session_id = %turn.session_id,
            turn_id = %turn.turn_id,
            role = %turn.role,
            model = %turn.model,
            input_tokens = Empty,
            output_tokens = Empty,
            cache_read_input_tokens = Empty,
            cache_creation_input_tokens = Empty,
            stop_reason = Empty,
            failed = Empty,
        );
        self.lock().insert(id, span);
        id
    }

    fn llm_call(&self, record: LlmCallRecord) {
        let parent = self.parent_of(record.parent);
        let span = info_span!(
            target: TARGET,
            parent: parent,
            "llm_call",
            model = %record.model,
            input_tokens = Empty,
            output_tokens = Empty,
            cache_read_input_tokens = Empty,
            cache_creation_input_tokens = Empty,
            latency_ms = record.latency_ms,
            error = Empty,
        );
        record_usage(&span, record.usage);
        if let Some(error) = &record.error {
            span.record("error", truncate(error, MAX_ERROR_LEN));
        }
    }

    fn tool_call(&self, record: ToolCallRecord) {
        let parent = self.parent_of(record.parent);
        let _span = info_span!(
            target: TARGET,
            parent: parent,
            "tool_call",
            tool = %record.name,
            duration_ms = record.duration_ms,
            ok = record.ok,
        );
    }

    fn turn_finished(&self, span: SpanId, usage: Usage, stop_reason: Option<StopReason>) {
        let Some(turn) = self.lock().remove(&span) else {
            tracing::debug!(target: TARGET, %span, "finished an unknown turn");
            return;
        };
        record_usage(&turn, usage);
        match stop_reason {
            Some(reason) => {
                turn.record("stop_reason", stop_reason_str(reason));
                turn.record("failed", false);
            }
            None => {
                turn.record("failed", true);
            }
        }
        // Dropping the last handle closes the span.
    }
}

fn record_usage(span: &Span, usage: Usage) {
    span.record("input_tokens", usage.input_tokens);
    span.record("output_tokens", usage.output_tokens);
    span.record("cache_read_input_tokens", usage.cache_read_input_tokens);
    span.record(
        "cache_creation_input_tokens",
        usage.cache_creation_input_tokens,
    );
}

/// Same names as the `snake_case` serialisation of [`StopReason`].
fn stop_reason_str(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxTokens => "max_tokens",
        StopReason::StopSequence => "stop_sequence",
        StopReason::PauseTurn => "pause_turn",
        StopReason::Refusal => "refusal",
        StopReason::Interrupted => "interrupted",
    }
}

/// Cuts `text` to at most `max` bytes on a char boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests;
