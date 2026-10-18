//! The composition root: builds every adapter and wires them into the
//! [`AgentService`]. This is the only place that knows them all.
//!
//! [`build_app`] takes the parsed [`Config`], the [`Frontend`] that shows
//! events and answers approvals (the terminal in headless mode, the UI
//! later) and [`Overrides`] that replace external services in tests (the
//! model above all, see `tests/headless_demo.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use adapter_anthropic::{AnthropicCatalog, AnthropicClient, AnthropicLlm, ApiKey, ClientConfig};
use adapter_fs::{LocalFs, WorkspaceGuard};
use adapter_lua::{HostOptions, LuaHost, default_sources};
use adapter_search::RipgrepSearch;
use adapter_shell::LocalShell;
use adapter_store::SqliteSessionStore;
use adapter_telemetry::TracingTelemetry;
use anyhow::{Context as _, anyhow};
use application::cost::CostTracker;
use application::ports::approval::ApprovalPort;
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::ports::fs::FileSystemPort;
use application::ports::llm::{LlmPort, ModelCatalogPort};
use application::ports::plugin::{
    AppOptions, CommandSpec, ConfigSources, KeymapSpec, PluginHostPort,
};
use application::ports::search::{ContentSearchPort, FileSearchPort};
use application::ports::store::{SessionStorePort, StoreError, StoredSession};
use application::ports::telemetry::TelemetryPort;
use application::tools::components::{COMPONENT_TOOLS_PROMPT, register_component_tools};
use application::tools::files::{FileToolPorts, register_file_tools};
use application::tools::plugin::PluginTool;
use application::tools::registry::ToolRegistry;
use application::tools::task::{TaskPorts, TaskTool};
use application::usecases::approval::lock_permissions;
use application::usecases::compact::CompactionPolicy;
use application::usecases::run_turn::{AgentDeps, AgentService, SharedPermissions, TurnOptions};
use domain::conversation::Session;
use domain::model::{ModelPolicy, ModelRole};
use domain::permission::{PermissionMode, PermissionPolicy};
use domain::workspace::WorkspaceRoot;
use ui::timeline::TimelineState;

use crate::args::Args;
use crate::mcp::{self, McpServers, TrustOptions};
use crate::startup::{self, CATALOG_TIMEOUT};

/// What the command line decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Workspace directory, not canonicalized yet.
    pub workspace: PathBuf,
    /// Initial permission mode.
    pub mode: PermissionMode,
    /// `--model`: alias, role or id for the turns.
    pub model: Option<String>,
    /// `--trust-mcp`: workspace MCP servers approved on this run.
    pub trust_mcp: Vec<String>,
}

impl Config {
    /// The config of `args`; the workspace defaults to the current
    /// directory.
    ///
    /// # Errors
    ///
    /// When no directory is given and the current one cannot be read.
    pub fn from_args(args: &Args) -> anyhow::Result<Self> {
        let workspace = match args.workspace_dir() {
            Some(dir) => dir.clone(),
            None => std::env::current_dir().context("no puedo leer el directorio actual")?,
        };
        Ok(Self {
            workspace,
            mode: args.mode.unwrap_or_default(),
            model: args.model.clone(),
            trust_mcp: args.trust_mcp.clone(),
        })
    }
}

/// Where events go and who approves actions.
#[derive(Clone)]
pub struct Frontend {
    /// Shows the domain events.
    pub events: Arc<dyn EventSink>,
    /// Answers approval requests.
    pub approval: Arc<dyn ApprovalPort>,
}

/// Replacements of the external services, for tests.
#[derive(Default, Clone)]
pub struct Overrides {
    /// The model; when set, no API key is needed and, unless `catalog` is
    /// set too, the models are not validated.
    pub llm: Option<Arc<dyn LlmPort>>,
    /// The model catalog.
    pub catalog: Option<Arc<dyn ModelCatalogPort>>,
    /// Config files instead of `~/.config/crest` and `<ws>/.crest`.
    pub config_sources: Option<ConfigSources>,
    /// Session database instead of the default one.
    pub db_path: Option<PathBuf>,
    /// Store of approved workspace MCP servers instead of the default one.
    pub mcp_trust_path: Option<PathBuf>,
}

/// Records the timeline of the session while forwarding every event.
struct TimelineSink {
    timeline: Arc<Mutex<TimelineState>>,
    inner: Arc<dyn EventSink>,
}

impl EventSink for TimelineSink {
    fn emit(&self, event: DomainEvent) {
        self.timeline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(&event);
        self.inner.emit(event);
    }
}

/// Everything a front end needs to run turns.
pub struct App {
    /// The agent.
    pub service: AgentService,
    /// The current session (always new for now).
    pub session: Session,
    /// Options of each turn (`main` role and the `--model` override).
    pub turn_options: TurnOptions,
    /// Canonical workspace root.
    pub workspace: WorkspaceRoot,
    /// Startup warnings, also emitted as `Notice` events.
    pub warnings: Vec<String>,
    /// Problems of the config files (`file:line: message`), also emitted as
    /// `Error` blocks; crest runs without what those files define.
    pub config_errors: Vec<String>,
    /// The MCP servers that were started, if any.
    pub mcp: Option<McpServers>,
    /// File search of the workspace (rg/fzf), for the tree and pickers.
    pub file_search: Arc<dyn FileSearchPort>,
    /// Text search of the workspace (rg), for the `<leader>fg` picker.
    pub content_search: Arc<dyn ContentSearchPort>,
    /// The Lua host that runs the callbacks of keymaps and commands, if Lua
    /// started.
    pub plugins: Option<Arc<dyn PluginHostPort>>,
    /// Keymaps of `init.lua`, in registration order.
    pub keymaps: Vec<KeymapSpec>,
    /// Commands of `init.lua` (`:name args`).
    pub commands: Vec<CommandSpec>,
    /// Options of `app.opt`.
    pub options: AppOptions,
    models: ModelPolicy,
    events: Arc<dyn EventSink>,
    timeline: Arc<Mutex<TimelineState>>,
    store: Option<Arc<dyn SessionStorePort>>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("workspace", &self.workspace)
            .field("warnings", &self.warnings)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Emits an event to the front end (and records it in the timeline
    /// that is saved).
    pub fn emit(&self, event: DomainEvent) {
        self.events.emit(event);
    }

    /// Emits a warning to the front end.
    pub fn warn(&self, message: impl Into<String>) {
        self.emit(DomainEvent::Notice {
            level: NoticeLevel::Warning,
            message: message.into(),
        });
    }

    /// The model policy the agent resolves models with.
    #[must_use]
    pub fn model_policy(&self) -> &ModelPolicy {
        &self.models
    }

    /// Replaces the session with a new, empty one (`:clear`). The old one
    /// stays in the store.
    pub fn new_session(&mut self) {
        self.session = Session::new(self.workspace.as_path().display().to_string(), now_ms());
        *self.timeline.lock().unwrap_or_else(PoisonError::into_inner) = TimelineState::new();
    }

    /// The session as it would be stored: history, timeline and the
    /// "allow always" rules granted so far.
    #[must_use]
    pub fn stored_session(&self) -> StoredSession {
        let timeline = self
            .timeline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries()
            .iter()
            .map(|entry| entry.block.clone())
            .collect();
        StoredSession {
            session: self.session.clone(),
            timeline,
            permission_rules: lock_permissions(self.service.permissions()).rules.clone(),
        }
    }

    /// Saves the session; without a store this does nothing.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn save(&self) -> Result<(), StoreError> {
        match &self.store {
            Some(store) => store.save(&self.stored_session()).await,
            None => Ok(()),
        }
    }

    /// The permission policy shared with the agent.
    #[must_use]
    pub fn permissions(&self) -> &SharedPermissions {
        self.service.permissions()
    }
}

/// Builds the app over `config`.
///
/// # Errors
///
/// Configuration errors that make running impossible: a workspace that
/// cannot be opened, or no API key when the real model is used. Everything
/// else becomes a warning.
pub async fn build_app(
    config: &Config,
    frontend: Frontend,
    overrides: Overrides,
) -> anyhow::Result<App> {
    let timeline = Arc::new(Mutex::new(TimelineState::new()));
    // Startup problems go straight to the front end: they belong to this
    // run, not to the saved timeline of the session.
    let startup_events = Arc::clone(&frontend.events);
    let events: Arc<dyn EventSink> = Arc::new(TimelineSink {
        timeline: Arc::clone(&timeline),
        inner: frontend.events,
    });
    let mut warnings = Vec::new();

    let guard = WorkspaceGuard::open(&config.workspace).map_err(|err| {
        anyhow!(
            "no puedo abrir el workspace {}: {err}",
            config.workspace.display()
        )
    })?;
    let root = guard.root().clone();

    let plugins = match LuaHost::spawn(HostOptions::default()) {
        Ok(handle) => Some(Arc::new(handle)),
        Err(err) => {
            warnings.push(format!("Lua no disponible: {err}"));
            None
        }
    };
    let sources = overrides
        .config_sources
        .clone()
        .unwrap_or_else(|| default_sources(root.as_path()));
    let host = plugins.as_deref().map(|h| h as &dyn PluginHostPort);
    let (loaded, config_warnings) = startup::load_config(host, sources).await;
    warnings.extend(config_warnings);
    let (rules, rules_warning) = startup::sensitive_rules(&loaded);
    warnings.extend(rules_warning);
    let guard = guard.with_sensitive_rules(rules.clone());

    let search = RipgrepSearch::new(root.clone()).with_sensitive_rules(rules);
    warnings.extend(search.check_tools().await.problems());

    let (llm, catalog) = model_ports(&overrides).await?;
    let mut policy = loaded.model_policy();
    if let Some(catalog) = catalog {
        let (checked, model_warnings) =
            startup::validate_models(policy, catalog.as_ref(), CATALOG_TIMEOUT).await;
        policy = checked;
        warnings.extend(model_warnings);
    }

    let store = match open_store(overrides.db_path.as_deref()) {
        Ok(store) => Some(Arc::new(store) as Arc<dyn SessionStorePort>),
        Err(err) => {
            warnings.push(format!("la sesión no se guardará: {err}"));
            None
        }
    };

    let search = Arc::new(search);
    let file_search: Arc<dyn FileSearchPort> = search.clone();
    let content_search: Arc<dyn ContentSearchPort> = search.clone();
    let fs: Arc<dyn FileSystemPort> = Arc::new(LocalFs::new(guard));
    let telemetry: Arc<dyn TelemetryPort> = Arc::new(TracingTelemetry::new());
    let mut tools = ToolRegistry::new();
    register_file_tools(
        &mut tools,
        FileToolPorts {
            file_search: search.clone(),
            content_search: search.clone(),
            fs: Arc::clone(&fs),
            shell: Arc::new(LocalShell::new(root.clone())),
        },
    )
    .context("registro de tools")?;
    register_component_tools(&mut tools).context("registro de tools")?;
    let task = TaskTool::new(
        TaskPorts {
            llm: Arc::clone(&llm),
            telemetry: Arc::clone(&telemetry),
            models: policy.clone(),
            file_search: search.clone(),
            content_search: search,
            fs,
        },
        tools.budget(),
    )
    .context("registro de tools")?;
    tools.register_typed(task).context("registro de tools")?;
    if let Some(host) = &plugins {
        for spec in loaded.tools.iter().cloned() {
            let name = spec.name.clone();
            let tool = PluginTool::new(spec, host.clone() as Arc<dyn PluginHostPort>);
            if let Err(err) = tools.register(Arc::new(tool)) {
                warnings.push(format!("tool de plugin `{name}` ignorada: {err}"));
            }
        }
    }

    let trust = TrustOptions {
        store: overrides
            .mcp_trust_path
            .clone()
            .or_else(mcp::default_trust_path),
        trust_now: config.trust_mcp.clone(),
    };
    let (mcp, mcp_warnings) = mcp::start(
        loaded.mcp_servers.clone(),
        root.as_path(),
        &trust,
        mcp::READY_TIMEOUT,
        &mut tools,
    )
    .await;
    warnings.extend(mcp_warnings);

    let permissions = Arc::new(Mutex::new(PermissionPolicy::new(config.mode)));
    let deps = AgentDeps {
        llm,
        events: Arc::clone(&events),
        approval: frontend.approval,
        telemetry,
    };
    let models = policy.clone();
    let mut service = AgentService::new(deps, tools, policy, permissions)
        .with_system_prompt(system_prompt(&root))
        .with_compaction(CompactionPolicy::default());
    let plugins = plugins.map(|host| host as Arc<dyn PluginHostPort>);
    if let Some(host) = &plugins {
        service = service.with_plugin_host(Arc::clone(host));
    }
    // The spending cap asks through the front end's approval port.
    match CostTracker::from_options(&loaded.options) {
        Ok(tracker) => service = service.with_cost_tracker(tracker),
        Err(err) => warnings.push(format!("costo: {err}; sin precios ni tope de gasto")),
    }

    for error in &loaded.errors {
        startup_events.emit(DomainEvent::BlockAdded {
            block: startup::config_error_block(error),
        });
    }
    for warning in &warnings {
        startup_events.emit(DomainEvent::Notice {
            level: NoticeLevel::Warning,
            message: warning.clone(),
        });
    }
    let mut turn_options = TurnOptions::new(ModelRole::Main);
    if let Some(model) = &config.model {
        turn_options = turn_options.with_override(model.clone());
    }
    Ok(App {
        service,
        session: Session::new(root.as_path().display().to_string(), now_ms()),
        turn_options,
        workspace: root,
        warnings,
        config_errors: loaded.errors.iter().map(startup::diagnostic).collect(),
        mcp,
        file_search,
        content_search,
        plugins,
        keymaps: loaded.keymaps,
        commands: loaded.commands,
        options: loaded.options,
        models,
        events,
        timeline,
        store,
    })
}

type ModelPorts = (Arc<dyn LlmPort>, Option<Arc<dyn ModelCatalogPort>>);

/// The model and its catalog: the overrides, or the Anthropic API with the
/// key from the environment or the keyring.
async fn model_ports(overrides: &Overrides) -> anyhow::Result<ModelPorts> {
    if let Some(llm) = &overrides.llm {
        return Ok((Arc::clone(llm), overrides.catalog.clone()));
    }
    // The keyring may block (D-Bus, Keychain).
    let key = tokio::task::spawn_blocking(ApiKey::from_env_or_keyring)
        .await
        .context("lectura de la API key")?
        .map_err(|_| {
            anyhow!(
                "falta la API key de Anthropic: exporta ANTHROPIC_API_KEY o guárdala en el \
                 llavero del sistema (servicio `crest`, entrada `ANTHROPIC_API_KEY`)"
            )
        })?;
    let client = AnthropicClient::new(&key, ClientConfig::default())
        .map_err(|err| anyhow!("no puedo crear el cliente de Anthropic: {err}"))?;
    let catalog = overrides
        .catalog
        .clone()
        .unwrap_or_else(|| Arc::new(AnthropicCatalog::new(client.clone())));
    Ok((Arc::new(AnthropicLlm::new(client)), Some(catalog)))
}

fn open_store(path: Option<&Path>) -> Result<SqliteSessionStore, StoreError> {
    match path {
        Some(path) => SqliteSessionStore::open(path),
        None => SqliteSessionStore::open_default(),
    }
}

/// The system prompt: role, workspace, platform, how to work and the
/// component tools.
#[must_use]
pub fn system_prompt(root: &WorkspaceRoot) -> String {
    format!(
        "You are crest, a coding agent. You help the user with the code in their workspace \
         using the tools below.\n\
         \n\
         # Environment\n\
         - Workspace root: {root}. Tool paths are relative to it; you cannot leave it.\n\
         - Platform: {os} ({arch}).\n\
         \n\
         # How to work\n\
         - Locate before reading: `find_file` for names, `grep` for contents, `tree` or \
         `list_files` for structure. Read the relevant part with `read_file` before changing it.\n\
         - Make the smallest change that solves the task, with `edit_file` (an exact, unique \
         match). Use `write_file` only for new files or full rewrites.\n\
         - Changes and commands may need the user's approval. If one is denied, do not retry \
         it; ask or take another approach.\n\
         - Be concise. When you change something, end with a short explanation of what changed \
         and why.\n\
         \n\
         {COMPONENT_TOOLS_PROMPT}",
        root = root.as_path().display(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_prompt_names_the_workspace_and_the_components() {
        let root = WorkspaceRoot::new("/tmp/ws").unwrap();
        let prompt = system_prompt(&root);
        assert!(prompt.contains("Workspace root: /tmp/ws."), "{prompt}");
        assert!(prompt.contains(std::env::consts::OS));
        assert!(prompt.ends_with(COMPONENT_TOOLS_PROMPT));
    }

    #[test]
    fn config_defaults_to_the_current_directory_and_ask() {
        use clap::Parser;
        let args = Args::parse_from(["crest"]);
        let config = Config::from_args(&args).unwrap();
        assert_eq!(config.workspace, std::env::current_dir().unwrap());
        assert_eq!(config.mode, PermissionMode::Ask);
        assert_eq!(config.model, None);
        assert!(config.trust_mcp.is_empty());
    }
}
