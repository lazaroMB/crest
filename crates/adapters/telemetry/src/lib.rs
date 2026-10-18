//! TelemetryPort adapter with tracing and Langfuse.
//!
//! [`TracingTelemetry`] turns the calls of the agent loop into `tracing`
//! spans: one `turn` span per turn with `llm_call` and `tool_call` children.
//! Spans only carry ids, model names, token counts, timings and errors;
//! never message content nor credentials.
//!
//! [`init_subscriber`] installs the global subscriber: human-readable logs
//! to stderr or a file, filtered by the `CREST_LOG` variable.

mod spans;
mod subscriber;

pub use spans::{TARGET, TracingTelemetry};
pub use subscriber::{
    LOG_ENV, LogOutput, SubscriberError, SubscriberOptions, init_subscriber, init_subscriber_with,
};
