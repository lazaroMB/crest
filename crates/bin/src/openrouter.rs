//! Checking an OpenRouter API key.
//!
//! The key is stored under `OPENROUTER_API_KEY` and asked against
//! `GET https://openrouter.ai/api/v1/models`. Model calls do not use it yet.

use adapter_openai::{OpenAiCatalog, OpenAiClient, OpenAiConfig};
use application::ports::credentials::KeyCheck;
use application::ports::llm::{LlmError, ModelCatalogPort};

/// OpenRouter's OpenAI-compatible base URL, including the version.
const BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Asks OpenRouter whether `key` works. A rejected key is not stored; a
/// key that cannot be checked (no network) is stored with a warning.
pub async fn check_key(key: &str) -> KeyCheck {
    let config = OpenAiConfig {
        base_url: BASE_URL.to_owned(),
        connect_timeout: adapter_anthropic::credentials::CHECK_TIMEOUT,
        read_timeout: adapter_anthropic::credentials::CHECK_TIMEOUT,
        user_agent: Some(format!("crest/{}", env!("CARGO_PKG_VERSION"))),
        provider: "OpenRouter".to_owned(),
        ..OpenAiConfig::default()
    };
    let client = match OpenAiClient::new(key, config) {
        Ok(client) => client,
        Err(_) => return KeyCheck::Malformed("paste your OpenRouter API key".into()),
    };
    let catalog = OpenAiCatalog::new(client);
    let timeout = adapter_anthropic::credentials::CHECK_TIMEOUT;
    match tokio::time::timeout(timeout, catalog.list_models()).await {
        Ok(Ok(_)) => KeyCheck::Valid,
        Ok(Err(LlmError::Auth { .. })) => KeyCheck::Rejected(
            "OpenRouter rejected this key; check that you copied the whole key".into(),
        ),
        Ok(Err(err)) => KeyCheck::Unreachable(err.to_string()),
        Err(_) => KeyCheck::Unreachable("no answer from OpenRouter".into()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn check_against(server: &MockServer, key: &str) -> KeyCheck {
        let config = OpenAiConfig {
            base_url: format!("{}/api/v1", server.uri()),
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_secs(2),
            provider: "OpenRouter".into(),
            ..OpenAiConfig::default()
        };
        let client = OpenAiClient::new(key, config).unwrap();
        let catalog = OpenAiCatalog::new(client);
        match catalog.list_models().await {
            Ok(_) => KeyCheck::Valid,
            Err(LlmError::Auth { .. }) => KeyCheck::Rejected("rejected".into()),
            Err(err) => KeyCheck::Unreachable(err.to_string()),
        }
    }

    #[tokio::test]
    async fn a_listed_catalog_accepts_the_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .and(header("authorization", "Bearer sk-or-good"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "id": "openai/gpt-4o", "name": "GPT-4o" }]
            })))
            .mount(&server)
            .await;
        assert_eq!(check_against(&server, "sk-or-good").await, KeyCheck::Valid);
    }

    #[tokio::test]
    async fn a_rejected_key_is_not_valid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": { "message": "invalid api key" }
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            check_against(&server, "sk-or-bad").await,
            KeyCheck::Rejected(_)
        ));
    }
}
