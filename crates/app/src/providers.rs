//! Several model providers behind one [`LlmPort`]: the provider of each
//! model is the prefix of its id (`opencode-go:glm-5.3`), and ids without a
//! prefix go to the default provider (`app.opt.providers.default`).
//!
//! [`ProviderRouter`] is the [`ModelBackend`] that sends each attempt to
//! the provider of its model, so [`RetryingLlm`](crate::llm_retry::RetryingLlm)
//! can walk a fallback chain across providers. [`RouterCatalog`] lists the
//! models of every provider with their prefixes.
//!
//! ```lua
//! app.opt.providers = {
//!   default = "anthropic",                       -- or "opencode-go", "opencode-zen"
//!   opencode = {
//!     formats = { ["glm-5.3"] = "chat" },        -- "anthropic", "chat" or "responses"
//!   },
//! }
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use domain::model::{ModelId, ProviderId};
use futures::StreamExt;
use serde_json::{Map, Value};

use crate::llm_retry::ModelBackend;
use crate::ports::llm::{LlmError, LlmEvent, LlmRequest, LlmStream, ModelCatalogPort, ModelInfo};
use crate::ports::plugin::AppOptions;

/// Key of the options in `app.opt`.
pub const PROVIDERS_OPTION: &str = "providers";
/// Fields of `app.opt.providers`.
const FIELDS: [&str; 2] = ["default", "opencode"];
/// Fields of `app.opt.providers.opencode`.
const OPENCODE_FIELDS: [&str; 2] = ["base_url", "formats"];

/// The API shape a model is served with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WireFormat {
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
    /// OpenAI Chat Completions (`/v1/chat/completions`).
    Chat,
    /// OpenAI Responses (`/v1/responses`).
    Responses,
}

impl WireFormat {
    /// The name used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Chat => "chat",
            Self::Responses => "responses",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Anthropic, Self::Chat, Self::Responses]
            .into_iter()
            .find(|f| f.as_str() == s)
    }

    /// How OpenCode serves `model` (an id without prefix) when the config
    /// does not say: Claude, MiniMax and Qwen speak Messages; GPT, Grok and
    /// Muse speak Responses; the other open models speak Chat Completions.
    #[must_use]
    pub fn opencode_default(model: &str) -> Self {
        let model = model.to_ascii_lowercase();
        let starts = |prefixes: &[&str]| prefixes.iter().any(|p| model.starts_with(p));
        if starts(&["claude-", "qwen"])
            || (model.starts_with("minimax-m") && !model.contains("flash"))
        {
            Self::Anthropic
        } else if starts(&["gpt-", "grok-", "muse-", "o1", "o3", "o4"]) {
            Self::Responses
        } else {
            Self::Chat
        }
    }
}

/// A malformed `app.opt.providers`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvidersConfigError {
    /// Not a table.
    #[error("app.opt.{PROVIDERS_OPTION}{0} must be a table")]
    NotATable(&'static str),
    /// A field crest does not know.
    #[error("app.opt.{PROVIDERS_OPTION}: unknown field `{0}`")]
    UnknownField(String),
    /// A field of the wrong type or value.
    #[error("app.opt.{PROVIDERS_OPTION}.{field} {problem}")]
    BadField {
        /// The field.
        field: String,
        /// What is wrong with it.
        problem: String,
    },
}

/// `app.opt.providers`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvidersOptions {
    /// Provider of ids without prefix, when the config names one; otherwise
    /// [`effective_default`] picks it from the accounts with a key.
    pub default: Option<ProviderId>,
    /// OpenCode's inference base URL instead of the real one (tests).
    pub opencode_base_url: Option<String>,
    /// Wire format of OpenCode models (ids without prefix) that
    /// [`WireFormat::opencode_default`] gets wrong.
    pub opencode_formats: BTreeMap<String, WireFormat>,
}

impl ProvidersOptions {
    /// The `providers` option of `app.opt`, or the defaults.
    ///
    /// # Errors
    ///
    /// [`ProvidersConfigError`] when it is malformed.
    pub fn from_options(options: &AppOptions) -> Result<Self, ProvidersConfigError> {
        match options.extra.get(PROVIDERS_OPTION) {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(value) => Self::from_json(value),
        }
    }

    /// Parses the `app.opt.providers` table.
    ///
    /// # Errors
    ///
    /// [`ProvidersConfigError`] when it is malformed.
    pub fn from_json(value: &Value) -> Result<Self, ProvidersConfigError> {
        let empty = Map::new();
        let table = as_table(value, &empty).ok_or(ProvidersConfigError::NotATable(""))?;
        if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(ProvidersConfigError::UnknownField(unknown.clone()));
        }
        let mut out = Self::default();
        if let Some(default) = table.get("default").filter(|v| !v.is_null()) {
            out.default = default
                .as_str()
                .and_then(|s| s.trim().parse().ok())
                .map(Some)
                .ok_or_else(|| ProvidersConfigError::BadField {
                    field: "default".into(),
                    problem: format!(
                        "must be one of {}",
                        ProviderId::ALL.map(ProviderId::as_str).join(", ")
                    ),
                })?;
        }
        if let Some(opencode) = table.get("opencode").filter(|v| !v.is_null()) {
            let opencode =
                as_table(opencode, &empty).ok_or(ProvidersConfigError::NotATable(".opencode"))?;
            if let Some(unknown) = opencode
                .keys()
                .find(|key| !OPENCODE_FIELDS.contains(&key.as_str()))
            {
                return Err(ProvidersConfigError::UnknownField(format!(
                    "opencode.{unknown}"
                )));
            }
            out.opencode_base_url = match opencode.get("base_url") {
                None | Some(Value::Null) => None,
                Some(Value::String(url))
                    if url.starts_with("http://") || url.starts_with("https://") =>
                {
                    Some(url.trim_end_matches('/').to_owned())
                }
                Some(_) => {
                    return Err(ProvidersConfigError::BadField {
                        field: "opencode.base_url".into(),
                        problem: "must be an http(s) URL".into(),
                    });
                }
            };
            if let Some(formats) = opencode.get("formats").filter(|v| !v.is_null()) {
                let formats = as_table(formats, &empty)
                    .ok_or(ProvidersConfigError::NotATable(".opencode.formats"))?;
                for (model, format) in formats {
                    let format = format.as_str().and_then(WireFormat::parse).ok_or_else(|| {
                        ProvidersConfigError::BadField {
                            field: format!("opencode.formats.{model}"),
                            problem: "must be \"anthropic\", \"chat\" or \"responses\"".into(),
                        }
                    })?;
                    out.opencode_formats.insert(model.clone(), format);
                }
            }
        }
        Ok(out)
    }

    /// Wire format of the OpenCode model `model` (an id without prefix).
    #[must_use]
    pub fn opencode_format(&self, model: &str) -> WireFormat {
        self.opencode_formats
            .get(model)
            .copied()
            .unwrap_or_else(|| WireFormat::opencode_default(model))
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

/// The provider of ids without prefix: the one `app.opt.providers.default`
/// names; else the one of the last `/login` (`preferred`) if it has a key;
/// else the only account with a key (its first provider in `with_key`,
/// which follows [`ProviderId::ALL`]); else Anthropic.
#[must_use]
pub fn effective_default(
    configured: Option<ProviderId>,
    preferred: Option<ProviderId>,
    with_key: &[ProviderId],
) -> ProviderId {
    configured
        .or_else(|| preferred.filter(|p| with_key.contains(p)))
        .or_else(|| {
            let first = *with_key.first()?;
            with_key
                .iter()
                .all(|p| p.account() == first.account())
                .then_some(first)
        })
        .unwrap_or(ProviderId::Anthropic)
}

/// Splits `model` into its provider and the id that provider knows.
#[must_use]
pub fn route(model: &ModelId, default: ProviderId) -> (ProviderId, &str) {
    model.provider().unwrap_or((default, model.as_str()))
}

/// One configured provider.
#[derive(Clone)]
pub struct Provider {
    /// One attempt on one of its models (ids without prefix).
    pub backend: Arc<dyn ModelBackend>,
    /// Its models (ids without prefix).
    pub catalog: Arc<dyn ModelCatalogPort>,
}

/// [`ModelBackend`] that sends each model to its provider.
#[derive(Clone)]
pub struct ProviderRouter {
    default: ProviderId,
    providers: BTreeMap<ProviderId, Provider>,
}

impl std::fmt::Debug for ProviderRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRouter")
            .field("default", &self.default)
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ProviderRouter {
    /// A router with no provider; ids without prefix go to `default`.
    #[must_use]
    pub fn new(default: ProviderId) -> Self {
        Self {
            default,
            providers: BTreeMap::new(),
        }
    }

    /// Adds (or replaces) `id`.
    #[must_use]
    pub fn with(mut self, id: ProviderId, provider: Provider) -> Self {
        self.providers.insert(id, provider);
        self
    }

    /// Whether no provider is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// The configured providers.
    pub fn providers(&self) -> impl Iterator<Item = ProviderId> + '_ {
        self.providers.keys().copied()
    }

    /// The catalog of every provider, prefixed.
    #[must_use]
    pub fn catalog(&self) -> RouterCatalog {
        RouterCatalog {
            router: self.clone(),
        }
    }

    fn provider(&self, id: ProviderId) -> Result<&Provider, LlmError> {
        self.providers.get(&id).ok_or_else(|| {
            let account = id.account();
            LlmError::Auth {
                message: format!(
                    "no {} API key for `{id}` models: run /login {account} or export {}, \
                     or pick a model of another provider with /model",
                    account.display_name(),
                    account.key_name()
                ),
            }
        })
    }
}

#[async_trait]
impl ModelBackend for ProviderRouter {
    async fn open(&self, req: &LlmRequest, model: &ModelId) -> Result<LlmStream, LlmError> {
        let prefixed = model.provider().is_some();
        let (id, inner) = route(model, self.default);
        let provider = self.provider(id)?;
        let stream = provider.backend.open(req, &ModelId::new(inner)).await?;
        if !prefixed {
            return Ok(stream);
        }
        // Report the model with its prefix, as the caller named it.
        Ok(Box::pin(stream.map(move |item| match item {
            Ok(LlmEvent::MessageStart { model }) => Ok(LlmEvent::MessageStart {
                model: ModelId::with_provider(id, model.as_str()),
            }),
            other => other,
        })))
    }

    fn accepts_images(&self, model: &ModelId) -> bool {
        let (id, inner) = route(model, self.default);
        self.providers
            .get(&id)
            .is_some_and(|p| p.backend.accepts_images(&ModelId::new(inner)))
    }
}

/// [`ModelCatalogPort`] over every provider of a [`ProviderRouter`]. Every
/// model is listed with its prefix, and those of the default provider also
/// without it.
#[derive(Clone)]
pub struct RouterCatalog {
    router: ProviderRouter,
}

#[async_trait]
impl ModelCatalogPort for RouterCatalog {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let lists = futures::future::join_all(
            self.router
                .providers
                .iter()
                .map(|(id, p)| async move { (*id, p.catalog.list_models().await) }),
        )
        .await;
        let mut out = Vec::new();
        for (id, list) in lists {
            for model in list? {
                if id == self.router.default {
                    out.push(model.clone());
                }
                out.push(ModelInfo {
                    id: ModelId::with_provider(id, model.id.as_str()),
                    ..model
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::Message;
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
    use futures::executor::block_on;
    use serde_json::json;

    use super::*;

    /// Answers every model with a `MessageStart` naming it and records them.
    struct Echo {
        seen: std::sync::Mutex<Vec<String>>,
        models: Vec<&'static str>,
    }

    impl Echo {
        fn new(models: Vec<&'static str>) -> Arc<Self> {
            Arc::new(Self {
                seen: std::sync::Mutex::default(),
                models,
            })
        }
    }

    #[async_trait]
    impl ModelBackend for Echo {
        async fn open(&self, _req: &LlmRequest, model: &ModelId) -> Result<LlmStream, LlmError> {
            self.seen.lock().unwrap().push(model.to_string());
            let event = LlmEvent::MessageStart {
                model: model.clone(),
            };
            Ok(Box::pin(futures::stream::iter([Ok(event)])))
        }

        fn accepts_images(&self, model: &ModelId) -> bool {
            model.as_str().starts_with("claude-")
        }
    }

    #[async_trait]
    impl ModelCatalogPort for Echo {
        async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
            Ok(self
                .models
                .iter()
                .map(|id| ModelInfo {
                    id: ModelId::from(*id),
                    display_name: (*id).to_owned(),
                    created_at: None,
                })
                .collect())
        }
    }

    fn provider(echo: &Arc<Echo>) -> Provider {
        Provider {
            backend: echo.clone(),
            catalog: echo.clone(),
        }
    }

    fn request(model: &str) -> LlmRequest {
        let model = ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new(model, 256),
            source: SpecSource::Default,
            warnings: vec![],
        };
        LlmRequest::new(model, vec![Message::user_text("Hola")])
    }

    fn first_model(router: &ProviderRouter, model: &str) -> Result<ModelId, LlmError> {
        block_on(async {
            let req = request(model);
            let mut stream = router.open(&req, &ModelId::from(model)).await?;
            match stream.next().await {
                Some(Ok(LlmEvent::MessageStart { model })) => Ok(model),
                other => panic!("unexpected {other:?}"),
            }
        })
    }

    #[test]
    fn each_model_goes_to_its_provider() {
        let anthropic = Echo::new(vec!["claude-haiku-5-5"]);
        let opencode = Echo::new(vec!["glm-5.3"]);
        let router = ProviderRouter::new(ProviderId::Anthropic)
            .with(ProviderId::Anthropic, provider(&anthropic))
            .with(ProviderId::OpencodeGo, provider(&opencode));

        assert_eq!(
            first_model(&router, "claude-haiku-5-5").unwrap().as_str(),
            "claude-haiku-5-5"
        );
        assert_eq!(
            first_model(&router, "opencode-go:glm-5.3")
                .unwrap()
                .as_str(),
            "opencode-go:glm-5.3"
        );
        assert_eq!(*anthropic.seen.lock().unwrap(), ["claude-haiku-5-5"]);
        assert_eq!(*opencode.seen.lock().unwrap(), ["glm-5.3"]);
        assert!(router.accepts_images(&ModelId::from("claude-haiku-5-5")));
        assert!(!router.accepts_images(&ModelId::from("opencode-go:glm-5.3")));
    }

    #[test]
    fn a_provider_without_key_says_how_to_log_in() {
        let router = ProviderRouter::new(ProviderId::Anthropic)
            .with(ProviderId::Anthropic, provider(&Echo::new(vec![])));
        let err = first_model(&router, "opencode-zen:claude-sonnet-5-5").unwrap_err();
        let LlmError::Auth { message } = err else {
            panic!("{err:?}")
        };
        assert!(message.contains("/login opencode"), "{message}");
        assert!(message.contains("OPENCODE_API_KEY"), "{message}");
    }

    #[test]
    fn ids_without_prefix_go_to_the_default_provider() {
        let opencode = Echo::new(vec!["glm-5.3"]);
        let router = ProviderRouter::new(ProviderId::OpencodeGo)
            .with(ProviderId::OpencodeGo, provider(&opencode));
        assert_eq!(first_model(&router, "glm-5.3").unwrap().as_str(), "glm-5.3");
        let ids: Vec<String> = block_on(router.catalog().list_models())
            .unwrap()
            .into_iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(ids, ["glm-5.3", "opencode-go:glm-5.3"]);
    }

    #[test]
    fn the_catalog_prefixes_every_provider() {
        let router = ProviderRouter::new(ProviderId::Anthropic)
            .with(
                ProviderId::Anthropic,
                provider(&Echo::new(vec!["claude-opus-5-5"])),
            )
            .with(
                ProviderId::OpencodeGo,
                provider(&Echo::new(vec!["kimi-k3"])),
            );
        let ids: Vec<String> = block_on(router.catalog().list_models())
            .unwrap()
            .into_iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(
            ids,
            [
                "claude-opus-5-5",
                "anthropic:claude-opus-5-5",
                "opencode-go:kimi-k3"
            ]
        );
    }

    #[test]
    fn options_parse_and_reject_mistakes() {
        let options = ProvidersOptions::from_json(&json!({
            "default": "opencode-go",
            "opencode": { "formats": { "my-model": "responses" } },
        }))
        .unwrap();
        assert_eq!(options.default, Some(ProviderId::OpencodeGo));
        assert_eq!(options.opencode_format("my-model"), WireFormat::Responses);
        assert_eq!(options.opencode_format("glm-5.3"), WireFormat::Chat);
        assert_eq!(
            ProvidersOptions::from_json(&json!([])).unwrap(),
            ProvidersOptions::default()
        );

        for bad in [
            json!({ "default": "openai" }),
            json!({ "deafult": "anthropic" }),
            json!({ "opencode": { "formats": { "x": "grpc" } } }),
            json!({ "opencode": { "base_url": "opencode.ai" } }),
            json!("anthropic"),
        ] {
            assert!(ProvidersOptions::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn opencode_formats_follow_the_model_family() {
        for (model, format) in [
            ("claude-haiku-5-5", WireFormat::Anthropic),
            ("minimax-m3", WireFormat::Anthropic),
            ("qwen3.8-max", WireFormat::Anthropic),
            ("gpt-6-luna", WireFormat::Responses),
            ("grok-4.7", WireFormat::Responses),
            ("glm-5.3", WireFormat::Chat),
            ("kimi-k3", WireFormat::Chat),
            ("deepseek-v4-pro", WireFormat::Chat),
            ("minimax-m3.1-flash", WireFormat::Chat),
        ] {
            assert_eq!(WireFormat::opencode_default(model), format, "{model}");
        }
    }

    #[test]
    fn the_effective_default_follows_config_login_and_keys() {
        use ProviderId::{Anthropic, OpencodeGo, OpencodeZen};
        let both = [Anthropic, OpencodeGo, OpencodeZen];
        let opencode = [OpencodeGo, OpencodeZen];
        assert_eq!(
            effective_default(Some(Anthropic), Some(OpencodeGo), &opencode),
            Anthropic
        );
        assert_eq!(effective_default(None, Some(OpencodeGo), &both), OpencodeGo);
        assert_eq!(effective_default(None, Some(Anthropic), &both), Anthropic);
        // The last login has no key any more: the remaining account.
        assert_eq!(
            effective_default(None, Some(Anthropic), &opencode),
            OpencodeGo
        );
        assert_eq!(effective_default(None, None, &opencode), OpencodeGo);
        assert_eq!(effective_default(None, None, &both), Anthropic);
        assert_eq!(effective_default(None, None, &[]), Anthropic);
    }
}
