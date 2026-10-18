//! TelemetryPort adapter with tracing and Langfuse.
//!
//! [`TracingTelemetry`] turns the calls of the agent loop into `tracing`
//! spans: one `turn` span per turn with `llm_call` and `tool_call` children.
//! Spans only carry ids, model names, token counts, timings and errors;
//! never message content nor credentials.

mod spans;

pub use spans::TracingTelemetry;
