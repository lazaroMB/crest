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
//! `:implement <ruta>` opens the implementation flow of a Spec on the
//! runtime ([`UiController::run_command`]); while it waits at a gate,
//! every line sent from the input answers it instead of starting a turn,
//! until the flow stops.
//!
//! `:research <problema>` starts the research flow of the workspace the
//! same way (see [`crate::research`]): while it is active, the lines sent
//! from the input go to it.
//!
//! When the agent cannot start (no API key) the window still opens: the
//! error is the first block of the timeline and every prompt answers with
//! it ([`UnavailableController`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use adapter_search::RipgrepSearch;
use application::flows::implement::flow::{FlowError, FlowReply};
use application::flows::implement::graph::{ImplementSession, status_notice};
use application::flows::implement::state::FlowStatus;
use application::flows::research::session::ResearchSession;
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::ports::plugin::{
    CallbackId, McpServerSpec, PluginError, PluginHostPort, PluginOutput,
};
use application::ports::search::{
    ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepRequest, GrepResult,
    ListFilesRequest, SearchError, TreeNode, TreeRequest,
};
use application::usecases::approval::lock_permissions;
use application::usecases::run_turn::SharedPermissions;
use async_trait::async_trait;
use domain::block::{Block, BlockKind};
use domain::conversation::StopReason;
use domain::model::{ModelId, ModelPolicy, ResolveRequest};
use domain::permission::PermissionMode;
use domain::workspace::WorkspaceRoot;
use futures::FutureExt as _;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use tokio::runtime::Handle;
use ui::approval::UiApproval;
use ui::session::{
    ModelChoice, Reply, SessionController, SessionParts, TurnEnd, TurnRequest, prompt_block,
};

use crate::args::Args;
use crate::research::ResearchCommand;
use crate::root::{App, Config, Frontend, Overrides, build_app};

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

/// The models of `policy` for the `<leader>m` menu.
#[must_use]
pub fn model_choices(policy: &ModelPolicy) -> Vec<ModelChoice> {
    policy
        .aliases()
        .into_iter()
        .map(|(alias, model)| ModelChoice { alias, model })
        .collect()
}

/// The UI's input port over the real [`App`].
pub struct UiController {
    handle: Handle,
    app: Arc<tokio::sync::Mutex<App>>,
    permissions: SharedPermissions,
    models: Vec<ModelChoice>,
    plugins: Option<Arc<dyn PluginHostPort>>,
    commands: HashMap<String, CallbackId>,
    mcp: Arc<McpTrust>,
    events: Arc<dyn EventSink>,
    flow: ActiveFlow,
    research: SharedResearch,
}

/// The research flow of the workspace, taken out of the [`App`] so a flow
/// line does not wait for (or block) the chat's turns.
type SharedResearch = Arc<tokio::sync::Mutex<Option<ResearchSession>>>;

/// `:research <args>`: starts, resumes or reports the research flow and
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
            ":research no está disponible: la base de datos de crest no abrió".into(),
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

/// Saves the chat session, warning on failure.
async fn save(app: &tokio::sync::Mutex<App>) {
    let app = app.lock().await;
    if let Err(err) = app.save().await {
        app.warn(format!("no pude guardar la sesión: {err}"));
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
                "el flujo de implementación se detuvo: {err}; `:implement {}` lo reanuda",
                session.plan_path()
            ),
        )),
    }
}

/// `:implement <target>`: opens the flow of `target` and runs it to its
/// first gate.
async fn start_implementation(
    app: Arc<tokio::sync::Mutex<App>>,
    flow: ActiveFlow,
    target: String,
) -> Reply {
    let mut active = flow.lock().await;
    if let Some(session) = active.as_ref() {
        return Reply::Notice(
            NoticeLevel::Warning,
            format!(
                "ya hay un flujo de implementación activo ({}); respóndelo o escribe /abort",
                session.plan_path()
            ),
        );
    }
    let opened = app.lock().await.open_implementation(&target).await;
    let session = match opened {
        Ok(session) => session,
        Err(err) => return Reply::Notice(NoticeLevel::Warning, format!(":implement: {err:#}")),
    };
    let resumed = session.resumed().then(|| {
        format!(
            "flujo de implementación de {} reanudado",
            session.plan_path()
        )
    });
    let result = session.run().await;
    match settle(&mut active, session, result) {
        Some((level, message)) => Reply::Notice(level, message),
        None => resumed.map_or(Reply::Done, |m| Reply::Notice(NoticeLevel::Info, m)),
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

/// What `:mcp trust <name>` needs.
struct McpTrust {
    specs: Vec<McpServerSpec>,
    workspace: PathBuf,
    store: Option<PathBuf>,
}

impl McpTrust {
    /// `:mcp <args>`.
    fn run(&self, args: &str) -> Reply {
        let mut words = args.split_whitespace();
        let (Some("trust"), Some(name), None) = (words.next(), words.next(), words.next()) else {
            return Reply::Notice(NoticeLevel::Warning, "uso: :mcp trust <nombre>".into());
        };
        match crate::mcp::trust_declared(&self.specs, &self.workspace, self.store.as_deref(), name)
        {
            Ok(message) => Reply::Notice(NoticeLevel::Info, message),
            Err(message) => Reply::Notice(NoticeLevel::Warning, format!(":mcp trust: {message}")),
        }
    }
}

impl UiController {
    /// A controller running the turns of `app` on `handle`.
    #[must_use]
    pub fn new(mut app: App, handle: Handle) -> Self {
        let research = Arc::new(tokio::sync::Mutex::new(app.research.take()));
        let permissions = Arc::clone(app.permissions());
        let models = model_choices(app.model_policy());
        let plugins = app.plugins.clone();
        let mcp = Arc::new(McpTrust {
            specs: app.mcp_specs.clone(),
            workspace: app.workspace.as_path().to_path_buf(),
            store: app.mcp_trust_path.clone(),
        });
        let events = app.events();
        let commands = app
            .commands
            .iter()
            .map(|c| (c.name.clone(), c.callback))
            .collect();
        Self {
            handle,
            app: Arc::new(tokio::sync::Mutex::new(app)),
            permissions,
            models,
            plugins,
            commands,
            mcp,
            events,
            flow: Arc::new(tokio::sync::Mutex::new(None)),
            research,
        }
    }
}

/// What the session shows or runs for the result of a Lua callback.
#[must_use]
pub fn plugin_reply(result: Result<PluginOutput, PluginError>) -> Reply {
    match result {
        Ok(PluginOutput::Actions(actions)) => Reply::Actions(actions),
        Ok(PluginOutput::Block(kind)) => Reply::Block(Block::new(kind)),
        Ok(PluginOutput::Value(_)) => Reply::Done,
        Err(err) => Reply::Block(Block::new(BlockKind::Error {
            message: format!("Lua: {err}"),
            hint: None,
        })),
    }
}

/// Runs one turn of `app` and saves the session, as the headless mode does.
pub async fn run_turn(app: &mut App, request: TurnRequest) -> TurnEnd {
    app.emit(DomainEvent::BlockAdded {
        block: prompt_block(&request.prompt),
    });
    let mut opts = app.turn_options.clone().with_cancel(request.cancel);
    if let Some(role) = request.role {
        opts.role = role;
    }
    if let Some(model) = request.model_override {
        opts = opts.with_override(model);
    }
    let result = app
        .service
        .run_turn(&mut app.session, request.prompt, opts)
        .await;
    if let Err(err) = app.save().await {
        app.warn(format!("no pude guardar la sesión: {err}"));
    }
    match result {
        Ok(outcome) => TurnEnd::Finished(outcome.stop_reason),
        Err(err) => TurnEnd::Failed(err.to_string()),
    }
}

impl SessionController for UiController {
    fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        let (tx, rx) = oneshot::channel();
        let (app, flow, events, research) = (
            Arc::clone(&self.app),
            Arc::clone(&self.flow),
            Arc::clone(&self.events),
            Arc::clone(&self.research),
        );
        self.handle.spawn(async move {
            let mut active = flow.lock().await;
            if active.is_some() {
                events.emit(DomainEvent::BlockAdded {
                    block: prompt_block(&request.prompt),
                });
                answer_gate(&mut active, &request.prompt, events.as_ref()).await;
                tx.send(TurnEnd::Finished(StopReason::EndTurn)).ok();
                return;
            }
            drop(active);
            let mut research = research.lock().await;
            if let Some(session) = research.as_mut().filter(|s| s.is_active()) {
                events.emit(DomainEvent::BlockAdded {
                    block: prompt_block(&request.prompt),
                });
                let (level, message) = match crate::research::line(session, &request.prompt).await {
                    Ok(message) => (NoticeLevel::Info, message),
                    Err(message) => (NoticeLevel::Warning, message),
                };
                events.emit(DomainEvent::Notice { level, message });
                drop(research);
                save(&app).await;
                tx.send(TurnEnd::Finished(StopReason::EndTurn)).ok();
                return;
            }
            drop(research);
            let mut app = app.lock().await;
            tx.send(run_turn(&mut app, request).await).ok();
        });
        rx.map(|end| {
            end.unwrap_or_else(|_| TurnEnd::Failed("el turno terminó sin responder".into()))
        })
        .boxed()
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

    fn new_session(&self) -> BoxFuture<'static, ()> {
        let app = Arc::clone(&self.app);
        let task = self
            .handle
            .spawn(async move { app.lock().await.new_session() });
        task.map(|_| ()).boxed()
    }

    fn run_callback(&self, id: u64, args: serde_json::Value) -> BoxFuture<'static, Reply> {
        let Some(host) = self.plugins.clone() else {
            return futures::future::ready(Reply::Notice(
                NoticeLevel::Warning,
                "Lua no está disponible".into(),
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
        if name == "implement" {
            let start = start_implementation(
                Arc::clone(&self.app),
                Arc::clone(&self.flow),
                args.to_owned(),
            );
            let task = self.handle.spawn(start);
            return Some(
                task.map(|joined| {
                    joined.unwrap_or_else(|err| {
                        Reply::Notice(NoticeLevel::Warning, format!(":implement: {err}"))
                    })
                })
                .boxed(),
            );
        }
        if name == "research" {
            let start = research_command(
                Arc::clone(&self.app),
                Arc::clone(&self.research),
                args.to_owned(),
            );
            let task = self.handle.spawn(start);
            return Some(
                task.map(|joined| {
                    joined.unwrap_or_else(|err| {
                        Reply::Notice(NoticeLevel::Warning, format!(":research: {err}"))
                    })
                })
                .boxed(),
            );
        }
        if name == "mcp" {
            // Reads and writes the store file: off GPUI's thread.
            let (mcp, args) = (Arc::clone(&self.mcp), args.to_owned());
            let task = self.handle.spawn_blocking(move || mcp.run(&args));
            return Some(
                task.map(|joined| {
                    joined.unwrap_or_else(|err| {
                        Reply::Notice(NoticeLevel::Warning, format!(":mcp: {err}"))
                    })
                })
                .boxed(),
            );
        }
        let id = self.commands.get(name)?;
        Some(self.run_callback(id.0, serde_json::Value::String(args.to_owned())))
    }
}

/// The controller of a window whose agent could not start.
pub struct UnavailableController {
    reason: String,
    mode: Mutex<PermissionMode>,
    models: Vec<ModelChoice>,
}

impl UnavailableController {
    /// Answers every turn with `reason`.
    #[must_use]
    pub fn new(reason: impl Into<String>, mode: PermissionMode) -> Self {
        Self {
            reason: reason.into(),
            mode: Mutex::new(mode),
            models: model_choices(&ModelPolicy::default()),
        }
    }
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
}

/// The model the first turn will use.
fn initial_model(app: &App) -> ModelId {
    let mut request = ResolveRequest::new(app.turn_options.role);
    request
        .turn_override
        .clone_from(&app.turn_options.turn_override);
    app.model_policy().resolve(&request).spec.id
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
    let frontend = Frontend {
        events: Arc::new(sink.clone()),
        approval: Arc::new(approval),
    };
    let built = handle.block_on(build_app(config, frontend, overrides));
    let (mut parts, workspace) = match built {
        Ok(app) => {
            let files = TokioPort::new(Arc::clone(&app.file_search), handle.clone());
            let content = TokioPort::new(Arc::clone(&app.content_search), handle.clone());
            let workspace = app.workspace.as_path().to_path_buf();
            let model = initial_model(&app);
            let (keymap, key_warnings) = crate::keys::keymap(&app.keymaps, &app.options);
            for warning in key_warnings {
                app.warn(warning);
            }
            let timeoutlen = crate::keys::timeoutlen(&app.options);
            let history = app.timeline_blocks();
            let controller = Arc::new(UiController::new(app, handle.clone()));
            let mut parts = SessionParts::new(
                controller,
                events,
                approvals,
                Arc::new(files),
                Arc::new(content),
            );
            parts.model = Some(model);
            parts.keymap = keymap;
            parts.timeoutlen = timeoutlen;
            parts.history = history;
            (parts, workspace)
        }
        Err(err) => {
            let reason = format!("{err:#}");
            let root = std::fs::canonicalize(&config.workspace)
                .map_err(anyhow::Error::from)
                .and_then(|dir| WorkspaceRoot::new(dir).map_err(anyhow::Error::from))
                // The workspace itself is the problem: report the original
                // error, there is nothing to show.
                .map_err(|_| err)?;
            // Also on the terminal that started the window.
            eprintln!("aviso: el agente no está disponible: {reason}");
            sink.emit(DomainEvent::BlockAdded {
                block: Block::new(BlockKind::Error {
                    message: reason.clone(),
                    hint: Some(
                        "el agente no puede responder; corrige esto y vuelve a abrir crest".into(),
                    ),
                }),
            });
            let workspace = root.as_path().to_path_buf();
            let search = Arc::new(RipgrepSearch::new(root));
            let files: Arc<dyn FileSearchPort> = search.clone();
            let content: Arc<dyn ContentSearchPort> = search;
            let controller = Arc::new(UnavailableController::new(reason, config.mode));
            let parts = SessionParts::new(
                controller,
                events,
                approvals,
                Arc::new(TokioPort::new(files, handle.clone())),
                Arc::new(TokioPort::new(content, handle.clone())),
            );
            (parts, workspace)
        }
    };
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

/// `crest [DIR]`: opens the window over the workspace. Returns the exit
/// code: 0, 1 when the window cannot open, 2 for configuration errors.
#[must_use]
pub fn run(args: &Args, handle: &Handle) -> u8 {
    let parts = Config::from_args(args)
        .and_then(|config| session_parts(&config, Overrides::default(), handle));
    let parts = match parts {
        Ok(parts) => parts,
        Err(err) => {
            eprintln!("error: {err:#}");
            return 2;
        }
    };
    let opts = ui::app::UiOptions {
        workspace_label: parts.workspace_label.clone(),
        ..ui::app::UiOptions::default()
    };
    match ui::app::run_with(opts, move |window, cx| {
        ui::session::open(parts, window, cx).0
    }) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use application::ports::search::FileList;
    use application::usecases::cancel::CancelToken;

    use super::*;

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
        let controller = UnavailableController::new("falta la API key", PermissionMode::Ask);
        let end = futures::executor::block_on(controller.run_turn(TurnRequest {
            prompt: "hola".into(),
            model_override: None,
            role: None,
            cancel: CancelToken::new(),
        }));
        assert_eq!(end, TurnEnd::Unavailable("falta la API key".into()));
        controller.set_permission_mode(PermissionMode::Plan);
        assert_eq!(controller.permission_mode(), PermissionMode::Plan);
        assert!(!controller.models().is_empty());
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
                },
            )
        };
        let trust = McpTrust {
            specs: vec![spec],
            workspace: dir.path().to_path_buf(),
            store: Some(dir.path().join("trust.json")),
        };
        assert!(matches!(
            trust.run("trust git"),
            Reply::Notice(NoticeLevel::Info, _)
        ));
        assert!(dir.path().join("trust.json").exists());
        let Reply::Notice(NoticeLevel::Warning, usage) = trust.run("list") else {
            panic!("a usage warning");
        };
        assert!(usage.contains(":mcp trust"), "{usage}");
    }

    #[test]
    fn the_label_is_the_directory_name() {
        assert_eq!(
            workspace_label(std::path::Path::new("/a/proyecto")),
            "proyecto"
        );
    }
}
