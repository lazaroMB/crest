//! The HTTP side of a model adapter over `reqwest` + rustls: the client,
//! its headers, the network error and what a refused response says.

use std::time::Duration;

use application::ports::llm::LlmError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};

/// A client that sends `headers` (plus `user_agent`, if any) on every
/// request, with the given connect and read timeouts.
///
/// # Errors
///
/// [`LlmError::InvalidRequest`] for a user agent that is not a header
/// value, or [`LlmError::Network`] if the TLS backend cannot be
/// initialised.
pub fn client(
    mut headers: HeaderMap,
    user_agent: Option<&str>,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<reqwest::Client, LlmError> {
    if let Some(agent) = user_agent {
        let agent = HeaderValue::from_str(agent).map_err(|err| LlmError::InvalidRequest {
            message: format!("invalid user agent: {err}"),
        })?;
        headers.insert(USER_AGENT, agent);
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .build()
        .map_err(|err| LlmError::Network(err.to_string()))
}

/// The header that carries the crest session id, if one is configured.
///
/// # Errors
///
/// [`LlmError::InvalidRequest`] when `name` is not a header name.
pub fn session_header(name: Option<&str>) -> Result<Option<HeaderName>, LlmError> {
    name.map(HeaderName::try_from)
        .transpose()
        .map_err(|err| LlmError::InvalidRequest {
            message: format!("invalid session header: {err}"),
        })
}

/// A request that did not get an answer.
#[must_use]
pub fn network_error(err: reqwest::Error) -> LlmError {
    // `without_url` keeps query strings out of the message; keys go in
    // headers and are never part of it.
    LlmError::Network(err.without_url().to_string())
}

/// Parses `retry-after` (whole or fractional seconds). HTTP dates are not
/// used by the model APIs and are ignored.
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

/// What a non-2xx response said, read for its typed error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The HTTP status.
    pub status: u16,
    /// The `retry-after` header, in milliseconds.
    pub retry_after_ms: Option<u64>,
    /// The `request-id` header, worth logging when reporting a problem.
    pub request_id: Option<String>,
    /// The body (empty if it could not be read).
    pub text: String,
}

/// Passes a 2xx `response` through; reads the rest into a [`Refusal`].
///
/// # Errors
///
/// The [`Refusal`] of a non-2xx answer.
pub async fn accepted(response: reqwest::Response) -> Result<reqwest::Response, Refusal> {
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
    Err(Refusal {
        status: status.as_u16(),
        retry_after_ms,
        request_id,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parsing() {
        let mut headers = HeaderMap::new();
        assert_eq!(retry_after_ms(&headers), None);
        headers.insert("retry-after", HeaderValue::from_static("2"));
        assert_eq!(retry_after_ms(&headers), Some(2000));
        headers.insert("retry-after", HeaderValue::from_static(" 1.5 "));
        assert_eq!(retry_after_ms(&headers), Some(1500));
        headers.insert("retry-after", HeaderValue::from_static("-1"));
        assert_eq!(retry_after_ms(&headers), None);
        headers.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(retry_after_ms(&headers), None);
    }

    #[test]
    fn bad_headers_are_invalid_requests() {
        let timeout = Duration::from_secs(1);
        assert!(matches!(
            client(HeaderMap::new(), Some("bad\nagent"), timeout, timeout),
            Err(LlmError::InvalidRequest { message }) if message.starts_with("invalid user agent")
        ));
        assert!(client(HeaderMap::new(), Some("crest/1"), timeout, timeout).is_ok());
        assert!(matches!(
            session_header(Some("bad header")),
            Err(LlmError::InvalidRequest { message }) if message.starts_with("invalid session header")
        ));
        assert_eq!(session_header(None), Ok(None));
        assert_eq!(
            session_header(Some("x-session")),
            Ok(Some(HeaderName::from_static("x-session")))
        );
    }
}
