//! The window mode: the composition root behind the GPUI session.
//!
//! GPUI runs its own executor on the main thread, while the agent and the
//! adapters (reqwest, rg/fzf processes, SQLite, Lua) need tokio. The
//! binary's multi-thread runtime runs them:
//!
//! - [`UiController`] implements the UI's [`SessionController`] by spawning
//!   each turn on the runtime and handing GPUI a channel that resolves when
//!   the turn ended and the session was saved.
//! - [`TokioPort`] wraps the search ports the tree and the pickers call
//!   from GPUI so the searches run on the runtime too.
//! - The domain events go back through [`ChannelSink`], a futures channel
//!   that the session consumes on GPUI's executor.
//!
//! `/implement <ruta>` opens the implementation flow of a Spec on the
//! runtime ([`UiController::run_command`]); while it waits at a gate,
//! every line sent from the input answers it instead of starting a turn,
//! until the flow stops.
//!
//! `/research <problema>` starts the research flow of the workspace the
//! same way (see [`crate::research`]): while it waits at a gate, the lines
//! sent from the input go to it. A side question (`/aside`, or a line the
//! decision model reads as one) goes to the chat instead, with the gate as
//! context, and `/flow` goes back with an answer (see [`crate::aside`]). `/stage`, `/back`, `/skip`, `/retry` and
//! `/abort` go to the active implementation flow, or else to the research
//! flow. `Ctrl+C` aborts the running commands
//! ([`UiController::interrupt_commands`](SessionController::interrupt_commands));
//! a stopped flow stays saved at its last checkpoint and `/research resume`
//! or `/implement resume` continue it.
//!
//! When the agent cannot start (no API key) the window still opens: the
//! error is the first block of the timeline and every prompt answers with
//! it ([`UnavailableController`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use adapter_fs::LocalProjectBrowser;
use adapter_search::RipgrepSearch;
use application::decide::Decider;
use application::flows::aside::{GateInfo, last_answer, preamble};
use application::flows::implement::flow::{FlowError, FlowReply};
use application::flows::implement::graph::{ImplementSession, status_notice};
use application::flows::implement::state::FlowStatus;
use application::ports::credentials::ApiKeyPort;
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::ports::fs::{
    DirEntry, EditFileRequest, FileChange, FileContent, FileSystemPort, FsError, PendingChange,
    ReadFileRequest, WriteFileRequest,
};
use application::ports::llm::{ModelCatalogPort, ModelInfo};
use application::ports::mcp::{McpPort, McpServerState};
use application::ports::plugin::{
    CallbackId, ConfigOrigin, McpServerSpec, PluginError, PluginHostPort, PluginOutput,
};
use application::ports::projects::{BrowseError, DirListing, ProjectBrowserPort, RecentWorkspace};
use application::ports::search::{
    ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepRequest, GrepResult,
    ListFilesRequest, SearchError, TreeNode, TreeRequest,
};
use application::ports::store::{SessionStorePort as _, StoreError};
use application::usecases::approval::lock_permissions;
use application::usecases::cancel::CancelToken;
use application::usecases::permissions::{
    ProjectRules, RuleListing, load_rules, promote_rule, remove_rule,
};
use application::usecases::run_turn::{SharedPermissions, UserInput};
use application::usecases::simulate::{SimulateRequest, SimulationReport, Simulator};
use application::usecases::user_blocks::{chat_only_notice, user_blocks};
use async_trait::async_trait;
use domain::block::{Block, BlockKind, Cell, Column, TableSpec};
use domain::conversation::{ImageAttachment, SessionId, StopReason};
use domain::git::GitStatus;
use domain::model::{Account, ModelId, ModelPolicy, ProviderId, ResolveRequest, SpecSource};
use domain::permission::{PermissionMode, PermissionRule};
use domain::workspace::{WorkspacePath, WorkspaceRoot};
use futures::FutureExt as _;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;
use ui::approval::UiApproval;
use ui::chats::{ChatEntry, ProjectChats, SessionOpen, chat_label, project_name};
use ui::form::UiForm;
use ui::help::HelpSnapshot;
use ui::session::{
    ModelChoice, Reply, SessionController, SessionParts, TurnEnd, TurnRequest, prompt_block,
};
use ui::workbench::{ProjectOpener, Workbench};

use crate::args::Args;
use crate::aside::{Aside, Route, opened_notice};
use crate::plugin_trust::PluginTrust;
use crate::preferences::Preferences;
use crate::research::ResearchCommand;
use crate::root::{
    App, Config, Frontend, MissingApiKey, Overrides, SharedPromptHistory, build_app,
};

/// [`EventSink`] that sends every event to the UI.
#[derive(Debug, Clone)]
pub struct ChannelSink(mpsc::UnboundedSender<DomainEvent>);

impl ChannelSink {
    /// A sink and the receiver the session consumes.
    #[must_use]
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<DomainEvent>) {
        let (tx, rx) = mpsc::unbounded();
        (Self(tx), rx)
    }
}

impl EventSink for ChannelSink {
    fn emit(&self, event: DomainEvent) {
        // A closed window drops the receiver; nothing is left to show.
        self.0.unbounded_send(event).ok();
    }
}

/// A port whose calls run on the tokio runtime of `handle`, awaited from
/// any executor.
pub struct TokioPort<T: ?Sized> {
    inner: Arc<T>,
    handle: Handle,
}

impl<T: ?Sized> TokioPort<T> {
    /// Wraps `inner`.
    pub fn new(inner: Arc<T>, handle: Handle) -> Self {
        Self { inner, handle }
    }

    async fn run<R, F>(&self, call: impl FnOnce(Arc<T>) -> F) -> Result<R, SearchError>
    where
        F: Future<Output = Result<R, SearchError>> + Send + 'static,
        R: Send + 'static,
    {
        // A runtime shutting down drops the task: report it as cancelled.
        self.handle
            .spawn(call(Arc::clone(&self.inner)))
            .await
            .unwrap_or(Err(SearchError::Cancelled))
    }
}

#[async_trait]
impl<T: FileSearchPort + ?Sized + 'static> FileSearchPort for TokioPort<T> {
    async fn list_files(&self, request: ListFilesRequest) -> Result<FileList, SearchError> {
        self.run(|port| async move { port.list_files(request).await })
            .await
    }

    async fn find_file(&self, request: FindFileRequest) -> Result<FileList, SearchError> {
        self.run(|port| async move { port.find_file(request).await })
            .await
    }

    async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
        self.run(|port| async move { port.tree(request).await })
            .await
    }
}

#[async_trait]
impl<T: ContentSearchPort + ?Sized + 'static> ContentSearchPort for TokioPort<T> {
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, SearchError> {
        self.run(|port| async move { port.grep(request).await })
            .await
    }
}

impl<T: ?Sized> TokioPort<T> {
    async fn run_fs<R, F>(&self, call: impl FnOnce(Arc<T>) -> F) -> Result<R, FsError>
    where
        F: Future<Output = Result<R, FsError>> + Send + 'static,
        R: Send + 'static,
    {
        self.handle
            .spawn(call(Arc::clone(&self.inner)))
            .await
            .unwrap_or_else(|_| Err(FsError::Io("the runtime is shutting down".into())))
    }
}

/// The file system the pickers preview through (only `read_file` is used
/// there; the rest forwards too, for completeness).
#[async_trait]
impl<T: FileSystemPort + ?Sized + 'static> FileSystemPort for TokioPort<T> {
    fn root(&self) -> &WorkspaceRoot {
        self.inner.root()
    }

    async fn read_file(&self, request: ReadFileRequest) -> Result<FileContent, FsError> {
        self.run_fs(|port| async move { port.read_file(request).await })
            .await
    }

    async fn read_bytes(&self, path: &WorkspacePath, max: u64) -> Result<Vec<u8>, FsError> {
        let path = path.clone();
        self.run_fs(|port| async move { port.read_bytes(&path, max).await })
            .await
    }

    async fn read_head(&self, path: &WorkspacePath, max: u64) -> Result<(Vec<u8>, u64), FsError> {
        let path = path.clone();
        self.run_fs(|port| async move { port.read_head(&path, max).await })
            .await
    }

    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError> {
        let path = path.clone();
        self.run_fs(|port| async move { port.exists(&path).await })
            .await
    }

    async fn list_dir(&self, path: &WorkspacePath) -> Result<Vec<DirEntry>, FsError> {
        let path = path.clone();
        self.run_fs(|port| async move { port.list_dir(&path).await })
            .await
    }

    async fn prepare_edit(&self, request: EditFileRequest) -> Result<PendingChange, FsError> {
        self.run_fs(|port| async move { port.prepare_edit(request).await })
            .await
    }

    async fn prepare_write(&self, request: WriteFileRequest) -> Result<PendingChange, FsError> {
        self.run_fs(|port| async move { port.prepare_write(request).await })
            .await
    }

    async fn apply(&self, change: PendingChange) -> Result<FileChange, FsError> {
        self.run_fs(|port| async move { port.apply(change).await })
            .await
    }
}

/// The models of a catalog for the `/model` picker, grouped by provider
/// (ids without prefix first) and sorted by id within each.
#[must_use]
pub fn catalog_choices(models: Vec<ModelInfo>) -> Vec<ModelChoice> {
    let mut choices: Vec<ModelChoice> = models
        .into_iter()
        .map(|info| ModelChoice {
            alias: info.id.as_str().to_owned(),
            model: info.id,
        })
        .collect();
    choices.sort_by(|a, b| {
        let provider = |c: &ModelChoice| c.model.provider().map(|(p, _)| p);
        provider(a)
            .cmp(&provider(b))
            .then_with(|| a.model.as_str().cmp(b.model.as_str()))
    });
    choices.dedup_by(|a, b| a.model == b.model);
    choices
}

/// The models of `policy` for the `<leader>m` menu.
#[must_use]
pub fn model_choices(policy: &ModelPolicy) -> Vec<ModelChoice> {
    let mut choices: Vec<ModelChoice> = policy
        .aliases()
        .into_iter()
        .map(|(alias, model)| ModelChoice { alias, model })
        .collect();
    // Grouped by provider: models without a prefix (the default provider)
    // first, then each prefix.
    choices.sort_by_key(|choice| choice.model.provider().map(|(provider, _)| provider));
    choices
}

/// The UI's input port over the real [`App`].
pub struct UiController {
    handle: Handle,
    app: Arc<tokio::sync::Mutex<App>>,
    permissions: SharedPermissions,
    /// The project's `permissions.local.toml`, for `/permissions`.
    project_rules: Option<ProjectRules>,
    prompt_history: SharedPromptHistory,
    models: Vec<ModelChoice>,
    plugins: Option<Arc<dyn PluginHostPort>>,
    commands: HashMap<String, CallbackId>,
    mcp: Arc<McpCommands>,
    plugin_trust: Arc<PluginTrust>,
    events: Arc<dyn EventSink>,
    flow: ActiveFlow,
    research: SharedResearch,
    /// The side chat of the waiting flow (`/aside`, `/flow`).
    aside: SharedAside,
    /// Reads the lines typed at a gate (side questions).
    decider: Decider,
    simulator: Simulator,
    shell: Arc<dyn application::ports::shell::ShellPort>,
    /// Commands running on the runtime, aborted by `Ctrl+C`.
    running: Arc<Mutex<Vec<AbortHandle>>>,
    /// Where `/login` stores the API key.
    api_keys: Arc<dyn ApiKeyPort>,
    /// Where `/login` and `/model` save their choices, if anywhere.
    preferences_path: Option<PathBuf>,
    /// The provider of ids without prefix; `/model` saves its pick for it.
    default_provider: ProviderId,
    /// The models of the providers with a key, for the `/model` picker.
    catalog: Option<Arc<dyn ModelCatalogPort>>,
    /// The themes `/theme` offers.
    themes: application::theme::ThemeCatalog,
    /// `app.opt` of the config: its `theme` wins at start, its `colors`
    /// go over every theme.
    theme_options: application::ports::plugin::AppOptions,
    /// The statistics engine, for Recompute on a card.
    compute: application::tools::compute::ComputeToolPorts,
}

/// The research flow of the workspace, taken out of the [`App`] so a flow
/// line does not wait for (or block) the chat's turns.
type SharedResearch = Arc<tokio::sync::Mutex<Option<crate::research::Research>>>;

/// `/research <args>`: starts, resumes or reports the research flow and
/// saves the chat session (its timeline holds the flow's blocks).
async fn research_command(
    app: Arc<tokio::sync::Mutex<App>>,
    research: SharedResearch,
    args: String,
) -> Reply {
    let mut guard = research.lock().await;
    let Some(session) = guard.as_mut() else {
        return Reply::Notice(
            NoticeLevel::Warning,
            "/research is unavailable: the crest database did not open".into(),
        );
    };
    let result = crate::research::command(
        session,
        ResearchCommand::parse(&args),
        crate::root::now_ms(),
    )
    .await;
    drop(guard);
    save(&app).await;
    match result {
        Ok(message) => Reply::Notice(NoticeLevel::Info, message),
        Err(message) => Reply::Notice(NoticeLevel::Warning, message),
    }
}

/// `/simulate <spec>`: simulates the Spec's solution; the use case opens
/// the board (`SimulationReady`) and the timeline gets the verdicts.
async fn simulate_command(simulator: Simulator, spec: String) -> Reply {
    let request = SimulateRequest {
        spec,
        now_ms: crate::root::now_ms(),
        open_board: true,
    };
    match simulator.run(request).await {
        Ok(report) => Reply::block(Block::new(BlockKind::Markdown {
            title: Some(format!("Simulation of {}", report.folder)),
            source: simulation_markdown(&report),
        })),
        Err(err) => Reply::Notice(NoticeLevel::Warning, format!("/simulate: {err}")),
    }
}

/// The verdicts of a simulation as markdown.
#[must_use]
pub fn simulation_markdown(report: &SimulationReport) -> String {
    let lines: String = report
        .summary()
        .lines()
        .map(|line| {
            if line.starts_with("  ") {
                format!("  - {}\n", line.trim_start())
            } else {
                format!("- {line}\n")
            }
        })
        .collect();
    lines
}

/// Saves the chat session, warning on failure.
async fn save(app: &tokio::sync::Mutex<App>) {
    let app = app.lock().await;
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
}

/// The implementation flow the chat drives, if one is active.
type ActiveFlow = Arc<tokio::sync::Mutex<Option<ImplementSession>>>;

/// Keeps `session` active while it waits at a gate; a stopped (or failed)
/// flow gives the notice that says what it means.
fn settle(
    active: &mut Option<ImplementSession>,
    session: ImplementSession,
    result: Result<FlowStatus, FlowError>,
) -> Option<(NoticeLevel, String)> {
    match result {
        Ok(status) if !status.is_finished() => {
            *active = Some(session);
            None
        }
        Ok(status) => status_notice(&status),
        // The step already showed its error; the flow stays stored at its
        // last checkpoint.
        Err(err) => Some((
            NoticeLevel::Warning,
            format!(
                "the implementation flow stopped: {err}; `/implement resume` runs the step again"
            ),
        )),
    }
}

/// `/implement <target>`: opens the flow of `target` and runs it to its
/// first gate.
async fn start_implementation(
    app: Arc<tokio::sync::Mutex<App>>,
    flow: ActiveFlow,
    target: String,
) -> Reply {
    let mut active = flow.lock().await;
    if matches!(target.trim(), "cancel" | "cancelar" | "descartar" | "drop") {
        return cancel_implementation(&app, &mut active).await;
    }
    if let Some(session) = active.as_ref() {
        return Reply::Notice(
            NoticeLevel::Warning,
            format!(
                "an implementation flow is already active ({}); answer it, type /abort or \
                 `/implement cancel`",
                session.plan_path()
            ),
        );
    }
    let target = match target.trim() {
        "" | "resume" | "retry" | "continue" | "retomar" | "reanudar" => {
            match app.lock().await.pending_implementation().await {
                Some(plan) => plan,
                None => {
                    return Reply::Notice(
                        NoticeLevel::Warning,
                        "no implementation flow to resume; `/implement <spec>` starts one".into(),
                    );
                }
            }
        }
        target => target.to_owned(),
    };
    let opened = app.lock().await.open_implementation(&target).await;
    let session = match opened {
        Ok(session) => session,
        Err(err) => return Reply::Notice(NoticeLevel::Warning, format!("/implement: {err:#}")),
    };
    let resumed = session
        .resumed()
        .then(|| format!("implementation flow of {} resumed", session.plan_path()));
    let result = session.run().await;
    match settle(&mut active, session, result) {
        Some((level, message)) => Reply::Notice(level, message),
        None => resumed.map_or(Reply::Done, |m| Reply::Notice(NoticeLevel::Info, m)),
    }
}

/// `/implement cancel`: stops the active implementation, or else the
/// pending one of the workspace, for good.
async fn cancel_implementation(
    app: &tokio::sync::Mutex<App>,
    active: &mut Option<ImplementSession>,
) -> Reply {
    let session = match active.take() {
        Some(session) => session,
        None => {
            let app = app.lock().await;
            let Some(plan) = app.pending_implementation().await else {
                return Reply::Notice(NoticeLevel::Info, "no implementation flow to cancel".into());
            };
            match app.open_implementation(&plan).await {
                Ok(session) => session,
                Err(err) => {
                    return Reply::Notice(NoticeLevel::Warning, format!("/implement: {err:#}"));
                }
            }
        }
    };
    match session.cancel().await {
        Ok(_) => Reply::Notice(
            NoticeLevel::Info,
            format!(
                "implementation of {} cancelled; its journal is kept",
                session.plan_path()
            ),
        ),
        Err(err) => Reply::Notice(NoticeLevel::Warning, format!("/implement cancel: {err}")),
    }
}

/// Answers the gate of the active flow with `line`; the flow stays active
/// while it waits at another gate.
async fn answer_gate(active: &mut Option<ImplementSession>, line: &str, events: &dyn EventSink) {
    let Some(session) = active.take() else {
        return;
    };
    let result = match session.handle_line(line).await {
        Ok(FlowReply::Status(status)) => Ok(status),
        Ok(FlowReply::Position(position)) => {
            events.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message: position.to_string(),
            });
            session.status().await
        }
        Err(err) => Err(err),
    };
    if let Some((level, message)) = settle(active, session, result) {
        events.emit(DomainEvent::Notice { level, message });
    }
}

/// Usage of `/mcp`.
const MCP_USAGE: &str = "usage: /mcp [list] | /mcp tools <name> | /mcp restart|enable|disable \
                         <name> | /mcp login|logout <name> | /mcp trust <name>";

/// What `/mcp` needs: the declared servers, the approvals store and the
/// running servers.
struct McpCommands {
    specs: Vec<McpServerSpec>,
    workspace: PathBuf,
    store: Option<PathBuf>,
    servers: Option<(Arc<adapter_mcp::McpManager>, Arc<dyn McpPort>)>,
}

impl McpCommands {
    /// `/mcp <args>`.
    async fn run(self: Arc<Self>, args: String) -> Reply {
        let words: Vec<&str> = args.split_whitespace().collect();
        match words.as_slice() {
            [] | ["list"] => self.list().await,
            ["tools", name] => self.tools(name).await,
            ["trust", name] => self.trust(name).await,
            [
                verb @ ("restart" | "enable" | "disable" | "login" | "logout"),
                name,
            ] => self.manage(verb, name).await,
            _ => Reply::Notice(NoticeLevel::Warning, MCP_USAGE.into()),
        }
    }

    fn port(&self) -> Option<&Arc<dyn McpPort>> {
        self.servers.as_ref().map(|(_, port)| port)
    }

    fn no_servers() -> Reply {
        Reply::Notice(
            NoticeLevel::Info,
            "no MCP server is declared (app.mcp in init.lua)".into(),
        )
    }

    /// Every server with its state, transport, origin and what to do next.
    async fn list(&self) -> Reply {
        let Some(port) = self.port() else {
            return Self::no_servers();
        };
        let statuses = port.servers().await;
        let origin = |name: &str| {
            self.specs
                .iter()
                .find(|s| s.name == name)
                .map_or("", |s| match s.origin {
                    ConfigOrigin::Global => "global",
                    ConfigOrigin::Workspace => "workspace",
                })
        };
        let rows = statuses
            .iter()
            .map(|status| {
                let detail = match &status.state {
                    McpServerState::Untrusted => format!("/mcp trust {}", status.name),
                    McpServerState::Disabled => format!("/mcp enable {}", status.name),
                    McpServerState::NeedsLogin { .. } => format!("/mcp login {}", status.name),
                    McpServerState::Ready { .. } => format!("/mcp tools {}", status.name),
                    state => state.message().unwrap_or_default().to_owned(),
                };
                vec![
                    Cell::Text(status.name.clone()),
                    Cell::Text(status.state.label()),
                    Cell::Text(status.transport.clone()),
                    Cell::Text(origin(&status.name).to_owned()),
                    Cell::Text(detail),
                ]
            })
            .collect();
        Reply::block(Block::new(BlockKind::Table(TableSpec {
            columns: ["MCP server", "State", "Transport", "Config", "Detail"]
                .map(Column::new)
                .to_vec(),
            rows,
        })))
    }

    /// The tools of `name`, as the model sees them.
    async fn tools(&self, name: &str) -> Reply {
        let Some(port) = self.port() else {
            return Self::no_servers();
        };
        let Some(status) = port.servers().await.into_iter().find(|s| s.name == name) else {
            return Reply::Notice(
                NoticeLevel::Warning,
                format!("/mcp: no MCP server `{name}`"),
            );
        };
        if !matches!(status.state, McpServerState::Ready { .. }) {
            let why = status.state.message().unwrap_or_default();
            return Reply::Notice(
                NoticeLevel::Warning,
                format!(
                    "MCP server `{name}` is {}{}",
                    status.state.label(),
                    if why.is_empty() {
                        String::new()
                    } else {
                        format!(": {why}")
                    }
                ),
            );
        }
        let tools = port.list_tools().await.unwrap_or_default();
        let rows: Vec<Vec<Cell>> = tools
            .into_iter()
            .filter(|t| t.server == name)
            .map(|tool| {
                let summary = tool.description.lines().next().unwrap_or_default();
                let summary: String = summary.chars().take(160).collect();
                vec![Cell::Text(tool.qualified_name()), Cell::Text(summary)]
            })
            .collect();
        if rows.is_empty() {
            return Reply::Notice(
                NoticeLevel::Info,
                format!("MCP server `{name}` offers no tools"),
            );
        }
        Reply::block(Block::new(BlockKind::Table(TableSpec {
            columns: vec![Column::new("Tool"), Column::new("Description")],
            rows,
        })))
    }

    /// `restart`, `enable`, `disable`, `login` or `logout` of `name`.
    async fn manage(&self, verb: &str, name: &str) -> Reply {
        let Some(port) = self.port() else {
            return Self::no_servers();
        };
        let done = match verb {
            "restart" => port
                .restart(name)
                .await
                .map(|()| format!("MCP server `{name}` restarting")),
            "enable" => port
                .set_enabled(name, true)
                .await
                .map(|()| format!("MCP server `{name}` enabled")),
            "disable" => port.set_enabled(name, false).await.map(|()| {
                format!("MCP server `{name}` disabled for this session (disabled = true in init.lua keeps it off)")
            }),
            "login" => port.login(name).await,
            _ => port.logout(name).await.map(|deleted| {
                if deleted {
                    format!("MCP server `{name}`: login tokens deleted")
                } else {
                    format!("MCP server `{name}` had no stored login")
                }
            }),
        };
        match done {
            Ok(message) => Reply::Notice(NoticeLevel::Info, message),
            Err(err) => Reply::Notice(NoticeLevel::Warning, format!("/mcp {verb}: {err}")),
        }
    }

    /// Approves the workspace server `name` and starts it.
    async fn trust(self: &Arc<Self>, name: &str) -> Reply {
        // Reads and writes the store file: off the async workers.
        let this = Arc::clone(self);
        let owned = name.to_owned();
        let saved = tokio::task::spawn_blocking(move || {
            crate::mcp::trust_declared(&this.specs, &this.workspace, this.store.as_deref(), &owned)
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()));
        match saved {
            Ok(message) => {
                if let Some((manager, _)) = &self.servers {
                    let _ = manager.trust(name);
                }
                Reply::Notice(NoticeLevel::Info, message)
            }
            Err(message) => Reply::Notice(NoticeLevel::Warning, format!("/mcp trust: {message}")),
        }
    }
}

/// `/plugin <args>`: `trust <name>` or `list`.
async fn plugin_command(trust: Arc<PluginTrust>, args: String) -> Reply {
    let words: Vec<&str> = args.split_whitespace().collect();
    match words.as_slice() {
        ["list"] => Reply::Notice(NoticeLevel::Info, trust.list()),
        ["trust", name] => match trust.trust(name).await {
            Ok(message) => Reply::Notice(NoticeLevel::Info, message),
            Err(message) => {
                Reply::Notice(NoticeLevel::Warning, format!("/plugin trust: {message}"))
            }
        },
        _ => Reply::Notice(
            NoticeLevel::Warning,
            "usage: /plugin list | /plugin trust <name>".into(),
        ),
    }
}

impl UiController {
    /// The router of the chat lines over this controller's flows.
    fn router(&self) -> Router {
        Router {
            app: Arc::clone(&self.app),
            flow: Arc::clone(&self.flow),
            research: Arc::clone(&self.research),
            events: Arc::clone(&self.events),
            aside: Arc::clone(&self.aside),
            decider: self.decider.clone(),
        }
    }

    /// A controller running the turns of `app` on `handle`.
    #[must_use]
    pub fn new(mut app: App, handle: Handle) -> Self {
        let research = Arc::new(tokio::sync::Mutex::new(app.research.take()));
        let permissions = Arc::clone(app.permissions());
        let project_rules = app.service.project_rules().cloned();
        let prompt_history = Arc::clone(app.shared_prompt_history());
        let models = model_choices(app.model_policy());
        let plugins = app.plugins.clone();
        let mcp = Arc::new(McpCommands {
            specs: app.mcp_specs.clone(),
            workspace: app.workspace.as_path().to_path_buf(),
            store: app.mcp_trust_path.clone(),
            servers: app
                .mcp
                .as_ref()
                .map(|servers| (Arc::clone(&servers.manager), Arc::clone(&servers.port))),
        });
        let plugin_trust = Arc::new(PluginTrust {
            decls: app.plugin_capabilities.clone(),
            store: app.plugin_trust_path.clone(),
            granted: Arc::clone(&app.plugin_grants),
            host: app.plugins.clone(),
        });
        let events = app.events();
        let simulator = app.simulator.clone();
        let decider = app.decider.clone();
        let shell = Arc::clone(&app.shell);
        let preferences_path = app.preferences_path.clone();
        let default_provider = app.default_provider;
        let catalog = app.catalog.clone();
        let themes = app.themes.clone();
        let theme_options = app.options.clone();
        let compute = app.compute.clone();
        let commands = app
            .commands
            .iter()
            .map(|c| (c.name.clone(), c.callback))
            .collect();
        Self {
            handle,
            app: Arc::new(tokio::sync::Mutex::new(app)),
            permissions,
            project_rules,
            prompt_history,
            models,
            plugins,
            commands,
            mcp,
            plugin_trust,
            events,
            flow: Arc::new(tokio::sync::Mutex::new(None)),
            research,
            aside: Arc::default(),
            decider,
            simulator,
            shell,
            running: Arc::new(Mutex::new(Vec::new())),
            api_keys: Arc::new(crate::credentials::KeyringCredentials::default()),
            preferences_path,
            default_provider,
            catalog,
            themes,
            theme_options,
            compute,
        }
    }
}

/// Recompute on a card: re-runs the envelope of `block` (reading its data
/// file again) and sends the new version, with `verification` filled in,
/// as a `BlockUpdated`.
async fn recompute_block(
    compute: application::tools::compute::ComputeToolPorts,
    events: Arc<dyn EventSink>,
    block: domain::block::Block,
) -> Result<(), String> {
    let domain::block::BlockKind::Computed { envelope, .. } = &block.kind else {
        return Err("only statistics cards can be recomputed".into());
    };
    let (computed, body) = application::usecases::compute::recompute(
        Arc::clone(&compute.engine),
        Some(compute.fs.as_ref()),
        &compute.offload,
        envelope,
        "user",
    )
    .await?;
    events.emit(DomainEvent::BlockUpdated {
        block: domain::block::Block {
            id: block.id,
            kind: domain::block::BlockKind::Computed {
                envelope: Box::new(computed),
                body: Box::new(body),
            },
        },
    });
    Ok(())
}

/// What the session shows or runs for the result of a Lua callback.
#[must_use]
pub fn plugin_reply(result: Result<PluginOutput, PluginError>) -> Reply {
    match result {
        Ok(PluginOutput::Actions(actions)) => Reply::Actions(actions),
        Ok(PluginOutput::Block(kind)) => Reply::block(Block::new(kind)),
        Ok(PluginOutput::Value(_)) => Reply::Done,
        Err(err) => Reply::block(Block::new(BlockKind::Error {
            message: format!("Lua: {err}"),
            hint: None,
        })),
    }
}

/// Shows the prompt and the images pasted with it in the timeline.
fn emit_user_blocks(app: &App, prompt: &str, images: &[ImageAttachment]) {
    for block in user_blocks(prompt, images) {
        app.emit(DomainEvent::BlockAdded { block });
    }
}

/// Runs one turn of `app` and saves the session, as the headless mode does.
/// The output of the `/!` commands run since the last turn goes before the
/// prompt the model gets (the timeline shows the prompt alone). Pasted
/// images go to the model in the same message, after the prompt.
pub async fn run_turn(app: &mut App, request: TurnRequest) -> TurnEnd {
    run_turn_in(app, request, None).await
}

/// [`run_turn`] with `context` (the gate of a side chat) before the prompt
/// the model gets, after the output of the `/!` commands.
pub async fn run_turn_in(app: &mut App, request: TurnRequest, context: Option<String>) -> TurnEnd {
    emit_user_blocks(app, &request.prompt, &request.images);
    let images = request.images;
    let mut prompt = request.prompt;
    let mut before = std::mem::take(&mut app.shell_context);
    before.extend(context);
    if !before.is_empty() {
        let mut context = before.join("\n\n");
        context.push_str("\n\n");
        context.push_str(&prompt);
        prompt = context;
    }
    let mut opts = app.turn_options.clone().with_cancel(request.cancel);
    if let Some(role) = request.role {
        opts.role = role;
    }
    if let Some(model) = request.model_override {
        opts = opts.with_override(model);
    }
    let result = app
        .service
        .run_turn(&mut app.session, UserInput::new(prompt, images), opts)
        .await;
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
    match result {
        Ok(outcome) => TurnEnd::Finished(outcome.stop_reason),
        Err(err) => TurnEnd::Failed(err.to_string()),
    }
}

/// Continues the turn a restart cut while `ask_user` waited (see
/// [`AgentService::resume_pending_tools`]) and saves the session.
pub async fn resume_pending(app: &mut App, cancel: CancelToken) -> TurnEnd {
    let opts = app.turn_options.clone().with_cancel(cancel);
    let result = app
        .service
        .resume_pending_tools(&mut app.session, opts)
        .await;
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
    match result {
        Ok(Some(outcome)) => TurnEnd::Finished(outcome.stop_reason),
        Ok(None) => TurnEnd::Finished(domain::conversation::StopReason::EndTurn),
        Err(err) => TurnEnd::Failed(err.to_string()),
    }
}

impl UiController {
    /// Returns the same controller storing `/login` keys in `keys`.
    #[must_use]
    pub fn with_api_keys(mut self, keys: Arc<dyn ApiKeyPort>) -> Self {
        self.api_keys = keys;
        self
    }

    /// Runs the command `name` on the runtime; `Ctrl+C` aborts it
    /// ([`SessionController::interrupt_commands`]).
    fn spawn_command(
        &self,
        name: &'static str,
        work: impl Future<Output = Reply> + Send + 'static,
    ) -> BoxFuture<'static, Reply> {
        let task = self.handle.spawn(work);
        let abort = task.abort_handle();
        let id = abort.id();
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(abort);
        let running = Arc::clone(&self.running);
        task.map(move |joined| {
            running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|handle| handle.id() != id);
            match joined {
                Ok(reply) => reply,
                Err(err) if err.is_cancelled() => {
                    Reply::Notice(NoticeLevel::Info, interrupted(name))
                }
                Err(err) => Reply::Notice(NoticeLevel::Warning, format!("/{name}: {err}")),
            }
        })
        .boxed()
    }
}

/// What the status bar says when `Ctrl+C` stopped the command `name`.
fn interrupted(name: &str) -> String {
    match name {
        "research" => crate::research::INTERRUPTED.to_owned(),
        "implement" => "implementation interrupted; it is saved at its last step and \
                        `/implement resume` continues it"
            .to_owned(),
        "!" => "command interrupted".to_owned(),
        other => format!("/{other} interrupted"),
    }
}

/// `/!command`: runs it in the workspace, shows its output and keeps it for
/// the model's next prompt.
async fn shell_command(app: Arc<tokio::sync::Mutex<App>>, line: String) -> Reply {
    let shell = Arc::clone(&app.lock().await.shell);
    let run = application::usecases::shell_command::run_user_command(shell.as_ref(), &line).await;
    let mut app = app.lock().await;
    app.shell_context.push(run.context);
    app.emit(DomainEvent::BlockAdded {
        block: Block::new(run.block),
    });
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
    Reply::Done
}

/// Images added to the chat only: their blocks (after the caption) are
/// saved with the session and the model is not called.
async fn post_images(
    app: Arc<tokio::sync::Mutex<App>>,
    caption: String,
    images: Vec<ImageAttachment>,
) -> Reply {
    let app = app.lock().await;
    emit_user_blocks(&app, &caption, &images);
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
    Reply::Notice(NoticeLevel::Info, chat_only_notice(images.len()))
}

/// `/open @path`: shows the file in a modal. The modal goes through the
/// app's sink, so a picture's path is checked again by the image guard; it
/// is not part of the saved timeline.
async fn open_command(app: Arc<tokio::sync::Mutex<App>>, args: String) -> Reply {
    let (fs, step) = {
        let app = app.lock().await;
        let step = application::usecases::open_file::StepReader {
            formats: Arc::clone(&app.formats),
            offload: Arc::clone(&app.compute.offload),
        };
        (Arc::clone(&app.fs), step)
    };
    match application::usecases::open_file::open_file_with(fs.as_ref(), Some(&step), &args).await {
        Ok(spec) => {
            app.lock().await.emit(DomainEvent::ModalOpened { spec });
            Reply::Done
        }
        Err(err) => Reply::Notice(NoticeLevel::Warning, err.to_string()),
    }
}

/// `/stage`, `/back`, `/skip`, `/retry` or `/abort`: for the active
/// implementation flow, else for the research flow of the workspace.
async fn flow_line(
    app: Arc<tokio::sync::Mutex<App>>,
    flow: ActiveFlow,
    research: SharedResearch,
    events: Arc<dyn EventSink>,
    aside: SharedAside,
    line: String,
) -> Reply {
    aside.lock().await.close();
    let mut active = flow.lock().await;
    if active.is_some() {
        answer_gate(&mut active, &line, events.as_ref()).await;
        return Reply::Done;
    }
    drop(active);
    let mut guard = research.lock().await;
    let Some(session) = guard.as_mut() else {
        return Reply::Notice(
            NoticeLevel::Warning,
            "the research flow is unavailable: the crest database did not open".into(),
        );
    };
    let result = crate::research::line(session, &line).await;
    drop(guard);
    save(&app).await;
    match result {
        Ok(message) => Reply::Notice(NoticeLevel::Info, message),
        Err(message) => Reply::Notice(NoticeLevel::Warning, message),
    }
}

/// The side chat of the waiting flow ([`Aside`]), shared by the turns and
/// the `/aside` and `/flow` commands.
type SharedAside = Arc<tokio::sync::Mutex<Aside>>;

/// Where a line of the chat goes: the waiting flow, its side chat or a
/// normal turn (see [`crate::aside`]).
#[derive(Clone)]
struct Router {
    app: Arc<tokio::sync::Mutex<App>>,
    flow: ActiveFlow,
    research: SharedResearch,
    events: Arc<dyn EventSink>,
    aside: SharedAside,
    decider: Decider,
}

impl Router {
    fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.events.emit(DomainEvent::Notice {
            level,
            message: message.into(),
        });
    }

    /// The gate waiting now: the implementation flow's, else the research
    /// flow's.
    async fn gate(&self) -> Option<GateInfo> {
        if let Some(session) = self.flow.lock().await.as_ref() {
            return session.gate_info().await;
        }
        let research = self.research.lock().await;
        research
            .as_ref()
            .filter(|s| s.is_active())?
            .gate_info()
            .await
    }

    /// A plain line sent from the input.
    async fn line(self, request: TurnRequest) -> TurnEnd {
        let gate = self.gate().await;
        let (route, notice) = {
            let mut aside = self.aside.lock().await;
            match gate {
                // A flow takes a line of text: images open the side chat.
                Some(gate) if !request.images.is_empty() && !aside.is_open() => {
                    let notice = opened_notice(&gate);
                    let context = preamble(&gate);
                    aside.open(gate);
                    (Route::Aside(context), Some(notice))
                }
                gate => {
                    aside
                        .route(&request.prompt, gate, &self.decider, true)
                        .await
                }
            }
        };
        if let Some(message) = notice {
            self.notice(NoticeLevel::Info, message);
        }
        match route {
            Route::Chat => run_turn(&mut *self.app.lock().await, request).await,
            Route::Aside(context) => {
                run_turn_in(&mut *self.app.lock().await, request, Some(context)).await
            }
            Route::Propose(context) => self.propose(request, context).await,
            Route::Answer(_) if !request.images.is_empty() => {
                let message = "images cannot answer the active flow; nothing was sent \
                               (add them with Chat only, or finish the flow first)";
                self.notice(NoticeLevel::Warning, message);
                TurnEnd::Failed(message.to_owned())
            }
            Route::Answer(text) => self.answer(&text, &request.cancel).await,
        }
    }

    /// Answers the waiting gate with `text`: the implementation flow's,
    /// else the research flow's.
    async fn answer(&self, text: &str, cancel: &CancelToken) -> TurnEnd {
        let mut active = self.flow.lock().await;
        if active.is_some() {
            self.events.emit(DomainEvent::BlockAdded {
                block: prompt_block(text),
            });
            answer_gate(&mut active, text, self.events.as_ref()).await;
            return TurnEnd::Finished(StopReason::EndTurn);
        }
        drop(active);
        let mut research = self.research.lock().await;
        let Some(session) = research.as_mut().filter(|s| s.is_active()) else {
            drop(research);
            self.notice(NoticeLevel::Warning, "no flow waits for an answer");
            return TurnEnd::Finished(StopReason::EndTurn);
        };
        self.events.emit(DomainEvent::BlockAdded {
            block: prompt_block(text),
        });
        let line = crate::research::line(session, text);
        let (level, message) = match cancel.run(line).await {
            Ok(Ok(message)) => (NoticeLevel::Info, message),
            Ok(Err(message)) => (NoticeLevel::Warning, message),
            Err(_) => (NoticeLevel::Info, crate::research::INTERRUPTED.to_owned()),
        };
        self.notice(level, message);
        drop(research);
        save(&self.app).await;
        TurnEnd::Finished(StopReason::EndTurn)
    }

    /// A chat turn that proposes the answer to the gate from the side
    /// conversation; the next line sends it.
    async fn propose(&self, request: TurnRequest, context: String) -> TurnEnd {
        let (end, answer) = {
            let mut app = self.app.lock().await;
            let before = app.session.turns.len();
            let end = run_turn_in(&mut app, request, Some(context)).await;
            let answer = app
                .session
                .turns
                .get(before..)
                .and_then(last_answer)
                .filter(|_| matches!(end, TurnEnd::Finished(_)));
            (end, answer)
        };
        let message = self.aside.lock().await.proposed(answer);
        self.notice(NoticeLevel::Info, message);
        end
    }

    /// `/aside [question]`: opens the side chat of the waiting gate, and
    /// asks `question` in it.
    async fn aside_command(self, args: String) -> Reply {
        let Some(gate) = self.gate().await else {
            return Reply::Notice(
                NoticeLevel::Warning,
                "no flow waits for an answer; `/aside` works while a flow waits at a gate".into(),
            );
        };
        let notice = opened_notice(&gate);
        let context = preamble(&gate);
        self.aside.lock().await.open(gate);
        let question = args.trim();
        if question.is_empty() {
            return Reply::Notice(NoticeLevel::Info, notice);
        }
        self.notice(NoticeLevel::Info, notice);
        let request = command_turn(question);
        run_turn_in(&mut *self.app.lock().await, request, Some(context)).await;
        Reply::Done
    }

    /// `/flow [answer]`: back to the waiting flow, answering it with
    /// `answer`, or proposing the answer from the side chat.
    async fn flow_command(self, args: String) -> Reply {
        let answer = args.trim();
        if !answer.is_empty() {
            self.aside.lock().await.close();
            if self.gate().await.is_none() {
                return Reply::Notice(NoticeLevel::Warning, "no flow waits for an answer".into());
            }
            self.answer(answer, &CancelToken::new()).await;
            return Reply::Done;
        }
        let proposal = self.aside.lock().await.propose();
        if let Some(context) = proposal {
            self.propose(command_turn("back to the flow"), context)
                .await;
            return Reply::Done;
        }
        match self.gate().await {
            Some(gate) => {
                Reply::Notice(NoticeLevel::Info, format!("{gate}: {}", gate.prompt.trim()))
            }
            None => Reply::Notice(NoticeLevel::Warning, "no flow waits for an answer".into()),
        }
    }
}

/// The turn a command runs in the chat, with `prompt` as the user's line.
#[must_use]
pub fn command_turn(prompt: &str) -> TurnRequest {
    TurnRequest {
        prompt: prompt.to_owned(),
        images: Vec::new(),
        model_override: None,
        role: None,
        cancel: CancelToken::new(),
    }
}

impl SessionController for UiController {
    fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        let (tx, rx) = oneshot::channel();
        let router = self.router();
        self.handle.spawn(async move {
            tx.send(router.line(request).await).ok();
        });
        rx.map(|end| {
            end.unwrap_or_else(|_| TurnEnd::Failed("the turn ended without an answer".into()))
        })
        .boxed()
    }

    fn resume_pending(&self, cancel: CancelToken) -> BoxFuture<'static, TurnEnd> {
        let (tx, rx) = oneshot::channel();
        let app = Arc::clone(&self.app);
        self.handle.spawn(async move {
            let end = resume_pending(&mut *app.lock().await, cancel).await;
            tx.send(end).ok();
        });
        rx.map(|end| {
            end.unwrap_or_else(|_| TurnEnd::Failed("the turn ended without an answer".into()))
        })
        .boxed()
    }

    fn recompute(&self, block: domain::block::Block) -> BoxFuture<'static, Result<(), String>> {
        let task = self.handle.spawn(recompute_block(
            self.compute.clone(),
            Arc::clone(&self.events),
            block,
        ));
        task.map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn post_images(
        &self,
        caption: String,
        images: Vec<ImageAttachment>,
    ) -> BoxFuture<'static, Reply> {
        self.spawn_command(
            "images",
            post_images(Arc::clone(&self.app), caption, images),
        )
    }

    fn permission_mode(&self) -> PermissionMode {
        lock_permissions(&self.permissions).mode
    }

    fn set_permission_mode(&self, mode: PermissionMode) {
        lock_permissions(&self.permissions).mode = mode;
    }

    fn models(&self) -> Vec<ModelChoice> {
        self.models.clone()
    }

    fn permission_rules(&self) -> BoxFuture<'static, RuleListing> {
        let (permissions, project) = (Arc::clone(&self.permissions), self.project_rules.clone());
        let task = self
            .handle
            .spawn(async move { load_rules(&permissions, project.as_ref()).await });
        task.map(Result::unwrap_or_default).boxed()
    }

    fn remove_permission_rule(
        &self,
        rule: PermissionRule,
    ) -> BoxFuture<'static, Result<(), String>> {
        let (permissions, project) = (Arc::clone(&self.permissions), self.project_rules.clone());
        let task = self.handle.spawn(async move {
            remove_rule(&permissions, project.as_ref(), &rule)
                .await
                .map_err(|err| err.to_string())
        });
        task.map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn promote_permission_rule(
        &self,
        rule: PermissionRule,
    ) -> BoxFuture<'static, Result<(), String>> {
        let (permissions, project) = (Arc::clone(&self.permissions), self.project_rules.clone());
        let task = self.handle.spawn(async move {
            promote_rule(&permissions, project.as_ref(), &rule)
                .await
                .map_err(|err| err.to_string())
        });
        task.map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn record_prompt(&self, line: String) {
        self.prompt_history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
    }

    fn new_session(&self) -> BoxFuture<'static, ()> {
        let app = Arc::clone(&self.app);
        let task = self
            .handle
            .spawn(async move { app.lock().await.new_session() });
        task.map(|_| ()).boxed()
    }

    fn set_chat_title(&self, title: String) -> BoxFuture<'static, Result<(), String>> {
        let app = Arc::clone(&self.app);
        // A running turn holds the app until it ends (and saves the old
        // title then): this waits for it, and the save after keeps the new
        // one. The UI shows the title meanwhile and does not wait.
        self.handle
            .spawn(async move {
                let mut guard = app.lock().await;
                guard.session.title = Some(title);
                guard.save().await.map_err(|err| err.to_string())
            })
            .map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn run_callback(&self, id: u64, args: serde_json::Value) -> BoxFuture<'static, Reply> {
        let Some(host) = self.plugins.clone() else {
            return futures::future::ready(Reply::Notice(
                NoticeLevel::Warning,
                "Lua is unavailable".into(),
            ))
            .boxed();
        };
        // The Lua host is called from tokio, never from GPUI's executor.
        let call = self
            .handle
            .spawn(async move { host.invoke_callback(CallbackId(id), args).await });
        call.map(|joined| {
            plugin_reply(
                joined.unwrap_or_else(|err| Err(PluginError::Unavailable(err.to_string()))),
            )
        })
        .boxed()
    }

    fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
        let args = args.to_owned();
        match name {
            "implement" => {
                let start =
                    start_implementation(Arc::clone(&self.app), Arc::clone(&self.flow), args);
                Some(self.spawn_command("implement", start))
            }
            "research" => {
                let start =
                    research_command(Arc::clone(&self.app), Arc::clone(&self.research), args);
                Some(self.spawn_command("research", start))
            }
            "aside" | "aparte" => {
                Some(self.spawn_command("aside", self.router().aside_command(args)))
            }
            "flow" | "volver" => Some(self.spawn_command("flow", self.router().flow_command(args))),
            "!" if args.trim().is_empty() => Some(
                futures::future::ready(Reply::Notice(
                    NoticeLevel::Warning,
                    "usage: /!<command> [args], e.g. /!git status".into(),
                ))
                .boxed(),
            ),
            "!" => Some(self.spawn_command("!", shell_command(Arc::clone(&self.app), args))),
            "stage" | "back" | "skip" | "abort" | "retry" | "reopen" => {
                let line = format!("/{name} {args}").trim_end().to_owned();
                let task = flow_line(
                    Arc::clone(&self.app),
                    Arc::clone(&self.flow),
                    Arc::clone(&self.research),
                    Arc::clone(&self.events),
                    Arc::clone(&self.aside),
                    line,
                );
                Some(self.spawn_command("research", task))
            }
            "simulate" => {
                let spec = args.trim().to_owned();
                if spec.is_empty() {
                    return Some(
                        futures::future::ready(Reply::Notice(
                            NoticeLevel::Warning,
                            "usage: /simulate <spec> (a folder of .crest/specs/ or its number)"
                                .into(),
                        ))
                        .boxed(),
                    );
                }
                Some(self.spawn_command("simulate", simulate_command(self.simulator.clone(), spec)))
            }
            "mcp" => Some(self.spawn_command("mcp", Arc::clone(&self.mcp).run(args))),
            "plugin" => {
                let task = plugin_command(Arc::clone(&self.plugin_trust), args);
                Some(self.spawn_command("plugin", task))
            }
            "open" => Some(self.spawn_command("open", open_command(Arc::clone(&self.app), args))),
            _ => {
                let id = self.commands.get(name)?;
                Some(self.run_callback(id.0, serde_json::Value::String(args)))
            }
        }
    }

    fn interrupt_commands(&self) -> bool {
        let running =
            std::mem::take(&mut *self.running.lock().unwrap_or_else(PoisonError::into_inner));
        for handle in &running {
            handle.abort();
        }
        !running.is_empty()
    }

    fn login(&self, account: Account, key: String) -> BoxFuture<'static, Result<String, String>> {
        login_on(
            &self.handle,
            Arc::clone(&self.api_keys),
            account,
            key,
            self.preferences_path.clone(),
        )
    }

    fn catalog_models(&self) -> BoxFuture<'static, Result<Vec<ModelChoice>, String>> {
        let Some(catalog) = self.catalog.clone() else {
            return futures::future::ready(Ok(Vec::new())).boxed();
        };
        let task = self
            .handle
            .spawn(async move { catalog.list_models().await });
        task.map(|joined| match joined {
            Ok(Ok(models)) => Ok(catalog_choices(models)),
            Ok(Err(err)) => Err(err.to_string()),
            Err(err) => Err(err.to_string()),
        })
        .boxed()
    }

    fn select_model(&self, model: ModelId) -> BoxFuture<'static, Result<String, String>> {
        let Some(path) = self.preferences_path.clone() else {
            return futures::future::ready(Err(
                "this session cannot save a model; use /model <id> for the next turn".to_owned(),
            ))
            .boxed();
        };
        let provider = self.default_provider;
        let task = self.handle.spawn_blocking(move || {
            crate::preferences::update(Some(&path), |p| {
                p.models.insert(provider, model.clone());
            })
            .map(|()| format!("model `{model}` selected for {provider}"))
        });
        task.map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn themes(&self) -> BoxFuture<'static, Result<Vec<ui::session::ThemeChoice>, String>> {
        let (catalog, options) = (self.themes.clone(), self.theme_options.clone());
        let task = self
            .handle
            .spawn_blocking(move || theme_choices(&catalog, &options));
        task.map(|joined| joined.map_err(|err| err.to_string()))
            .boxed()
    }

    fn save_theme(
        &self,
        name: String,
    ) -> BoxFuture<'static, Result<ui::session::SavedTheme, String>> {
        let (catalog, options) = (self.themes.clone(), self.theme_options.clone());
        let path = self.preferences_path.clone();
        let task = self.handle.spawn_blocking(move || {
            let (overrides, _) = application::theme::overrides_from_options(&options);
            let resolved = catalog
                .resolve(&name, overrides.as_ref())
                .map_err(|err| err.to_string())?;
            let name = resolved.theme.name.clone();
            let mut notice = match &path {
                Some(path) => {
                    crate::preferences::update(Some(path), |p| p.theme = Some(name.clone()))?;
                    format!("theme `{name}` saved")
                }
                None => format!("theme `{name}` for this window (preferences cannot be saved)"),
            };
            if let Some(fixed) = options.theme.as_deref() {
                notice.push_str(&format!(
                    "; init.lua sets app.opt.theme = \"{fixed}\", which wins when crest starts"
                ));
            }
            for warning in resolved.warnings.iter().take(3) {
                notice.push('\n');
                notice.push_str(warning);
            }
            Ok(ui::session::SavedTheme {
                theme: resolved.theme,
                notice,
            })
        });
        task.map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn logout(&self, account: Option<Account>) -> BoxFuture<'static, Result<String, String>> {
        logout_on(&self.handle, Arc::clone(&self.api_keys), account)
    }

    fn git_status(&self) -> BoxFuture<'static, Option<GitStatus>> {
        let shell = Arc::clone(&self.shell);
        let task = self.handle.spawn(async move {
            application::usecases::git_status::git_status(shell.as_ref()).await
        });
        task.map(|joined| joined.ok().flatten()).boxed()
    }

    fn help(&self) -> BoxFuture<'static, HelpSnapshot> {
        let (app, trust) = (Arc::clone(&self.app), Arc::clone(&self.plugin_trust));
        let task = self
            .handle
            .spawn(async move { help_snapshot(&app, &trust).await });
        task.map(Result::unwrap_or_default).boxed()
    }
}

/// The help's snapshot of `app`, with the MCP servers' states now.
pub async fn help_snapshot(app: &tokio::sync::Mutex<App>, trust: &PluginTrust) -> HelpSnapshot {
    let app = app.lock().await;
    let statuses = match &app.mcp {
        Some(mcp) => mcp.manager.servers().await,
        None => Vec::new(),
    };
    crate::help::snapshot(&app, trust, &statuses)
}

/// The controller of a window whose agent could not start.
pub struct UnavailableController {
    reason: String,
    mode: Mutex<PermissionMode>,
    models: Vec<ModelChoice>,
    /// `/login` and `/logout`: the runtime and where the key goes.
    login: Option<(Handle, Arc<dyn ApiKeyPort>)>,
    /// Where a `/login` saves its provider as the default, if anywhere.
    preferences_path: Option<PathBuf>,
}

impl UnavailableController {
    /// Answers every turn with `reason`.
    #[must_use]
    pub fn new(reason: impl Into<String>, mode: PermissionMode) -> Self {
        Self {
            reason: reason.into(),
            mode: Mutex::new(mode),
            models: model_choices(&ModelPolicy::default()),
            login: None,
            preferences_path: None,
        }
    }

    /// Returns the same controller saving the provider of a `/login` in the
    /// preferences at `path`.
    #[must_use]
    pub fn with_preferences(mut self, path: Option<PathBuf>) -> Self {
        self.preferences_path = path;
        self
    }

    /// Returns the same controller storing `/login` keys in `keys`, on
    /// `handle`'s runtime.
    #[must_use]
    pub fn with_login(mut self, handle: Handle, keys: Arc<dyn ApiKeyPort>) -> Self {
        self.login = Some((handle, keys));
        self
    }
}

/// `/login` on the runtime of `handle` (the UI polls the future on its own
/// executor; the keyring and the API check need tokio).
fn login_on(
    handle: &Handle,
    keys: Arc<dyn ApiKeyPort>,
    account: Account,
    key: String,
    preferences: Option<PathBuf>,
) -> BoxFuture<'static, Result<String, String>> {
    let task = handle.spawn(async move {
        let stored = application::usecases::login::login(keys.as_ref(), account, &key).await?;
        // The account logged in last serves the models without a prefix.
        let Some(path) = preferences else {
            return Ok(stored);
        };
        let saved = tokio::task::spawn_blocking(move || {
            crate::preferences::update(Some(&path), |p| {
                p.provider = Some(Preferences::provider_of(account));
                // A new key: the next start asks what it may use.
                if account == Account::OpenCode {
                    p.opencode_access = None;
                }
            })
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|saved| saved);
        Ok(match saved {
            Ok(()) => stored,
            Err(err) => format!("{stored} (it could not be made the default: {err})"),
        })
    });
    // The dialog drops the future when the user cancels or the check
    // times out: the check stops then instead of storing the key later.
    let abort = AbortOnDrop(task.abort_handle());
    task.map(move |joined| {
        drop(abort);
        joined.unwrap_or_else(|err| Err(err.to_string()))
    })
    .boxed()
}

/// Aborts a runtime task when dropped.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `/logout` on the runtime of `handle`.
fn logout_on(
    handle: &Handle,
    keys: Arc<dyn ApiKeyPort>,
    account: Option<Account>,
) -> BoxFuture<'static, Result<String, String>> {
    use application::usecases::login::{logout, logout_all};
    let task = handle.spawn(async move {
        match account {
            Some(account) => logout(keys.as_ref(), account).await,
            None => logout_all(keys.as_ref()).await,
        }
    });
    task.map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
        .boxed()
}

impl SessionController for UnavailableController {
    fn run_turn(&self, _request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        futures::future::ready(TurnEnd::Unavailable(self.reason.clone())).boxed()
    }

    fn permission_mode(&self) -> PermissionMode {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_permission_mode(&self, mode: PermissionMode) {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner) = mode;
    }

    fn models(&self) -> Vec<ModelChoice> {
        self.models.clone()
    }

    fn new_session(&self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn login(&self, account: Account, key: String) -> BoxFuture<'static, Result<String, String>> {
        match &self.login {
            Some((handle, keys)) => login_on(
                handle,
                Arc::clone(keys),
                account,
                key,
                self.preferences_path.clone(),
            ),
            None => futures::future::ready(Err("API keys cannot be stored here".into())).boxed(),
        }
    }

    fn logout(&self, account: Option<Account>) -> BoxFuture<'static, Result<String, String>> {
        match &self.login {
            Some((handle, keys)) => logout_on(handle, Arc::clone(keys), account),
            None => futures::future::ready(Err("API keys cannot be stored here".into())).boxed(),
        }
    }
}

/// The model the first turn will use.
fn initial_model(app: &App) -> ModelId {
    let mut request = ResolveRequest::new(app.turn_options.role);
    request
        .turn_override
        .clone_from(&app.turn_options.turn_override);
    let resolved = app.model_policy().resolve(&request);
    if resolved.source == SpecSource::Unselected {
        // The placeholder id is never sent; say what is missing instead.
        return ModelId::new("no model (/model)");
    }
    resolved.spec.id
}

/// The session of the window over `config`, with the agent built on
/// `handle`'s runtime and `overrides` (tests replace the model).
///
/// # Errors
///
/// Only when the workspace cannot be opened. Any other startup error (no
/// API key) still gives a session, whose timeline starts with it.
pub fn session_parts(
    config: &Config,
    overrides: Overrides,
    handle: &Handle,
) -> anyhow::Result<SessionParts> {
    let (sink, events) = ChannelSink::channel();
    let (approval, approvals) = UiApproval::channel();
    let (form, forms) = UiForm::channel();
    let frontend = Frontend {
        events: Arc::new(sink.clone()),
        approval: Arc::new(approval),
        form: Arc::new(form),
    };
    let api_keys: Arc<dyn ApiKeyPort> = overrides
        .api_keys
        .clone()
        .unwrap_or_else(|| Arc::new(crate::credentials::KeyringCredentials::default()));
    // As in build_app: a scripted model (tests) saves no preferences.
    let preferences_path = overrides.preferences_path.clone().or_else(|| {
        overrides
            .llm
            .is_none()
            .then(crate::preferences::default_path)
            .flatten()
    });
    let built = handle.block_on(build_app(config, frontend, overrides));
    let (mut parts, workspace) = match built {
        Ok(app) => {
            let files = TokioPort::new(Arc::clone(&app.file_search), handle.clone());
            let content = TokioPort::new(Arc::clone(&app.content_search), handle.clone());
            let fs: Arc<dyn FileSystemPort> =
                Arc::new(TokioPort::new(Arc::clone(&app.fs), handle.clone()));
            let workspace = app.workspace.as_path().to_path_buf();
            let model = initial_model(&app);
            let (keymap, key_warnings) = crate::keys::keymap(&app.keymaps, &app.options);
            for warning in key_warnings {
                app.warn(warning);
            }
            let timeoutlen = crate::keys::timeoutlen(&app.options);
            let picked = preferences_path
                .as_deref()
                .and_then(|path| Preferences::load(path).ok())
                .and_then(|preferences| preferences.theme);
            let (theme, theme_warnings) =
                crate::startup::theme(&app.options, &app.themes, picked.as_deref());
            for warning in theme_warnings {
                app.warn(warning);
            }
            let history = app.timeline_blocks();
            let prompt_history = app.prompt_history().clone();
            let needs_model = app.model_policy().needs_selection();
            let session_id = app.session.id.to_string();
            let resume_pending = !app.session.pending_tool_uses().is_empty();
            let controller = Arc::new(
                UiController::new(app, handle.clone()).with_api_keys(Arc::clone(&api_keys)),
            );
            let mut parts = SessionParts::new(
                controller,
                events,
                approvals,
                Arc::new(files),
                Arc::new(content),
            );
            // The previews read with the app's guards (sensitive paths).
            parts.fs = Some(fs);
            parts.model = Some(model);
            parts.needs_model = needs_model;
            parts.keymap = keymap;
            parts.timeoutlen = timeoutlen;
            parts.history = history;
            parts.prompt_history = prompt_history;
            parts.theme = theme;
            parts.session_id = Some(session_id);
            parts.resume_pending = resume_pending;
            (parts, workspace)
        }
        Err(err) => {
            let needs_login = err.chain().any(|e| e.is::<MissingApiKey>());
            let reason = format!("{err:#}");
            let root = adapter_fs::guard::canonicalize_blocking(&config.workspace)
                .map_err(anyhow::Error::from)
                .and_then(|dir| WorkspaceRoot::new(dir).map_err(anyhow::Error::from))
                // The workspace itself is the problem: report the original
                // error, there is nothing to show.
                .map_err(|_| err)?;
            // Also on the terminal that started the window.
            eprintln!("warning: the agent is unavailable: {reason}");
            sink.emit(DomainEvent::BlockAdded {
                block: Block::new(BlockKind::Error {
                    message: reason.clone(),
                    hint: Some(if needs_login {
                        "run /login to paste your API key; it is stored in the system keyring"
                            .into()
                    } else {
                        "the agent cannot answer; fix this and reopen crest".into()
                    }),
                }),
            });
            let workspace = root.as_path().to_path_buf();
            let search = Arc::new(RipgrepSearch::new(root));
            let files: Arc<dyn FileSearchPort> = search.clone();
            let content: Arc<dyn ContentSearchPort> = search;
            let controller = Arc::new(
                UnavailableController::new(reason, config.mode)
                    .with_login(handle.clone(), api_keys)
                    .with_preferences(preferences_path),
            );
            let mut parts = SessionParts::new(
                controller,
                events,
                approvals,
                Arc::new(TokioPort::new(files, handle.clone())),
                Arc::new(TokioPort::new(content, handle.clone())),
            );
            parts.needs_login = needs_login;
            (parts, workspace)
        }
    };
    parts.forms = Some(forms);
    parts.workspace_label = workspace_label(&workspace);
    parts.workspace_dir = Some(workspace);
    Ok(parts)
}

/// The directory name shown in the title and the status bar.
#[must_use]
pub fn workspace_label(workspace: &std::path::Path) -> String {
    workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// `crest [DIR]`: opens the window over the workspace, or in the project
/// picker without one (or with `--pick`). Returns the exit code: 0, 1 when
/// the window cannot open, 2 for configuration errors.
#[must_use]
pub fn run(args: &Args, handle: &Handle) -> u8 {
    let launcher = Arc::new(Launcher::new(
        args.clone(),
        Overrides::default(),
        handle.clone(),
    ));
    // The window opens before the project: only the global config sets it.
    let (global, global_warnings) = crate::startup::global_config(handle);
    let (window, window_warning) = crate::startup::window(&global.options);
    for warning in global_warnings.iter().chain(&window_warning) {
        eprintln!("warning: {warning}");
    }
    if args.wants_picker() {
        // No project yet: the theme of the global config (or `/theme`).
        let global_init = platform::dirs::config_dir().map(|dir| dir.join("init.lua"));
        let (catalog, _) = crate::startup::theme_catalog(global_init.as_deref(), &global.themes);
        let picked = crate::preferences::default_path()
            .and_then(|path| Preferences::load(&path).ok())
            .and_then(|preferences| preferences.theme);
        let (theme, _) = crate::startup::theme(&global.options, &catalog, picked.as_deref());
        let opts = ui::app::UiOptions {
            window,
            theme,
            ..ui::app::UiOptions::default()
        };
        return window_exit(ui::app::run_with(opts, move |window, cx| {
            Workbench::new_picker(launcher, window, cx)
        }));
    }
    let config = match Config::from_args(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err:#}");
            return 2;
        }
    };
    let parts = match session_parts(&config, Overrides::default(), handle) {
        Ok(parts) => parts,
        Err(err) => {
            eprintln!("error: {err:#}");
            return 2;
        }
    };
    let opts = ui::app::UiOptions {
        workspace_label: parts.workspace_label.clone(),
        theme: std::sync::Arc::clone(&parts.theme),
        window,
        ..ui::app::UiOptions::default()
    };
    let dir = parts.workspace_dir.clone().unwrap_or(config.workspace);
    window_exit(ui::app::run_with(opts, move |window, cx| {
        Workbench::new_with_session(launcher, parts, dir, window, cx)
    }))
}

fn window_exit(result: Result<(), ui::app::UiError>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    }
}

/// Opens the projects chosen in the window's picker with the options of
/// the command line, as `crest DIR` would.
pub struct Launcher {
    args: Args,
    overrides: Overrides,
    handle: Handle,
    browser: Arc<dyn ProjectBrowserPort>,
}

impl Launcher {
    /// A launcher for `args`, over the runtime of `handle`.
    #[must_use]
    pub fn new(args: Args, overrides: Overrides, handle: Handle) -> Self {
        let browser: Arc<dyn ProjectBrowserPort> = Arc::new(TokioPort::new(
            Arc::new(LocalProjectBrowser::new()) as Arc<dyn ProjectBrowserPort>,
            handle.clone(),
        ));
        Self {
            args,
            overrides,
            handle,
            browser,
        }
    }

    /// The session database, when its path is known.
    fn session_db(&self) -> Option<PathBuf> {
        self.overrides
            .db_path
            .clone()
            .or_else(adapter_store::path::default_db_path)
    }

    /// The same launcher listing directories with `browser` (tests).
    #[must_use]
    pub fn with_browser(mut self, browser: Arc<dyn ProjectBrowserPort>) -> Self {
        self.browser = browser;
        self
    }
}

impl ProjectOpener for Launcher {
    fn open(&self, dir: PathBuf) -> BoxFuture<'static, Result<SessionParts, String>> {
        self.open_as(dir, SessionOpen::Latest)
    }

    fn open_as(
        &self,
        dir: PathBuf,
        which: SessionOpen,
    ) -> BoxFuture<'static, Result<SessionParts, String>> {
        let mut config = Config::for_workspace(&self.args, dir);
        match which {
            SessionOpen::Latest => {}
            SessionOpen::New => config.resume = false,
            SessionOpen::Id(id) => {
                config.resume = false;
                config.session_id = Some(id);
            }
        }
        let (overrides, handle) = (self.overrides.clone(), self.handle.clone());
        // `session_parts` blocks on the runtime: off its worker threads.
        self.handle
            .spawn_blocking(move || {
                session_parts(&config, overrides, &handle).map_err(|err| format!("{err:#}"))
            })
            .map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn catalog(&self) -> BoxFuture<'static, Vec<ProjectChats>> {
        let path = self
            .overrides
            .db_path
            .clone()
            .or_else(adapter_store::path::default_db_path);
        self.handle
            .spawn(async move {
                let Some(path) = path.filter(|path| path.exists()) else {
                    return Vec::new();
                };
                let Ok(store) = adapter_store::SqliteSessionStore::open(&path) else {
                    return Vec::new();
                };
                let Ok(workspaces) = store.recent_workspaces(RECENT_PROJECTS).await else {
                    return Vec::new();
                };
                let mut projects = Vec::with_capacity(workspaces.len());
                for workspace in workspaces {
                    let Ok(sessions) = store.list(&workspace.root).await else {
                        continue;
                    };
                    projects.push(ProjectChats {
                        name: project_name(&workspace.root),
                        root: workspace.root,
                        chats: sessions
                            .into_iter()
                            .map(|session| ChatEntry {
                                id: session.id.to_string(),
                                title: chat_label(session.title.as_deref()),
                            })
                            .collect(),
                    });
                }
                projects
            })
            .map(Result::unwrap_or_default)
            .boxed()
    }

    fn rename_chat(&self, id: String, title: String) -> BoxFuture<'static, Result<(), String>> {
        let path = self.session_db();
        self.handle
            .spawn(async move {
                let store = open_session_store(path)?;
                let Some(id) = SessionId::parse(&id) else {
                    return Err("that chat is not saved yet".into());
                };
                let mut stored = store.load(id).await.map_err(|err| err.to_string())?;
                stored.session.title = Some(title);
                store.save(&stored).await.map_err(|err| err.to_string())
            })
            .map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn delete_chat(&self, id: String) -> BoxFuture<'static, Result<(), String>> {
        let path = self.session_db();
        self.handle
            .spawn(async move {
                let store = open_session_store(path)?;
                let Some(id) = SessionId::parse(&id) else {
                    return Ok(());
                };
                match store.delete(id).await {
                    Ok(()) | Err(StoreError::NotFound(_)) => Ok(()),
                    Err(err) => Err(err.to_string()),
                }
            })
            .map(|joined| joined.unwrap_or_else(|err| Err(err.to_string())))
            .boxed()
    }

    fn recent(&self) -> BoxFuture<'static, Vec<RecentWorkspace>> {
        let path = self
            .overrides
            .db_path
            .clone()
            .or_else(adapter_store::path::default_db_path);
        self.handle
            .spawn(async move {
                let Some(path) = path.filter(|path| path.exists()) else {
                    return Vec::new();
                };
                match adapter_store::SqliteSessionStore::open(&path) {
                    Ok(store) => store
                        .recent_workspaces(RECENT_PROJECTS)
                        .await
                        .unwrap_or_default(),
                    Err(_) => Vec::new(),
                }
            })
            .map(Result::unwrap_or_default)
            .boxed()
    }

    fn browser(&self) -> Arc<dyn ProjectBrowserPort> {
        Arc::clone(&self.browser)
    }
}

/// Most recent projects the picker lists.
const RECENT_PROJECTS: usize = 20;

/// The session store at `path`, when the file exists.
fn open_session_store(path: Option<PathBuf>) -> Result<adapter_store::SqliteSessionStore, String> {
    let Some(path) = path.filter(|path| path.exists()) else {
        return Err("there is no session database".into());
    };
    adapter_store::SqliteSessionStore::open(&path).map_err(|err| err.to_string())
}

#[async_trait]
impl<T: ProjectBrowserPort + ?Sized + 'static> ProjectBrowserPort for TokioPort<T> {
    fn start_dir(&self) -> Option<PathBuf> {
        self.inner.start_dir()
    }

    async fn list_dirs(&self, path: &std::path::Path) -> Result<DirListing, BrowseError> {
        let (port, path) = (Arc::clone(&self.inner), path.to_path_buf());
        self.handle
            .spawn(async move { port.list_dirs(&path).await })
            .await
            .unwrap_or_else(|err| Err(cancelled(&err)))
    }

    async fn resolve(&self, input: &str) -> Result<PathBuf, BrowseError> {
        let (port, input) = (Arc::clone(&self.inner), input.to_owned());
        self.handle
            .spawn(async move { port.resolve(&input).await })
            .await
            .unwrap_or_else(|err| Err(cancelled(&err)))
    }
}

fn cancelled(err: &tokio::task::JoinError) -> BrowseError {
    BrowseError::Unreadable {
        path: String::new(),
        reason: err.to_string(),
    }
}

/// Every theme of `catalog`, resolved with `app.opt.colors` over it; a
/// theme that does not resolve (a broken file) is left out.
fn theme_choices(
    catalog: &application::theme::ThemeCatalog,
    options: &application::ports::plugin::AppOptions,
) -> Vec<ui::session::ThemeChoice> {
    use application::theme::ThemeOrigin;
    let (overrides, _) = application::theme::overrides_from_options(options);
    catalog
        .entries()
        .into_iter()
        .filter_map(|entry| {
            let resolved = catalog.resolve(&entry.name, overrides.as_ref()).ok()?;
            let origin = match entry.origin {
                ThemeOrigin::Builtin => "built in",
                ThemeOrigin::File => "file",
                ThemeOrigin::Config => "init.lua",
            };
            Some(ui::session::ThemeChoice {
                name: entry.name,
                origin: origin.to_owned(),
                theme: resolved.theme,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use application::ports::search::FileList;
    use application::usecases::cancel::CancelToken;

    use super::*;

    #[tokio::test]
    async fn recompute_reruns_the_card_with_the_real_engine_and_rereads_its_file() {
        use application::ports::compute::{ComputePort, DescribeRequest};
        use application::testing::fs::{InMemoryFs, test_root};
        use application::testing::system::RecordingSink;
        use application::tools::compute::ComputeToolPorts;
        use application::usecases::compute::{inline_offload, load_file};
        use domain::block::{Block, BlockKind};

        let fs = Arc::new(InMemoryFs::new(test_root()).with_file("d.csv", "x\n1\n2\n4\n"));
        let engine = Arc::new(adapter_compute::StatsEngine::new());
        let data = load_file(fs.as_ref(), "d.csv", None).await.unwrap();
        let request = DescribeRequest {
            data,
            variables: Vec::new(),
            by: None,
        };
        let (envelope, body) = engine.describe(&request).unwrap();
        let block = Block::new(BlockKind::Computed {
            envelope: Box::new(envelope),
            body: Box::new(body),
        });
        let compute = ComputeToolPorts {
            engine,
            fs: fs.clone(),
            offload: inline_offload(),
        };
        let sink = Arc::new(RecordingSink::new());
        let verified = |sink: &RecordingSink| match sink.events().last() {
            Some(DomainEvent::BlockUpdated {
                block:
                    Block {
                        id,
                        kind: BlockKind::Computed { envelope, .. },
                    },
            }) => (*id, envelope.verification.matched),
            other => panic!("expected a computed update, got {other:?}"),
        };

        recompute_block(compute.clone(), sink.clone(), block.clone())
            .await
            .unwrap();
        assert_eq!(verified(&sink), (block.id, Some(true)));

        fs.set_file("d.csv", "x\n1\n2\n40\n");
        recompute_block(compute.clone(), sink.clone(), block.clone())
            .await
            .unwrap();
        assert_eq!(verified(&sink), (block.id, Some(false)));

        let text = Block::new(BlockKind::Text {
            markdown: "hi".into(),
        });
        assert!(recompute_block(compute, sink, text).await.is_err());
    }

    #[tokio::test]
    async fn the_lua_reproduce_line_of_a_card_runs_and_gives_the_same_values() {
        use adapter_lua::{HostOptions, LuaHost};
        use application::ports::compute::{
            ColumnValues, ComputePort, DataColumn, DataSet, TestRequest,
        };
        use application::ports::plugin::{ConfigSources, PluginHostPort, PluginOutput};
        use application::testing::fs::{InMemoryFs, test_root};
        use domain::block::BlockKind;

        let engine = Arc::new(adapter_compute::StatsEngine::new());
        let request = TestRequest {
            data: DataSet::inline(vec![
                DataColumn {
                    name: "y".into(),
                    values: ColumnValues::Numeric(vec![
                        Some(1.0),
                        Some(2.5),
                        None,
                        Some(3.0),
                        Some(4.5),
                        Some(5.0),
                        Some(2.0),
                    ]),
                    unit: None,
                },
                DataColumn::categorical("g", &["a", "a", "a", "b", "b", "b", "a"]),
            ]),
            y: Some("y".into()),
            group: Some("g".into()),
            ..TestRequest::default()
        };
        let (original, _) = engine.test(&request).unwrap();
        let line = original.reproduce.lua.clone().unwrap();
        assert!(line.contains("nil"), "{line}");

        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("init.lua");
        std::fs::write(
            &init,
            format!(
                "app.tools.register({{ name = 'rep', handler = function() return {line} end }})"
            ),
        )
        .unwrap();
        let host = LuaHost::spawn(HostOptions::default()).unwrap();
        host.provide_compute(engine, Arc::new(InMemoryFs::new(test_root())))
            .await
            .unwrap();
        host.load_config(ConfigSources {
            global: Some(init),
            workspace: None,
        })
        .await
        .unwrap();
        let out = host
            .call_tool("rep", serde_json::Value::Null)
            .await
            .unwrap();
        let PluginOutput::Block(BlockKind::Computed { envelope, .. }) = out else {
            panic!("not a card: {out:?}");
        };
        assert_eq!(envelope.id, original.id);
        assert!(original.diff_values(&envelope, 0.0).is_empty());
    }

    /// A port that only works inside tokio (its timer panics elsewhere).
    struct NeedsTokio;

    #[async_trait]
    impl FileSearchPort for NeedsTokio {
        async fn list_files(&self, _: ListFilesRequest) -> Result<FileList, SearchError> {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(FileList::default())
        }

        async fn find_file(&self, _: FindFileRequest) -> Result<FileList, SearchError> {
            Err(SearchError::Cancelled)
        }

        async fn tree(&self, _: TreeRequest) -> Result<TreeNode, SearchError> {
            Err(SearchError::Cancelled)
        }
    }

    #[test]
    fn tokio_ports_run_on_the_runtime_from_any_executor() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let port = TokioPort::new(Arc::new(NeedsTokio), runtime.handle().clone());
        let listed = futures::executor::block_on(port.list_files(ListFilesRequest::default()));
        assert_eq!(listed, Ok(FileList::default()));
    }

    #[test]
    fn without_an_agent_every_turn_reports_why() {
        let controller = UnavailableController::new("the API key is missing", PermissionMode::Ask);
        let end = futures::executor::block_on(controller.run_turn(TurnRequest {
            prompt: "hola".into(),
            images: Vec::new(),
            model_override: None,
            role: None,
            cancel: CancelToken::new(),
        }));
        assert_eq!(end, TurnEnd::Unavailable("the API key is missing".into()));
        controller.set_permission_mode(PermissionMode::Plan);
        assert_eq!(controller.permission_mode(), PermissionMode::Plan);
        assert!(!controller.models().is_empty());
    }

    #[test]
    fn login_and_logout_go_to_the_key_store_on_the_runtime() {
        use application::ports::credentials::KeyCheck;
        use application::testing::credentials::FakeKeys;

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let keys = FakeKeys::new(KeyCheck::Valid);
        let controller = UnavailableController::new(MissingApiKey.to_string(), PermissionMode::Ask)
            .with_login(runtime.handle().clone(), Arc::new(keys.clone()));
        let stored =
            futures::executor::block_on(controller.login(Account::Anthropic, "sk-ant-new".into()));
        assert_eq!(
            stored,
            Ok("Anthropic API key stored in the system keyring".into())
        );
        assert_eq!(keys.stored(), Some("sk-ant-new".into()));
        let deleted = futures::executor::block_on(controller.logout(Some(Account::Anthropic)));
        assert_eq!(
            deleted,
            Ok("Anthropic API key deleted from the system keyring".into())
        );
        assert_eq!(keys.stored(), None);

        let without = UnavailableController::new("no", PermissionMode::Ask);
        assert!(
            futures::executor::block_on(without.login(Account::Anthropic, "sk-ant-x".into()))
                .is_err()
        );
    }

    #[test]
    fn lua_results_become_replies() {
        use application::ports::plugin::PluginAction;
        let clear = vec![PluginAction::Cmd {
            command: "clear".into(),
        }];
        assert_eq!(
            plugin_reply(Ok(PluginOutput::Actions(clear.clone()))),
            Reply::Actions(clear)
        );
        assert_eq!(
            plugin_reply(Ok(PluginOutput::Value(serde_json::json!(1)))),
            Reply::Done
        );
        let Reply::Block(block) = plugin_reply(Err(PluginError::Runtime("boom".into()))) else {
            panic!("an error is a block");
        };
        assert!(
            matches!(&block.kind, BlockKind::Error { message, .. } if message.contains("boom")),
            "{block:?}"
        );
    }

    #[tokio::test]
    async fn plugin_command_lists_or_explains_the_usage() {
        let trust = Arc::new(PluginTrust {
            decls: Vec::new(),
            store: None,
            granted: Arc::default(),
            host: None,
        });
        let Reply::Notice(NoticeLevel::Info, list) =
            plugin_command(Arc::clone(&trust), "list".into()).await
        else {
            panic!("list is informative");
        };
        assert!(list.contains("no plugin declares capabilities"), "{list}");
        let Reply::Notice(NoticeLevel::Warning, usage) =
            plugin_command(Arc::clone(&trust), "approve x".into()).await
        else {
            panic!("a bad subcommand warns");
        };
        assert!(usage.contains("/plugin trust <name>"), "{usage}");
        let Reply::Notice(NoticeLevel::Warning, missing) =
            plugin_command(trust, "trust x".into()).await
        else {
            panic!("an unknown plugin warns");
        };
        assert!(missing.contains("no plugin `x`"), "{missing}");
    }

    #[test]
    fn mcp_trust_saves_the_approval_or_explains_the_usage() {
        use application::ports::plugin::{ConfigOrigin, McpTransport};
        let dir = tempfile::tempdir().unwrap();
        let spec = McpServerSpec {
            origin: ConfigOrigin::Workspace,
            ..McpServerSpec::new(
                "git",
                McpTransport::Stdio {
                    command: "mcp-git".into(),
                    args: Vec::new(),
                    env: std::collections::BTreeMap::new(),
                    cwd: None,
                },
            )
        };
        let trust = Arc::new(McpCommands {
            specs: vec![spec],
            workspace: dir.path().to_path_buf(),
            store: Some(dir.path().join("trust.json")),
            servers: None,
        });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let run = |args: &str| runtime.block_on(Arc::clone(&trust).run(args.to_owned()));
        assert!(matches!(
            run("trust git"),
            Reply::Notice(NoticeLevel::Info, _)
        ));
        assert!(dir.path().join("trust.json").exists());
        let Reply::Notice(NoticeLevel::Warning, usage) = run("frobnicate git") else {
            panic!("a usage warning");
        };
        assert!(usage.contains("/mcp trust"), "{usage}");
        let Reply::Notice(NoticeLevel::Info, none) = run("list") else {
            panic!("a notice without servers");
        };
        assert!(none.contains("no MCP server"), "{none}");
    }

    #[test]
    fn the_label_is_the_directory_name() {
        assert_eq!(
            workspace_label(std::path::Path::new("/a/proyecto")),
            "proyecto"
        );
    }
}
