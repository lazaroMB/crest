//! Traces per turn and per model call.
//!
//! The use cases report what happened; the adapter turns it into `tracing`
//! spans (and optionally Langfuse traces). All methods are synchronous and
//! must be cheap: they run inside the agent loop.

use std::fmt;

use domain::conversation::{SessionId, StopReason, TurnId, Usage};
use domain::model::{ModelId, ModelRole};

/// Handle to a span opened by [`TelemetryPort::turn_started`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpanId(pub u64);

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Start of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSpan {
    /// Session of the turn.
    pub session_id: SessionId,
    /// The turn.
    pub turn_id: TurnId,
    /// Role the model was resolved from.
    pub role: ModelRole,
    /// Model answering the turn.
    pub model: ModelId,
}

/// One request to the model, finished or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmCallRecord {
    /// Turn span it belongs to.
    pub parent: SpanId,
    /// Model actually called (after fallbacks).
    pub model: ModelId,
    /// Tokens used by the call.
    pub usage: Usage,
    /// Wall time until the stream ended, in milliseconds.
    pub latency_ms: u64,
    /// Error message when the call failed.
    pub error: Option<String>,
}

/// One tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRecord {
    /// Turn span it belongs to.
    pub parent: SpanId,
    /// Tool name.
    pub name: String,
    /// Execution time, in milliseconds.
    pub duration_ms: u64,
    /// Whether it succeeded.
    pub ok: bool,
}

/// Records traces of turns, model calls and tool calls.
///
/// Implementations must not block nor fail: telemetry problems are logged
/// by the adapter and never interrupt a turn.
pub trait TelemetryPort: Send + Sync {
    /// Opens the span of a turn and returns its handle.
    fn turn_started(&self, span: TurnSpan) -> SpanId;

    /// Records a model call inside a turn.
    fn llm_call(&self, record: LlmCallRecord);

    /// Records a tool call inside a turn.
    fn tool_call(&self, record: ToolCallRecord);

    /// Closes the span of a turn with its total usage and stop reason
    /// (`None` when the turn failed).
    fn turn_finished(&self, span: SpanId, usage: Usage, stop_reason: Option<StopReason>);
}

/// Telemetry that records nothing; the default when tracing is off.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopTelemetry;

impl TelemetryPort for NoopTelemetry {
    fn turn_started(&self, _span: TurnSpan) -> SpanId {
        SpanId(0)
    }

    fn llm_call(&self, _record: LlmCallRecord) {}

    fn tool_call(&self, _record: ToolCallRecord) {}

    fn turn_finished(&self, _span: SpanId, _usage: Usage, _stop_reason: Option<StopReason>) {}
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn noop_telemetry_is_usable_as_a_port() {
        let telemetry: Arc<dyn TelemetryPort> = Arc::new(NoopTelemetry);
        let span = telemetry.turn_started(TurnSpan {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            role: ModelRole::Main,
            model: ModelId::new("claude-sonnet-5-5"),
        });
        telemetry.tool_call(ToolCallRecord {
            parent: span,
            name: "grep".into(),
            duration_ms: 3,
            ok: true,
        });
        telemetry.turn_finished(span, Usage::default(), Some(StopReason::EndTurn));
    }
}
