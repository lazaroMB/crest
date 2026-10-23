//! HTTP client for OpenAI-style APIs over `reqwest` + rustls.
//!
//! [`OpenAiClient`] sends one request and turns its answer into either a
//! decoded value, the server-sent events of a stream, or a typed
//! [`LlmError`]. It does not retry: the caller wraps it in
//! [`RetryingLlm`](application::llm_retry::RetryingLlm).
//!
//! The key goes out only as an `Authorization: Bearer` header marked
//! sensitive; it is never logged, traced nor included in errors.

use std::time::Duration;

use application::ports::llm::LlmError;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Connection settings of [`OpenAiClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    /// Base URL up to and including the version, without trailing slash
    /// (`https://opencode.ai/inference/go/openai/v1`).
    pub base_url: String,
    /// Time allowed to establish the connection.
    pub connect_timeout: Duration,
    /// Maximum silence between two reads of a response.
    pub read_timeout: Duration,
    /// `User-Agent` of every request, if any.
    pub user_agent: Option<String>,
    /// Header that carries the crest session id on each request, if any.
    pub session_header: Option<String>,
    /// Who the key belongs to, for error messages (`OpenCode`).
    pub provider: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_owned(),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(300),
            user_agent: None,
            session_header: None,
            provider: "OpenAI".to_owned(),
        }
    }
}

/// One HTTP client for an OpenAI-style API.
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    session_header: Option<HeaderName>,
    provider: String,
}

impl OpenAiClient {
    /// Builds a client that authenticates with `key`.
    ///
    /// # Errors
    ///
    /// [`LlmError::Auth`] if the key is empty or not a valid header value,
    /// [`LlmError::InvalidRequest`] for a bad user agent or session header,
    /// or [`LlmError::Network`] if the TLS backend cannot be initialised.
    pub fn new(key: &str, config: OpenAiConfig) -> Result<Self, LlmError> {
        let key = key.trim();
        let auth = || LlmError::Auth {
            message: format!("the {} API key is missing or invalid", config.provider),
        };
        if key.is_empty() {
            return Err(auth());
        }
        let mut headers = HeaderMap::new();
        let mut bearer = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| auth())?;
        bearer.set_sensitive(true);
        headers.insert(AUTHORIZATION, bearer);
        if let Some(agent) = &config.user_agent {
            let agent = HeaderValue::from_str(agent).map_err(|err| LlmError::InvalidRequest {
                message: format!("invalid user agent: {err}"),
            })?;
            headers.insert(USER_AGENT, agent);
        }
        let session_header = config
            .session_header
            .as_deref()
            .map(HeaderName::try_from)
            .transpose()
            .map_err(|err| LlmError::InvalidRequest {
                message: format!("invalid session header: {err}"),
            })?;
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .build()
            .map_err(|err| LlmError::Network(err.to_string()))?;
        Ok(Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            session_header,
            provider: config.provider,
        })
    }

    /// `POST {base}{path}` with a JSON `body`, for the crest session
    /// `session`; returns the response once the API accepted it (2xx).
    ///
    /// # Errors
    ///
    /// The typed error of a non-2xx answer, or [`LlmError::Network`].
    pub async fn post(
        &self,
        path: &str,
        body: &Value,
        session: Option<&str>,
    ) -> Result<reqwest::Response, LlmError> {
        let url = format!("{}{path}", self.base_url);
        tracing::debug!(%path, model = ?body.get("model"), "POST");
        let mut request = self.http.post(url).json(body);
        if let (Some(name), Some(session)) = (&self.session_header, session) {
            request = request.header(name.clone(), session);
        }
        let response = request.send().await.map_err(network_error)?;
        check(response, &self.provider).await
    }

    /// `GET {base}{path}`, decoding a JSON answer.
    ///
    /// # Errors
    ///
    /// The typed error of a non-2xx answer, [`LlmError::Network`] or
    /// [`LlmError::Decode`].
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, LlmError> {
        let url = format!("{}{path}", self.base_url);
        tracing::debug!(%path, "GET");
        let response = self.http.get(url).send().await.map_err(network_error)?;
        check(response, &self.provider)
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
async fn check(response: reqwest::Response, provider: &str) -> Result<reqwest::Response, LlmError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after_ms = retry_after_ms(response.headers());
    let text = response.text().await.unwrap_or_default();
    let err = from_status(status.as_u16(), &text, retry_after_ms, provider);
    tracing::warn!(status = status.as_u16(), error = %err, "{provider} API error");
    Err(err)
}

/// The message of an OpenAI-style error body (`{"error": {"message"}}`),
/// or the body itself.
#[must_use]
pub fn error_message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|body| {
            let error = body.get("error")?;
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.trim().to_owned())
}

/// The typed error of an HTTP `status` with body `text`.
#[must_use]
pub fn from_status(
    status: u16,
    text: &str,
    retry_after_ms: Option<u64>,
    provider: &str,
) -> LlmError {
    let message = error_message(text);
    match status {
        401 => LlmError::Auth {
            message: format!("{provider} rejected the API key ({status}): {message}"),
        },
        403 if is_key_error(&message) => LlmError::Auth {
            message: format!("{provider} rejected the API key ({status}): {message}"),
        },
        403 => LlmError::NotEntitled {
            message: format!("{provider} refused the request ({status}): {message}"),
        },
        429 => LlmError::RateLimited { retry_after_ms },
        529 => LlmError::Overloaded,
        400 | 404 | 413 | 422 if is_context_error(&message) => LlmError::ContextTooLong,
        400..=499 => LlmError::InvalidRequest { message },
        _ => LlmError::Server { status, message },
    }
}

/// The typed error of an error object sent inside a stream.
#[must_use]
pub fn from_stream_error(error: &Value) -> LlmError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the stream failed")
        .to_owned();
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match code {
        "rate_limit_exceeded" | "rate_limit_error" => LlmError::RateLimited {
            retry_after_ms: None,
        },
        "overloaded_error" | "server_is_overloaded" => LlmError::Overloaded,
        "context_length_exceeded" => LlmError::ContextTooLong,
        "invalid_request_error" | "invalid_prompt" => LlmError::InvalidRequest { message },
        _ if is_context_error(&message) => LlmError::ContextTooLong,
        _ => LlmError::Server {
            status: 500,
            message,
        },
    }
}

/// Whether a 403 is about the key itself rather than what the account may
/// use (OpenCode answers 403 to a missing subscription or a free tier used
/// outside its own client).
fn is_key_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "invalid api key",
        "invalid_api_key",
        "incorrect api key",
        "api key is invalid",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn is_context_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("context_length_exceeded")
        || message.contains("context length")
        || message.contains("context window")
        || message.contains("maximum context")
}

/// Parses `retry-after` (whole or fractional seconds).
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
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn statuses_become_typed_errors() {
        let body = json!({"error": {"message": "bad key", "type": "invalid_request_error"}});
        let body = body.to_string();
        assert!(matches!(
            from_status(401, &body, None, "OpenCode"),
            LlmError::Auth { message } if message.contains("OpenCode") && message.contains("bad key")
        ));
        assert_eq!(
            from_status(429, "", Some(1500), "x"),
            LlmError::RateLimited {
                retry_after_ms: Some(1500)
            }
        );
        assert_eq!(
            from_status(
                400,
                r#"{"error":{"message":"This model's maximum context length is 128000 tokens"}}"#,
                None,
                "x"
            ),
            LlmError::ContextTooLong
        );
        assert_eq!(
            from_status(400, "nope", None, "x"),
            LlmError::InvalidRequest {
                message: "nope".into()
            }
        );
        assert!(from_status(503, "", None, "x").triggers_fallback());
    }

    #[test]
    fn a_403_is_the_plan_unless_it_names_the_key() {
        // Bodies OpenCode sent to a valid key.
        let free = r#"{"type":"error","error":{"type":"FreeTierError","message":"OpenCode's free tier can only be used from within OpenCode."}}"#;
        assert!(matches!(
            from_status(403, free, None, "OpenCode"),
            LlmError::NotEntitled { message } if message.contains("free tier")
        ));
        let go = r#"{"error":{"message":"Upstream request failed: An active OpenCode Go subscription is required to use Go models."}}"#;
        assert!(matches!(
            from_status(403, go, None, "OpenCode"),
            LlmError::NotEntitled { message } if message.contains("subscription")
        ));
        assert!(matches!(
            from_status(
                403,
                r#"{"error":{"message":"Invalid API key"}}"#,
                None,
                "OpenCode"
            ),
            LlmError::Auth { .. }
        ));
    }

    #[test]
    fn stream_errors_are_typed() {
        assert_eq!(
            from_stream_error(&json!({"code": "context_length_exceeded", "message": "too long"})),
            LlmError::ContextTooLong
        );
        assert!(matches!(
            from_stream_error(&json!({"message": "boom"})),
            LlmError::Server { status: 500, .. }
        ));
    }

    #[test]
    fn empty_keys_are_refused() {
        assert!(matches!(
            OpenAiClient::new("  ", OpenAiConfig::default()),
            Err(LlmError::Auth { .. })
        ));
    }
}
