//! Mapping of API errors (HTTP error bodies and stream `error` events) to
//! the port's [`LlmError`].

use adapter_support::errors::{self, Wording, is_context_error};
use application::ports::llm::LlmError;

use crate::wire::ApiError;

/// Message of [`LlmError::Auth`]. Never includes the key itself.
pub const AUTH_HINT: &str = "ANTHROPIC_API_KEY is invalid or missing; export a valid key \
     in ANTHROPIC_API_KEY or store it in the system keyring (service \"crest\")";

/// Converts an API error to an [`LlmError`].
///
/// `status` is the HTTP status, or `None` for an `error` event inside a
/// stream (which arrives with a 200). The error `type` decides first; the
/// status is used for types this crate does not know.
#[must_use]
pub fn from_api(status: Option<u16>, error: &ApiError, retry_after_ms: Option<u64>) -> LlmError {
    let message = error.message.clone();
    match error.kind.as_str() {
        "authentication_error" => LlmError::Auth {
            message: AUTH_HINT.to_owned(),
        },
        "permission_error" => LlmError::NotEntitled { message },
        "rate_limit_error" => LlmError::RateLimited { retry_after_ms },
        "overloaded_error" => LlmError::Overloaded,
        "api_error" => LlmError::Server {
            status: status.unwrap_or(500),
            message,
        },
        "timeout_error" => LlmError::Server {
            status: status.unwrap_or(504),
            message,
        },
        "invalid_request_error" if is_context_error(&message) => LlmError::ContextTooLong,
        "invalid_request_error" | "not_found_error" | "request_too_large" | "billing_error" => {
            LlmError::InvalidRequest {
                message: format!("{}: {message}", error.kind),
            }
        }
        _ => from_status(status.unwrap_or(500), error.to_string(), retry_after_ms),
    }
}

/// Converts a bare HTTP status (no parsable error body, or an error type
/// this crate does not know) to an [`LlmError`], with the mapping every
/// adapter shares: a 403 that names the key and a client error that names
/// the context are recognised as such.
#[must_use]
pub fn from_status(status: u16, message: String, retry_after_ms: Option<u64>) -> LlmError {
    errors::from_status(status, message, retry_after_ms, &Anthropic)
}

/// Anthropic's wording: a rejected key gets [`AUTH_HINT`] (which the
/// client turns into where the key came from), a refusal the API's own
/// message.
struct Anthropic;

impl Wording for Anthropic {
    fn rejected_key(&self, _status: u16, _message: &str) -> String {
        AUTH_HINT.to_owned()
    }

    fn refused(&self, _status: u16, message: String) -> String {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(kind: &str, message: &str) -> ApiError {
        ApiError {
            kind: kind.into(),
            message: message.into(),
        }
    }

    #[test]
    fn error_types_map_to_port_errors() {
        assert!(matches!(
            from_api(Some(401), &api("authentication_error", "invalid x-api-key"), None),
            LlmError::Auth { message } if message.starts_with("ANTHROPIC_API_KEY is invalid or missing")
        ));
        assert_eq!(
            from_api(Some(429), &api("rate_limit_error", "slow"), Some(2000)),
            LlmError::RateLimited {
                retry_after_ms: Some(2000)
            }
        );
        assert_eq!(
            from_api(None, &api("overloaded_error", "Overloaded"), None),
            LlmError::Overloaded
        );
        assert_eq!(
            from_api(None, &api("api_error", "boom"), None),
            LlmError::Server {
                status: 500,
                message: "boom".into()
            }
        );
        assert_eq!(
            from_api(
                Some(400),
                &api(
                    "invalid_request_error",
                    "prompt is too long: 1100000 tokens > 1000000 maximum"
                ),
                None
            ),
            LlmError::ContextTooLong
        );
        assert_eq!(
            from_api(Some(404), &api("not_found_error", "model: claude-x"), None),
            LlmError::InvalidRequest {
                message: "not_found_error: model: claude-x".into()
            }
        );
    }

    #[test]
    fn unknown_types_fall_back_to_status() {
        assert_eq!(
            from_api(Some(529), &api("brand_new_error", "x"), None),
            LlmError::Overloaded
        );
        assert!(matches!(
            from_api(Some(503), &api("brand_new_error", "x"), None),
            LlmError::Server { status: 503, .. }
        ));
        assert!(matches!(
            from_status(418, "teapot".into(), None),
            LlmError::InvalidRequest { .. }
        ));
    }

    #[test]
    fn the_shared_fallback_finds_rejected_keys_and_full_contexts() {
        // A server speaking the Messages API (OpenCode) with its own types.
        assert!(matches!(
            from_api(Some(403), &api("AuthError", "Invalid API key"), None),
            LlmError::Auth { message } if message == AUTH_HINT
        ));
        assert!(matches!(
            from_api(Some(403), &api("FreeTierError", "subscription required"), None),
            LlmError::NotEntitled { message } if message == "FreeTierError: subscription required"
        ));
        // A body that does not parse.
        assert_eq!(
            from_status(413, "Prompt is too long".into(), None),
            LlmError::ContextTooLong
        );
        assert_eq!(
            from_api(
                Some(400),
                &api(
                    "invalid_request_error",
                    "This model's maximum context length is 200000 tokens"
                ),
                None
            ),
            LlmError::ContextTooLong
        );
        // Unchanged: 401 is the key and any other 4xx an invalid request.
        assert!(matches!(
            from_status(401, String::new(), None),
            LlmError::Auth { message } if message == AUTH_HINT
        ));
        assert_eq!(
            from_status(400, "bad".into(), None),
            LlmError::InvalidRequest {
                message: "bad".into()
            }
        );
    }
}
