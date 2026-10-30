//! One model call for the engine: resolve the role, stream, assemble,
//! meter.

use std::sync::Arc;

use domain::conversation::Message;
use domain::model::{ModelPolicy, ModelRole, ResolveRequest, ResolvedModel};
use serde::de::DeserializeOwned;

use super::error::DiscoveryError;
use crate::flows::model_call;
use crate::flows::research::json::{parse_valid, text_of};
use crate::flows::research::usage::{StageUsage, UsageMeter};
use crate::ports::events::{DomainEvent, EventSink};
use crate::ports::llm::LlmPort;

/// The model the engine writes, reviews and evaluates with.
#[derive(Clone)]
pub struct Model {
    llm: Arc<dyn LlmPort>,
    models: ModelPolicy,
    events: Arc<dyn EventSink>,
    meter: Arc<UsageMeter>,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model").finish_non_exhaustive()
    }
}

impl Model {
    /// A model over `llm` resolving roles with `models`.
    #[must_use]
    pub fn new(llm: Arc<dyn LlmPort>, models: ModelPolicy, events: Arc<dyn EventSink>) -> Self {
        Self {
            llm,
            models,
            events,
            meter: Arc::new(UsageMeter::new(None)),
        }
    }

    /// The model of `role`.
    #[must_use]
    pub fn resolve(&self, role: ModelRole) -> ResolvedModel {
        self.models.resolve(&ResolveRequest {
            role,
            turn_override: None,
            pinned_role: None,
        })
    }

    /// The id of the model of `role`, for provenance.
    #[must_use]
    pub fn id_of(&self, role: ModelRole) -> String {
        self.resolve(role).spec.id.to_string()
    }

    /// Calls and tokens so far.
    #[must_use]
    pub fn usage(&self) -> StageUsage {
        self.meter.snapshot()
    }

    fn activity(&self, message: impl Into<String>) {
        self.events.emit(DomainEvent::Activity {
            message: message.into(),
        });
    }

    /// Sends `system` and `messages` to the model of `role` and returns
    /// the text of the answer, trimmed.
    ///
    /// # Errors
    ///
    /// The model's error, or [`DiscoveryError::Model`] on an empty answer.
    pub async fn complete(
        &self,
        role: ModelRole,
        system: &str,
        messages: Vec<Message>,
        what: &str,
    ) -> Result<String, DiscoveryError> {
        let req = model_call::request(self.resolve(role), system, messages);
        self.activity(format!("discovery: {what} ({})", req.model.spec.id));
        let assembled =
            model_call::stream_metered(self.llm.as_ref(), req, &self.meter, || {}).await?;
        let text = text_of(&assembled.message.content).trim().to_owned();
        if text.is_empty() {
            return Err(DiscoveryError::Model(format!("{what}: empty answer")));
        }
        Ok(text)
    }

    /// One user prompt to the model of `role`.
    ///
    /// # Errors
    ///
    /// As [`Model::complete`].
    pub async fn ask(
        &self,
        role: ModelRole,
        system: &str,
        prompt: &str,
        what: &str,
    ) -> Result<String, DiscoveryError> {
        self.complete(role, system, vec![Message::user_text(prompt)], what)
            .await
    }

    /// [`Model::ask`] for a JSON value of type `T` that passes `validate`,
    /// with one retry carrying the error.
    ///
    /// # Errors
    ///
    /// As [`Model::ask`], or [`DiscoveryError::Model`] when the second
    /// answer is invalid too.
    pub async fn ask_json<T: DeserializeOwned>(
        &self,
        role: ModelRole,
        system: &str,
        prompt: &str,
        what: &str,
        validate: impl Fn(&T) -> Result<(), String>,
    ) -> Result<T, DiscoveryError> {
        let first = self.ask(role, system, prompt, what).await?;
        let error = match parse_valid::<T>(&first, &validate) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let messages =
            model_call::retry_conversation(vec![Message::user_text(prompt)], first, &error);
        let second = self.complete(role, system, messages, what).await?;
        parse_valid::<T>(&second, &validate)
            .map_err(|e| DiscoveryError::Model(format!("{what}: {e}")))
    }
}
