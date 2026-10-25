//! Developer and operations viewers: distributed traces (`show_trace`),
//! profiles as flame graphs (`show_flamegraph`) and structured logs
//! (`show_logs`).
//!
//! Adapters parse the files (OTLP, Jaeger and Zipkin JSON; folded stacks
//! and pprof; JSON lines, logfmt and plain text) into these specs, which
//! the blocks draw on the shared widgets ([`crate::widget`]). The pure
//! helpers here (critical path, self times, hottest frames, level counts)
//! are what the tools summarise for the model.

pub mod flame;
pub mod logs;
pub mod trace;

use crate::block::BlockError;

/// A [`BlockError::Invalid`] of block `kind`.
pub(crate) fn invalid(kind: &'static str, reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind,
        reason: reason.into(),
    }
}
