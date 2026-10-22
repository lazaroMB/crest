//! Checking an Anthropic API key with the API, and the system keyring
//! entries crest keeps keys in (service `crest`, one entry per key name,
//! e.g. `ANTHROPIC_API_KEY`).

use std::time::Duration;

use application::ports::credentials::{CredentialError, KeyCheck};
use application::ports::llm::{LlmError, ModelCatalogPort};

use crate::catalog::AnthropicCatalog;
use crate::client::{AnthropicClient, ApiKey, ClientConfig, KEYRING_SERVICE};

/// Longest wait for the check of a new key.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Asks the API of `config` (`GET /v1/models`) whether `key` works.
pub async fn check_key(config: &ClientConfig, key: &str) -> KeyCheck {
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
        ..config.clone()
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

fn entry(name: &str) -> Result<keyring::Entry, CredentialError> {
    keyring::Entry::new(KEYRING_SERVICE, name).map_err(|err| CredentialError(err.to_string()))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CredentialError> + Send + 'static,
) -> Result<T, CredentialError> {
    // The keyring may block (D-Bus, Keychain).
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| CredentialError(err.to_string()))?
}

/// The key named `name` from the environment variable of that name or,
/// when unset or empty, from the keyring. Blocks on the keyring.
#[must_use]
pub fn read_key(name: &str) -> Option<String> {
    if let Ok(key) = std::env::var(name)
        && !key.trim().is_empty()
    {
        return Some(key.trim().to_owned());
    }
    match entry(name).ok()?.get_password() {
        Ok(key) if !key.trim().is_empty() => Some(key.trim().to_owned()),
        Ok(_) => None,
        Err(err) => {
            tracing::debug!(error = %err, name, "no key in the keyring");
            None
        }
    }
}

/// Stores `key` in the keyring entry `name`, replacing the previous one.
///
/// # Errors
///
/// When the keyring cannot be used.
pub async fn save_key(name: &str, key: &str) -> Result<(), CredentialError> {
    let (name, key) = (name.to_owned(), key.trim().to_owned());
    blocking(move || {
        entry(&name)?
            .set_password(&key)
            .map_err(|err| CredentialError(err.to_string()))
    })
    .await
}

/// Deletes the keyring entry `name`; `false` when there was none.
///
/// # Errors
///
/// When the keyring cannot be used.
pub async fn delete_key(name: &str) -> Result<bool, CredentialError> {
    let name = name.to_owned();
    blocking(move || match entry(&name)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(err) => Err(CredentialError(err.to_string())),
    })
    .await
}

/// Name of the environment variable `name` when it is set and not empty.
#[must_use]
pub fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|_| name.to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    async fn credentials(status: u16) -> (MockServer, ClientConfig) {
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
        (server, config)
    }

    #[tokio::test]
    async fn the_api_decides_whether_a_key_is_valid() {
        let (_server, keys) = credentials(200).await;
        assert_eq!(check_key(&keys, "sk-ant-good").await, KeyCheck::Valid);
        let (_server, keys) = credentials(401).await;
        assert!(matches!(
            check_key(&keys, "sk-ant-bad").await,
            KeyCheck::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn malformed_keys_are_not_sent() {
        let (server, keys) = credentials(200).await;
        assert!(matches!(check_key(&keys, "").await, KeyCheck::Malformed(_)));
        assert!(matches!(
            check_key(&keys, "hello").await,
            KeyCheck::Malformed(_)
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_api_is_not_a_rejection() {
        let keys = ClientConfig {
            base_url: "http://127.0.0.1:9".into(),
            ..ClientConfig::default()
        };
        assert!(matches!(
            check_key(&keys, "sk-ant-x").await,
            KeyCheck::Unreachable(_)
        ));
    }
}
