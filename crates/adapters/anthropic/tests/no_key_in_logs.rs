//! The API key never reaches the logs, even at TRACE level and including
//! the logs of reqwest/hyper. Own test binary so the global subscriber
//! sees every callsite.

use std::io::Write;
use std::sync::{Arc, Mutex};

use adapter_anthropic::client::{AnthropicClient, ApiKey, ClientConfig};
use adapter_anthropic::wire::{MessagesRequest, WireContentBlock, WireMessage, WireRole};
use futures::StreamExt;
use wiremock::matchers::{header, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Obviously fake key; the test checks it never leaks.
const FAKE_KEY: &str = "sk-ant-fake-0000-never-a-real-key";

/// Shared buffer that collects everything the subscriber writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn the_key_never_reaches_the_logs() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let server = MockServer::start().await;
    Mock::given(path("/v1/messages"))
        .and(header("x-api-key", FAKE_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_string(include_str!("fixtures/text.sse")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid x-api-key"}
        })))
        .mount(&server)
        .await;
    let config = ClientConfig {
        base_url: server.uri(),
        ..ClientConfig::default()
    };
    let key = ApiKey::new(FAKE_KEY).unwrap();
    let client = AnthropicClient::new(&key, config).unwrap();
    let mut body = MessagesRequest::new(
        "claude-haiku-5-5",
        64,
        vec![WireMessage {
            role: WireRole::User,
            content: vec![WireContentBlock::text("Hola")],
        }],
    );
    body.stream = true;

    let ok: Vec<_> = client.stream_messages(&body).await.unwrap().collect().await;
    assert!(ok.iter().all(Result::is_ok));
    let err = client.stream_messages(&body).await.err();
    tracing::info!(client = ?client, key = ?key, error = ?err, "after the calls");

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("POST /v1/messages"), "{logs}");
    assert!(logs.contains("Anthropic API error"), "{logs}");
    assert!(logs.contains("authentication failed"), "{logs}");
    assert!(
        !logs.contains(FAKE_KEY),
        "the key leaked into the logs:\n{logs}"
    );
}
