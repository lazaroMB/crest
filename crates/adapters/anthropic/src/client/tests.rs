use application::ports::llm::LlmEvent;
use futures::StreamExt;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, headers, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::wire::{WireContentBlock, WireMessage, WireRole};

/// Obviously fake key; the tests check it never leaks.
const FAKE_KEY: &str = "sk-ant-fake-0000-never-a-real-key";

fn key() -> ApiKey {
    ApiKey::new(FAKE_KEY).unwrap()
}

fn client(server: &MockServer) -> AnthropicClient {
    let config = ClientConfig {
        base_url: format!("{}/", server.uri()),
        ..ClientConfig::default()
    };
    AnthropicClient::new(&key(), config).unwrap()
}

fn body() -> MessagesRequest {
    let mut req = MessagesRequest::new(
        "claude-haiku-5-5",
        64,
        vec![WireMessage {
            role: WireRole::User,
            content: vec![WireContentBlock::text("Hola")],
        }],
    );
    req.stream = true;
    req
}

fn api_error(kind: &str, message: &str) -> serde_json::Value {
    json!({
        "type": "error",
        "error": {"type": kind, "message": message},
        "request_id": "req_011CSHoEeqs5C35K2UUqR7Fy"
    })
}

#[test]
fn api_key_is_redacted() {
    let key = key();
    assert_eq!(format!("{key:?}"), "ApiKey(<redacted>)");
    assert_eq!(key.to_string(), "<redacted>");
    assert!(matches!(ApiKey::new("  "), Err(LlmError::Auth { .. })));
    assert_eq!(ApiKey::new(format!(" {FAKE_KEY}\n")).unwrap(), key);
}

#[test]
fn client_debug_does_not_show_the_key() {
    let client = AnthropicClient::new(&key(), ClientConfig::default()).unwrap();
    assert!(!format!("{client:?}").contains(FAKE_KEY));
}

#[tokio::test]
async fn sends_auth_and_version_headers_and_streams_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", FAKE_KEY))
        .and(header("anthropic-version", ANTHROPIC_VERSION))
        .and(body_partial_json(
            json!({"model": "claude-haiku-5-5", "stream": true}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(include_str!("../../tests/fixtures/text.sse")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let stream = client(&server).stream_messages(&body()).await.unwrap();
    let events: Vec<_> = stream.collect().await;
    assert!(matches!(
        events.first(),
        Some(Ok(LlmEvent::MessageStart { model })) if model.as_str() == "claude-opus-5-5"
    ));
    assert!(matches!(
        events.last(),
        Some(Ok(LlmEvent::MessageStop { .. }))
    ));
}

#[tokio::test]
async fn betas_go_in_the_beta_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(headers(
            "anthropic-beta",
            vec!["a-2026-01-01", "b-2026-02-02"],
        ))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    let config = ClientConfig {
        base_url: server.uri(),
        betas: vec!["a-2026-01-01".into(), "b-2026-02-02".into()],
        ..ClientConfig::default()
    };
    let client = AnthropicClient::new(&key(), config).unwrap();
    let value: serde_json::Value = client
        .get_json("/v1/models", &[("limit", "2".into())])
        .await
        .unwrap();
    assert_eq!(value, json!({"ok": true}));
}

#[tokio::test]
async fn unauthorized_gives_a_clear_error_without_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(api_error("authentication_error", "invalid x-api-key")),
        )
        .mount(&server)
        .await;
    let Err(err) = client(&server).stream_messages(&body()).await else {
        panic!("401 must fail");
    };
    let LlmError::Auth { message } = &err else {
        panic!("expected Auth, got {err:?}");
    };
    assert!(message.starts_with("ANTHROPIC_API_KEY is invalid or missing;"));
    assert!(!err.to_string().contains(FAKE_KEY));
    assert!(!format!("{err:?}").contains(FAKE_KEY));
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn a_rejected_key_names_where_it_came_from() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(api_error("authentication_error", "invalid x-api-key")),
        )
        .mount(&server)
        .await;
    let config = || ClientConfig {
        base_url: format!("{}/", server.uri()),
        ..ClientConfig::default()
    };
    for (source, expected) in [
        (KeySource::Env, "ANTHROPIC_API_KEY variable"),
        (KeySource::Keyring, "system keyring"),
    ] {
        let key = ApiKey::from(FAKE_KEY.to_owned(), source).unwrap();
        assert_eq!(key.source(), source);
        let client = AnthropicClient::new(&key, config()).unwrap();
        let Err(LlmError::Auth { message }) = client.stream_messages(&body()).await else {
            panic!("401 must be an Auth error");
        };
        assert!(message.contains(expected), "{source:?}: {message}");
        assert!(message.contains("rejected"), "{message}");
        assert!(!message.contains(FAKE_KEY));
    }
}

#[tokio::test]
async fn http_errors_are_typed() {
    let server = MockServer::start().await;
    Mock::given(path("/429"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "2")
                .set_body_json(api_error("rate_limit_error", "Number of requests exceeded")),
        )
        .mount(&server)
        .await;
    Mock::given(path("/529"))
        .respond_with(
            ResponseTemplate::new(529).set_body_json(api_error("overloaded_error", "Overloaded")),
        )
        .mount(&server)
        .await;
    Mock::given(path("/502"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .mount(&server)
        .await;
    let client = client(&server);
    let get = |p: &'static str| {
        let client = client.clone();
        async move { client.get_json::<serde_json::Value>(p, &[]).await }
    };
    assert_eq!(
        get("/429").await,
        Err(LlmError::RateLimited {
            retry_after_ms: Some(2000)
        })
    );
    assert_eq!(get("/529").await, Err(LlmError::Overloaded));
    assert!(matches!(
        get("/502").await,
        Err(LlmError::Server { status: 502, .. })
    ));
}

#[tokio::test]
async fn connection_refused_is_a_network_error() {
    // A port nobody listens on (MockServers are pooled, so do not reuse one).
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let config = ClientConfig {
        base_url: format!("http://{addr}"),
        ..ClientConfig::default()
    };
    let client = AnthropicClient::new(&key(), config).unwrap();
    let err = client.stream_messages(&body()).await.err();
    assert!(matches!(err, Some(LlmError::Network(_))), "{err:?}");
}

#[test]
fn retry_after_parsing() {
    let mut headers = HeaderMap::new();
    assert_eq!(retry_after_ms(&headers), None);
    headers.insert("retry-after", HeaderValue::from_static("1.5"));
    assert_eq!(retry_after_ms(&headers), Some(1500));
    headers.insert(
        "retry-after",
        HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
    );
    assert_eq!(retry_after_ms(&headers), None);
}
