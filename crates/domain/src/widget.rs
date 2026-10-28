//! Specs of the shared widgets: a timeline of spans, a filterable list, a
//! node-link graph and a tree table.
//!
//! Each spec is plain data a `Custom` block node carries
//! ([`crate::block::CustomNode`]) and that domain viewers (traces, logs,
//! plans, waveforms) build from their parsers. The UI draws them; the
//! pure helpers here (validation, extents, copy-as text) are shared by the
//! UI and the tools that summarise them for the model.

pub mod graph;
pub mod list;
pub mod timeline;
pub mod tree;

use crate::block::BlockError;

/// A [`BlockError::Invalid`] of widget `kind`.
pub(crate) fn invalid(kind: &'static str, reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind,
        reason: reason.into(),
    }
}

/// Fails when `values` holds the same key twice; `what` names the key in
/// the message (`row id`, `node id`).
pub(crate) fn unique<'a>(
    kind: &'static str,
    what: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), BlockError> {
    let mut seen = std::collections::HashSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(invalid(kind, format!("{what} {value:?} appears twice")));
        }
    }
    Ok(())
}
