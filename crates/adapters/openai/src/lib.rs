//! Model backends over OpenAI-style APIs: Chat Completions ([`chat`]) and
//! Responses ([`responses`]), plus the `/models` catalog. Retries and
//! fallbacks are not done here: wrap the backend in
//! [`RetryingLlm`](application::llm_retry::RetryingLlm).

pub mod chat;
pub mod client;
pub mod responses;
pub mod sse;

use application::llm_retry::ModelBackend;
use application::ports::llm::{LlmError, LlmRequest, LlmStream, ModelCatalogPort, ModelInfo};
use async_trait::async_trait;
use domain::model::ModelId;
use serde::Deserialize;

pub use client::{OpenAiClient, OpenAiConfig};

/// The API shape an [`OpenAiBackend`] speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// `POST /chat/completions`.
    Chat,
    /// `POST /responses`.
    Responses,
}

/// One attempt on an OpenAI-style API.
#[derive(Debug, Clone)]
pub struct OpenAiBackend {
    client: OpenAiClient,
    api: Api,
}

impl OpenAiBackend {
    /// A backend that speaks `api` through `client`.
    #[must_use]
    pub fn new(client: OpenAiClient, api: Api) -> Self {
        Self { client, api }
    }
}

#[async_trait]
impl ModelBackend for OpenAiBackend {
    async fn open(&self, req: &LlmRequest, model: &ModelId) -> Result<LlmStream, LlmError> {
        let session = req.metadata.session_id.map(|id| id.to_string());
        let session = session.as_deref();
        let stream: LlmStream = match self.api {
            Api::Chat => {
                let body = chat::build(req, model);
                let response = self
                    .client
                    .post("/chat/completions", &body, session)
                    .await?;
                Box::pin(sse::llm_events(
                    Box::pin(response.bytes_stream()),
                    chat::ChatDecoder::new(model.clone()),
                ))
            }
            Api::Responses => {
                let body = responses::build(req, model);
                let response = self.client.post("/responses", &body, session).await?;
                Box::pin(sse::llm_events(
                    Box::pin(response.bytes_stream()),
                    responses::ResponsesDecoder::new(model.clone()),
                ))
            }
        };
        Ok(stream)
    }
}

/// [`ModelCatalogPort`] over `GET /models`.
#[derive(Debug, Clone)]
pub struct OpenAiCatalog {
    client: OpenAiClient,
}

impl OpenAiCatalog {
    /// The catalog of the API behind `client`.
    #[must_use]
    pub fn new(client: OpenAiClient) -> Self {
        Self { client }
    }
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    created: Option<i64>,
}

#[async_trait]
impl ModelCatalogPort for OpenAiCatalog {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let list: ModelList = self.client.get_json("/models").await?;
        Ok(list
            .data
            .into_iter()
            .map(|entry| ModelInfo {
                display_name: entry.name.unwrap_or_else(|| entry.id.clone()),
                id: ModelId::new(entry.id),
                created_at: entry.created.map(|secs| secs.to_string()),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use application::ports::llm::MessageAssembler;
    use domain::conversation::{Message, SessionId};
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
    use futures::StreamExt;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const CHAT: &str = include_str!("../tests/fixtures/chat_text.sse");

    fn client(server: &MockServer) -> OpenAiClient {
        let config = OpenAiConfig {
            base_url: format!("{}/v1", server.uri()),
            user_agent: Some("crest/test".into()),
            session_header: Some("x-opencode-session".into()),
            provider: "OpenCode".into(),
            ..OpenAiConfig::default()
        };
        OpenAiClient::new("oc-key", config).unwrap()
    }

    fn request(session: SessionId) -> LlmRequest {
        let model = ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new("glm-5.3", 256),
            source: SpecSource::Default,
            warnings: vec![],
        };
        let mut req = LlmRequest::new(model, vec![Message::user_text("Hola")]);
        req.metadata.session_id = Some(session);
        req
    }

    #[tokio::test]
    async fn a_chat_answer_streams_with_the_key_agent_and_session() {
        let server = MockServer::start().await;
        let session = SessionId::new();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer oc-key"))
            .and(header("user-agent", "crest/test"))
            .and(header("x-opencode-session", session.to_string().as_str()))
            .and(body_partial_json(
                json!({ "model": "glm-5.3", "stream": true }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(CHAT),
            )
            .expect(1)
            .mount(&server)
            .await;
        let backend = OpenAiBackend::new(client(&server), Api::Chat);
        let stream = backend
            .open(&request(session), &ModelId::from("glm-5.3"))
            .await
            .unwrap();
        let mut assembler = MessageAssembler::new();
        for event in stream.collect::<Vec<_>>().await {
            assembler.push(event.unwrap()).unwrap();
        }
        let message = assembler.finish().unwrap();
        assert_eq!(message.model, Some(ModelId::from("glm-5.3")));
        assert_eq!(message.message.content.len(), 2);
    }

    #[tokio::test]
    async fn http_errors_come_back_before_streaming() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": { "message": "invalid api key" }
            })))
            .mount(&server)
            .await;
        let backend = OpenAiBackend::new(client(&server), Api::Responses);
        let Err(err) = backend
            .open(&request(SessionId::new()), &ModelId::from("gpt-6-luna"))
            .await
        else {
            panic!("expected an error")
        };
        assert!(matches!(err, LlmError::Auth { message } if message.contains("OpenCode")));
    }

    #[tokio::test]
    async fn the_catalog_lists_the_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    { "id": "glm-5.3", "object": "model", "owned_by": "opencode" },
                    { "id": "kimi-k3", "object": "model", "created": 1760000000 },
                ]
            })))
            .mount(&server)
            .await;
        let ids: Vec<String> = OpenAiCatalog::new(client(&server))
            .list_models()
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(ids, ["glm-5.3", "kimi-k3"]);
    }
}
