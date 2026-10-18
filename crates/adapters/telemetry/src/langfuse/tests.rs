use std::collections::HashMap;

use application::ports::telemetry::{LlmCallRecord, TelemetryPort, ToolCallRecord, TurnSpan};
use base64::Engine as _;
use domain::conversation::{SessionId, StopReason, TurnId, Usage};
use domain::model::{ModelId, ModelRole};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::TracingTelemetry;

// Obviously fake values: no real credentials in tests.
const PUBLIC: &str = "public-test-value";
const SECRET: &str = "secret-test-value";

fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| vars.get(name).cloned()
}

fn config(host: &str) -> LangfuseConfig {
    LangfuseConfig {
        host: host.to_owned(),
        public_key: PUBLIC.to_owned(),
        secret_key: SECRET.to_owned(),
    }
}

#[test]
fn config_is_off_without_keys() {
    assert_eq!(LangfuseConfig::from_lookup(lookup(&[])).unwrap(), None);
    let blank = lookup(&[(PUBLIC_KEY_ENV, " "), (SECRET_KEY_ENV, "")]);
    assert_eq!(LangfuseConfig::from_lookup(blank).unwrap(), None);
}

#[test]
fn config_reads_keys_and_defaults_the_host() {
    let vars = lookup(&[(PUBLIC_KEY_ENV, PUBLIC), (SECRET_KEY_ENV, SECRET)]);
    let config = LangfuseConfig::from_lookup(vars).unwrap().unwrap();
    assert_eq!(config, self::config(DEFAULT_HOST));

    let vars = lookup(&[
        (HOST_ENV, "http://localhost:3000"),
        (PUBLIC_KEY_ENV, PUBLIC),
        (SECRET_KEY_ENV, SECRET),
    ]);
    let config = LangfuseConfig::from_lookup(vars).unwrap().unwrap();
    assert_eq!(config.host, "http://localhost:3000");
}

#[test]
fn config_with_one_key_is_an_error() {
    let err = LangfuseConfig::from_lookup(lookup(&[(PUBLIC_KEY_ENV, PUBLIC)])).unwrap_err();
    assert!(matches!(err, LangfuseError::MissingVar(SECRET_KEY_ENV)));
    let err = LangfuseConfig::from_lookup(lookup(&[(SECRET_KEY_ENV, SECRET)])).unwrap_err();
    assert!(matches!(err, LangfuseError::MissingVar(PUBLIC_KEY_ENV)));
}

#[test]
fn debug_hides_the_keys() {
    let debug = format!("{:?}", config("http://localhost:3000"));
    assert!(debug.contains("http://localhost:3000"));
    assert!(!debug.contains(PUBLIC), "{debug}");
    assert!(!debug.contains(SECRET), "{debug}");
}

#[test]
fn init_outside_tokio_is_an_error() {
    let err = init_langfuse(&config("http://localhost:3000")).unwrap_err();
    assert!(matches!(err, LangfuseError::NoRuntime));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn building_the_exporter_sends_nothing() {
    let server = MockServer::start().await;
    let exporter = init_langfuse(&config(&server.uri())).unwrap();
    tokio::task::spawn_blocking(move || exporter.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exports_telemetry_spans_with_genai_attributes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/public/otel/v1/traces"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let exporter = init_langfuse(&config(&server.uri())).unwrap();

    let subscriber = Registry::default().with(exporter.layer());
    tracing::subscriber::with_default(subscriber, || {
        let _noise = tracing::info_span!(target: "other", "unrelated_span").entered();
        let telemetry = TracingTelemetry::new();
        let span = telemetry.turn_started(TurnSpan {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            role: ModelRole::Main,
            model: ModelId::new("claude-sonnet-5-5"),
        });
        telemetry.llm_call(LlmCallRecord {
            parent: span,
            model: ModelId::new("claude-sonnet-5-5"),
            usage: Usage {
                input_tokens: 12,
                output_tokens: 3,
                ..Usage::default()
            },
            latency_ms: 40,
            error: None,
        });
        telemetry.tool_call(ToolCallRecord {
            parent: span,
            name: "grep".into(),
            duration_ms: 2,
            ok: true,
        });
        telemetry.turn_finished(span, Usage::default(), Some(StopReason::EndTurn));
    });
    tokio::task::spawn_blocking(move || exporter.shutdown())
        .await
        .unwrap()
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert!(!requests.is_empty());
    let expected_auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{PUBLIC}:{SECRET}"))
    );
    let body: Vec<u8> = requests.iter().flat_map(|r| r.body.clone()).collect();
    for request in &requests {
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            expected_auth.as_str()
        );
    }
    let contains = |needle: &str| body.windows(needle.len()).any(|w| w == needle.as_bytes());
    for needle in [
        "chat claude-sonnet-5-5",
        "execute_tool grep",
        "gen_ai.request.model",
        "gen_ai.usage.input_tokens",
        "session.id",
        "crest",
    ] {
        assert!(contains(needle), "missing {needle}");
    }
    assert!(!contains("unrelated_span"));
    assert!(!contains(SECRET));
}
