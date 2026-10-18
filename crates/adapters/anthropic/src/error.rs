//! Mapping of API errors (HTTP error bodies and stream `error` events) to
//! the port's [`LlmError`].

use application::ports::llm::LlmError;

use crate::wire::ApiError;

/// Message of [`LlmError::Auth`]. Never includes the key itself.
pub const AUTH_HINT: &str = "ANTHROPIC_API_KEY inválida o ausente; exporta una key válida \
     en ANTHROPIC_API_KEY o guárdala en el llavero del sistema (servicio \"crest\")";

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
        "permission_error" => LlmError::Auth {
            message: format!("permiso denegado para esta API key: {message}"),
        },
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
        "invalid_request_error" if is_prompt_too_long(&message) => LlmError::ContextTooLong,
        "invalid_request_error" | "not_found_error" | "request_too_large" | "billing_error" => {
            LlmError::InvalidRequest {
                message: format!("{}: {message}", error.kind),
            }
        }
        _ => from_status(status.unwrap_or(500), error.to_string(), retry_after_ms),
    }
}

/// Converts a bare HTTP status (no parsable error body) to an [`LlmError`].
#[must_use]
pub fn from_status(status: u16, message: String, retry_after_ms: Option<u64>) -> LlmError {
    match status {
        401 => LlmError::Auth {
            message: AUTH_HINT.to_owned(),
        },
        403 => LlmError::Auth {
            message: format!("permiso denegado para esta API key: {message}"),
        },
        429 => LlmError::RateLimited { retry_after_ms },
        529 => LlmError::Overloaded,
        500.. => LlmError::Server { status, message },
        _ => LlmError::InvalidRequest { message },
    }
}

fn is_prompt_too_long(message: &str) -> bool {
    message.to_ascii_lowercase().contains("prompt is too long")
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
            LlmError::Auth { message } if message.starts_with("ANTHROPIC_API_KEY inválida o ausente")
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
}
