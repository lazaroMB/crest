//! [`ApiKeyPort`] over the Anthropic API and the system keyring: the entry
//! [`ApiKey::from_env_or_keyring`] reads (service `crest`, entry
//! `ANTHROPIC_API_KEY`).

use std::time::Duration;

use application::ports::credentials::{ApiKeyPort, CredentialError, KeyCheck};
use application::ports::llm::{LlmError, ModelCatalogPort};
use async_trait::async_trait;

use crate::catalog::AnthropicCatalog;
use crate::client::{
    API_KEY_ENV, AnthropicClient, ApiKey, ClientConfig, KEYRING_SERVICE, KEYRING_USER,
};

/// Longest wait for the check of a new key.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Checks keys with `GET /v1/models` and stores them in the keyring.
#[derive(Debug, Clone)]
pub struct KeyringCredentials {
    config: ClientConfig,
}

impl Default for KeyringCredentials {
    fn default() -> Self {
        Self::new(ClientConfig::default())
    }
}

impl KeyringCredentials {
    /// Checks against the API of `config` (tests point it at a mock).
    #[must_use]
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }
}

fn entry() -> Result<keyring::Entry, CredentialError> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|err| CredentialError(err.to_string()))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CredentialError> + Send + 'static,
) -> Result<T, CredentialError> {
    // The keyring may block (D-Bus, Keychain).
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| CredentialError(err.to_string()))?
}

#[async_trait]
impl ApiKeyPort for KeyringCredentials {
    async fn check(&self, key: &str) -> KeyCheck {
        let Ok(api_key) = ApiKey::new(key) else {
            return KeyCheck::Malformed("paste your API key (it starts with sk-ant-)".into());
        };
        if !key.trim().starts_with("sk-ant-") {
            return KeyCheck::Malformed(
                "that does not look like an Anthropic API key (it starts with sk-ant-)".into(),
            );
        }
        let config = ClientConfig {
            connect_timeout: CHECK_TIMEOUT,
            read_timeout: CHECK_TIMEOUT,
            ..self.config.clone()
        };
        let client = match AnthropicClient::new(&api_key, config) {
            Ok(client) => client,
            Err(err) => return KeyCheck::Malformed(err.to_string()),
        };
        let catalog = AnthropicCatalog::new(client);
        match tokio::time::timeout(CHECK_TIMEOUT, catalog.list_models()).await {
            Ok(Ok(_)) => KeyCheck::Valid,
            Ok(Err(LlmError::Auth { .. })) => KeyCheck::Rejected(
                "Anthropic rejected this key; check that you copied the whole key".into(),
            ),
            Ok(Err(err)) => KeyCheck::Unreachable(err.to_string()),
            Err(_) => KeyCheck::Unreachable("no answer from the API".into()),
        }
    }

    async fn save(&self, key: &str) -> Result<(), CredentialError> {
        let key = key.trim().to_owned();
        blocking(move || {
            entry()?
                .set_password(&key)
                .map_err(|err| CredentialError(err.to_string()))
        })
        .await
    }

    async fn delete(&self) -> Result<bool, CredentialError> {
        blocking(|| match entry()?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(CredentialError(err.to_string())),
        })
        .await
    }

    fn env_override(&self) -> Option<String> {
        std::env::var(API_KEY_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|_| API_KEY_ENV.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    async fn credentials(status: u16) -> (MockServer, KeyringCredentials) {
        let server = MockServer::start().await;
        let body = match status {
            200 => json!({ "data": [], "has_more": false }),
            _ => json!({
                "type": "error",
                "error": { "type": "authentication_error", "message": "invalid x-api-key" }
            }),
        };
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        let config = ClientConfig {
            base_url: server.uri(),
            ..ClientConfig::default()
        };
        (server, KeyringCredentials::new(config))
    }

    #[tokio::test]
    async fn the_api_decides_whether_a_key_is_valid() {
        let (_server, keys) = credentials(200).await;
        assert_eq!(keys.check("sk-ant-good").await, KeyCheck::Valid);
        let (_server, keys) = credentials(401).await;
        assert!(matches!(
            keys.check("sk-ant-bad").await,
            KeyCheck::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn malformed_keys_are_not_sent() {
        let (server, keys) = credentials(200).await;
        assert!(matches!(keys.check("").await, KeyCheck::Malformed(_)));
        assert!(matches!(keys.check("hello").await, KeyCheck::Malformed(_)));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_api_is_not_a_rejection() {
        let keys = KeyringCredentials::new(ClientConfig {
            base_url: "http://127.0.0.1:9".into(),
            ..ClientConfig::default()
        });
        assert!(matches!(
            keys.check("sk-ant-x").await,
            KeyCheck::Unreachable(_)
        ));
    }
}
