//! Installation of the global `tracing` subscriber.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing::Subscriber;
use tracing_subscriber::filter::{EnvFilter, ParseError};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::{Identity, Layer, Layered, SubscriberExt};
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};
use tracing_subscriber::{Registry, fmt};

/// Environment variable with the log filter, in `EnvFilter` syntax
/// (for example `info,crest::telemetry=debug`).
pub const LOG_ENV: &str = "CREST_LOG";

/// Where the human-readable log goes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LogOutput {
    /// Standard error, with colours when it is a terminal.
    #[default]
    Stderr,
    /// A file, opened in append mode and created if missing.
    File(PathBuf),
}

/// Options of [`init_subscriber`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberOptions {
    /// Where the log goes.
    pub output: LogOutput,
    /// Filter used when `CREST_LOG` is unset or empty.
    pub default_filter: String,
}

impl Default for SubscriberOptions {
    fn default() -> Self {
        Self {
            output: LogOutput::Stderr,
            default_filter: "warn".into(),
        }
    }
}

/// Why the subscriber could not be installed.
#[derive(Debug, thiserror::Error)]
pub enum SubscriberError {
    /// `CREST_LOG` or the default filter is not a valid filter.
    #[error("invalid log filter {filter:?}: {source}")]
    InvalidFilter {
        /// The filter as written.
        filter: String,
        /// Parser error.
        source: ParseError,
    },
    /// The log file could not be opened.
    #[error("cannot open log file {}: {source}", path.display())]
    OpenLog {
        /// Path of the log file.
        path: PathBuf,
        /// I/O error.
        source: io::Error,
    },
    /// A global subscriber was already installed.
    #[error("a global tracing subscriber is already installed: {0}")]
    AlreadyInstalled(#[from] TryInitError),
}

/// Installs the global subscriber: logs to `opts.output`, filtered by
/// `CREST_LOG` (or `opts.default_filter`). Turn spans are logged when
/// they close, with their timing and fields.
///
/// # Errors
///
/// Fails when the filter is invalid, the log file cannot be opened or a
/// global subscriber already exists.
pub fn init_subscriber(opts: &SubscriberOptions) -> Result<(), SubscriberError> {
    init_subscriber_with(opts, Identity::new())
}

/// Like [`init_subscriber`], with an extra layer next to the log (for
/// example an OpenTelemetry exporter). `CREST_LOG` only filters the log:
/// the extra layer brings its own filter, so exporting traces does not
/// depend on how verbose the log is.
///
/// # Errors
///
/// Same as [`init_subscriber`].
pub fn init_subscriber_with<L>(opts: &SubscriberOptions, layer: L) -> Result<(), SubscriberError>
where
    L: Layer<Registry> + Send + Sync + 'static,
{
    let env = std::env::var(LOG_ENV).ok();
    build_subscriber(opts, env.as_deref(), layer)?.try_init()?;
    Ok(())
}

/// Builds the subscriber without installing it; `env` is the value of
/// `CREST_LOG`.
fn build_subscriber<L>(
    opts: &SubscriberOptions,
    env: Option<&str>,
    layer: L,
) -> Result<impl Subscriber + Send + Sync + 'static, SubscriberError>
where
    L: Layer<Registry> + Send + Sync + 'static,
{
    let filter = parse_filter(env, &opts.default_filter)?;
    let log: Box<dyn Layer<Layered<L, Registry>> + Send + Sync> = match &opts.output {
        LogOutput::Stderr => fmt::layer()
            .with_writer(io::stderr)
            .with_span_events(FmtSpan::CLOSE)
            .boxed(),
        LogOutput::File(path) => fmt::layer()
            .with_writer(Mutex::new(open_log(path)?))
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .boxed(),
    };
    Ok(Registry::default()
        .with(layer)
        .with(log.with_filter(filter)))
}

/// The filter from `CREST_LOG` when it is set and not blank, otherwise
/// `default`.
fn parse_filter(env: Option<&str>, default: &str) -> Result<EnvFilter, SubscriberError> {
    let filter = env
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default);
    EnvFilter::builder()
        .parse(filter)
        .map_err(|source| SubscriberError::InvalidFilter {
            filter: filter.to_owned(),
            source,
        })
}

fn open_log(path: &PathBuf) -> Result<File, SubscriberError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| SubscriberError::OpenLog {
            path: path.clone(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use application::ports::telemetry::{LlmCallRecord, TelemetryPort, TurnSpan};
    use domain::conversation::{SessionId, StopReason, TurnId, Usage};
    use domain::model::{ModelId, ModelRole};

    use super::*;
    use crate::TracingTelemetry;

    fn file_opts(path: PathBuf, default_filter: &str) -> SubscriberOptions {
        SubscriberOptions {
            output: LogOutput::File(path),
            default_filter: default_filter.into(),
        }
    }

    fn run_turn() {
        let telemetry = TracingTelemetry::new();
        let span = telemetry.turn_started(TurnSpan {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            role: ModelRole::Main,
            model: ModelId::new("claude-sonnet-5-5"),
        });
        telemetry.llm_call(LlmCallRecord {
            parent: span,
            model: ModelId::new("claude-sonnet-5-5"),
            usage: Usage {
                input_tokens: 7,
                ..Usage::default()
            },
            latency_ms: 9,
            error: None,
        });
        telemetry.turn_finished(span, Usage::default(), Some(StopReason::EndTurn));
    }

    #[test]
    fn env_filter_wins_over_default_unless_blank() {
        assert_eq!(
            parse_filter(Some("debug"), "warn").unwrap().to_string(),
            "debug"
        );
        assert_eq!(
            parse_filter(Some("  "), "warn").unwrap().to_string(),
            "warn"
        );
        assert_eq!(parse_filter(None, "info").unwrap().to_string(), "info");
    }

    #[test]
    fn invalid_filter_is_an_error() {
        let err = parse_filter(Some("crest=loud"), "warn").unwrap_err();
        assert!(matches!(err, SubscriberError::InvalidFilter { .. }));
    }

    #[test]
    fn file_output_logs_closed_spans_with_their_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crest.log");
        let subscriber =
            build_subscriber(&file_opts(path.clone(), "info"), None, Identity::new()).unwrap();
        tracing::subscriber::with_default(subscriber, run_turn);

        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("}:llm_call{"), "{log}");
        assert!(log.contains("input_tokens=7"), "{log}");
        assert!(log.contains("stop_reason=\"end_turn\""), "{log}");
        assert!(!log.contains('\u{1b}'), "no ANSI colours in files: {log}");
    }

    #[test]
    fn filter_from_env_silences_spans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crest.log");
        let subscriber = build_subscriber(
            &file_opts(path.clone(), "info"),
            Some("error"),
            Identity::new(),
        )
        .unwrap();
        tracing::subscriber::with_default(subscriber, run_turn);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    /// Counts the spans it sees.
    #[derive(Clone, Default)]
    struct CountSpans(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl<S: Subscriber> Layer<S> for CountSpans {
        fn on_new_span(
            &self,
            _attrs: &tracing::span::Attributes<'_>,
            _id: &tracing::span::Id,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn log_filter_does_not_silence_the_extra_layer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crest.log");
        let count = CountSpans::default();
        let subscriber = build_subscriber(
            &file_opts(path.clone(), "info"),
            Some("error"),
            count.clone(),
        )
        .unwrap();
        tracing::subscriber::with_default(subscriber, run_turn);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert_eq!(count.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn unopenable_log_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let opts = file_opts(dir.path().join("missing").join("crest.log"), "info");
        let err = build_subscriber(&opts, None, Identity::new())
            .err()
            .unwrap();
        assert!(matches!(err, SubscriberError::OpenLog { .. }));
    }
}
