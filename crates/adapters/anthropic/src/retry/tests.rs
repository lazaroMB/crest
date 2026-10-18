use std::sync::Mutex;

use application::ports::llm::MessageAssembler;
use domain::conversation::{ContentPart, Message};
use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

use super::*;

const TEXT: &str = include_str!("../../tests/fixtures/text.sse");
const ERROR_MID_STREAM: &str = include_str!("../../tests/fixtures/error_mid_stream.sse");

/// Records every wait and returns at once; jitter is always 1 (full delay).
#[derive(Default)]
struct FakeClock {
    sleeps: Mutex<Vec<Duration>>,
}

impl Clock for FakeClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.sleeps.lock().unwrap().push(duration);
        Box::pin(std::future::ready(()))
    }

    fn jitter(&self) -> f64 {
        1.0
    }
}

struct Setup {
    server: MockServer,
    clock: Arc<FakeClock>,
    llm: AnthropicLlm,
}

async fn setup(max_attempts: u32) -> Setup {
    let server = MockServer::start().await;
    let config = ClientConfig {
        base_url: server.uri(),
        ..ClientConfig::default()
    };
    let key = ApiKey::new("sk-ant-fake-retry-tests").unwrap();
    let clock = Arc::new(FakeClock::default());
    let llm = AnthropicLlm::new(AnthropicClient::new(&key, config).unwrap())
        .with_policy(RetryPolicy {
            max_attempts,
            ..RetryPolicy::default()
        })
        .with_clock(clock.clone());
    Setup { server, clock, llm }
}

fn request(primary: &str, fallbacks: &[&str]) -> LlmRequest {
    let spec = ModelSpec::new(primary, 256).with_fallbacks(fallbacks.iter().copied());
    let model = ResolvedModel {
        role: ModelRole::Main,
        spec,
        source: SpecSource::Default,
        warnings: vec![],
    };
    LlmRequest::new(model, vec![Message::user_text("Hola")])
}

/// The recorded text stream, answered by `model`.
fn ok_from(model: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(TEXT.replace("claude-opus-5-5", model))
}

fn error(status: u16, kind: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "type": "error",
        "error": {"type": kind, "message": kind}
    }))
}

fn for_model(model: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({"model": model, "stream": true})))
}

async fn collect(llm: &AnthropicLlm, req: LlmRequest) -> Vec<Result<LlmEvent, LlmError>> {
    llm.stream(req).await.unwrap().collect().await
}

/// The events before the first `MessageStart`.
fn prelude(items: &[Result<LlmEvent, LlmError>]) -> Vec<LlmEvent> {
    items
        .iter()
        .map(|item| item.clone().unwrap())
        .take_while(|event| !matches!(event, LlmEvent::MessageStart { .. }))
        .collect()
}

fn assemble(items: Vec<Result<LlmEvent, LlmError>>) -> Vec<ContentPart> {
    let mut asm = MessageAssembler::new();
    for item in items {
        asm.push(item.unwrap()).unwrap();
    }
    asm.finish().unwrap().message.content
}

#[tokio::test]
async fn rate_limit_waits_for_retry_after_then_succeeds() {
    let Setup { server, clock, llm } = setup(3).await;
    for_model("claude-haiku-5-5")
        .respond_with(error(429, "rate_limit_error").insert_header("retry-after", "3"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    for_model("claude-haiku-5-5")
        .respond_with(ok_from("claude-haiku-5-5"))
        .mount(&server)
        .await;
    let items = collect(&llm, request("claude-haiku-5-5", &["claude-sonnet-5-5"])).await;
    assert_eq!(
        prelude(&items),
        vec![LlmEvent::Retrying {
            attempt: 1,
            delay_ms: 3000,
            reason: LlmError::RateLimited {
                retry_after_ms: Some(3000)
            },
        }]
    );
    assert_eq!(*clock.sleeps.lock().unwrap(), vec![Duration::from_secs(3)]);
    assert_eq!(assemble(items), vec![ContentPart::text("Hello!")]);
}

#[tokio::test]
async fn overloaded_retries_with_backoff_then_falls_back() {
    let Setup { server, clock, llm } = setup(3).await;
    for_model("claude-opus-5-5")
        .respond_with(error(529, "overloaded_error"))
        .expect(3)
        .mount(&server)
        .await;
    for_model("claude-sonnet-5-5")
        .respond_with(ok_from("claude-sonnet-5-5"))
        .expect(1)
        .mount(&server)
        .await;
    let items = collect(&llm, request("claude-opus-5-5", &["claude-sonnet-5-5"])).await;
    let retry = |attempt, delay_ms| LlmEvent::Retrying {
        attempt,
        delay_ms,
        reason: LlmError::Overloaded,
    };
    assert_eq!(
        prelude(&items),
        vec![
            retry(1, 500),
            retry(2, 1000),
            LlmEvent::ModelFallback {
                from: ModelId::new("claude-opus-5-5"),
                to: ModelId::new("claude-sonnet-5-5"),
                reason: LlmError::Overloaded,
            },
        ]
    );
    assert_eq!(
        *clock.sleeps.lock().unwrap(),
        vec![Duration::from_millis(500), Duration::from_millis(1000)]
    );
    assert!(items.iter().any(|item| matches!(
        item,
        Ok(LlmEvent::MessageStart { model }) if model.as_str() == "claude-sonnet-5-5"
    )));
    assert_eq!(assemble(items), vec![ContentPart::text("Hello!")]);
}

#[tokio::test]
async fn server_errors_on_every_model_end_in_all_models_failed() {
    let Setup { server, llm, .. } = setup(2).await;
    Mock::given(method("POST"))
        .respond_with(error(500, "api_error"))
        .expect(4)
        .mount(&server)
        .await;
    let items = collect(&llm, request("claude-opus-5-5", &["claude-sonnet-5-5"])).await;
    let last = LlmError::Server {
        status: 500,
        message: "api_error".into(),
    };
    assert_eq!(items.len(), 4, "{items:?}");
    assert!(matches!(
        items[0],
        Ok(LlmEvent::Retrying { attempt: 1, .. })
    ));
    assert!(matches!(items[1], Ok(LlmEvent::ModelFallback { .. })));
    assert!(matches!(
        items[2],
        Ok(LlmEvent::Retrying { attempt: 1, .. })
    ));
    assert_eq!(
        items[3],
        Err(LlmError::AllModelsFailed {
            last: Box::new(last)
        })
    );
}

#[tokio::test]
async fn gateway_error_is_retried_on_the_same_model() {
    let Setup { server, llm, .. } = setup(3).await;
    for_model("claude-haiku-5-5")
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    for_model("claude-haiku-5-5")
        .respond_with(ok_from("claude-haiku-5-5"))
        .mount(&server)
        .await;
    let items = collect(&llm, request("claude-haiku-5-5", &[])).await;
    assert!(matches!(
        prelude(&items).as_slice(),
        [LlmEvent::Retrying {
            attempt: 1,
            reason: LlmError::Server { status: 503, .. },
            ..
        }]
    ));
    assert_eq!(assemble(items), vec![ContentPart::text("Hello!")]);
}

#[tokio::test]
async fn unauthorized_is_not_retried_and_is_returned_directly() {
    let Setup { server, clock, llm } = setup(3).await;
    Mock::given(method("POST"))
        .respond_with(error(401, "authentication_error"))
        .expect(1)
        .mount(&server)
        .await;
    let err = llm
        .stream(request("claude-haiku-5-5", &["claude-sonnet-5-5"]))
        .await
        .err();
    assert!(matches!(err, Some(LlmError::Auth { .. })), "{err:?}");
    assert!(clock.sleeps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn error_event_before_content_is_retried() {
    let Setup { server, llm, .. } = setup(3).await;
    let start = TEXT.split("event: content_block_start").next().unwrap();
    let overloaded = format!(
        "{start}event: error\ndata: {{\"type\": \"error\", \"error\": {{\"type\": \"overloaded_error\", \"message\": \"Overloaded\"}}}}\n\n"
    );
    for_model("claude-haiku-5-5")
        .respond_with(ResponseTemplate::new(200).set_body_string(overloaded))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    for_model("claude-haiku-5-5")
        .respond_with(ok_from("claude-haiku-5-5"))
        .mount(&server)
        .await;
    let items = collect(&llm, request("claude-haiku-5-5", &[])).await;
    // The held-back MessageStart of the failed attempt is never emitted.
    assert_eq!(
        items
            .iter()
            .filter(|item| matches!(item, Ok(LlmEvent::MessageStart { .. })))
            .count(),
        1
    );
    assert!(matches!(items[0], Ok(LlmEvent::Retrying { .. })));
    assert_eq!(assemble(items), vec![ContentPart::text("Hello!")]);
}

#[tokio::test]
async fn error_after_content_is_not_retried() {
    let Setup { server, clock, llm } = setup(3).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ERROR_MID_STREAM))
        .expect(1)
        .mount(&server)
        .await;
    let items = collect(&llm, request("claude-haiku-5-5", &["claude-sonnet-5-5"])).await;
    assert!(matches!(
        items.first(),
        Some(Ok(LlmEvent::MessageStart { .. }))
    ));
    assert_eq!(items.last(), Some(&Err(LlmError::Overloaded)));
    assert!(clock.sleeps.lock().unwrap().is_empty());
}

#[test]
fn backoff_is_exponential_capped_and_jittered() {
    let policy = RetryPolicy {
        max_attempts: 10,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(1000),
        max_retry_after: Duration::from_secs(5),
    };
    let err = LlmError::Overloaded;
    assert_eq!(policy.delay(1, &err, 1.0), Duration::from_millis(100));
    assert_eq!(policy.delay(3, &err, 1.0), Duration::from_millis(400));
    assert_eq!(policy.delay(9, &err, 1.0), Duration::from_millis(1000));
    assert_eq!(policy.delay(3, &err, 0.0), Duration::from_millis(200));
    let limited = LlmError::RateLimited {
        retry_after_ms: Some(60_000),
    };
    assert_eq!(policy.delay(1, &limited, 0.0), Duration::from_secs(5));
    let jitter = TokioClock.jitter();
    assert!((0.0..=1.0).contains(&jitter));
}
