//! OpenCode (Go and Zen) as a model provider: one key, three API shapes.
//!
//! OpenCode serves each model with the API of its family (see
//! [`ProvidersOptions::opencode_format`]): Claude, MiniMax and Qwen through
//! an Anthropic Messages endpoint, GPT and Grok through OpenAI Responses,
//! and the other open models through Chat Completions. Gemini models use
//! Google's API, which crest does not speak.
//!
//! Every request carries `User-Agent: crest/<version>` and the
//! `x-opencode-session` header OpenCode requires (requests without it are
//! refused with `MissingSessionID`): the crest session id, or one id per
//! process for calls outside a session.
//!
//! OpenCode lists every model of a plan to any key, but answers 403 to the
//! ones the account cannot use: Go without a subscription, and Zen's free
//! models outside OpenCode's own client. [`probe_access`] asks once with a
//! one-token request, and [`provider`] hides what the answer ruled out.

use std::sync::Arc;

use adapter_anthropic::{AnthropicBackend, AnthropicClient, ApiKey, ClientConfig};
use adapter_openai::{Api, OpenAiBackend, OpenAiCatalog, OpenAiClient, OpenAiConfig};
use application::llm_retry::ModelBackend;
use application::ports::credentials::KeyCheck;
use application::ports::llm::{LlmError, LlmRequest, LlmStream, ModelCatalogPort, ModelInfo};
use application::providers::{Provider, ProvidersOptions, WireFormat};
use async_trait::async_trait;
use domain::conversation::{Message, SessionId};
use domain::model::{ModelId, ModelRole, ModelSpec, ProviderId, ResolvedModel, SpecSource};
use serde::{Deserialize, Serialize};

/// OpenCode's inference base URL.
pub const BASE_URL: &str = "https://opencode.ai/inference";
/// Header OpenCode routes requests with.
const SESSION_HEADER: &str = "x-opencode-session";

/// The user agent crest identifies itself with.
fn user_agent() -> String {
    format!("crest/{}", env!("CARGO_PKG_VERSION"))
}

/// Where the endpoints of one OpenCode plan live.
fn plan_base(base: &str, plan: ProviderId) -> String {
    match plan {
        ProviderId::OpencodeGo => format!("{base}/go"),
        _ => base.to_owned(),
    }
}

fn openai_config(base: &str) -> OpenAiConfig {
    OpenAiConfig {
        base_url: base.to_owned(),
        user_agent: Some(user_agent()),
        session_header: Some(SESSION_HEADER.to_owned()),
        provider: "OpenCode".to_owned(),
        ..OpenAiConfig::default()
    }
}

/// What an OpenCode key may use, as far as [`probe_access`] found out;
/// `None` is not known (never asked, or the answer was not a yes or a no).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAccess {
    /// The Go plan (a subscription).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub go: Option<bool>,
    /// Zen's paid models (a balance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zen: Option<bool>,
    /// Zen's free models, which OpenCode keeps to its own client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zen_free: Option<bool>,
    /// When it was asked, in seconds since the Unix epoch.
    #[serde(default)]
    pub checked_at: u64,
}

impl PlanAccess {
    /// Whether `plan` may serve anything: only a known no rules it out.
    #[must_use]
    pub fn serves(&self, plan: ProviderId) -> bool {
        match plan {
            ProviderId::OpencodeGo => self.go != Some(false),
            ProviderId::OpencodeZen => self.zen != Some(false) || self.zen_free != Some(false),
            _ => true,
        }
    }

    /// Whether every plan answered a yes or a no.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.go.is_some() && self.zen.is_some() && self.zen_free.is_some()
    }

    /// Whether `plan` is known to serve something: it answered.
    #[must_use]
    pub fn works(&self, plan: ProviderId) -> bool {
        match plan {
            ProviderId::OpencodeGo => self.go == Some(true),
            ProviderId::OpencodeZen => self.zen == Some(true) || self.zen_free == Some(true),
            _ => true,
        }
    }

    /// Whether `model` (an id without prefix) of `plan` can be used.
    #[must_use]
    pub fn allows(&self, plan: ProviderId, model: &str) -> bool {
        match plan {
            ProviderId::OpencodeGo => self.go != Some(false),
            ProviderId::OpencodeZen if is_free(model) => self.zen_free != Some(false),
            ProviderId::OpencodeZen => self.zen != Some(false),
            _ => true,
        }
    }
}

/// Whether `model` is one of Zen's free models. OpenCode does not say so
/// in the model list: free ids end in `-free`, and `big-pickle` is the
/// stealth model it offers for free.
fn is_free(model: &str) -> bool {
    model.ends_with("-free") || model == "big-pickle"
}

/// Asks OpenCode with `key` which plans it may use: one request of one
/// token per plan (to a model of its own list), whose 403 means no. Each
/// question is cut off after `timeout`, which leaves it unknown.
pub async fn probe_access(
    key: &str,
    options: &ProvidersOptions,
    timeout: std::time::Duration,
) -> PlanAccess {
    let all = PlanAccess::default();
    let (Ok(go), Ok(zen)) = (
        provider(key, ProviderId::OpencodeGo, options, all),
        provider(key, ProviderId::OpencodeZen, options, all),
    ) else {
        return all;
    };
    let ask = |provider: Provider, pick: fn(&[ModelInfo]) -> Option<ModelId>| async move {
        let models = tokio::time::timeout(timeout, provider.catalog.list_models())
            .await
            .ok()?
            .ok()?;
        let model = pick(&models)?;
        let request = probe_request(&model);
        match tokio::time::timeout(timeout, provider.backend.open(&request, &model)).await {
            // Dropping the stream closes the connection.
            Ok(Ok(_)) => Some(true),
            Ok(Err(LlmError::NotEntitled { .. })) => Some(false),
            Ok(Err(_)) | Err(_) => None,
        }
    };
    let (go, zen, zen_free) = futures::join!(
        ask(go, |models| first_of(models, |id| !is_free(id))),
        ask(zen.clone(), cheapest_paid),
        ask(zen, |models| first_of(models, is_free)),
    );
    PlanAccess {
        go,
        zen,
        zen_free,
        checked_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    }
}

/// The first model of `models` crest can speak to that `keep` accepts.
fn first_of(models: &[ModelInfo], keep: fn(&str) -> bool) -> Option<ModelId> {
    models
        .iter()
        .map(|m| m.id.as_str())
        .find(|id| !id.starts_with("gemini-") && keep(id))
        .map(ModelId::from)
}

/// A paid Zen model likely to cost least for one token: a small one if
/// the list has it, else the first paid one.
fn cheapest_paid(models: &[ModelInfo]) -> Option<ModelId> {
    let paid = |id: &str| !is_free(id) && id != "test";
    ["flash", "haiku", "mini", "nano"]
        .iter()
        .find_map(|small| {
            models
                .iter()
                .map(|m| m.id.as_str())
                .find(|id| paid(id) && !id.starts_with("gemini-") && id.contains(small))
        })
        .map(ModelId::from)
        .or_else(|| first_of(models, |id| !is_free(id) && id != "test"))
}

/// One token of "hi" to `model`.
fn probe_request(model: &ModelId) -> LlmRequest {
    let resolved = ResolvedModel {
        role: ModelRole::Title,
        spec: ModelSpec::new(model.clone(), 1),
        source: SpecSource::Default,
        warnings: vec![],
    };
    let mut request = LlmRequest::new(resolved, vec![Message::user_text("hi")]);
    request.max_tokens = Some(1);
    request
}

/// The model list of one plan without the models `access` rules out.
struct PlanCatalog {
    inner: OpenAiCatalog,
    plan: ProviderId,
    access: PlanAccess,
}

#[async_trait]
impl ModelCatalogPort for PlanCatalog {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        if !self.access.serves(self.plan) {
            return Ok(Vec::new());
        }
        let mut models = self.inner.list_models().await?;
        models.retain(|m| self.access.allows(self.plan, m.id.as_str()));
        Ok(models)
    }
}

/// The OpenCode provider `plan` (Go or Zen) with `key`; its catalog leaves
/// out what `access` rules out.
///
/// # Errors
///
/// When a client cannot be built (an invalid key or TLS failure).
pub fn provider(
    key: &str,
    plan: ProviderId,
    options: &ProvidersOptions,
    access: PlanAccess,
) -> Result<Provider, LlmError> {
    let base = options.opencode_base_url.as_deref().unwrap_or(BASE_URL);
    let base = plan_base(base, plan);
    let anthropic = AnthropicClient::new(
        &ApiKey::new(key)?,
        ClientConfig {
            base_url: format!("{base}/anthropic"),
            bearer: true,
            user_agent: Some(user_agent()),
            session_header: Some(SESSION_HEADER.to_owned()),
            ..ClientConfig::default()
        },
    )?;
    let openai = OpenAiClient::new(key, openai_config(&format!("{base}/openai/v1")))?;
    let catalog = OpenAiClient::new(key, openai_config(&format!("{base}/v1")))?;
    let backend = OpencodeBackend {
        anthropic: AnthropicBackend::new(anthropic),
        chat: OpenAiBackend::new(openai.clone(), Api::Chat),
        responses: OpenAiBackend::new(openai, Api::Responses),
        options: options.clone(),
        fallback_session: SessionId::new(),
    };
    Ok(Provider {
        backend: Arc::new(backend),
        catalog: Arc::new(PlanCatalog {
            inner: OpenAiCatalog::new(catalog),
            plan,
            access,
        }),
    })
}

/// Asks OpenCode whether `key` works: the model list answers 401 to a bad
/// key (and to no key, it answers anyway).
pub async fn check_key(key: &str, options: &ProvidersOptions) -> KeyCheck {
    let base = options.opencode_base_url.as_deref().unwrap_or(BASE_URL);
    let config = OpenAiConfig {
        connect_timeout: adapter_anthropic::credentials::CHECK_TIMEOUT,
        read_timeout: adapter_anthropic::credentials::CHECK_TIMEOUT,
        ..openai_config(&format!("{base}/v1"))
    };
    let client = match OpenAiClient::new(key, config) {
        Ok(client) => client,
        Err(_) => return KeyCheck::Malformed("paste your OpenCode API key".into()),
    };
    let catalog = OpenAiCatalog::new(client);
    let timeout = adapter_anthropic::credentials::CHECK_TIMEOUT;
    match tokio::time::timeout(timeout, catalog.list_models()).await {
        Ok(Ok(_)) => KeyCheck::Valid,
        Ok(Err(LlmError::Auth { .. })) => KeyCheck::Rejected(
            "OpenCode rejected this key; check that you copied the whole key".into(),
        ),
        Ok(Err(err)) => KeyCheck::Unreachable(err.to_string()),
        Err(_) => KeyCheck::Unreachable("no answer from OpenCode".into()),
    }
}

/// [`ModelBackend`] that sends each OpenCode model to the endpoint of its
/// API shape.
struct OpencodeBackend {
    anthropic: AnthropicBackend,
    chat: OpenAiBackend,
    responses: OpenAiBackend,
    options: ProvidersOptions,
    fallback_session: SessionId,
}

#[async_trait]
impl ModelBackend for OpencodeBackend {
    async fn open(&self, req: &LlmRequest, model: &ModelId) -> Result<LlmStream, LlmError> {
        if model.as_str().starts_with("gemini-") {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "OpenCode serves `{model}` with Google's API, which crest does not speak; \
                     choose another model"
                ),
            });
        }
        let with_session;
        let req = if req.metadata.session_id.is_some() {
            req
        } else {
            let mut copy = req.clone();
            copy.metadata.session_id = Some(self.fallback_session);
            with_session = copy;
            &with_session
        };
        let opened = match self.options.opencode_format(model.as_str()) {
            WireFormat::Anthropic => self.anthropic.open(req, model).await,
            WireFormat::Chat => self.chat.open(req, model).await,
            WireFormat::Responses => self.responses.open(req, model).await,
        };
        opened.map_err(|err| match err {
            LlmError::Auth { .. } => LlmError::Auth {
                message: "OpenCode rejected the API key (OPENCODE_API_KEY or the keyring entry \
                          crest / OPENCODE_API_KEY); run /login opencode"
                    .to_owned(),
            },
            other => other,
        })
    }

    fn accepts_images(&self, model: &ModelId) -> bool {
        let model = model.as_str();
        model.starts_with("claude-") || model.starts_with("gpt-")
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::Message;
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
    use futures::StreamExt;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn options(server: &MockServer) -> ProvidersOptions {
        ProvidersOptions {
            opencode_base_url: Some(server.uri()),
            ..ProvidersOptions::default()
        }
    }

    fn request(model: &str) -> LlmRequest {
        let model = ResolvedModel {
            role: ModelRole::Title,
            spec: ModelSpec::new(model, 64),
            source: SpecSource::Default,
            warnings: vec![],
        };
        LlmRequest::new(model, vec![Message::user_text("Hola")])
    }

    fn sse(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    #[tokio::test]
    async fn each_family_goes_to_its_endpoint_with_a_session() {
        let server = MockServer::start().await;
        let chat = "data: {\"model\":\"glm-5.3\",\"choices\":[{\"delta\":{\"content\":\"Hola\"},\
                    \"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/go/openai/v1/chat/completions"))
            .and(header("authorization", "Bearer oc-key"))
            .and(header_exists("x-opencode-session"))
            .respond_with(sse(chat))
            .expect(1)
            .mount(&server)
            .await;
        let messages = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\
            \"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-haiku-5-5\",\"content\":[],\
            \"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n\
            event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        Mock::given(method("POST"))
            .and(path("/go/anthropic/v1/messages"))
            .and(header("authorization", "Bearer oc-key"))
            .and(header("x-api-key", "oc-key"))
            .and(header_exists("x-opencode-session"))
            .respond_with(sse(messages))
            .expect(1)
            .mount(&server)
            .await;
        let provider = provider(
            "oc-key",
            ProviderId::OpencodeGo,
            &options(&server),
            PlanAccess::default(),
        )
        .unwrap();
        for model in ["glm-5.3", "claude-haiku-5-5"] {
            let stream = provider
                .backend
                .open(&request(model), &ModelId::from(model))
                .await
                .unwrap();
            let events: Vec<_> = stream.collect().await;
            assert!(events.iter().all(Result::is_ok), "{model}: {events:?}");
        }
        let err = provider
            .backend
            .open(&request("gemini-4-argon"), &ModelId::from("gemini-4-argon"))
            .await;
        assert!(matches!(err, Err(LlmError::InvalidRequest { .. })));
    }

    #[tokio::test]
    async fn a_rejected_key_names_opencode() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/openai/v1/responses"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error", "error": { "type": "AuthError", "message": "Invalid API key." }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error", "error": { "type": "AuthError", "message": "Invalid API key." }
            })))
            .mount(&server)
            .await;
        let provider = provider(
            "bad",
            ProviderId::OpencodeZen,
            &options(&server),
            PlanAccess::default(),
        )
        .unwrap();
        let Err(LlmError::Auth { message }) = provider
            .backend
            .open(&request("gpt-6-luna"), &ModelId::from("gpt-6-luna"))
            .await
        else {
            panic!("expected an auth error")
        };
        assert!(message.contains("/login opencode"), "{message}");
        assert!(matches!(
            check_key("bad", &options(&server)).await,
            KeyCheck::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn the_probe_rules_out_what_answers_403_and_the_catalog_hides_it() {
        let server = MockServer::start().await;
        let list = |ids: &[&str]| {
            ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": ids.iter().map(|id| json!({"id": id, "object": "model"})).collect::<Vec<_>>(),
            }))
        };
        let refused = |kind: &str, message: &str| {
            ResponseTemplate::new(403).set_body_json(json!({
                "type": "error", "error": { "type": kind, "message": message }
            }))
        };
        let ok = "data: {\"model\":\"x\",\"choices\":[{\"delta\":{\"content\":\"h\"},\
                  \"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("GET"))
            .and(path("/go/v1/models"))
            .respond_with(list(&["glm-5.3"]))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/go/openai/v1/chat/completions"))
            .respond_with(refused(
                "Error",
                "An active OpenCode Go subscription is required to use Go models.",
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(list(&["beam-free", "glm-5.3", "glm-5.3-flash"]))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/openai/v1/chat/completions"))
            .and(body_partial_json(json!({"model": "beam-free"})))
            .respond_with(refused(
                "FreeTierError",
                "free tier can only be used from within OpenCode",
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/openai/v1/chat/completions"))
            .and(body_partial_json(json!({"model": "glm-5.3-flash"})))
            .respond_with(sse(ok))
            .expect(1)
            .mount(&server)
            .await;

        let options = options(&server);
        let access = probe_access("oc-key", &options, std::time::Duration::from_secs(5)).await;
        assert_eq!(
            (access.go, access.zen, access.zen_free),
            (Some(false), Some(true), Some(false))
        );
        let ids = |plan| {
            let provider = provider("oc-key", plan, &options, access).unwrap();
            async move {
                let models = provider.catalog.list_models().await.unwrap();
                models
                    .into_iter()
                    .map(|m| m.id.as_str().to_owned())
                    .collect::<Vec<_>>()
            }
        };
        assert!(ids(ProviderId::OpencodeGo).await.is_empty());
        assert_eq!(
            ids(ProviderId::OpencodeZen).await,
            ["glm-5.3", "glm-5.3-flash"]
        );
    }
}
