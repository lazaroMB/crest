//! Decoding of recorded streams (`tests/fixtures/*.sse`, copied from the
//! official streaming docs).

use application::ports::llm::{AssembledMessage, Citation, MessageAssembler};
use domain::conversation::{ContentPart, Usage};
use futures::executor::block_on;
use serde_json::json;

use super::*;

const TEXT: &str = include_str!("../../tests/fixtures/text.sse");
const TOOL_USE: &str = include_str!("../../tests/fixtures/tool_use.sse");
const THINKING: &str = include_str!("../../tests/fixtures/thinking.sse");
const WEB_SEARCH: &str = include_str!("../../tests/fixtures/web_search.sse");
const ERROR_MID_STREAM: &str = include_str!("../../tests/fixtures/error_mid_stream.sse");

/// Decodes `sse` delivered in chunks of `chunk` bytes, to exercise events
/// split across network reads (even inside a UTF-8 character).
fn decode_chunked(sse: &str, chunk: usize) -> Vec<Result<LlmEvent, LlmError>> {
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = sse
        .as_bytes()
        .chunks(chunk)
        .map(|c| Ok(c.to_vec()))
        .collect();
    let source = futures::stream::iter(chunks);
    block_on(llm_events(source, ModelId::new("claude-requested")).collect())
}

fn decode(sse: &str) -> Vec<LlmEvent> {
    let events: Result<Vec<_>, _> = decode_chunked(sse, 7).into_iter().collect();
    let events = events.unwrap();
    // Chunking must not change the result.
    let whole: Vec<_> = decode_chunked(sse, sse.len())
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(events, whole);
    events
}

fn assemble(events: &[LlmEvent]) -> AssembledMessage {
    let mut asm = MessageAssembler::new();
    for event in events {
        asm.push(event.clone()).unwrap();
    }
    asm.finish().unwrap()
}

#[test]
fn text_stream() {
    let events = decode(TEXT);
    insta::assert_debug_snapshot!(events);
    let out = assemble(&events);
    assert_eq!(out.message.content, vec![ContentPart::text("Hello!")]);
    assert_eq!(out.model, Some(ModelId::new("claude-opus-5-5")));
    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert_eq!(
        out.usage,
        Usage {
            input_tokens: 25,
            output_tokens: 15,
            ..Usage::default()
        }
    );
}

#[test]
fn tool_use_stream() {
    let events = decode(TOOL_USE);
    insta::assert_debug_snapshot!(events);
    let out = assemble(&events);
    assert_eq!(
        out.message.content,
        vec![
            ContentPart::text("Okay, let's check the weather for San Francisco, CA:"),
            ContentPart::ToolUse {
                id: ToolUseId::new("toolu_01T1x1fJ34qAmk2tNTrN7Up6"),
                name: "get_weather".into(),
                input: json!({"location": "San Francisco, CA"}),
            },
        ]
    );
    assert_eq!(out.stop_reason, StopReason::ToolUse);
    assert_eq!(out.usage.input_tokens, 472);
    assert_eq!(out.usage.output_tokens, 89);
}

#[test]
fn thinking_stream() {
    let events = decode(THINKING);
    insta::assert_debug_snapshot!(events);
    let out = assemble(&events);
    let [
        ContentPart::Thinking {
            thinking,
            signature,
        },
        ContentPart::Text { text },
    ] = out.message.content.as_slice()
    else {
        panic!("unexpected content {:?}", out.message.content);
    };
    assert!(thinking.starts_with("I need to find the GCD"));
    assert!(thinking.ends_with("GCD(1071, 462) = 21."));
    assert_eq!(
        signature.as_deref(),
        Some("EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds...")
    );
    assert_eq!(
        text,
        "The greatest common divisor of 1071 and 462 is **21**."
    );
    // No usage in message_start nor message_delta: zero, not an error.
    assert_eq!(out.usage, Usage::default());
}

#[test]
fn web_search_stream_with_citations() {
    let events = decode(WEB_SEARCH);
    insta::assert_debug_snapshot!(events);
    let out = assemble(&events);
    let content = &out.message.content;
    assert_eq!(content.len(), 4);
    assert_eq!(
        content[0],
        ContentPart::text("I'll check the current weather in New York City for you.")
    );
    assert_eq!(
        content[1],
        ContentPart::ProviderBlock {
            raw: json!({
                "type": "server_tool_use",
                "id": "srvtoolu_014hJH82Qum7Td6UV8gDXThB",
                "name": "web_search",
                "input": {"query": "weather NYC today"}
            })
        }
    );
    let ContentPart::ProviderBlock { raw } = &content[2] else {
        panic!("expected the search result, got {:?}", content[2]);
    };
    assert_eq!(raw["type"], "web_search_tool_result");
    assert_eq!(
        raw["content"][0]["encrypted_content"],
        "Ev0DCioIAxgCIiQ3NmU4ZmI4OC1k..."
    );
    assert!(
        matches!(&content[3], ContentPart::Text { text } if text.starts_with("Here's the current weather"))
    );
    assert_eq!(
        out.citations,
        vec![Citation {
            url: "https://world-weather.info/forecast/usa/new_york/may-2025/".into(),
            title: Some(
                "Weather in New York City in May 2025 (New York) - detailed Weather Forecast for a month"
                    .into()
            ),
            cited_text: "Weather in New York City in May 2025".into(),
        }]
    );
    assert_eq!(
        out.usage,
        Usage {
            input_tokens: 10682,
            output_tokens: 510,
            ..Usage::default()
        }
    );
}

#[test]
fn error_mid_stream_is_typed_and_ends_the_stream() {
    let items = decode_chunked(ERROR_MID_STREAM, 5);
    let (last, events) = items.split_last().unwrap();
    assert_eq!(last, &Err(LlmError::Overloaded));
    assert!(events.iter().all(Result::is_ok));
    assert!(matches!(
        events.last(),
        Some(Ok(LlmEvent::TextDelta { text, .. })) if text == "Hello"
    ));
}

#[test]
fn truncated_stream_is_a_network_error() {
    let cut = TEXT.find("event: message_delta").unwrap();
    let items = decode_chunked(&TEXT[..cut], 64);
    assert!(matches!(items.last(), Some(Err(LlmError::Network(_)))));
}

#[test]
fn transport_error_is_a_network_error() {
    let source = futures::stream::iter(vec![
        Ok(TEXT.as_bytes()[..40].to_vec()),
        Err("connection reset"),
    ]);
    let items: Vec<_> = block_on(llm_events(source, ModelId::new("m")).collect());
    assert_eq!(
        items,
        vec![Err(LlmError::Network("connection reset".into()))]
    );
}

#[test]
fn invalid_json_is_a_decode_error() {
    let items = decode_chunked("event: message_start\ndata: {not json}\n\n", 100);
    assert!(matches!(items.as_slice(), [Err(LlmError::Decode(_))]));
}

#[test]
fn delta_for_unknown_block_is_a_decode_error() {
    let mut decoder = StreamDecoder::new(ModelId::new("m"));
    let event = parse_event(
        r#"{"type":"content_block_delta","index":4,"delta":{"type":"text_delta","text":"x"}}"#,
    )
    .unwrap();
    assert!(matches!(decoder.decode(event), Err(LlmError::Decode(_))));
}

#[test]
fn unknown_blocks_and_events_are_ignored() {
    let mut decoder = StreamDecoder::new(ModelId::new("m"));
    for data in [
        r#"{"type":"future_event","x":1}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"container_upload","file_id":"f"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"future_delta"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
    ] {
        assert_eq!(decoder.decode(parse_event(data).unwrap()), Ok(vec![]));
    }
}

#[test]
fn message_start_without_model_reports_the_requested_one() {
    let mut decoder = StreamDecoder::new(ModelId::new("claude-haiku-5-5"));
    let event = parse_event(r#"{"type":"message_start","message":{"id":"msg_1"}}"#).unwrap();
    assert_eq!(
        decoder.decode(event).unwrap()[0],
        LlmEvent::MessageStart {
            model: ModelId::new("claude-haiku-5-5")
        }
    );
}
