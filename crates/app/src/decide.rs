//! The optional decision model and its options (`app.opt.decision`).
//!
//! A [`Decider`] wraps the [`DecisionPort`] when there is one. Its calls
//! return `None` when there is none or it fails (the failure is logged),
//! so each use keeps its previous behaviour as the fallback:
//!
//! - the measurability judge of the research flow's Definition stage;
//! - answers typed at the flows' gates that the fixed phrases miss;
//! - the tool groups and the model of each chat prompt.
//!
//! ```lua
//! app.opt.decision = {
//!   enabled = true,          -- used when CLOUDFLARE_API_TOKEN/ACCOUNT_ID are set
//!   model = "clef-flash",    -- or "clef"
//!   account_id = "…",        -- instead of CLOUDFLARE_ACCOUNT_ID
//!   timeout_ms = 1500,
//!   tools = true,            -- pick the tool groups of each prompt
//!   model_routing = true,    -- pick the model of each prompt
//!   models = { trivial = "haiku", hard = "opus" },
//! }
//! ```

use std::sync::Arc;

use domain::decision::{DecisionRequest, DecisionResponse};
use serde_json::{Map, Value};

use crate::ports::decision::DecisionPort;
use crate::ports::plugin::AppOptions;

/// Key of the options in `app.opt`.
pub const DECISION_OPTION: &str = "decision";
/// The default model.
pub const DEFAULT_MODEL: &str = "clef-flash";
/// The default timeout of one decision, in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 1_500;
/// Longest timeout allowed, in milliseconds.
const MAX_TIMEOUT_MS: u64 = 30_000;
/// Fields of `app.opt.decision`.
const FIELDS: [&str; 8] = [
    "enabled",
    "model",
    "account_id",
    "base_url",
    "timeout_ms",
    "tools",
    "model_routing",
    "models",
];

/// A malformed `app.opt.decision`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionConfigError {
    /// Not a table.
    #[error("app.opt.{DECISION_OPTION} must be a table")]
    NotATable,
    /// A field crest does not know.
    #[error("app.opt.{DECISION_OPTION}: unknown field `{0}` (known: {known})", known = FIELDS.join(", "))]
    UnknownField(String),
    /// A field of the wrong type or out of range.
    #[error("app.opt.{DECISION_OPTION}.{field} {problem}")]
    BadField {
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: String,
    },
}

/// The models a prompt is routed to by difficulty; a normal one keeps the
/// `main` role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedModels {
    /// Model (or alias) of trivial prompts.
    pub trivial: String,
    /// Model (or alias) of hard prompts.
    pub hard: String,
}

impl Default for RoutedModels {
    fn default() -> Self {
        Self {
            trivial: "haiku".into(),
            hard: "opus".into(),
        }
    }
}

/// `app.opt.decision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOptions {
    /// Whether the decision model is used (when it has credentials).
    pub enabled: bool,
    /// Model id: `clef-flash` or `clef`.
    pub model: String,
    /// Cloudflare account, instead of `CLOUDFLARE_ACCOUNT_ID`.
    pub account_id: Option<String>,
    /// API base URL instead of Cloudflare's (tests, a gateway).
    pub base_url: Option<String>,
    /// Timeout of one decision, in milliseconds.
    pub timeout_ms: u64,
    /// Whether each chat prompt gets only the tool groups it needs.
    pub tools: bool,
    /// Whether each chat prompt is routed to a model by difficulty.
    pub model_routing: bool,
    /// The models of the routing.
    pub models: RoutedModels,
}

impl Default for DecisionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            model: DEFAULT_MODEL.into(),
            account_id: None,
            base_url: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            tools: true,
            model_routing: true,
            models: RoutedModels::default(),
        }
    }
}

impl DecisionOptions {
    /// The `decision` option of `app.opt`, or the defaults.
    ///
    /// # Errors
    ///
    /// [`DecisionConfigError`] when it is malformed.
    pub fn from_options(options: &AppOptions) -> Result<Self, DecisionConfigError> {
        match options.extra.get(DECISION_OPTION) {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(value) => Self::from_json(value),
        }
    }

    /// Parses the `app.opt.decision` table.
    ///
    /// # Errors
    ///
    /// [`DecisionConfigError`] when it is malformed.
    pub fn from_json(value: &Value) -> Result<Self, DecisionConfigError> {
        let empty = Map::new();
        let table = as_table(value, &empty).ok_or(DecisionConfigError::NotATable)?;
        if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(DecisionConfigError::UnknownField(unknown.clone()));
        }
        let mut out = Self::default();
        for (field, slot) in [
            ("enabled", &mut out.enabled),
            ("tools", &mut out.tools),
            ("model_routing", &mut out.model_routing),
        ] {
            if let Some(value) = table.get(field) {
                *slot = value.as_bool().ok_or(DecisionConfigError::BadField {
                    field,
                    problem: "must be true or false".into(),
                })?;
            }
        }
        if let Some(model) = text(table, "model")? {
            out.model = model;
        }
        out.account_id = text(table, "account_id")?;
        out.base_url = match text(table, "base_url")? {
            Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
                Some(url.trim_end_matches('/').to_owned())
            }
            Some(_) => {
                return Err(DecisionConfigError::BadField {
                    field: "base_url",
                    problem: "must be an http or https URL".into(),
                });
            }
            None => None,
        };
        if let Some(value) = table.get("timeout_ms") {
            out.timeout_ms = value
                .as_u64()
                .filter(|n| (1..=MAX_TIMEOUT_MS).contains(n))
                .ok_or(DecisionConfigError::BadField {
                    field: "timeout_ms",
                    problem: format!("must be a whole number from 1 to {MAX_TIMEOUT_MS}"),
                })?;
        }
        if let Some(value) = table.get("models") {
            let models = as_table(value, &empty).ok_or(DecisionConfigError::BadField {
                field: "models",
                problem: "must be a table with trivial and hard".into(),
            })?;
            if let Some(unknown) = models
                .keys()
                .find(|k| !matches!(k.as_str(), "trivial" | "hard"))
            {
                return Err(DecisionConfigError::BadField {
                    field: "models",
                    problem: format!("has an unknown field `{unknown}` (known: trivial, hard)"),
                });
            }
            if let Some(trivial) = text(models, "trivial")? {
                out.models.trivial = trivial;
            }
            if let Some(hard) = text(models, "hard")? {
                out.models.hard = hard;
            }
        }
        Ok(out)
    }
}

fn as_table<'a>(value: &'a Value, empty: &'a Map<String, Value>) -> Option<&'a Map<String, Value>> {
    match value {
        Value::Object(table) => Some(table),
        // `{}` in Lua can come as an empty list.
        Value::Array(items) if items.is_empty() => Some(empty),
        _ => None,
    }
}

fn text(
    table: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, DecisionConfigError> {
    match table.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_owned())),
        Some(_) => Err(DecisionConfigError::BadField {
            field,
            problem: "must be a non-empty string".into(),
        }),
    }
}

/// The decision model, if there is one, and its options.
#[derive(Clone, Default)]
pub struct Decider {
    port: Option<Arc<dyn DecisionPort>>,
    options: DecisionOptions,
}

impl std::fmt::Debug for Decider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decider")
            .field("model", &self.port.as_ref().map(|p| p.model().to_owned()))
            .field("options", &self.options)
            .finish()
    }
}

impl Decider {
    /// No decision model: every call returns `None`.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// `port` with `options`; disabled options turn it off.
    #[must_use]
    pub fn new(port: Arc<dyn DecisionPort>, options: DecisionOptions) -> Self {
        let port = options.enabled.then_some(port);
        Self { port, options }
    }

    /// Whether a decision model is available.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.port.is_some()
    }

    /// The model id, when available.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.port.as_deref().map(DecisionPort::model)
    }

    /// The options.
    #[must_use]
    pub fn options(&self) -> &DecisionOptions {
        &self.options
    }

    /// Answers `request`, or `None` without a model, for an invalid
    /// request or when the model fails (logged).
    pub async fn decide(&self, request: &DecisionRequest) -> Option<DecisionResponse> {
        let port = self.port.as_ref()?;
        if let Err(err) = request.validate() {
            tracing::warn!(error = %err, "invalid decision request");
            return None;
        }
        match port.decide(request).await {
            Ok(response) => Some(response),
            Err(err) => {
                tracing::warn!(error = %err, model = port.model(), "decision failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::decision::Question;
    use serde_json::json;

    use super::*;
    use crate::testing::decision::ScriptedDecider;

    #[test]
    fn options_parse_with_defaults() {
        assert_eq!(
            DecisionOptions::from_json(&json!({})).unwrap(),
            DecisionOptions::default()
        );
        let options = DecisionOptions::from_json(&json!({
            "enabled": true, "model": "clef", "account_id": "abc",
            "base_url": "http://localhost:9/", "timeout_ms": 900, "tools": false,
            "model_routing": false, "models": { "trivial": "haiku", "hard": "fable" }
        }))
        .unwrap();
        assert_eq!(options.model, "clef");
        assert_eq!(options.account_id.as_deref(), Some("abc"));
        assert_eq!(options.base_url.as_deref(), Some("http://localhost:9"));
        assert_eq!(options.timeout_ms, 900);
        assert!(!options.tools && !options.model_routing);
        assert_eq!(options.models.hard, "fable");
    }

    #[test]
    fn malformed_options_are_explained() {
        for (value, needle) in [
            (json!(3), "must be a table"),
            (json!({ "colour": 1 }), "unknown field `colour`"),
            (json!({ "enabled": "yes" }), "enabled must be true or false"),
            (json!({ "base_url": "ftp://x" }), "base_url must be an http"),
            (
                json!({ "timeout_ms": 0 }),
                "timeout_ms must be a whole number",
            ),
            (
                json!({ "models": { "easy": "haiku" } }),
                "unknown field `easy`",
            ),
            (json!({ "model": "" }), "model must be a non-empty string"),
        ] {
            let err = DecisionOptions::from_json(&value).unwrap_err().to_string();
            assert!(err.contains(needle), "{value}: {err}");
        }
    }

    #[tokio::test]
    async fn the_decider_answers_only_when_it_can() {
        let request = DecisionRequest::new("hi").ask("q", Question::yes_no("?"));
        assert!(Decider::none().decide(&request).await.is_none());

        let scripted = Arc::new(ScriptedDecider::yes(0.9));
        let decider = Decider::new(Arc::clone(&scripted) as _, DecisionOptions::default());
        assert!(decider.is_available());
        assert_eq!(decider.decide(&request).await.unwrap().yes("q"), Some(0.9));
        assert!(
            decider.decide(&DecisionRequest::new("x")).await.is_none(),
            "invalid requests are not sent"
        );
        assert_eq!(scripted.requests().len(), 1);

        let off = DecisionOptions {
            enabled: false,
            ..DecisionOptions::default()
        };
        assert!(!Decider::new(scripted as _, off).is_available());

        let failing = Decider::new(
            Arc::new(ScriptedDecider::failing()),
            DecisionOptions::default(),
        );
        assert!(failing.decide(&request).await.is_none());
    }
}
