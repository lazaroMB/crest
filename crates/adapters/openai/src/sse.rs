//! The server-sent events of a response: the shared [`Decoder`] loop of
//! `adapter-support` and the JSON of each event.

use application::ports::llm::LlmError;

pub use adapter_support::sse::{Decoder, llm_events};

/// Parses the JSON data of one event.
///
/// # Errors
///
/// [`LlmError::Decode`] when it is not JSON.
pub fn json(data: &str) -> Result<serde_json::Value, LlmError> {
    serde_json::from_str(data).map_err(|err| LlmError::Decode(format!("invalid event data: {err}")))
}
