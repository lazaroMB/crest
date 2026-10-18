//! Process-wide telemetry: the `tracing` subscriber (`CREST_LOG`) and,
//! with the `langfuse` feature and its environment variables, the export
//! of the spans to Langfuse.

use adapter_telemetry::SubscriberOptions;

/// Installed telemetry; [`Telemetry::shutdown`] flushes it at exit.
#[derive(Debug, Default)]
pub struct Telemetry {
    #[cfg(feature = "langfuse")]
    langfuse: Option<adapter_telemetry::LangfuseExporter>,
}

impl Telemetry {
    /// Installs the subscriber. Must run inside the tokio runtime (the
    /// Langfuse exporter spawns its batch task there). Problems are
    /// returned as warnings: telemetry never stops crest.
    #[must_use]
    pub fn init() -> (Self, Vec<String>) {
        let opts = SubscriberOptions::default();
        let mut warnings = Vec::new();
        #[cfg(feature = "langfuse")]
        {
            use adapter_telemetry::{LangfuseConfig, init_langfuse, init_subscriber_with};
            match LangfuseConfig::from_env().and_then(|c| c.map(|c| init_langfuse(&c)).transpose())
            {
                Ok(Some(exporter)) => {
                    if let Err(err) = init_subscriber_with(&opts, exporter.layer()) {
                        warnings.push(format!("telemetría: {err}"));
                    }
                    return (
                        Self {
                            langfuse: Some(exporter),
                        },
                        warnings,
                    );
                }
                Ok(None) => {}
                Err(err) => warnings.push(format!("Langfuse desactivado: {err}")),
            }
        }
        if let Err(err) = adapter_telemetry::init_subscriber(&opts) {
            warnings.push(format!("telemetría: {err}"));
        }
        (Self::default(), warnings)
    }

    /// Flushes and stops the exporters. Call with the runtime still alive.
    pub fn shutdown(self) {
        #[cfg(feature = "langfuse")]
        if let Some(exporter) = self.langfuse
            && let Err(err) = exporter.shutdown()
        {
            eprintln!("aviso: Langfuse: {err}");
        }
    }
}
