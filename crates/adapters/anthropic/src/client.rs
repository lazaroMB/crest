//! HTTP client for the Anthropic API over `reqwest` + rustls.
//!
//! [`AnthropicClient`] sends one request and turns its response into either
//! a decoded value, an [`LlmStream`] of a streaming Messages answer, or a
//! typed [`LlmError`]. It does not retry: that is [`crate::retry`]'s job.
//!
//! The API key lives in an [`ApiKey`], whose `Debug` and `Display` are
//! redacted, and goes out only as a header marked sensitive. It is never
//! logged, traced nor included in errors.

use std::fmt;
use std::time::Duration;

use application::ports::llm::{LlmError, LlmStream};
use domain::model::ModelId;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;

use crate::error::{self, AUTH_HINT};
use crate::stream::llm_events;
use crate::wire::{ApiErrorBody, MessagesRequest};

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Value of the `anthropic-version` header.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Environment variable read by [`ApiKey::from_env_or_keyring`].
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// Keyring service under which the key may be stored.
pub const KEYRING_SERVICE: &str = "crest";
/// Keyring user (entry name) of the key within [`KEYRING_SERVICE`].
pub const KEYRING_USER: &str = "ANTHROPIC_API_KEY";

/// Where an [`ApiKey`] came from, so a rejected key can be traced back to
/// the place the user has to fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// The `ANTHROPIC_API_KEY` environment variable.
    Env,
    /// The system keyring (service `crest`, entry `ANTHROPIC_API_KEY`).
    Keyring,
    /// Given directly by the caller.
    Explicit,
}

impl KeySource {
    /// The message shown when the API rejects a key from this source.
    #[must_use]
    pub fn rejected_hint(self) -> String {
        match self {
            Self::Env => "Anthropic rejected the API key from the ANTHROPIC_API_KEY variable \
                 (401); check that it is a valid key (it starts with `sk-ant-`)"
                .to_owned(),
            Self::Keyring => format!(
                "Anthropic rejected the API key stored in the system keyring (service \
                 `{KEYRING_SERVICE}`, entry `{KEYRING_USER}`) (401); store the right one or \
                 delete it (on Linux: `secret-tool clear service {KEYRING_SERVICE} username \
                 {KEYRING_USER}`) and export ANTHROPIC_API_KEY"
            ),
            Self::Explicit => AUTH_HINT.to_owned(),
        }
    }
}

/// An Anthropic API key. `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey {
    key: String,
    source: KeySource,
}

impl ApiKey {
    /// Wraps a key; surrounding whitespace is trimmed.
    ///
    /// # Errors
    ///
    /// [`LlmError::Auth`] if the key is empty.
    pub fn new(key: impl Into<String>) -> Result<Self, LlmError> {
        let key = key.into();
        let key = key.trim();
        if key.is_empty() {
            return Err(auth_error());
        }
        Ok(Self {
            key: key.to_owned(),
            source: KeySource::Explicit,
        })
    }

    /// Where the key came from.
    #[must_use]
    pub fn source(&self) -> KeySource {
        self.source
    }

    fn from(key: String, source: KeySource) -> Result<Self, LlmError> {
        let mut key = Self::new(key)?;
        key.source = source;
        if !key.key.starts_with("sk-ant-") {
            // Never log the key itself, only that it looks wrong.
            tracing::warn!(
                ?source,
                "the Anthropic API key does not start with `sk-ant-`; it is probably not a key"
            );
        }
        Ok(key)
    }

    /// Reads the key from `ANTHROPIC_API_KEY` or, if unset or empty, from
    /// the system keyring (service `crest`, entry `ANTHROPIC_API_KEY`).
    ///
    /// The keyring call may block (D-Bus, Keychain); call this from a
    /// blocking context or through `spawn_blocking`.
    ///
    /// # Errors
    ///
    /// [`LlmError::Auth`] when neither source has a key.
    pub fn from_env_or_keyring() -> Result<Self, LlmError> {
        if let Ok(key) = std::env::var(API_KEY_ENV)
            && let Ok(key) = Self::from(key, KeySource::Env)
        {
            return Ok(key);
        }
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|err| {
            tracing::debug!(error = %err, "keyring unavailable");
            auth_error()
        })?;
        match entry.get_password() {
            Ok(key) => Self::from(key, KeySource::Keyring),
            Err(err) => {
                tracing::debug!(error = %err, "no API key in the keyring");
                Err(auth_error())
            }
        }
    }

    /// The key, for building the request header only.
    fn expose(&self) -> &str {
        &self.key
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

fn auth_error() -> LlmError {
    LlmError::Auth {
        message: AUTH_HINT.to_owned(),
    }
}

/// Connection settings of [`AnthropicClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// Base URL without trailing slash (tests point it at a mock server).
    pub base_url: String,
    /// Time allowed to establish the connection.
    pub connect_timeout: Duration,
    /// Maximum silence between two reads of a response. Streams send
    /// `ping` events, so this only fires on a stalled connection.
    pub read_timeout: Duration,
    /// Values of the `anthropic-beta` header, if any.
    pub betas: Vec<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(300),
            betas: Vec::new(),
        }
    }
}

/// One HTTP client for the Anthropic API.
#[derive(Debug, Clone)]
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    key_source: KeySource,
}

impl AnthropicClient {
    /// Builds a client that authenticates with `key`.
    ///
    /// # Errors
    ///
    /// [`LlmError::Auth`] if the key is not a valid header value, or
    /// [`LlmError::Network`] if the TLS backend cannot be initialised.
    pub fn new(key: &ApiKey, config: ClientConfig) -> Result<Self, LlmError> {
        let mut headers = HeaderMap::new();
        let mut key_value = HeaderValue::from_str(key.expose()).map_err(|_| auth_error())?;
        key_value.set_sensitive(true);
        headers.insert(HeaderName::from_static("x-api-key"), key_value);
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        if !config.betas.is_empty() {
            let betas = HeaderValue::from_str(&config.betas.join(",")).map_err(|err| {
                LlmError::InvalidRequest {
                    message: format!("invalid anthropic-beta value: {err}"),
                }
            })?;
            headers.insert(HeaderName::from_static("anthropic-beta"), betas);
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .build()
            .map_err(|err| LlmError::Network(err.to_string()))?;
        Ok(Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            key_source: key.source(),
        })
    }

    /// `POST /v1/messages` with `stream: true`, returning the decoded event
    /// stream once the API accepted the request (2xx).
    ///
    /// # Errors
    ///
    /// The typed error of a non-2xx answer, or [`LlmError::Network`].
    pub async fn stream_messages(&self, body: &MessagesRequest) -> Result<LlmStream, LlmError> {
        let url = format!("{}/v1/messages", self.base_url);
        tracing::debug!(model = %body.model, stream = body.stream, "POST /v1/messages");
        let response = self
            .http
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(network_error)?;
        let response = check(response, self.key_source).await?;
        let model = ModelId::new(body.model.clone());
        Ok(Box::pin(llm_events(
            Box::pin(response.bytes_stream()),
            model,
        )))
    }

    /// `GET {path}` with `query`, decoding a JSON answer.
    ///
    /// # Errors
    ///
    /// The typed error of a non-2xx answer, [`LlmError::Network`] or
    /// [`LlmError::Decode`].
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, LlmError> {
        let mut url = reqwest::Url::parse(&format!("{}{path}", self.base_url)).map_err(|err| {
            LlmError::InvalidRequest {
                message: format!("invalid URL: {err}"),
            }
        })?;
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        tracing::debug!(%path, "GET");
        let response = self.http.get(url).send().await.map_err(network_error)?;
        check(response, self.key_source)
            .await?
            .json()
            .await
            .map_err(|err| LlmError::Decode(err.to_string()))
    }
}

fn network_error(err: reqwest::Error) -> LlmError {
    // `without_url` keeps query strings out of the message; the key is a
    // header and never part of it.
    LlmError::Network(err.without_url().to_string())
}

/// Passes 2xx responses through and turns the rest into typed errors.
async fn check(
    response: reqwest::Response,
    key_source: KeySource,
) -> Result<reqwest::Response, LlmError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after_ms = retry_after_ms(response.headers());
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let text = response.text().await.unwrap_or_default();
    let err = match serde_json::from_str::<ApiErrorBody>(&text) {
        Ok(body) => error::from_api(Some(status.as_u16()), &body.error, retry_after_ms),
        Err(_) => error::from_status(status.as_u16(), text, retry_after_ms),
    };
    // Say where the rejected key came from instead of "invalid or missing".
    let err = match err {
        LlmError::Auth { message } if message == AUTH_HINT => LlmError::Auth {
            message: key_source.rejected_hint(),
        },
        other => other,
    };
    tracing::warn!(status = status.as_u16(), request_id, error = %err, "Anthropic API error");
    Err(err)
}

/// Parses `retry-after` (whole or fractional seconds). HTTP dates are not
/// used by the API and are ignored.
#[must_use]
pub fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    let secs: f64 = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| (secs * 1000.0).round() as u64)
}

#[cfg(test)]
mod tests;
