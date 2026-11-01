//! Runs a [`Decoder`] over the server-sent events of a streaming answer.

use std::collections::VecDeque;
use std::fmt::Display;

use application_core::ports::llm::{LlmError, LlmEvent};
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

/// The events of `bytes` (the body of a streaming response, such as
/// `reqwest::Response::bytes_stream()`) decoded with `decoder`.
///
/// Events with blank data are skipped. The stream ends once the decoder
/// [is stopped](Decoder::is_stopped), at the end of the body (with what
/// [`Decoder::finish`] owes) or after the first error: transport failures
/// become [`LlmError::Network`] and malformed server-sent events
/// [`LlmError::Decode`].
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

/// The typed error of a failure reading server-sent events.
#[must_use]
pub fn sse_error<E: Display>(err: &EventStreamError<E>) -> LlmError {
    match err {
        EventStreamError::Transport(err) => LlmError::Network(err.to_string()),
        EventStreamError::Utf8(_) | EventStreamError::Parser(_) => {
            LlmError::Decode(format!("invalid server-sent events: {err}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    /// Turns each event's data into a text delta; `stop` stops it, `fail`
    /// is an error, and the end of the stream owes a last delta unless
    /// stopped.
    #[derive(Default)]
    struct Echo {
        stopped: bool,
    }

    fn delta(text: &str) -> LlmEvent {
        LlmEvent::TextDelta {
            index: 0,
            text: text.to_owned(),
        }
    }

    impl Decoder for Echo {
        fn decode(&mut self, data: &str) -> Result<Vec<LlmEvent>, LlmError> {
            match data {
                "fail" => Err(LlmError::Decode("bad".into())),
                "stop" => {
                    self.stopped = true;
                    Ok(vec![delta("stop"), delta("after stop, same event")])
                }
                _ => Ok(vec![delta(data)]),
            }
        }

        fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
            Ok(vec![delta("end")])
        }

        fn is_stopped(&self) -> bool {
            self.stopped
        }
    }

    fn run(body: &str) -> Vec<Result<LlmEvent, LlmError>> {
        let source = futures::stream::iter(vec![Ok::<_, &str>(body.as_bytes().to_vec())]);
        block_on(llm_events(source, Echo::default()).collect())
    }

    #[test]
    fn blank_events_are_skipped_and_the_end_is_finished() {
        assert_eq!(
            run("data: a\n\ndata:  \n\ndata: b\n\n"),
            [Ok(delta("a")), Ok(delta("b")), Ok(delta("end"))]
        );
    }

    #[test]
    fn a_stopped_decoder_ends_the_stream() {
        assert_eq!(
            run("data: stop\n\ndata: ignored\n\n"),
            [Ok(delta("stop")), Ok(delta("after stop, same event"))]
        );
    }

    #[test]
    fn the_first_error_ends_the_stream() {
        assert_eq!(
            run("data: a\n\ndata: fail\n\ndata: b\n\n"),
            [Ok(delta("a")), Err(LlmError::Decode("bad".into()))]
        );
        let source = futures::stream::iter(vec![Ok(b"data: a\n\n".to_vec()), Err("reset")]);
        let items: Vec<_> = block_on(llm_events(source, Echo::default()).collect());
        assert_eq!(
            items,
            [Ok(delta("a")), Err(LlmError::Network("reset".into()))]
        );
    }
}
