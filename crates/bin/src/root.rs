//! The composition root: builds every adapter and wires them into the
//! [`AgentService`]. This is the only place that knows them all.
//!
//! [`build_app`] takes the parsed [`Config`], the [`Frontend`] that shows
//! events and answers approvals (the terminal in headless mode, the UI
//! later) and [`Overrides`] that replace external services in tests (the
//! model above all, see `tests/headless_demo.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use adapter_anthropic::{
    AnthropicBackend, AnthropicCatalog, AnthropicClient, ApiKey, ClientConfig, TokioClock,
};
use adapter_compute::StatsEngine;
use adapter_export::HtmlExporter;
use adapter_fs::{LocalFs, WorkspaceGuard};
use adapter_lua::{HostOptions, LuaHost, default_sources};
use adapter_search::RipgrepSearch;
use adapter_shell::LocalShell;
use adapter_store::{SqliteFlowStorage, SqliteSessionStore};
use adapter_telemetry::TracingTelemetry;
use anyhow::{Context as _, anyhow};
use application::cost::CostTracker;
use application::flows::implement::graph::{ImplementSession, plan_path};
use application::flows::implement::node::{AgentKit, ImplementDeps};
use application::flows::research::agents::ExplorePorts;
use application::flows::research::graph::ResearchWorkspace;
use application::flows::research::session::ResearchSession;
use application::llm_retry::RetryingLlm;
use application::ports::approval::ApprovalPort;
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::ports::fs::FileSystemPort;
use application::ports::llm::{LlmPort, ModelCatalogPort};
use application::ports::plugin::{
    AppOptions, CommandSpec, ConfigSources, HookSpec, KeymapSpec, McpServerSpec,
    PluginCapabilities, PluginHostPort, PluginToolSpec,
};
use application::ports::search::{ContentSearchPort, FileSearchPort};
use application::ports::shell::ShellPort;
use application::ports::store::{SessionStorePort, StoreError, StoredSession};
use application::ports::telemetry::TelemetryPort;
use application::ports::web::WebPort;
use application::providers::{Provider, ProviderRouter, ProvidersOptions, effective_default};
use application::tools::components::{COMPONENT_TOOLS_PROMPT, register_component_tools};
use application::tools::compute::{COMPUTE_TOOLS_PROMPT, ComputeToolPorts, register_compute_tools};
use application::tools::files::{FileToolPorts, register_file_tools};
use application::tools::plugin::PluginTool;
use application::tools::registry::ToolRegistry;
use application::tools::simulate::SimulateTool;
use application::tools::task::{TaskPorts, TaskTool};
use application::tools::web::WEB_TOOLS_PROMPT;
use application::usecases::approval::lock_permissions;
use application::usecases::compact::CompactionPolicy;
use application::usecases::resume::{restore_permission_rules, resume_latest};
use application::usecases::run_turn::{AgentDeps, AgentService, SharedPermissions, TurnOptions};
use application::usecases::simulate::{SimulateDeps, Simulator};
use domain::block::Block;
use domain::conversation::Session;
use domain::model::{Account, BuiltinDefaults, ModelPolicy, ModelRole, ProviderId};
use domain::permission::{PermissionMode, PermissionPolicy};
use domain::workspace::WorkspaceRoot;
use ui::timeline::TimelineState;

use crate::args::Args;
use crate::images::{ImageGuardSink, SharedGuard};
use crate::mcp::{self, McpServers, TrustOptions};
use crate::opencode::PlanAccess;
use crate::plugin_trust;
use crate::preferences::{self, Preferences};
use crate::research;
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
    /// `--trust-plugin`: plugins whose capabilities are approved on this
    /// run.
    pub trust_plugins: Vec<String>,
    /// Resume the latest session of the workspace: the window does unless
    /// `--new`, headless only with `--continue`.
    pub resume: bool,
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
            None => std::env::current_dir().context("cannot read the current directory")?,
        };
        Ok(Self::for_workspace(args, workspace))
    }

    /// The config of `args` over the workspace `workspace` (a project
    /// chosen in the picker).
    #[must_use]
    pub fn for_workspace(args: &Args, workspace: PathBuf) -> Self {
        Self {
            workspace,
            mode: args.mode.unwrap_or_default(),
            model: args.model.clone(),
            trust_mcp: args.trust_mcp.clone(),
            trust_plugins: args.trust_plugin.clone(),
            resume: if args.headless.is_some() {
                args.continue_session
            } else {
                !args.new
            },
        }
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
    /// Store of approved plugin capabilities instead of the default one.
    pub plugin_trust_path: Option<PathBuf>,
    /// The web instead of curl (still subject to `app.opt.web.enabled`).
    pub web: Option<Arc<dyn WebPort>>,
    /// The decision model instead of Clef (still subject to
    /// `app.opt.decision.enabled`).
    pub decision: Option<Arc<dyn application::ports::decision::DecisionPort>>,
    /// Where `/login` checks and stores the API key, instead of the
    /// Anthropic API and the system keyring.
    pub api_keys: Option<Arc<dyn application::ports::credentials::ApiKeyPort>>,
    /// The preferences file (last `/login`, models picked with `/model`)
    /// instead of the one in crest's data directory.
    pub preferences_path: Option<PathBuf>,
}

/// No API key of any provider in the environment nor in the keyring; the
/// window then opens the `/login` dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingApiKey;

impl std::fmt::Display for MissingApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "no model provider has an API key: run /login in the window, or export \
             ANTHROPIC_API_KEY or OPENCODE_API_KEY (or store it in the system keyring, service \
             `crest`, entry of the same name)",
        )
    }
}

impl std::error::Error for MissingApiKey {}

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
    /// The current session: the latest one of the workspace when resumed,
    /// otherwise a new one.
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
    /// Reads the workspace's files with the guard of the file tools
    /// (`/open @path`).
    pub fs: Arc<dyn FileSystemPort>,
    /// File search of the workspace (rg/fzf), for the tree and pickers.
    pub file_search: Arc<dyn FileSearchPort>,
    /// Text search of the workspace (rg), for the `<leader>fg` picker.
    pub content_search: Arc<dyn ContentSearchPort>,
    /// Runs programs in the workspace: `/!cmd` and the git state of the
    /// status bar.
    pub shell: Arc<dyn ShellPort>,
    /// Output of the `/!` commands run since the last prompt; it goes to
    /// the model with the next one.
    pub shell_context: Vec<String>,
    /// The Lua host that runs the callbacks of keymaps and commands, if Lua
    /// started.
    pub plugins: Option<Arc<dyn PluginHostPort>>,
    /// Keymaps of `init.lua`, in registration order.
    pub keymaps: Vec<KeymapSpec>,
    /// Commands of `init.lua` (`:name args`).
    pub commands: Vec<CommandSpec>,
    /// Options of `app.opt`.
    pub options: AppOptions,
    /// The themes `app.opt.theme` and `/theme` can name.
    pub themes: application::theme::ThemeCatalog,
    /// MCP servers declared in the config, started or not.
    pub mcp_specs: Vec<McpServerSpec>,
    /// Store of approved workspace MCP servers, if there is one.
    pub mcp_trust_path: Option<PathBuf>,
    /// Capability declarations of the config and plugins.
    pub plugin_capabilities: Vec<PluginCapabilities>,
    /// The declarations granted now (`/plugin trust` adds to them).
    pub plugin_grants: Arc<Mutex<Vec<PluginCapabilities>>>,
    /// Store of approved plugin capabilities, if there is one.
    pub plugin_trust_path: Option<PathBuf>,
    /// Tools of the plugins (registered in the agent when Lua started).
    pub plugin_tools: Vec<PluginToolSpec>,
    /// Hooks of the plugins, in the order they run.
    pub hooks: Vec<HookSpec>,
    /// The config files that were loaded.
    pub config_sources: ConfigSources,
    /// Database of sessions and flows, when known.
    pub db_path: Option<PathBuf>,
    /// The decision model (Clef), when it has credentials.
    pub decider: application::decide::Decider,
    /// The provider of model ids without prefix (see
    /// [`application::providers::effective_default`]).
    pub default_provider: ProviderId,
    /// The preferences file `/login` and `/model` write, if there is one.
    pub preferences_path: Option<PathBuf>,
    /// The models of every provider with a key, for the `/model` picker.
    pub catalog: Option<Arc<dyn ModelCatalogPort>>,
    models: ModelPolicy,
    events: Arc<dyn EventSink>,
    timeline: Arc<Mutex<TimelineState>>,
    prompt_history: SharedPromptHistory,
    store: Option<Arc<dyn SessionStorePort>>,
    implement: ImplementDeps,
    flow_storage: Option<Arc<SqliteFlowStorage>>,
    /// The research flow of the workspace (`/research`), when the flow
    /// store opened.
    pub research: Option<ResearchSession>,
    /// Simulates the solution of a Spec (`/simulate`, `--simulate`, the
    /// `simulate_solution` tool).
    pub simulator: Simulator,
    /// The statistics engine with the file port and blocking pool it runs
    /// on: the `stats_*` tools, Lua's `app.compute` and Recompute.
    pub compute: ComputeToolPorts,
}

/// The lines sent from the prompt in the session, oldest first; the UI adds
/// to them as they are sent and they are saved with the session.
pub type SharedPromptHistory = Arc<Mutex<Vec<String>>>;

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

    /// The sink of the front end (recording the saved timeline).
    #[must_use]
    pub fn events(&self) -> Arc<dyn EventSink> {
        Arc::clone(&self.events)
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

    /// Replaces the session with a new, empty one (`/clear`). The old one
    /// stays in the store.
    pub fn new_session(&mut self) {
        self.session = Session::new(self.workspace.as_path().display().to_string(), now_ms());
        *self.timeline.lock().unwrap_or_else(PoisonError::into_inner) = TimelineState::new();
        self.prompt_history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The blocks of the session's timeline, including those of a resumed
    /// session.
    #[must_use]
    pub fn timeline_blocks(&self) -> Vec<Block> {
        self.timeline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries()
            .iter()
            .map(|entry| entry.block.clone())
            .collect()
    }

    /// The session as it would be stored: history, timeline and the
    /// "allow always" rules granted so far.
    #[must_use]
    pub fn stored_session(&self) -> StoredSession {
        StoredSession {
            session: self.session.clone(),
            timeline: self.timeline_blocks(),
            permission_rules: lock_permissions(self.service.permissions()).rules.clone(),
            prompt_history: self.prompt_history().clone(),
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

    /// The lines sent from the prompt in the session, oldest first.
    pub fn prompt_history(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.prompt_history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The prompt history shared with the UI, which adds the sent lines.
    #[must_use]
    pub fn shared_prompt_history(&self) -> &SharedPromptHistory {
        &self.prompt_history
    }

    /// The permission policy shared with the agent.
    #[must_use]
    pub fn permissions(&self) -> &SharedPermissions {
        self.service.permissions()
    }

    /// The plan of the last implementation of the workspace, if it stopped
    /// before the end: what `/implement resume` continues.
    pub async fn pending_implementation(&self) -> Option<String> {
        let storage = self.flow_storage.clone()?;
        ImplementSession::pending(storage, self.workspace.as_path())
            .await
            .ok()
            .flatten()
            .map(|(plan, _)| plan)
    }

    /// Opens the implementation flow of the plan named by `target` (a Spec
    /// folder, its name or a `plan.json`; see [`plan_path`]), resuming it
    /// if it stopped before the end. Its checkpoints go to the SQLite flow
    /// store; nothing runs yet.
    ///
    /// # Errors
    ///
    /// A target outside the workspace, no flow store, or a storage error.
    pub async fn open_implementation(&self, target: &str) -> anyhow::Result<ImplementSession> {
        let path = plan_path(self.workspace.as_path(), target)?;
        let storage = self
            .flow_storage
            .clone()
            .ok_or_else(|| anyhow!("nowhere to save the flow: the crest database did not open"))?;
        Ok(ImplementSession::open(self.implement.clone(), storage, path).await?)
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
    let guard = WorkspaceGuard::open(&config.workspace).map_err(|err| {
        anyhow!(
            "cannot open the workspace {}: {err}",
            config.workspace.display()
        )
    })?;
    let root = guard.root().clone();
    // Image files named by plugins are checked like the file tools' paths;
    // the configured sensitive-path rules replace the defaults below.
    let image_guard: SharedGuard = Arc::new(RwLock::new(guard.clone()));

    let timeline = Arc::new(Mutex::new(TimelineState::new()));
    // Startup problems go straight to the front end: they belong to this
    // run, not to the saved timeline of the session.
    let startup_events = Arc::clone(&frontend.events);
    let events: Arc<dyn EventSink> = Arc::new(ImageGuardSink::new(
        Arc::new(TimelineSink {
            timeline: Arc::clone(&timeline),
            inner: frontend.events,
        }),
        Arc::clone(&image_guard),
    ));
    let mut warnings = Vec::new();

    let lua_options = HostOptions {
        workspace: Some(root.as_path().to_path_buf()),
        // Plugin modals belong to this run, not to the saved timeline.
        events: Some(Arc::new(ImageGuardSink::new(
            Arc::clone(&startup_events),
            Arc::clone(&image_guard),
        ))),
        ..HostOptions::default()
    };
    let plugins = match LuaHost::spawn(lua_options) {
        Ok(handle) => Some(Arc::new(handle)),
        Err(err) => {
            warnings.push(format!("Lua unavailable: {err}"));
            None
        }
    };
    let sources = overrides
        .config_sources
        .clone()
        .unwrap_or_else(|| default_sources(root.as_path()));
    let host = plugins.as_deref().map(|h| h as &dyn PluginHostPort);
    let config_sources = sources.clone();
    let (loaded, config_warnings) = startup::load_config(host, sources).await;
    warnings.extend(config_warnings);
    let plugin_trust = plugin_trust::TrustOptions {
        store: overrides
            .plugin_trust_path
            .clone()
            .or_else(plugin_trust::default_trust_path),
        trust_now: config.trust_plugins.clone(),
    };
    let (granted, grant_warnings) =
        plugin_trust::start(host, &loaded.plugin_capabilities, &plugin_trust).await;
    warnings.extend(grant_warnings);
    let (rules, rules_warning) = startup::sensitive_rules(&loaded);
    warnings.extend(rules_warning);
    let guard = guard.with_sensitive_rules(rules.clone());
    *image_guard.write().unwrap_or_else(PoisonError::into_inner) = guard.clone();

    let search = RipgrepSearch::new(root.clone()).with_sensitive_rules(rules);
    warnings.extend(search.check_tools().await.problems());

    // A scripted model (tests) only sees the preferences it is given.
    let preferences_path = overrides.preferences_path.clone().or_else(|| {
        overrides
            .llm
            .is_none()
            .then(preferences::default_path)
            .flatten()
    });
    let preferences = match preferences_path.as_deref().map(Preferences::load) {
        Some(Ok(preferences)) => preferences,
        Some(Err(err)) => {
            warnings.push(format!("the saved preferences were ignored: {err}"));
            Preferences::default()
        }
        None => Preferences::default(),
    };
    let (llm, catalog, default_provider) = model_ports(
        &overrides,
        &loaded.options,
        &preferences,
        preferences_path.as_deref(),
        &mut warnings,
    )
    .await?;
    let defaults = if default_provider == ProviderId::Anthropic {
        BuiltinDefaults::Claude
    } else {
        BuiltinDefaults::None
    };
    let selected = preferences.models.get(&default_provider).cloned();
    let mut policy = loaded.model_policy().with_selection(selected, defaults);
    if policy.needs_selection() {
        warnings.push(format!(
            "no model is selected for {default_provider}: pick one with /model"
        ));
    }
    let catalog_for_picker = catalog.clone();
    if let Some(catalog) = catalog {
        let (checked, model_warnings) =
            startup::validate_models(policy, catalog.as_ref(), CATALOG_TIMEOUT).await;
        policy = checked;
        warnings.extend(model_warnings);
    }

    let store = match open_store(overrides.db_path.as_deref()) {
        Ok(store) => Some(Arc::new(store) as Arc<dyn SessionStorePort>),
        Err(err) => {
            warnings.push(format!("the session will not be saved: {err}"));
            None
        }
    };

    let search = Arc::new(search);
    let file_search: Arc<dyn FileSearchPort> = search.clone();
    let content_search: Arc<dyn ContentSearchPort> = search.clone();
    let fs: Arc<dyn FileSystemPort> = Arc::new(LocalFs::new(guard));
    let (themes, theme_warnings) =
        startup::theme_catalog(config_sources.global.as_deref(), &loaded.themes);
    warnings.extend(theme_warnings);
    let (line_shell, shell_warning) = startup::line_shell(&loaded.options);
    warnings.extend(shell_warning);
    let shell: Arc<dyn ShellPort> =
        Arc::new(LocalShell::new(root.clone()).with_line_shell(line_shell));
    let telemetry: Arc<dyn TelemetryPort> = Arc::new(TracingTelemetry::new());
    let (web, web_warnings) = startup::web_tools(&loaded.options, overrides.web.clone()).await;
    warnings.extend(web_warnings);
    let (decider, decider_warnings) =
        startup::decider(&loaded.options, overrides.decision.clone()).await;
    warnings.extend(decider_warnings);
    let (redactor, privacy_warning) = startup::redactor(&loaded.options);
    warnings.extend(privacy_warning);
    let (rewriter, rtk_warning) = startup::rtk(&loaded.options, &root);
    warnings.extend(rtk_warning);
    let mut tools = ToolRegistry::new().with_redactor(redactor);
    register_file_tools(
        &mut tools,
        FileToolPorts {
            file_search: search.clone(),
            content_search: search.clone(),
            fs: Arc::clone(&fs),
            shell: Arc::clone(&shell),
            rewriter,
        },
    )
    .context("tool registration")?;
    register_component_tools(&mut tools).context("tool registration")?;
    let compute = compute_ports(Arc::clone(&fs));
    register_compute_tools(&mut tools, &compute).context("tool registration")?;
    if let Some(web) = &web {
        web.register(&mut tools).context("tool registration")?;
    }
    let task = TaskTool::new(
        TaskPorts {
            llm: Arc::clone(&llm),
            telemetry: Arc::clone(&telemetry),
            models: policy.clone(),
            file_search: search.clone(),
            content_search: search,
            fs: Arc::clone(&fs),
            web: web.clone(),
        },
        tools.budget(),
    )
    .context("tool registration")?;
    tools.register_typed(task).context("tool registration")?;
    let simulator = Simulator::new(SimulateDeps {
        llm: Arc::clone(&llm),
        models: policy.clone(),
        fs: Arc::clone(&fs),
        search: file_search.clone(),
        events: Some(Arc::clone(&events)),
        export: Some(Arc::new(HtmlExporter::new())),
    });
    tools
        .register_typed(SimulateTool::new(simulator.clone(), now_ms))
        .context("tool registration")?;
    if let Some(host) = &plugins {
        for spec in loaded.tools.iter().cloned() {
            let name = spec.name.clone();
            let tool = PluginTool::new(spec, host.clone() as Arc<dyn PluginHostPort>);
            if let Err(err) = tools.register(Arc::new(tool)) {
                warnings.push(format!("plugin tool `{name}` ignored: {err}"));
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
    let mcp_trust_path = trust.store.clone();
    let (mcp, mcp_warnings) = mcp::start(
        loaded.mcp_servers.clone(),
        root.as_path(),
        &trust,
        mcp::READY_TIMEOUT,
        &mut tools,
        Some(Arc::clone(&events)),
    )
    .await;
    warnings.extend(mcp_warnings);

    let permissions = Arc::new(Mutex::new(PermissionPolicy::new(config.mode)));
    let mut session = Session::new(root.as_path().display().to_string(), now_ms());
    let mut resumed_notice = None;
    let mut prompt_history = Vec::new();
    if config.resume
        && let Some(store) = &store
    {
        match resume_latest(store.as_ref(), &session.workspace_root).await {
            Ok(Some(stored)) => {
                restore_permission_rules(&permissions, &stored.permission_rules);
                let mut state = timeline.lock().unwrap_or_else(PoisonError::into_inner);
                for block in stored.timeline {
                    state.apply(&DomainEvent::BlockAdded { block });
                }
                resumed_notice = Some(format!(
                    "previous session resumed ({} turns); `crest --new` starts a new one",
                    stored.session.turns.len()
                ));
                session = stored.session;
                prompt_history = stored.prompt_history;
            }
            Ok(None) => {}
            Err(err) => warnings.push(format!("could not resume the last session: {err}")),
        }
    }
    let deps = AgentDeps {
        llm,
        events: Arc::clone(&events),
        approval: frontend.approval,
        telemetry,
    };
    let models = policy.clone();
    let mut kit = AgentKit::new(
        deps.clone(),
        tools.clone(),
        policy.clone(),
        Arc::clone(&permissions),
    );
    kit.system_prompt = system_prompt(&root);
    let implement = ImplementDeps::new(Arc::clone(&events), Arc::clone(&fs), Arc::clone(&shell))
        .with_agent(kit)
        .with_decider(decider.clone());
    let flow_storage = match open_flow_storage(overrides.db_path.as_deref()) {
        Ok(storage) => Some(Arc::new(storage)),
        Err(err) => {
            warnings.push(format!("flows will not be saved: {err}"));
            None
        }
    };
    // Built before the research flow, which prices its stages with it.
    let cost_tracker = CostTracker::from_options(&loaded.options);
    let pricing = cost_tracker
        .as_ref()
        .map(|tracker| tracker.pricing().clone())
        .unwrap_or_default();
    let research = match &flow_storage {
        Some(storage) => {
            let parts = research::ResearchParts {
                agent: deps.clone(),
                tools: tools.clone(),
                permissions: Arc::clone(&permissions),
                models: policy.clone(),
                explore: ExplorePorts {
                    llm: Arc::clone(&deps.llm),
                    telemetry: Arc::clone(&deps.telemetry),
                    models: policy.clone(),
                    file_search: Arc::clone(&file_search),
                    content_search: Arc::clone(&content_search),
                    fs: Arc::clone(&fs),
                    shell: Some(Arc::clone(&shell)),
                    extra_tools: Vec::new(),
                    activity: Arc::clone(&events),
                    web: web.clone(),
                },
                workspace: ResearchWorkspace {
                    fs: Arc::clone(&fs),
                    search: Arc::clone(&file_search),
                },
                system_prompt: system_prompt(&root),
                root: root.clone(),
                storage: Arc::clone(storage),
                decider: decider.clone(),
                pricing,
            };
            match research::build(parts) {
                Ok(session) => Some(session),
                Err(err) => {
                    warnings.push(format!("research flow unavailable: {err:#}"));
                    None
                }
            }
        }
        None => None,
    };
    let research_notice = match &research {
        Some(session) => research::pending_notice(session).await,
        None => None,
    };
    let implement_notice = match &flow_storage {
        Some(storage) => ImplementSession::pending(Arc::clone(storage) as _, root.as_path())
            .await
            .ok()
            .flatten()
            .map(|(plan, _)| {
                format!("the implementation of {plan} is pending; `/implement resume` continues it")
            }),
        None => None,
    };
    let mut prompt = system_prompt(&root);
    if web.is_some() {
        prompt.push_str("\n\n");
        prompt.push_str(WEB_TOOLS_PROMPT);
    }
    let mut service = AgentService::new(deps, tools, policy, permissions)
        .with_system_prompt(prompt)
        .with_compaction(CompactionPolicy::default())
        .with_decider(decider.clone());
    let plugins = plugins.map(|host| host as Arc<dyn PluginHostPort>);
    if let Some(host) = &plugins {
        service = service.with_plugin_host(Arc::clone(host));
    }
    // The spending cap asks through the front end's approval port.
    match cost_tracker {
        Ok(tracker) => service = service.with_cost_tracker(tracker),
        Err(err) => warnings.push(format!("cost: {err}; no pricing and no spend cap")),
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
    for message in resumed_notice
        .into_iter()
        .chain(research_notice)
        .chain(implement_notice)
    {
        startup_events.emit(DomainEvent::Notice {
            level: NoticeLevel::Info,
            message,
        });
    }
    let mut turn_options = TurnOptions::new(ModelRole::Main);
    if let Some(model) = &config.model {
        turn_options = turn_options.with_override(model.clone());
    }
    Ok(App {
        service,
        session,
        turn_options,
        workspace: root,
        warnings,
        config_errors: loaded.errors.iter().map(startup::diagnostic).collect(),
        mcp,
        fs,
        file_search,
        shell: Arc::clone(&shell),
        shell_context: Vec::new(),
        content_search,
        plugins,
        keymaps: loaded.keymaps,
        commands: loaded.commands,
        options: loaded.options,
        themes,
        mcp_specs: loaded.mcp_servers,
        mcp_trust_path,
        plugin_capabilities: loaded.plugin_capabilities,
        plugin_grants: Arc::new(Mutex::new(granted)),
        plugin_trust_path: plugin_trust.store,
        plugin_tools: loaded.tools,
        hooks: loaded.hooks,
        config_sources,
        db_path: overrides
            .db_path
            .clone()
            .or_else(adapter_store::path::default_db_path),
        decider,
        default_provider,
        preferences_path,
        catalog: catalog_for_picker,
        models,
        events,
        timeline,
        prompt_history: Arc::new(Mutex::new(prompt_history)),
        store,
        implement,
        flow_storage,
        research,
        simulator,
        compute,
    })
}

/// The statistics engine, reading data through `fs` and running on the
/// blocking pool of the current runtime.
fn compute_ports(fs: Arc<dyn FileSystemPort>) -> ComputeToolPorts {
    let handle = tokio::runtime::Handle::current();
    ComputeToolPorts {
        engine: Arc::new(StatsEngine::new()),
        fs,
        offload: Arc::new(move |job| {
            // The caller awaits the job's own channel, not the join handle.
            drop(handle.spawn_blocking(job));
        }),
    }
}

type ModelPorts = (
    Arc<dyn LlmPort>,
    Option<Arc<dyn ModelCatalogPort>>,
    ProviderId,
);

/// The model and its catalog: the overrides, or every provider with a key
/// in the environment or the keyring (Anthropic; OpenCode Go and Zen),
/// routed by the prefix of each model id (`app.opt.providers`).
/// Asks OpenCode what `key` may use and saves the answer in the
/// preferences at `path`, unless nothing could be found out (offline): then
/// the next start asks again.
async fn opencode_access(
    key: &str,
    providers: &ProvidersOptions,
    path: Option<&Path>,
    warnings: &mut Vec<String>,
) -> PlanAccess {
    let access = crate::opencode::probe_access(key, providers, CATALOG_TIMEOUT).await;
    let known = access.go.is_some() || access.zen.is_some() || access.zen_free.is_some();
    if let (true, Some(path)) = (known, path) {
        let path = path.to_path_buf();
        let saved = tokio::task::spawn_blocking(move || {
            preferences::update(Some(&path), |p| p.opencode_access = Some(access))
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|saved| saved);
        if let Err(err) = saved {
            warnings.push(format!(
                "what the OpenCode key may use was not saved: {err}"
            ));
        }
    }
    access
}

async fn model_ports(
    overrides: &Overrides,
    options: &AppOptions,
    preferences: &Preferences,
    preferences_path: Option<&Path>,
    warnings: &mut Vec<String>,
) -> anyhow::Result<ModelPorts> {
    let providers = ProvidersOptions::from_options(options).unwrap_or_else(|err| {
        warnings.push(format!("{err}; using the default providers"));
        ProvidersOptions::default()
    });
    if let Some(llm) = &overrides.llm {
        // A scripted model: only the config names the default provider.
        let default = effective_default(providers.default, None, &[]);
        return Ok((Arc::clone(llm), overrides.catalog.clone(), default));
    }
    // The keyring may block (D-Bus, Keychain).
    let (anthropic, opencode) = tokio::task::spawn_blocking(|| {
        (
            ApiKey::from_env_or_keyring().ok(),
            crate::credentials::stored_key(Account::OpenCode),
        )
    })
    .await
    .context("reading the API keys")?;
    // A plan left unknown (offline, a timeout) is asked again at each start.
    let access = match (&opencode, preferences.opencode_access) {
        (Some(key), saved) if saved.is_none_or(|a| !a.is_complete()) => {
            opencode_access(key, &providers, preferences_path, warnings).await
        }
        (_, saved) => saved.unwrap_or_default(),
    };
    let mut with_key = Vec::new();
    if anthropic.is_some() {
        with_key.push(ProviderId::Anthropic);
    }
    if opencode.is_some() {
        with_key.extend(
            [ProviderId::OpencodeGo, ProviderId::OpencodeZen]
                .into_iter()
                .filter(|plan| access.serves(*plan)),
        );
    }
    let preferred = preferences
        .provider
        .map(|p| Preferences::opencode_plan(p, &access));
    let default = effective_default(providers.default, preferred, &with_key);
    if opencode.is_some() && !access.serves(default) {
        warnings.push(format!(
            "your OpenCode key cannot use {default} (no subscription or balance); \
             pick another model with /model or set app.opt.providers.default"
        ));
    }
    let mut router = ProviderRouter::new(default);
    if let Some(key) = anthropic {
        let client = AnthropicClient::new(&key, ClientConfig::default())
            .map_err(|err| anyhow!("cannot create the Anthropic client: {err}"))?;
        router = router.with(
            ProviderId::Anthropic,
            Provider {
                backend: Arc::new(AnthropicBackend::new(client.clone())),
                catalog: Arc::new(AnthropicCatalog::new(client)),
            },
        );
    }
    if let Some(key) = opencode {
        for plan in [ProviderId::OpencodeGo, ProviderId::OpencodeZen] {
            let provider = crate::opencode::provider(&key, plan, &providers, access)
                .map_err(|err| anyhow!("cannot create the OpenCode client: {err}"))?;
            router = router.with(plan, provider);
        }
    }
    if router.is_empty() {
        return Err(MissingApiKey.into());
    }
    if !router.providers().any(|p| p == default) {
        let account = default.account();
        warnings.push(format!(
            "no {} API key, so models without a provider prefix (the defaults) cannot run: \
             /login {account}, or point app.models.roles at prefixed models \
             (opencode-go:glm-5.3) or set app.opt.providers.default",
            account.display_name()
        ));
    }
    let catalog = overrides
        .catalog
        .clone()
        .unwrap_or_else(|| Arc::new(router.catalog()));
    let llm = RetryingLlm::new(Arc::new(router), Arc::new(TokioClock));
    Ok((Arc::new(llm), Some(catalog), default))
}

fn open_store(path: Option<&Path>) -> Result<SqliteSessionStore, StoreError> {
    match path {
        Some(path) => SqliteSessionStore::open(path),
        None => SqliteSessionStore::open_default(),
    }
}

fn open_flow_storage(path: Option<&Path>) -> Result<SqliteFlowStorage, StoreError> {
    match path {
        Some(path) => SqliteFlowStorage::open(path),
        None => SqliteFlowStorage::open_default(),
    }
}

/// The system prompt: role, workspace, platform, how to work, the
/// component tools and the statistics tools.
#[must_use]
pub fn system_prompt(root: &WorkspaceRoot) -> String {
    format!(
        "You are crest, a coding agent. You help the user with the code in their workspace \
         using the tools below.\n\
         \n\
         # Environment\n\
         - Workspace root: {root}. Tool paths are relative to it; you cannot leave it.\n\
         - Platform: {os} ({arch}).\n\
         {windows}\
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
         {COMPONENT_TOOLS_PROMPT}\n\
         \n\
         {COMPUTE_TOOLS_PROMPT}",
        root = root.as_path().display(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        windows = if cfg!(windows) { WINDOWS_NOTE } else { "" },
    )
}

/// What the model must know on Windows, where its Unix habits fail.
const WINDOWS_NOTE: &str = "- `run_command` starts programs directly, without a shell: \
     Unix tools (`ls`, `cat`, `sh`, `sed`) are usually missing and shell builtins \
     (`dir`, `echo`) are not programs. Prefer crest's file tools; run builtins and \
     pipelines with `powershell -NoProfile -Command \"...\"`.\n";

/// Wall clock in Unix epoch milliseconds.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_prompt_names_the_workspace_and_the_components() {
        let dir = std::env::temp_dir().join("ws");
        let root = WorkspaceRoot::new(&dir).unwrap();
        let prompt = system_prompt(&root);
        let line = format!("Workspace root: {}.", dir.display());
        assert!(prompt.contains(&line), "{prompt}");
        assert!(prompt.contains(std::env::consts::OS));
        assert_eq!(prompt.contains(WINDOWS_NOTE), cfg!(windows));
        assert!(prompt.contains(COMPONENT_TOOLS_PROMPT));
        assert!(prompt.ends_with(COMPUTE_TOOLS_PROMPT));
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
        assert!(config.trust_plugins.is_empty());
        assert!(config.resume, "the window resumes by default");
        let headless = Config::from_args(&Args::parse_from(["crest", "--headless", "x"])).unwrap();
        assert!(!headless.resume);
    }
}
