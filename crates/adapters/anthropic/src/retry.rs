//! Retries with exponential backoff and model fallback; the [`LlmPort`]
//! implementation [`AnthropicLlm`].
//!
//! For each model of [`ResolvedModel::fallback_chain`] the request is tried
//! up to [`RetryPolicy::max_attempts`] times. A failed attempt is retried
//! when [`LlmError::is_retryable`], after a backoff of
//! `base_delay · 2^(n-1)` (capped at `max_delay`) with "equal jitter"
//! (half fixed, half random), or after the `retry-after` the API sent for
//! a 429. When the retries of a model are exhausted and the error
//! [`LlmError::triggers_fallback`] (529, 5xx), the next model of the chain
//! is tried; past the last one the stream fails with
//! [`LlmError::AllModelsFailed`]. Other errors (401, 400, ...) end the call
//! as they are.
//!
//! An attempt counts as failed only while nothing but `MessageStart` and
//! `Usage` has arrived: those events are held back until the first content
//! event, so [`LlmEvent::Retrying`] and [`LlmEvent::ModelFallback`] always
//! precede the `MessageStart` of the attempt that succeeds, and an error
//! after content has been emitted ends the stream without a retry.
//!
//! Waiting goes through [`Clock`], so tests run without real delays.
//!
//! [`ResolvedModel::fallback_chain`]: domain::model::ResolvedModel::fallback_chain

use std::collections::VecDeque;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::Duration;

use application::ports::llm::{LlmError, LlmEvent, LlmPort, LlmRequest, LlmStream};
use async_trait::async_trait;
use domain::model::ModelId;
use futures::StreamExt;
use futures::future::BoxFuture;

use crate::client::{AnthropicClient, ApiKey, ClientConfig};
use crate::request;
use crate::server_tools::ServerToolOptions;
use crate::wire::MessagesRequest;

/// Retry limits and backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts per model, the first one included (at least 1).
    pub max_attempts: u32,
    /// Backoff before the first retry.
    pub base_delay: Duration,
    /// Cap of the exponential backoff.
    pub max_delay: Duration,
    /// Cap applied to a `retry-after` sent by the API.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(20),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Delay before retrying after failed attempt number `attempt` (from
    /// 1), with `jitter` in `[0, 1]`.
    #[must_use]
    pub fn delay(&self, attempt: u32, err: &LlmError, jitter: f64) -> Duration {
        if let LlmError::RateLimited {
            retry_after_ms: Some(ms),
        } = err
        {
            return Duration::from_millis(*ms).min(self.max_retry_after);
        }
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
        let backoff = self.base_delay.saturating_mul(factor).min(self.max_delay);
        let half = backoff / 2;
        half + half.mul_f64(jitter.clamp(0.0, 1.0))
    }
}

/// Source of waiting and randomness for [`AnthropicLlm`].
pub trait Clock: Send + Sync {
    /// Waits for `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
    /// A random number in `[0, 1]` for the backoff jitter.
    fn jitter(&self) -> f64;
}

/// [`Clock`] backed by `tokio::time::sleep` and std's random hasher seed.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }

    fn jitter(&self) -> f64 {
        // Each `RandomState` is seeded randomly; good enough for jitter.
        let bits = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        (bits >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// [`LlmPort`] over the Anthropic Messages API.
#[derive(Clone)]
pub struct AnthropicLlm {
    client: AnthropicClient,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
    server_tools: ServerToolOptions,
}

impl std::fmt::Debug for AnthropicLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicLlm")
            .field("client", &self.client)
            .field("policy", &self.policy)
            .field("server_tools", &self.server_tools)
            .finish_non_exhaustive()
    }
}

impl AnthropicLlm {
    /// An adapter over `client` with the default policy and real clock.
    #[must_use]
    pub fn new(client: AnthropicClient) -> Self {
        Self {
            client,
            policy: RetryPolicy::default(),
            clock: Arc::new(TokioClock),
            server_tools: ServerToolOptions::default(),
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
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Replaces the options written on every web search and web fetch
    /// tool the adapter sends (domain filters, location, fetch limits).
    #[must_use]
    pub fn with_server_tool_options(mut self, options: ServerToolOptions) -> Self {
        self.server_tools = options;
        self
    }

    /// Replaces the clock (tests).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
}

#[async_trait]
impl LlmPort for AnthropicLlm {
    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError> {
        let chain: Vec<ModelId> = req.model.fallback_chain().cloned().collect();
        let mut run = Run {
            llm: self.clone(),
            req,
            chain,
            model: 0,
            attempt: 1,
            queue: VecDeque::new(),
            phase: Phase::Attempt,
        };
        // The first attempt runs now, so an error that ends the call before
        // anything was emitted (401, 400, ...) is returned directly.
        run.attempt().await;
        if matches!(run.phase, Phase::Done)
            && run.queue.len() == 1
            && let Some(Err(err)) = run.queue.pop_front()
        {
            return Err(err);
        }
        Ok(Box::pin(futures::stream::unfold(run, Run::next)))
    }

    /// Every Claude model the Messages API serves takes image input
    /// (png, jpeg, gif, webp).
    fn accepts_images(&self, model: &ModelId) -> bool {
        model.as_str().starts_with("claude-")
    }
}

enum Phase {
    /// Send the request to `chain[model]`.
    Attempt,
    /// Wait, then attempt again.
    Sleep(Duration),
    /// An attempt succeeded; forward its events.
    Live(LlmStream),
    /// Nothing more to produce once the queue is drained.
    Done,
}

/// What to do after a failed attempt.
enum Decision {
    Retry(Duration),
    Fallback(ModelId),
    Fail(LlmError),
}

/// State of one `stream` call, driven by `futures::stream::unfold`.
struct Run {
    llm: AnthropicLlm,
    req: LlmRequest,
    chain: Vec<ModelId>,
    model: usize,
    attempt: u32,
    queue: VecDeque<Result<LlmEvent, LlmError>>,
    phase: Phase,
}

impl Run {
    async fn next(mut self) -> Option<(Result<LlmEvent, LlmError>, Self)> {
        loop {
            if let Some(item) = self.queue.pop_front() {
                return Some((item, self));
            }
            match &mut self.phase {
                Phase::Done => return None,
                Phase::Live(stream) => {
                    let item = stream.next().await?;
                    if item.is_err() {
                        self.phase = Phase::Done;
                    }
                    return Some((item, self));
                }
                Phase::Sleep(delay) => {
                    let delay = *delay;
                    self.llm.clock.sleep(delay).await;
                    self.phase = Phase::Attempt;
                }
                Phase::Attempt => self.attempt().await,
            }
        }
    }

    /// Runs one attempt and sets the next phase.
    async fn attempt(&mut self) {
        let Some(model) = self.chain.get(self.model).cloned() else {
            self.fail(LlmError::InvalidRequest {
                message: "the request has no model".into(),
            });
            return;
        };
        let mut body = request::build(&self.req, &model);
        self.llm.server_tools.apply(&mut body);
        let err = match try_model(&self.llm.client, &body).await {
            Ok((held, stream)) => {
                self.queue.extend(held.into_iter().map(Ok));
                self.phase = Phase::Live(stream);
                return;
            }
            Err(err) => err,
        };
        match self.decide(&err) {
            Decision::Retry(delay) => {
                let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
                tracing::info!(%model, attempt = self.attempt, delay_ms, error = %err, "retrying");
                self.queue.push_back(Ok(LlmEvent::Retrying {
                    attempt: self.attempt,
                    delay_ms,
                    reason: err,
                }));
                self.attempt += 1;
                self.phase = Phase::Sleep(delay);
            }
            Decision::Fallback(to) => {
                tracing::warn!(from = %model, %to, error = %err, "falling back to the next model");
                self.queue.push_back(Ok(LlmEvent::ModelFallback {
                    from: model,
                    to,
                    reason: err,
                }));
                self.model += 1;
                self.attempt = 1;
                self.phase = Phase::Attempt;
            }
            Decision::Fail(err) => self.fail(err),
        }
    }

    fn fail(&mut self, err: LlmError) {
        self.queue.push_back(Err(err));
        self.phase = Phase::Done;
    }

    fn decide(&self, err: &LlmError) -> Decision {
        let policy = &self.llm.policy;
        if err.is_retryable() && self.attempt < policy.max_attempts.max(1) {
            return Decision::Retry(policy.delay(self.attempt, err, self.llm.clock.jitter()));
        }
        if err.triggers_fallback() {
            return match self.chain.get(self.model + 1) {
                Some(next) => Decision::Fallback(next.clone()),
                None => Decision::Fail(LlmError::AllModelsFailed {
                    last: Box::new(err.clone()),
                }),
            };
        }
        Decision::Fail(err.clone())
    }
}

/// Sends `body` and holds back the first events of the answer until
/// content arrives. Returns those events and the rest of the stream, or the
/// error that broke the attempt before any content.
async fn try_model(
    client: &AnthropicClient,
    body: &MessagesRequest,
) -> Result<(Vec<LlmEvent>, LlmStream), LlmError> {
    let mut stream = client.stream_messages(body).await?;
    let mut held = Vec::new();
    while let Some(item) = stream.next().await {
        let event = item?;
        let preamble = matches!(event, LlmEvent::MessageStart { .. } | LlmEvent::Usage(_));
        held.push(event);
        if !preamble {
            break;
        }
    }
    Ok((held, stream))
}

#[cfg(test)]
mod tests;
