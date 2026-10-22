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

use std::sync::Arc;

use adapter_anthropic::{AnthropicBackend, AnthropicClient, ApiKey, ClientConfig};
use adapter_openai::{Api, OpenAiBackend, OpenAiCatalog, OpenAiClient, OpenAiConfig};
use application::llm_retry::ModelBackend;
use application::ports::credentials::KeyCheck;
use application::ports::llm::{LlmError, LlmRequest, LlmStream, ModelCatalogPort};
use application::providers::{Provider, ProvidersOptions, WireFormat};
use async_trait::async_trait;
use domain::conversation::SessionId;
use domain::model::{ModelId, ProviderId};

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

/// The OpenCode provider `plan` (Go or Zen) with `key`.
///
/// # Errors
///
/// When a client cannot be built (an invalid key or TLS failure).
pub fn provider(
    key: &str,
    plan: ProviderId,
    options: &ProvidersOptions,
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
        catalog: Arc::new(OpenAiCatalog::new(catalog)),
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
    use wiremock::matchers::{header, header_exists, method, path};
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
        let provider = provider("oc-key", ProviderId::OpencodeGo, &options(&server)).unwrap();
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
        let provider = provider("bad", ProviderId::OpencodeZen, &options(&server)).unwrap();
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
}
