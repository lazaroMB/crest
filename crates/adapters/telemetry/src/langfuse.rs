//! Export of the telemetry spans to Langfuse (feature `langfuse`).
//!
//! The spans of [`crate::TracingTelemetry`] already carry the GenAI
//! attributes Langfuse understands; this module adds a
//! `tracing-opentelemetry` layer that sends them over OTLP/HTTP with the
//! exporter of `opentelemetry-langfuse`.

use std::fmt;

/// Langfuse Cloud, used when `LANGFUSE_HOST` is unset.
pub const DEFAULT_HOST: &str = "https://cloud.langfuse.com";

const HOST_ENV: &str = "LANGFUSE_HOST";
const PUBLIC_KEY_ENV: &str = "LANGFUSE_PUBLIC_KEY";
const SECRET_KEY_ENV: &str = "LANGFUSE_SECRET_KEY";

/// Where and as whom to send traces.
///
/// `Debug` hides both keys.
#[derive(Clone, PartialEq, Eq)]
pub struct LangfuseConfig {
    /// Base URL of the Langfuse instance.
    pub host: String,
    /// Project public key (`pk-lf-...`).
    pub public_key: String,
    /// Project secret key (`sk-lf-...`).
    pub secret_key: String,
}

impl fmt::Debug for LangfuseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LangfuseConfig")
            .field("host", &self.host)
            .field("public_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .finish()
    }
}

impl LangfuseConfig {
    /// Reads `LANGFUSE_HOST`, `LANGFUSE_PUBLIC_KEY` and
    /// `LANGFUSE_SECRET_KEY`. Returns `None` when neither key is set
    /// (Langfuse off).
    ///
    /// # Errors
    ///
    /// [`LangfuseError::MissingVar`] when only one of the keys is set.
    pub fn from_env() -> Result<Option<Self>, LangfuseError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Like [`Self::from_env`], reading variables through `lookup`; blank
    /// values count as unset.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, LangfuseError> {
        let var = |name: &str| lookup(name).filter(|value| !value.trim().is_empty());
        let (public_key, secret_key) = match (var(PUBLIC_KEY_ENV), var(SECRET_KEY_ENV)) {
            (None, None) => return Ok(None),
            (Some(_), None) => return Err(LangfuseError::MissingVar(SECRET_KEY_ENV)),
            (None, Some(_)) => return Err(LangfuseError::MissingVar(PUBLIC_KEY_ENV)),
            (Some(public), Some(secret)) => (public, secret),
        };
        Ok(Some(Self {
            host: var(HOST_ENV).unwrap_or_else(|| DEFAULT_HOST.to_owned()),
            public_key,
            secret_key,
        }))
    }
}

/// Why the Langfuse exporter could not be set up or flushed.
///
/// Messages never include the keys.
#[derive(Debug, thiserror::Error)]
pub enum LangfuseError {
    /// One of the two keys is set and the other is not.
    #[error("{0} is not set")]
    MissingVar(&'static str),
}

#[cfg(test)]
mod tests;
