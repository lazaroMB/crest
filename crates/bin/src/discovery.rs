//! The discovery engine in the binary: how the composition root builds it
//! and how it is chosen.
//!
//! `/research` runs on the discovery engine when the config says so:
//!
//! ```lua
//! app.opt.research = { engine = "discovery" }
//! ```
//!
//! Without it (or with `engine = "graph"`) the graph-flow research flow
//! runs as before. Both write their Spec under `.crest/specs/`.

use std::sync::Arc;

use adapter_store::SqliteDiscoveryStore;
use application::decide::Decider;
use application::flows::discovery::engine::{DiscoveryDeps, DiscoveryEngine};
use application::flows::discovery::lock::DiscoveryLock;
use application::flows::discovery::model::Model;
use application::flows::discovery::session::DiscoverySession;
use application::flows::research::nodes::spec::sink::FsSpecSink;
use application::ports::events::EventSink;
use application::ports::fs::FileSystemPort;
use application::ports::llm::LlmPort;
use application::ports::plugin::AppOptions;
use application::ports::search::FileSearchPort;
use application::ports::shell::ShellPort;
use application::ports::web::WebPort;
use application::usecases::run_turn::AgentService;
use domain::model::ModelPolicy;

/// The config option that selects the engine.
pub const OPTION: &str = "research";

/// Which engine `/research` runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Engine {
    /// The graph-flow pipelines (the default).
    #[default]
    Graph,
    /// The event-sourced discovery engine.
    Discovery,
}

impl Engine {
    /// The engine `options` select: `app.opt.research.engine`.
    #[must_use]
    pub fn from_options(options: &AppOptions) -> (Self, Option<String>) {
        let Some(value) = options.extra.get(OPTION) else {
            return (Self::Graph, None);
        };
        let Some(table) = value.as_object() else {
            return (
                Self::Graph,
                Some(format!(
                    "app.opt.research must be a table such as {{ engine = \"discovery\" }}, got {value}; using `graph`"
                )),
            );
        };
        match table.get("engine") {
            None => (Self::Graph, None),
            Some(engine) => match engine.as_str() {
                Some("graph") => (Self::Graph, None),
                Some("discovery") => (Self::Discovery, None),
                Some(other) => (
                    Self::Graph,
                    Some(format!(
                        "app.opt.research.engine = {other:?} is not `graph` or `discovery`; using `graph`"
                    )),
                ),
                None => (
                    Self::Graph,
                    Some(format!(
                        "app.opt.research.engine must be a string, got {engine}; using `graph`"
                    )),
                ),
            },
        }
    }

    /// The name shown in the status line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Discovery => "discovery",
        }
    }
}

/// What the discovery engine is built from.
pub struct DiscoveryParts {
    /// The log and source store.
    pub store: Arc<SqliteDiscoveryStore>,
    /// The model.
    pub llm: Arc<dyn LlmPort>,
    /// Model selection.
    pub models: ModelPolicy,
    /// The chat's events (blocks and notices).
    pub events: Arc<dyn EventSink>,
    /// The decision model that reads free-text answers.
    pub decider: Decider,
    /// The read-only agent that explores the repository and the web.
    pub explorer: Arc<AgentService>,
    /// The acting agent that runs spikes.
    pub worker: Arc<AgentService>,
    /// The workspace files (sources, and the Spec folder through the sink).
    pub fs: Arc<dyn FileSystemPort>,
    /// Listing, to number the Spec folders.
    pub search: Arc<dyn FileSearchPort>,
    /// The web, when configured.
    pub web: Option<Arc<dyn WebPort>>,
    /// The shell, for spike worktrees.
    pub shell: Arc<dyn ShellPort>,
    /// The lock the tools' file port consults.
    pub lock: DiscoveryLock,
    /// The workspace root, as a string.
    pub root: String,
    /// The name recorded for the person's verdicts.
    pub human: String,
}

/// The discovery session of the workspace over `parts`.
#[must_use]
pub fn build(parts: DiscoveryParts) -> DiscoverySession {
    let model = Model::new(
        Arc::clone(&parts.llm),
        parts.models,
        Arc::clone(&parts.events),
    );
    let sink = Arc::new(FsSpecSink::new(Arc::clone(&parts.fs), parts.search));
    let log: Arc<dyn application::ports::discovery::DiscoveryLogPort> = parts.store.clone();
    let sources: Arc<dyn application::ports::discovery::SourceStorePort> = parts.store;
    let mut deps = DiscoveryDeps::new(log, sources, model, parts.events)
        .with_decider(parts.decider)
        .with_explorer(parts.explorer)
        .with_worker(parts.worker)
        .with_fs(parts.fs)
        .with_shell(parts.shell)
        .with_sink(sink)
        .with_lock(parts.lock)
        .with_workspace_root(parts.root)
        .with_human(parts.human);
    if let Some(web) = parts.web {
        deps = deps.with_web(web);
    }
    DiscoverySession::new(Arc::new(DiscoveryEngine::new(deps)))
}

/// The name of the person, for the log: the OS user, else `user`.
#[must_use]
pub fn human_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "user".to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn options(value: serde_json::Value) -> AppOptions {
        let mut options = AppOptions::default();
        options.extra.insert(OPTION.to_owned(), value);
        options
    }

    #[test]
    fn the_engine_option_is_read() {
        assert_eq!(
            Engine::from_options(&AppOptions::default()),
            (Engine::Graph, None)
        );
        assert_eq!(
            Engine::from_options(&options(json!({"engine": "discovery"}))),
            (Engine::Discovery, None)
        );
        assert_eq!(
            Engine::from_options(&options(json!({"engine": "graph"}))),
            (Engine::Graph, None)
        );
        let (engine, warning) = Engine::from_options(&options(json!({"engine": "x"})));
        assert_eq!(engine, Engine::Graph);
        assert!(warning.unwrap().contains("not `graph` or `discovery`"));
    }
}
