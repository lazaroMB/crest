//! [`ModelCatalogPort`] over `GET /v1/models`.
//!
//! The endpoint is paginated: each page has `data`, `has_more` and
//! `last_id`, and the next page is requested with `after_id=<last_id>`.

use application::ports::llm::{LlmError, ModelCatalogPort, ModelInfo};
use async_trait::async_trait;
use domain::model::ModelId;
use serde::Deserialize;

use crate::client::AnthropicClient;

/// Page size requested (the API maximum).
pub const PAGE_LIMIT: u32 = 1000;

/// Upper bound of pages followed, against a server that never says
/// `has_more: false`.
const MAX_PAGES: usize = 100;

/// One page of `GET /v1/models`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelsPage {
    /// Models of this page, newest first.
    #[serde(default)]
    pub data: Vec<WireModel>,
    /// Whether more pages follow.
    #[serde(default)]
    pub has_more: bool,
    /// Id of the last model of the page, the cursor of the next one.
    #[serde(default)]
    pub last_id: Option<String>,
}

/// One model object. Capability fields are not modelled.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WireModel {
    /// Model id.
    pub id: String,
    /// Human-readable name.
    #[serde(default)]
    pub display_name: String,
    /// RFC 3339 release date.
    #[serde(default)]
    pub created_at: Option<String>,
}

impl From<WireModel> for ModelInfo {
    fn from(model: WireModel) -> Self {
        Self {
            id: ModelId::new(model.id),
            display_name: model.display_name,
            created_at: model.created_at,
        }
    }
}

/// Lists the models available to the API key.
#[derive(Debug, Clone)]
pub struct AnthropicCatalog {
    client: AnthropicClient,
}

impl AnthropicCatalog {
    /// A catalog that queries through `client`.
    #[must_use]
    pub fn new(client: AnthropicClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ModelCatalogPort for AnthropicCatalog {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("limit", PAGE_LIMIT.to_string())];
            if let Some(after) = &after {
                query.push(("after_id", after.clone()));
            }
            let page: ModelsPage = self.client.get_json("/v1/models", &query).await?;
            models.extend(page.data.into_iter().map(ModelInfo::from));
            if !page.has_more {
                return Ok(models);
            }
            match page.last_id {
                Some(last) if after.as_ref() != Some(&last) => after = Some(last),
                _ => {
                    return Err(LlmError::Decode(
                        "GET /v1/models: has_more without a new last_id".into(),
                    ));
                }
            }
        }
        Err(LlmError::Decode(format!(
            "GET /v1/models: more than {MAX_PAGES} pages"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use domain::model::{ModelPolicy, ModelRole, ModelSpecPatch, ModelWarning, RoleConfig};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::client::{ApiKey, ClientConfig};

    fn model(id: &str) -> serde_json::Value {
        json!({
            "type": "model",
            "id": id,
            "display_name": id.replace('-', " "),
            "created_at": "2026-09-01T00:00:00Z",
            "max_input_tokens": 1_000_000,
            "max_tokens": 128_000,
            "capabilities": {}
        })
    }

    async fn catalog(server: &MockServer) -> AnthropicCatalog {
        let config = ClientConfig {
            base_url: server.uri(),
            ..ClientConfig::default()
        };
        let key = ApiKey::new("sk-ant-fake-catalog-tests").unwrap();
        AnthropicCatalog::new(AnthropicClient::new(&key, config).unwrap())
    }

    async fn two_pages() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(query_param("limit", "1000"))
            .and(query_param_is_missing("after_id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [model("claude-fable-5-1"), model("claude-opus-5-5")],
                "has_more": true,
                "first_id": "claude-fable-5-1",
                "last_id": "claude-opus-5-5"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(query_param("after_id", "claude-opus-5-5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [model("claude-sonnet-5-5"), model("claude-haiku-5-5")],
                "has_more": false,
                "first_id": "claude-sonnet-5-5",
                "last_id": "claude-haiku-5-5"
            })))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn follows_pagination_to_the_end() {
        let server = two_pages().await;
        let models = catalog(&server).await.list_models().await.unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-fable-5-1",
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-haiku-5-5"
            ]
        );
        assert_eq!(models[1].display_name, "claude opus 5 5");
        assert_eq!(
            models[1].created_at.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn warns_about_roles_with_missing_models() {
        let server = two_pages().await;
        let models = catalog(&server).await.list_models().await.unwrap();
        let available: Vec<ModelId> = models.into_iter().map(|m| m.id).collect();

        let mut roles = BTreeMap::new();
        roles.insert(
            ModelRole::Explore,
            ModelSpecPatch {
                id: Some(ModelId::new("claude-haiku-9")),
                ..ModelSpecPatch::default()
            },
        );
        let workspace = RoleConfig {
            roles,
            aliases: BTreeMap::new(),
        };
        let policy = ModelPolicy::new(RoleConfig::default(), workspace);
        let warnings = policy.validate_against(&available);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(matches!(
            &warnings[0],
            ModelWarning::UnavailableModel { role: ModelRole::Explore, model, .. }
                if model.as_str() == "claude-haiku-9"
        ));
        // The default roles all exist in the catalog.
        assert!(
            ModelPolicy::default()
                .validate_against(&available)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn has_more_without_cursor_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data": [], "has_more": true})),
            )
            .mount(&server)
            .await;
        let err = catalog(&server).await.list_models().await.unwrap_err();
        assert!(matches!(err, LlmError::Decode(_)));
    }

    #[tokio::test]
    async fn unauthorized_is_an_auth_error() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error",
                "error": {"type": "authentication_error", "message": "invalid x-api-key"}
            })))
            .mount(&server)
            .await;
        let err = catalog(&server).await.list_models().await.unwrap_err();
        assert!(matches!(err, LlmError::Auth { .. }));
    }
}
