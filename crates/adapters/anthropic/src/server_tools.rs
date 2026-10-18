//! Web search and web fetch, the Anthropic server tools of the research
//! flow, and which model roles get them.
//!
//! Two pieces of configuration, both plain data:
//!
//! - [`ServerToolConfig`] says which [`ServerTool`]s each [`ModelRole`]
//!   gets. The port's [`LlmRequest::server_tools`] carries the result, so
//!   whoever builds the request (the composition root or the research flow)
//!   calls [`ServerToolConfig::apply`] on it. By default `explore` and
//!   `plan` get web search and web fetch; `main` and the rest get none.
//! - [`ServerToolOptions`] holds what the port does not model: domain
//!   filters, the user location for search and the content cap and
//!   citations of fetch. It is adapter configuration, applied by
//!   [`AnthropicLlm`](crate::AnthropicLlm) to every request it sends.
//!
//! Citations come back as [`LlmEvent::Citation`] and end up in
//! `AssembledMessage::citations`; the research flow turns them into
//! `Evidence` with `application::usecases::evidence::citations_to_evidence`.
//!
//! Tool shapes follow the official web search and web fetch tool docs
//! (platform.claude.com, checked 2026-10): `max_uses`, `allowed_domains`
//! or `blocked_domains` (never both), `user_location` (search only),
//! `citations` and `max_content_tokens` (fetch only).
//!
//! [`LlmRequest::server_tools`]: application::ports::llm::LlmRequest::server_tools
//! [`LlmEvent::Citation`]: application::ports::llm::LlmEvent::Citation

use std::collections::BTreeMap;

use application::ports::llm::{LlmRequest, ServerTool};
use domain::model::ModelRole;
use serde::{Deserialize, Serialize};

use crate::wire::{CitationsConfig, MessagesRequest, ServerToolDef, UserLocation, WireTool};

/// Searches per request given to a role by [`ServerToolConfig::default`].
pub const DEFAULT_WEB_SEARCH_MAX_USES: u32 = 5;
/// Fetches per request given to a role by [`ServerToolConfig::default`].
pub const DEFAULT_WEB_FETCH_MAX_USES: u32 = 5;

/// Which server tools each model role gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerToolConfig {
    /// Tools per role. A role that is missing gets none.
    pub by_role: BTreeMap<ModelRole, Vec<ServerTool>>,
}

impl Default for ServerToolConfig {
    /// `explore` and `plan` search and read the web; other roles do not.
    fn default() -> Self {
        let web = vec![
            ServerTool::WebSearch {
                max_uses: Some(DEFAULT_WEB_SEARCH_MAX_USES),
            },
            ServerTool::WebFetch {
                max_uses: Some(DEFAULT_WEB_FETCH_MAX_USES),
            },
        ];
        Self {
            by_role: BTreeMap::from([(ModelRole::Explore, web.clone()), (ModelRole::Plan, web)]),
        }
    }
}

impl ServerToolConfig {
    /// No server tools for any role.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            by_role: BTreeMap::new(),
        }
    }

    /// Server tools of `role` (empty when it has none).
    #[must_use]
    pub fn for_role(&self, role: ModelRole) -> &[ServerTool] {
        self.by_role.get(&role).map_or(&[], Vec::as_slice)
    }

    /// Sets `req.server_tools` to the tools of the request's role,
    /// replacing whatever it had.
    pub fn apply(&self, req: &mut LlmRequest) {
        req.server_tools = self.for_role(req.model.role).to_vec();
    }
}

/// Which domains a server tool may reach.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainFilter {
    /// Any domain.
    #[default]
    Any,
    /// Only these domains (subdomains included).
    Allow(Vec<String>),
    /// Every domain except these.
    Block(Vec<String>),
}

impl DomainFilter {
    /// Writes the filter on `def` as `allowed_domains`/`blocked_domains`.
    /// An empty list is the same as [`DomainFilter::Any`].
    fn write(&self, def: &mut ServerToolDef) {
        let (allowed, blocked) = match self {
            Self::Any => (Vec::new(), Vec::new()),
            Self::Allow(domains) => (domains.clone(), Vec::new()),
            Self::Block(domains) => (Vec::new(), domains.clone()),
        };
        def.allowed_domains = allowed;
        def.blocked_domains = blocked;
    }
}

/// Options of the web search tool.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchOptions {
    /// Domains the search may return.
    pub domains: DomainFilter,
    /// Approximate location to localise results.
    pub user_location: Option<UserLocation>,
}

/// Options of the web fetch tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebFetchOptions {
    /// Domains that may be fetched.
    pub domains: DomainFilter,
    /// Whether fetched documents can be cited (default on). Fetch
    /// citations point into the document; the citation itself carries no
    /// URL, so only web search citations become evidence directly.
    pub citations: bool,
    /// Cap on the tokens of a fetched document; `None` is the API default.
    pub max_content_tokens: Option<u32>,
}

impl Default for WebFetchOptions {
    fn default() -> Self {
        Self {
            domains: DomainFilter::Any,
            citations: true,
            max_content_tokens: None,
        }
    }
}

/// Adapter-wide options of the server tools.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerToolOptions {
    /// Web search options.
    pub web_search: WebSearchOptions,
    /// Web fetch options.
    pub web_fetch: WebFetchOptions,
}

impl ServerToolOptions {
    /// Wire definition of `tool` with these options.
    #[must_use]
    pub fn tool_def(&self, tool: ServerTool) -> ServerToolDef {
        match tool {
            ServerTool::WebSearch { max_uses } => {
                let mut def = ServerToolDef::web_search(max_uses);
                self.write_search(&mut def);
                def
            }
            ServerTool::WebFetch { max_uses } => {
                let mut def = ServerToolDef::web_fetch(max_uses);
                self.write_fetch(&mut def);
                def
            }
        }
    }

    /// Writes these options on the server tools of `body`, matched by
    /// name; `max_uses` and `allowed_callers` are left as they are.
    pub fn apply(&self, body: &mut MessagesRequest) {
        for tool in &mut body.tools {
            if let WireTool::Server(def) = tool {
                match def.name {
                    "web_search" => self.write_search(def),
                    "web_fetch" => self.write_fetch(def),
                    _ => {}
                }
            }
        }
    }

    fn write_search(&self, def: &mut ServerToolDef) {
        self.web_search.domains.write(def);
        def.user_location.clone_from(&self.web_search.user_location);
    }

    fn write_fetch(&self, def: &mut ServerToolDef) {
        self.web_fetch.domains.write(def);
        def.citations = Some(CitationsConfig {
            enabled: self.web_fetch.citations,
        });
        def.max_content_tokens = self.web_fetch.max_content_tokens;
    }
}

#[cfg(test)]
mod tests {
    use application::ports::llm::LlmRequest;
    use domain::conversation::Message;
    use domain::model::{ModelId, ModelSpec, ResolvedModel, SpecSource};
    use serde_json::{Value, json};

    use super::*;
    use crate::request;

    fn request_for(role: ModelRole) -> LlmRequest {
        let model = ResolvedModel {
            role,
            spec: ModelSpec::new("claude-opus-5-5", 16_000),
            source: SpecSource::Default,
            warnings: vec![],
        };
        LlmRequest::new(model, vec![Message::user_text("Investigate")])
    }

    /// The `tools` JSON sent for `role`.
    fn tools_json(
        config: &ServerToolConfig,
        options: &ServerToolOptions,
        role: ModelRole,
    ) -> Value {
        let mut req = request_for(role);
        config.apply(&mut req);
        let mut body = request::build(&req, &ModelId::new("claude-opus-5-5"));
        options.apply(&mut body);
        serde_json::to_value(&body.tools).unwrap()
    }

    #[test]
    fn default_config_gives_the_web_to_explore_and_plan_only() {
        let config = ServerToolConfig::default();
        let options = ServerToolOptions::default();
        let by_role: BTreeMap<&str, Value> = ModelRole::ALL
            .into_iter()
            .map(|role| (role.as_str(), tools_json(&config, &options, role)))
            .collect();
        insta::assert_json_snapshot!(by_role);
        assert!(config.for_role(ModelRole::Main).is_empty());
    }

    #[test]
    fn options_produce_filters_location_and_content_cap() {
        let options = ServerToolOptions {
            web_search: WebSearchOptions {
                domains: DomainFilter::Allow(vec!["docs.rs".into(), "rust-lang.org".into()]),
                user_location: Some(UserLocation {
                    city: Some("Guadalajara".into()),
                    country: Some("MX".into()),
                    ..UserLocation::default()
                }),
            },
            web_fetch: WebFetchOptions {
                domains: DomainFilter::Block(vec!["example.com".into()]),
                citations: true,
                max_content_tokens: Some(40_000),
            },
        };
        insta::assert_json_snapshot!(tools_json(
            &ServerToolConfig::default(),
            &options,
            ModelRole::Explore
        ));
    }

    #[test]
    fn apply_replaces_the_request_tools() {
        let mut req = request_for(ModelRole::Main);
        req.server_tools = vec![ServerTool::WebSearch { max_uses: None }];
        ServerToolConfig::default().apply(&mut req);
        assert!(req.server_tools.is_empty());
        ServerToolConfig::none().apply(&mut req);
        assert!(req.server_tools.is_empty());
    }

    #[test]
    fn tool_def_keeps_max_uses_and_disables_fetch_citations() {
        let options = ServerToolOptions {
            web_fetch: WebFetchOptions {
                citations: false,
                ..WebFetchOptions::default()
            },
            ..ServerToolOptions::default()
        };
        let def = options.tool_def(ServerTool::WebFetch { max_uses: Some(2) });
        assert_eq!(
            serde_json::to_value(def).unwrap(),
            json!({
                "type": "web_fetch_20260318",
                "name": "web_fetch",
                "max_uses": 2,
                "citations": {"enabled": false}
            })
        );
    }

    #[test]
    fn config_round_trips_through_serde() {
        let config = ServerToolConfig::default();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value,
            json!({"by_role": {
                "plan": [
                    {"type": "web_search", "max_uses": 5},
                    {"type": "web_fetch", "max_uses": 5}
                ],
                "explore": [
                    {"type": "web_search", "max_uses": 5},
                    {"type": "web_fetch", "max_uses": 5}
                ]
            }})
        );
        assert_eq!(
            serde_json::from_value::<ServerToolConfig>(value).unwrap(),
            config
        );
    }
}
