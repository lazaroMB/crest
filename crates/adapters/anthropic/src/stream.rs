//! Streaming responses: server-sent events → [`StreamEvent`] → [`LlmEvent`].
//!
//! [`StreamDecoder`] is the pure state machine (one [`StreamEvent`] in,
//! zero or more [`LlmEvent`]s out); [`llm_events`] runs it over the raw
//! bytes of a response with `eventsource-stream`.
//!
//! Mapping:
//!
//! - `message_start` → [`LlmEvent::MessageStart`] and a first
//!   [`LlmEvent::Usage`]; `message_delta` → cumulative [`LlmEvent::Usage`]
//!   (counts it carries replace those of `message_start`, missing ones are
//!   kept) and the stop reason, emitted with `message_stop`.
//! - Text, thinking, signatures and client `tool_use` map one to one. Every
//!   text or thinking block emits a (possibly empty) first delta at
//!   `content_block_start`, so the block exists even if no delta follows.
//! - `citations_delta` with a web search location → [`LlmEvent::Citation`].
//! - Server tool blocks (`server_tool_use`, `web_search_tool_result`,
//!   `web_fetch_tool_result`) emit [`LlmEvent::ServerToolUse`] at start
//!   (calls only), accumulate their input internally and, at
//!   `content_block_stop`, emit [`LlmEvent::ProviderBlock`] with the full
//!   block JSON followed by [`LlmEvent::BlockStop`].
//! - `ping`, unknown events, unknown deltas and unknown block types are
//!   ignored (unknown blocks also swallow their `content_block_stop`).
//! - A stream `error` event becomes a typed [`LlmError`] and ends the
//!   stream; so does a stream that ends without `message_stop`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt::Display;

use application::ports::llm::{LlmError, LlmEvent};
use domain::conversation::{StopReason, ToolUseId};
use domain::model::ModelId;
use eventsource_stream::{EventStreamError, Eventsource};
use futures::{Stream, StreamExt};
use serde_json::Value;

use crate::convert;
use crate::wire::{ContentDelta, StreamEvent, WireContentBlock, WireUsage};

/// Kind of an open content block, as far as decoding needs to know.
#[derive(Debug)]
enum Block {
    /// Text, thinking or redacted thinking: deltas map one to one.
    Plain,
    /// A client tool call; input deltas are forwarded.
    ToolUse,
    /// A server tool call; input deltas are accumulated in `json`.
    ServerToolUse {
        block: WireContentBlock,
        json: String,
    },
    /// A complete server tool result.
    Provider(WireContentBlock),
    /// A block type this crate does not handle; ignored.
    Ignored,
}

/// Turns parsed stream events into port events. See the module docs.
#[derive(Debug)]
pub struct StreamDecoder {
    requested: ModelId,
    usage: WireUsage,
    blocks: HashMap<usize, Block>,
    stop_reason: Option<StopReason>,
    stopped: bool,
}

impl StreamDecoder {
    /// A decoder for an answer of `requested`, the model reported if
    /// `message_start` does not name one.
    #[must_use]
    pub fn new(requested: ModelId) -> Self {
        Self {
            requested,
            usage: WireUsage::default(),
            blocks: HashMap::new(),
            stop_reason: None,
            stopped: false,
        }
    }

    /// Whether `message_stop` was seen.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Decodes one event.
    ///
    /// # Errors
    ///
    /// The typed error of a stream `error` event, or [`LlmError::Decode`]
    /// when the stream breaks the protocol (a delta for a block that never
    /// started, invalid server tool input JSON).
    pub fn decode(&mut self, event: StreamEvent) -> Result<Vec<LlmEvent>, LlmError> {
        Ok(match event {
            StreamEvent::MessageStart { message } => {
                let model = if message.model.is_empty() {
                    self.requested.clone()
                } else {
                    ModelId::new(message.model)
                };
                self.usage = message.usage;
                vec![
                    LlmEvent::MessageStart { model },
                    LlmEvent::Usage(convert::usage(&self.usage)),
                ]
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.start(index, content_block),
            StreamEvent::ContentBlockDelta { index, delta } => self.delta(index, delta)?,
            StreamEvent::ContentBlockStop { index } => self.stop(index)?,
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    self.stop_reason = Some(convert::stop_reason(&reason).unwrap_or_else(|| {
                        tracing::warn!(%reason, "unknown stop_reason; treating it as end_turn");
                        StopReason::EndTurn
                    }));
                }
                match usage {
                    Some(usage) => {
                        merge_usage(&mut self.usage, &usage);
                        vec![LlmEvent::Usage(convert::usage(&self.usage))]
                    }
                    None => Vec::new(),
                }
            }
            StreamEvent::MessageStop => {
                self.stopped = true;
                vec![LlmEvent::MessageStop {
                    stop_reason: self.stop_reason.unwrap_or(StopReason::EndTurn),
                }]
            }
            StreamEvent::Ping => Vec::new(),
            StreamEvent::Error { error } => return Err(crate::error::from_api(None, &error, None)),
            StreamEvent::Unknown(value) => {
                tracing::debug!(kind = ?value.get("type"), "ignoring unknown stream event");
                Vec::new()
            }
        })
    }

    fn start(&mut self, index: usize, block: WireContentBlock) -> Vec<LlmEvent> {
        let mut out = Vec::new();
        let kind = match block {
            WireContentBlock::Text {
                text, citations, ..
            } => {
                out.push(LlmEvent::TextDelta { index, text });
                out.extend(
                    citations
                        .iter()
                        .flatten()
                        .filter_map(convert::citation)
                        .map(|citation| LlmEvent::Citation { index, citation }),
                );
                Block::Plain
            }
            WireContentBlock::Thinking {
                thinking,
                signature,
            } => {
                out.push(LlmEvent::ThinkingDelta { index, thinking });
                if !signature.is_empty() {
                    out.push(LlmEvent::SignatureDelta { index, signature });
                }
                Block::Plain
            }
            WireContentBlock::RedactedThinking { data } => {
                out.push(LlmEvent::RedactedThinking { index, data });
                Block::Plain
            }
            WireContentBlock::ToolUse { id, name, input } => {
                out.push(LlmEvent::ToolUseStart {
                    index,
                    id: ToolUseId::new(id),
                    name,
                });
                if !is_empty_input(&input) {
                    out.push(LlmEvent::ToolInputDelta {
                        index,
                        partial_json: input.to_string(),
                    });
                }
                Block::ToolUse
            }
            block @ WireContentBlock::ServerToolUse { .. } => {
                if let WireContentBlock::ServerToolUse { name, .. } = &block {
                    out.push(LlmEvent::ServerToolUse {
                        index,
                        name: name.clone(),
                    });
                }
                Block::ServerToolUse {
                    block,
                    json: String::new(),
                }
            }
            block @ (WireContentBlock::WebSearchToolResult { .. }
            | WireContentBlock::WebFetchToolResult { .. }) => Block::Provider(block),
            WireContentBlock::ToolResult { .. }
            | WireContentBlock::Image { .. }
            | WireContentBlock::Unknown(_) => {
                tracing::debug!(index, "ignoring content block of an unhandled type");
                Block::Ignored
            }
        };
        self.blocks.insert(index, kind);
        out
    }

    fn delta(&mut self, index: usize, delta: ContentDelta) -> Result<Vec<LlmEvent>, LlmError> {
        let Some(block) = self.blocks.get_mut(&index) else {
            return Err(LlmError::Decode(format!(
                "content_block_delta for unknown block {index}"
            )));
        };
        Ok(match (block, delta) {
            (_, ContentDelta::TextDelta { text }) => vec![LlmEvent::TextDelta { index, text }],
            (_, ContentDelta::ThinkingDelta { thinking }) => {
                vec![LlmEvent::ThinkingDelta { index, thinking }]
            }
            (_, ContentDelta::SignatureDelta { signature }) => {
                vec![LlmEvent::SignatureDelta { index, signature }]
            }
            (Block::ServerToolUse { json, .. }, ContentDelta::InputJsonDelta { partial_json }) => {
                json.push_str(&partial_json);
                Vec::new()
            }
            (Block::ToolUse, ContentDelta::InputJsonDelta { partial_json }) => {
                vec![LlmEvent::ToolInputDelta {
                    index,
                    partial_json,
                }]
            }
            (_, ContentDelta::CitationsDelta { citation }) => convert::citation(&citation)
                .map(|citation| LlmEvent::Citation { index, citation })
                .into_iter()
                .collect(),
            (_, ContentDelta::InputJsonDelta { .. } | ContentDelta::Unknown(_)) => Vec::new(),
        })
    }

    fn stop(&mut self, index: usize) -> Result<Vec<LlmEvent>, LlmError> {
        let Some(block) = self.blocks.remove(&index) else {
            return Err(LlmError::Decode(format!(
                "content_block_stop for unknown block {index}"
            )));
        };
        let raw = match block {
            Block::Plain | Block::ToolUse => return Ok(vec![LlmEvent::BlockStop { index }]),
            Block::Ignored => return Ok(Vec::new()),
            Block::ServerToolUse { mut block, json } => {
                if let WireContentBlock::ServerToolUse { input, .. } = &mut block
                    && !json.trim().is_empty()
                {
                    *input = serde_json::from_str(&json).map_err(|err| {
                        LlmError::Decode(format!(
                            "invalid server tool input (block {index}): {err}"
                        ))
                    })?;
                }
                block
            }
            Block::Provider(block) => block,
        };
        let raw = serde_json::to_value(&raw).map_err(|err| LlmError::Decode(err.to_string()))?;
        Ok(vec![
            LlmEvent::ProviderBlock { index, raw },
            LlmEvent::BlockStop { index },
        ])
    }
}

fn is_empty_input(input: &Value) -> bool {
    match input {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// Applies the counts of a `message_delta` usage on top of `acc`. Counts
/// absent from the delta keep their previous value.
fn merge_usage(acc: &mut WireUsage, delta: &WireUsage) {
    acc.output_tokens = delta.output_tokens;
    if delta.input_tokens.is_some() {
        acc.input_tokens = delta.input_tokens;
    }
    if delta.cache_creation_input_tokens.is_some() {
        acc.cache_creation_input_tokens = delta.cache_creation_input_tokens;
    }
    if delta.cache_read_input_tokens.is_some() {
        acc.cache_read_input_tokens = delta.cache_read_input_tokens;
    }
    if delta.server_tool_use.is_some() {
        acc.server_tool_use = delta.server_tool_use;
    }
}

/// Parses the `data` of one SSE event.
///
/// # Errors
///
/// [`LlmError::Decode`] when the data is not a valid stream event.
pub fn parse_event(data: &str) -> Result<StreamEvent, LlmError> {
    serde_json::from_str(data)
        .map_err(|err| LlmError::Decode(format!("invalid stream event: {err}")))
}

struct State<S> {
    source: S,
    decoder: StreamDecoder,
    pending: VecDeque<Result<LlmEvent, LlmError>>,
    done: bool,
}

/// Decodes the body of a streaming Messages response.
///
/// `bytes` is the raw body (e.g. `reqwest::Response::bytes_stream()`) and
/// `requested` the model the request named. The stream ends after
/// `message_stop` or after its first error: transport failures become
/// [`LlmError::Network`], malformed SSE or JSON [`LlmError::Decode`], and
/// an end of body before `message_stop` [`LlmError::Network`].
pub fn llm_events<S, B, E>(
    bytes: S,
    requested: ModelId,
) -> impl Stream<Item = Result<LlmEvent, LlmError>> + Send + 'static
where
    S: Stream<Item = Result<B, E>> + Send + Unpin + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: Display + Send + 'static,
{
    let state = State {
        source: bytes.eventsource(),
        decoder: StreamDecoder::new(requested),
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
                Some(Ok(event)) => parse_event(&event.data).and_then(|ev| state.decoder.decode(ev)),
                Some(Err(err)) => Err(sse_error(&err)),
                None if state.decoder.is_stopped() => return None,
                None => Err(LlmError::Network(
                    "the stream ended before message_stop".into(),
                )),
            };
            match item {
                Ok(events) => {
                    state.done = state.decoder.is_stopped();
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

#[cfg(test)]
mod tests;
