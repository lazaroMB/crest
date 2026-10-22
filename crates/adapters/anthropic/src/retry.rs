//! The [`LlmPort`] implementation [`AnthropicLlm`]: one attempt per model
//! through [`AnthropicBackend`], with the retries and the fallback walk of
//! [`RetryingLlm`].

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::Duration;

pub use application::llm_retry::{Clock, RetryPolicy};
use application::llm_retry::{ModelBackend, RetryingLlm};
use application::ports::llm::{LlmError, LlmPort, LlmRequest, LlmStream};
use async_trait::async_trait;
use domain::model::ModelId;
use futures::future::BoxFuture;

use crate::client::{AnthropicClient, ApiKey, ClientConfig};
use crate::request;
use crate::server_tools::ServerToolOptions;

/// [`Clock`] backed by `tokio::time::sleep` and std's random hasher seed.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }

    fn jitter(&self) -> f64 {
        // Each `RandomState` is seeded randomly; good enough for jitter.
        let bits = RandomState::new().build_hasher().finish();
        (bits >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// One attempt on the Anthropic Messages API (or a server speaking it,
/// such as OpenCode's `/anthropic` endpoint).
#[derive(Debug, Clone)]
pub struct AnthropicBackend {
    client: AnthropicClient,
    server_tools: ServerToolOptions,
}

impl AnthropicBackend {
    /// A backend over `client` with the default server tool options.
    #[must_use]
    pub fn new(client: AnthropicClient) -> Self {
        Self {
            client,
            server_tools: ServerToolOptions::default(),
        }
    }

    /// Replaces the options written on every web search and web fetch
    /// tool the backend sends (domain filters, location, fetch limits).
    #[must_use]
    pub fn with_server_tool_options(mut self, options: ServerToolOptions) -> Self {
        self.server_tools = options;
        self
    }
}

#[async_trait]
impl ModelBackend for AnthropicBackend {
    async fn open(&self, req: &LlmRequest, model: &ModelId) -> Result<LlmStream, LlmError> {
        let mut body = request::build(req, model);
        self.server_tools.apply(&mut body);
        let session = req.metadata.session_id.map(|id| id.to_string());
        self.client
            .stream_messages_in(&body, session.as_deref())
            .await
    }

    /// Every Claude model the Messages API serves takes image input
    /// (png, jpeg, gif, webp).
    fn accepts_images(&self, model: &ModelId) -> bool {
        model.as_str().starts_with("claude-")
    }
}

/// [`LlmPort`] over the Anthropic Messages API.
#[derive(Clone)]
pub struct AnthropicLlm {
    backend: AnthropicBackend,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
    inner: RetryingLlm,
}

impl std::fmt::Debug for AnthropicLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicLlm")
            .field("backend", &self.backend)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl AnthropicLlm {
    /// An adapter over `client` with the default policy and real clock.
    #[must_use]
    pub fn new(client: AnthropicClient) -> Self {
        Self::build(
            AnthropicBackend::new(client),
            RetryPolicy::default(),
            Arc::new(TokioClock),
        )
    }

    fn build(backend: AnthropicBackend, policy: RetryPolicy, clock: Arc<dyn Clock>) -> Self {
        let inner =
            RetryingLlm::new(Arc::new(backend.clone()), Arc::clone(&clock)).with_policy(policy);
        Self {
            backend,
            policy,
            clock,
            inner,
        }
    }

    /// Reads the key (see [`ApiKey::from_env_or_keyring`]) and builds the
    /// adapter. May block on the keyring.
    ///
    /// # Errors
    ///
    /// [`LlmError::Auth`] without a key; see [`AnthropicClient::new`].
    pub fn from_env(config: ClientConfig) -> Result<Self, LlmError> {
        let key = ApiKey::from_env_or_keyring()?;
        Ok(Self::new(AnthropicClient::new(&key, config)?))
    }

    /// Replaces the retry policy.
    #[must_use]
    pub fn with_policy(self, policy: RetryPolicy) -> Self {
        Self::build(self.backend, policy, self.clock)
    }

    /// Replaces the options written on every web search and web fetch
    /// tool the adapter sends (domain filters, location, fetch limits).
    #[must_use]
    pub fn with_server_tool_options(self, options: ServerToolOptions) -> Self {
        Self::build(
            self.backend.with_server_tool_options(options),
            self.policy,
            self.clock,
        )
    }

    /// Replaces the clock (tests).
    #[must_use]
    pub fn with_clock(self, clock: Arc<dyn Clock>) -> Self {
        Self::build(self.backend, self.policy, clock)
    }
}

#[async_trait]
impl LlmPort for AnthropicLlm {
    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        self.inner.stream(req).await
    }

    fn accepts_images(&self, model: &ModelId) -> bool {
        self.backend.accepts_images(model)
    }
}

#[cfg(test)]
mod tests;
