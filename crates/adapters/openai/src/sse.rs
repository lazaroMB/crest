//! Runs a [`Decoder`] over the server-sent events of a response.

use std::collections::VecDeque;
use std::fmt::Display;

use application::ports::llm::{LlmError, LlmEvent};
use eventsource_stream::{EventStreamError, Eventsource};
use futures::{Stream, StreamExt};

/// A pure state machine from the `data` of each event to [`LlmEvent`]s.
pub trait Decoder: Send + 'static {
    /// Decodes the data of one event.
    ///
    /// # Errors
    ///
    /// When the data is not valid or reports an error.
    fn decode(&mut self, data: &str) -> Result<Vec<LlmEvent>, LlmError>;

    /// The events owed once the stream ends.
    ///
    /// # Errors
    ///
    /// When the stream ended before the answer was complete.
    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError>;

    /// Whether the answer is complete: nothing else will be decoded.
    fn is_stopped(&self) -> bool;
}

struct State<S, D> {
    source: S,
    decoder: D,
    pending: VecDeque<Result<LlmEvent, LlmError>>,
    done: bool,
}

/// The events of `bytes` (the body of a streaming response) decoded with
/// `decoder`. The stream ends after the first error.
pub fn llm_events<S, B, E, D>(
    bytes: S,
    decoder: D,
) -> impl Stream<Item = Result<LlmEvent, LlmError>> + Send + 'static
where
    S: Stream<Item = Result<B, E>> + Send + Unpin + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: Display + Send + 'static,
    D: Decoder,
{
    let state = State {
        source: bytes.eventsource(),
        decoder,
        pending: VecDeque::new(),
        done: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(item) = state.pending.pop_front() {
                if item.is_err() {
                    state.pending.clear();
                    state.done = true;
                }
                return Some((item, state));
            }
            if state.done {
                return None;
            }
            let item = match state.source.next().await {
                Some(Ok(event)) if event.data.trim().is_empty() => continue,
                Some(Ok(event)) => state.decoder.decode(&event.data),
                Some(Err(err)) => Err(sse_error(&err)),
                None => {
                    state.done = true;
                    state.decoder.finish()
                }
            };
            match item {
                Ok(events) => {
                    state.done |= state.decoder.is_stopped();
                    state.pending.extend(events.into_iter().map(Ok));
                }
                Err(err) => state.pending.push_back(Err(err)),
            }
        }
    })
}

fn sse_error<E: Display>(err: &EventStreamError<E>) -> LlmError {
    match err {
        EventStreamError::Transport(err) => LlmError::Network(err.to_string()),
        EventStreamError::Utf8(_) | EventStreamError::Parser(_) => {
            LlmError::Decode(format!("invalid server-sent events: {err}"))
        }
    }
}

/// Parses the JSON data of one event.
///
/// # Errors
///
/// [`LlmError::Decode`] when it is not JSON.
pub fn json(data: &str) -> Result<serde_json::Value, LlmError> {
    serde_json::from_str(data).map_err(|err| LlmError::Decode(format!("invalid event data: {err}")))
}
