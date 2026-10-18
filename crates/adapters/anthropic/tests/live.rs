//! Tests against the real Anthropic API.
//!
//! Ignored by default and run with `make test-live`
//! (`ANTHROPIC_LIVE_TEST=1 cargo test --workspace -- --ignored live`).
//! They need `ANTHROPIC_API_KEY` in the environment or the key in the
//! system keyring; without `ANTHROPIC_LIVE_TEST=1` or without a key they
//! print why and pass. They spend a few cents on `claude-haiku-5-5`.

use adapter_anthropic::{AnthropicLlm, ClientConfig};
use application::ports::llm::{LlmPort, LlmRequest, MessageAssembler, SystemBlock, ToolDefinition};
use domain::conversation::{ContentPart, Message, StopReason};
use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
use futures::StreamExt;
use serde_json::json;

const MODEL: &str = "claude-haiku-5-5";

/// The adapter, or `None` (with the reason printed) when the test must be
/// skipped.
async fn live_llm() -> Option<AnthropicLlm> {
    if std::env::var("ANTHROPIC_LIVE_TEST").as_deref() != Ok("1") {
        eprintln!("skipping: set ANTHROPIC_LIVE_TEST=1 to run live tests");
        return None;
    }
    let built = tokio::task::spawn_blocking(|| AnthropicLlm::from_env(ClientConfig::default()))
        .await
        .ok()?;
    match built {
        Ok(llm) => Some(llm),
        Err(err) => {
            eprintln!("skipping: no API key available ({err})");
            None
        }
    }
}

fn request(messages: Vec<Message>) -> LlmRequest {
    let model = ResolvedModel {
        role: ModelRole::Explore,
        spec: ModelSpec::new(MODEL, 1024),
        source: SpecSource::Default,
        warnings: vec![],
    };
    LlmRequest::new(model, messages)
}

/// Streams `req` and assembles the answer.
async fn run(
    llm: &AnthropicLlm,
    req: LlmRequest,
) -> Result<application::ports::llm::AssembledMessage, String> {
    let mut stream = llm.stream(req).await.map_err(|e| e.to_string())?;
    let mut asm = MessageAssembler::new();
    while let Some(event) = stream.next().await {
        asm.push(event.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    asm.finish().map_err(|e| e.to_string())
}

#[tokio::test]
#[ignore = "calls the real API; run with `make test-live`"]
async fn live_short_text_answer() {
    let Some(llm) = live_llm().await else {
        return;
    };
    let mut req = request(vec![Message::user_text(
        "Reply with exactly the word: pong",
    )]);
    req.system = vec![SystemBlock::new("Answer in one word, no punctuation.")];
    let out = run(&llm, req).await.unwrap();
    assert_eq!(out.stop_reason, StopReason::EndTurn);
    assert!(out.model.is_some_and(|m| m.as_str().starts_with("claude-")));
    let text: String = out
        .message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.to_lowercase().contains("pong"), "answer: {text:?}");
    assert!(out.usage.input_tokens > 0 && out.usage.output_tokens > 0);
}

#[tokio::test]
#[ignore = "calls the real API; run with `make test-live`"]
async fn live_simple_tool_use() {
    let Some(llm) = live_llm().await else {
        return;
    };
    let mut req = request(vec![Message::user_text(
        "What is the weather in Paris? Use the get_weather tool.",
    )]);
    req.tools = vec![ToolDefinition::new(
        "get_weather",
        "Current weather for a city.",
        json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
    )];
    let out = run(&llm, req).await.unwrap();
    assert_eq!(out.stop_reason, StopReason::ToolUse);
    let input = out.message.content.iter().find_map(|part| match part {
        ContentPart::ToolUse { name, input, .. } if name == "get_weather" => Some(input),
        _ => None,
    });
    let city = input
        .and_then(|input| input["city"].as_str())
        .unwrap_or_default();
    assert!(
        city.to_lowercase().contains("paris"),
        "tool input: {input:?}"
    );
}
