//! TelemetryPort adapter with tracing and Langfuse.
//!
//! [`TracingTelemetry`] turns the calls of the agent loop into `tracing`
//! spans: one `turn` span per turn with `llm_call` and `tool_call` children.
//! Spans only carry ids, model names, token counts, timings and errors;
//! never message content nor credentials.
//!
//! [`init_subscriber`] installs the global subscriber: human-readable logs
//! to stderr or a file, filtered by the `CREST_LOG` variable.
//!
//! With the `langfuse` feature, [`LangfuseConfig`] reads the Langfuse
//! project from `LANGFUSE_HOST`, `LANGFUSE_PUBLIC_KEY` and
//! `LANGFUSE_SECRET_KEY`.

#[cfg(feature = "langfuse")]
mod langfuse;
mod spans;
mod subscriber;

#[cfg(feature = "langfuse")]
pub use langfuse::{DEFAULT_HOST, LangfuseConfig, LangfuseError};
pub use spans::{TARGET, TracingTelemetry};
pub use subscriber::{
    LOG_ENV, LogOutput, SubscriberError, SubscriberOptions, init_subscriber, init_subscriber_with,
};
