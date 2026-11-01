//! The typed [`LlmError`] of an HTTP status, for refused requests whose
//! body says nothing more precise (no body, a body that does not parse,
//! or an error type the adapter does not know).
//!
//! The status decides, with two looks at the message: a 403 that names
//! the key is a rejected key rather than something the account may not
//! use, and a 400, 404, 413 or 422 that names the context is a prompt
//! over the model's context window, which the caller can fix by
//! compacting.

use application::ports::llm::LlmError;

/// How a provider words the errors that concern the user's account.
pub trait Wording {
    /// The message of [`LlmError::Auth`]: the provider rejected the key.
    fn rejected_key(&self, status: u16, message: &str) -> String;

    /// The message of [`LlmError::NotEntitled`]: the provider refused
    /// what the account may not use.
    fn refused(&self, status: u16, message: String) -> String;
}

/// The wording of a provider known by its name (`OpenCode`): the status
/// and the provider's message after the name.
#[derive(Debug, Clone, Copy)]
pub struct Named<'a>(pub &'a str);

impl Wording for Named<'_> {
    fn rejected_key(&self, status: u16, message: &str) -> String {
        format!("{} rejected the API key ({status}): {message}", self.0)
    }

    fn refused(&self, status: u16, message: String) -> String {
        format!("{} refused the request ({status}): {message}", self.0)
    }
}

/// The typed error of a refused request: HTTP `status`, the provider's
/// `message` and its `retry-after`.
#[must_use]
pub fn from_status(
    status: u16,
    message: String,
    retry_after_ms: Option<u64>,
    wording: &(impl Wording + ?Sized),
) -> LlmError {
    match status {
        401 => LlmError::Auth {
            message: wording.rejected_key(status, &message),
        },
        403 if is_key_error(&message) => LlmError::Auth {
            message: wording.rejected_key(status, &message),
        },
        403 => LlmError::NotEntitled {
            message: wording.refused(status, message),
        },
        429 => LlmError::RateLimited { retry_after_ms },
        529 => LlmError::Overloaded,
        400 | 404 | 413 | 422 if is_context_error(&message) => LlmError::ContextTooLong,
        500.. => LlmError::Server { status, message },
        _ => LlmError::InvalidRequest { message },
    }
}

/// Whether a message is about the key itself rather than what the account
/// may use (OpenCode answers 403 to a missing subscription or a free tier
/// used outside its own client, and to a bad key).
#[must_use]
pub fn is_key_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "invalid api key",
        "invalid_api_key",
        "incorrect api key",
        "api key is invalid",
        "invalid x-api-key",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

/// Whether a message says the prompt does not fit the model's context:
/// OpenAI's `context_length_exceeded` and its wordings, and Anthropic's
/// `prompt is too long`.
#[must_use]
pub fn is_context_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "context_length_exceeded",
        "context length",
        "context window",
        "maximum context",
        "prompt is too long",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(status: u16, message: &str) -> LlmError {
        from_status(status, message.to_owned(), Some(1500), &Named("OpenCode"))
    }

    #[test]
    fn statuses_become_typed_errors() {
        assert_eq!(
            named(401, "bad key"),
            LlmError::Auth {
                message: "OpenCode rejected the API key (401): bad key".into()
            }
        );
        assert_eq!(
            named(429, ""),
            LlmError::RateLimited {
                retry_after_ms: Some(1500)
            }
        );
        assert_eq!(named(529, ""), LlmError::Overloaded);
        assert_eq!(
            named(503, "down"),
            LlmError::Server {
                status: 503,
                message: "down".into()
            }
        );
        assert_eq!(
            named(418, "teapot"),
            LlmError::InvalidRequest {
                message: "teapot".into()
            }
        );
    }

    #[test]
    fn a_403_is_the_plan_unless_it_names_the_key() {
        assert_eq!(
            named(403, "An active subscription is required"),
            LlmError::NotEntitled {
                message: "OpenCode refused the request (403): An active subscription is required"
                    .into()
            }
        );
        assert!(matches!(
            named(403, "Invalid API key"),
            LlmError::Auth { .. }
        ));
        assert!(matches!(
            named(403, "authentication_error: invalid x-api-key"),
            LlmError::Auth { .. }
        ));
    }

    #[test]
    fn context_errors_are_found_in_client_errors_only() {
        for message in [
            "This model's maximum context length is 128000 tokens",
            "prompt is too long: 210000 tokens > 200000 maximum",
            "error code: context_length_exceeded",
        ] {
            assert_eq!(named(400, message), LlmError::ContextTooLong, "{message}");
        }
        assert_eq!(
            named(413, "invalid_request_error: Prompt is too long"),
            LlmError::ContextTooLong
        );
        assert!(matches!(
            named(500, "context window"),
            LlmError::Server { .. }
        ));
        assert!(matches!(
            named(400, "max_tokens: must be at most 64000"),
            LlmError::InvalidRequest { .. }
        ));
    }
}
