//! Export of the telemetry spans to Langfuse (feature `langfuse`).
//!
//! The spans of [`crate::TracingTelemetry`] already carry the GenAI
//! attributes Langfuse understands; this module adds a
//! `tracing-opentelemetry` layer that sends them over OTLP/HTTP with the
//! exporter of `opentelemetry-langfuse`.

use std::fmt;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::registry::LookupSpan;

use crate::TARGET;

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
    /// The exporter needs a Tokio runtime to send batches.
    #[error("the Langfuse exporter must be started inside a Tokio runtime")]
    NoRuntime,
    /// The OTLP exporter could not be built.
    #[error("cannot build the Langfuse exporter: {0}")]
    Exporter(#[from] opentelemetry_langfuse::Error),
    /// Flushing or shutting down the exporter failed.
    #[error("Langfuse export failed: {0}")]
    Export(#[from] opentelemetry_sdk::error::OTelSdkError),
}

/// A running Langfuse exporter; spans reach it through [`Self::layer`].
///
/// Spans are sent in batches from a Tokio task. Call [`Self::shutdown`]
/// before exiting to send the last batch.
pub struct LangfuseExporter {
    provider: SdkTracerProvider,
}

impl fmt::Debug for LangfuseExporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LangfuseExporter").finish_non_exhaustive()
    }
}

/// Builds the Langfuse exporter. Must run inside a Tokio runtime.
///
/// Building sends nothing: the first request goes out with the first
/// batch of finished spans.
///
/// # Errors
///
/// [`LangfuseError::NoRuntime`] outside Tokio, [`LangfuseError::Exporter`]
/// when the exporter cannot be built.
pub fn init_langfuse(config: &LangfuseConfig) -> Result<LangfuseExporter, LangfuseError> {
    if tokio::runtime::Handle::try_current().is_err() {
        return Err(LangfuseError::NoRuntime);
    }
    let exporter = opentelemetry_langfuse::ExporterBuilder::new()
        .with_host(&config.host)
        .with_basic_auth(&config.public_key, &config.secret_key)
        .build()?;
    let provider = SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter, Tokio).build())
        .with_resource(
            Resource::builder()
                .with_service_name("crest")
                .with_attributes([KeyValue::new("service.version", env!("CARGO_PKG_VERSION"))])
                .build(),
        )
        .build();
    Ok(LangfuseExporter { provider })
}

impl LangfuseExporter {
    /// Layer that exports the spans of [`crate::TracingTelemetry`] (target
    /// [`TARGET`], level info and above) and ignores everything else.
    pub fn layer<S>(&self) -> impl Layer<S> + Send + Sync + 'static
    where
        S: Subscriber + for<'a> LookupSpan<'a> + Send + Sync + 'static,
    {
        tracing_opentelemetry::layer()
            .with_tracer(self.provider.tracer("crest"))
            .with_location(false)
            .with_threads(false)
            .with_filter(Targets::new().with_target(TARGET, LevelFilter::INFO))
    }

    /// Sends the spans finished so far.
    ///
    /// # Errors
    ///
    /// When the export fails.
    pub fn force_flush(&self) -> Result<(), LangfuseError> {
        Ok(self.provider.force_flush()?)
    }

    /// Sends the remaining spans and stops the exporter.
    ///
    /// # Errors
    ///
    /// When the last export fails.
    pub fn shutdown(self) -> Result<(), LangfuseError> {
        Ok(self.provider.shutdown()?)
    }
}

#[cfg(test)]
mod tests;
